// SPDX-License-Identifier: Apache-2.0
//! `deliver_changes` agent tool — the single call that carries code work
//! through git delivery (commit → push → PR → CI → merge → release) up to the
//! user-configured [`DeliveryCeiling`]. This is the capability whose absence
//! made the agent stall at a green build, re-listing the missing PR instead of
//! opening one. The model invokes this instead of hand-running git in bash.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[cfg(test)]
use super::ToolExecutionStatus;
use super::{ExecCtx, ToolOutput};
use crate::agent::delivery::{self, DeliverOpts, ReleaseUrgency};
use crate::agent::delivery_run::{
    self, CoreInputRequest, DeliveryObservation, NewDeliveryRun, ProcessIdentity,
};
use crate::config::settings::DeliveryCeiling;
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "deliver_changes".into(),
            description: "Deliver the current code changes through git: stage only the real \
                changed source files (never local noise), commit, push, open (or reuse) a pull \
                request, and — depending on the user's configured delivery ceiling — wait for CI, \
                merge, and release. Call this after tests pass to carry code work to done; \
                do NOT hand-run git in bash and do NOT stop at a green build to describe a missing \
                PR. It generates or repairs the PR README decision block. Idempotent: when a \
                recoverable result supplies next_action, perform that action and call this again \
                to resume the SAME PR; the agent loop bounds retries. Returns the steps taken, the \
                PR URL, and a state (delivered / blocked / noop)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Optional PR/commit title. Defaults to a message derived from the branch + changed files." },
                    "body":  { "type": "string", "description": "Optional PR body." },
                    "release_urgency": {
                        "type": "string",
                        "enum": ["immediate", "hold"],
                        "description": "Optional release-cadence signal. `immediate` is yours to judge: use it when the rubric applies (main path broken, data loss, security bypass, a released version exposing the defect, the user said urgent, or a large self-contained capability just landed). `hold` is NOT yours to set — it stops the entire batch including other people's merges and only the user can clear it. Pass it only when the user explicitly asked to withhold the release, and quote their instruction in `hold_requested_by_user`; an unmandated `hold` is ignored and delivery proceeds normally."
                    },
                    "ceiling": {
                        "type": "string",
                        "enum": ["off", "pr_only", "through_ci_green", "through_merge", "through_release"],
                        "description": "Optional per-call ceiling. Clamped to at most the user's configured ceiling — a call can lower, never raise it. Use `through_ci_green` to WAIT for CI on the PR this branch already has: this tool polls with backoff (10s→60s) and is the supported way to wait. Never shell out to `gh pr checks --watch` or a tight `gh` polling loop — those refresh every 10s until CI ends, exhaust the shared GitHub quota, and the resulting 403s break unrelated releases."
                    },
                    "hold_requested_by_user": {
                        "type": "string",
                        "description": "Required to make `hold` take effect: quote the user's own instruction to withhold this release. Without it a `hold` is dropped, because gating a batch is the user's decision, not the agent's."
                    },
                    "expect_branch": {
                        "type": "string",
                        "description": "Optional guard: the branch you believe you are delivering. This tool has no branch argument — it delivers whatever branch the working directory is on. When resuming a specific PR, pass its head branch here; if the working directory is somewhere else the call stops before touching anything instead of opening a second PR for unrelated work."
                    },
                    "autonomous_completion": {
                        "type": "boolean",
                        "description": "Defaults true once deliver_changes is authorized, so technical waits survive restart and apply the recommended recovery without asking the user to continue. Set false only when the user explicitly limited unattended continuation; it never expands authority or bypasses release/test/signing gates."
                    }
                }
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Some(settings) = ctx.settings.clone() else {
        return Ok(ToolOutput::err("交付不可用:当前上下文没有可用的设置快照。"));
    };

    let title = args.get("title").and_then(Value::as_str).map(String::from);
    let body = args.get("body").and_then(Value::as_str).map(String::from);
    let release_urgency = match release_urgency_from_args(&args) {
        Ok(value) => value,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let requested_ceiling = args
        .get("ceiling")
        .and_then(Value::as_str)
        .and_then(parse_ceiling);

    let opts = DeliverOpts {
        title,
        body,
        release_urgency,
        requested_ceiling,
        extra_excludes: settings.delivery_exclude_globs.clone(),
        expect_branch: args
            .get("expect_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from),
    };

    let durable =
        prepare_durable_run(&args, ctx, settings.delivery_ceiling, requested_ceiling).await?;

    let remote = delivery::resolve_delivery_remote(&ctx.cwd, &settings);

    loop {
        let outcome = delivery::deliver(
            &ctx.cwd,
            settings.delivery_ceiling,
            settings.delivery_merge_method,
            settings.delivery_ci_timeout_secs,
            &opts,
            remote.as_ref(),
            None,
        )
        .await;

        if let (Some(db), Some(session_id)) = (ctx.db.as_ref(), ctx.session_id.as_deref()) {
            persist_delivery_ref(db, session_id, &outcome).await?;
        }
        if let (Some(db), Some(durable)) = (ctx.db.as_ref(), durable.as_ref()) {
            persist_durable_outcome(db, durable, &outcome).await?;
        }

        if outcome.validate_contract().is_err() {
            return Ok(tool_output_for_outcome(&outcome));
        }

        if outcome.final_state != "waiting" {
            return Ok(tool_output_for_outcome(&outcome));
        }

        // A waiting delivery remains one in-flight tool call. The shared loop
        // keeps emitting its 30s heartbeat and can cancel by dropping this
        // future; no extra model request or user "continue" is required.
        let retry_after_ms = outcome.retry_after_ms.unwrap_or(30_000).clamp(1, 60_000);
        tokio::time::sleep(std::time::Duration::from_millis(retry_after_ms)).await;
    }
}

struct PreparedDurableRun {
    id: String,
    process: ProcessIdentity,
    expected_head_sha: String,
    head_branch: String,
}

async fn prepare_durable_run(
    args: &Value,
    ctx: &ExecCtx,
    configured_ceiling: DeliveryCeiling,
    requested_ceiling: Option<DeliveryCeiling>,
) -> Result<Option<PreparedDurableRun>> {
    let Some(db) = ctx.db.as_ref() else {
        return Ok(None);
    };
    let repo = delivery::resolve_repo(&ctx.cwd, None).map_err(crate::errors::AppError::Other)?;
    let source_identity = ctx
        .root_turn_id
        .as_deref()
        .or(ctx.task_id.as_deref())
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "deliver_changes refused external mutation without a durable root-turn/task identity"
                    .into(),
            )
        })?;
    let repo_identity = durable_repo_identity(&repo);
    let expected_head_sha = git2::Repository::open(&repo.root)
        .ok()
        .and_then(|repository| repository.head().ok()?.target().map(|oid| oid.to_string()))
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "deliver_changes could not establish the expected git head before mutation".into(),
            )
        })?;
    let change_set_digest = durable_change_set_digest(&repo.root, &expected_head_sha)?;
    let id = durable_run_id(source_identity, &repo_identity);
    let process = ProcessIdentity::new(
        format!(
            "{}:{}",
            std::process::id(),
            crate::storage::db::current_process_start_token()
                .unwrap_or_else(|| "unknown-start".into())
        ),
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    );
    let selected_ceiling = requested_ceiling
        .map(|requested| configured_ceiling.clamp_request(requested))
        .unwrap_or(configured_ceiling);
    let head_branch = repo.branch.clone();
    let run = NewDeliveryRun {
        id: id.clone(),
        run_kind: "deliver_changes".into(),
        session_id: ctx.session_id.clone(),
        root_turn_id: ctx.root_turn_id.clone(),
        task_segment_id: None,
        task_id: ctx.task_id.clone(),
        workspace_path: repo.root.to_string_lossy().into_owned(),
        repo_identity,
        base_branch: repo.default_branch,
        head_branch: head_branch.clone(),
        change_set_digest,
        expected_head_sha: expected_head_sha.clone(),
        canonical_pr_number: None,
        canonical_pr_url: None,
        canonical_head_sha: None,
        requested_ceiling: ceiling_label(selected_ceiling).into(),
        reached_ceiling: "local".into(),
        stage: "preflight".into(),
        status: "running".into(),
        wait_class: None,
        next_action: Some("deliver".into()),
        next_action_authorized: true,
        autonomous_completion: autonomous_completion_from_args(args),
    };
    delivery_run::create_delivery_run(
        db,
        &run,
        &process,
        chrono::Utc::now().timestamp_millis(),
        90_000,
    )
    .await?;
    Ok(Some(PreparedDurableRun {
        id,
        process,
        expected_head_sha,
        head_branch,
    }))
}

fn autonomous_completion_from_args(args: &Value) -> bool {
    // Invoking deliver_changes is already the user's authorization to reach
    // the selected ceiling. Technical waits/restarts must not manufacture a
    // second confirmation gate; only an explicit false opts out.
    args.get("autonomous_completion")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

async fn persist_durable_outcome(
    db: &sqlx::SqlitePool,
    durable: &PreparedDurableRun,
    outcome: &delivery::DeliveryOutcome,
) -> Result<()> {
    let core_input = core_input_request_for_outcome(outcome);
    let status = match outcome.final_state.as_str() {
        "delivered" | "noop" => "completed",
        "waiting" => "waiting",
        _ if core_input.is_some() => "core_input_required",
        _ if outcome.recoverable => "agent_action_required",
        _ if outcome.recovery_class == delivery::RecoveryClass::ExternalStateUncertain => {
            "platform_incident"
        }
        _ => "failed_internal",
    };
    let recovery_class = serde_json::to_value(outcome.recovery_class)
        .ok()
        .and_then(|value| value.as_str().map(String::from));
    let expected_head_sha = outcome
        .commit_sha
        .clone()
        .unwrap_or_else(|| durable.expected_head_sha.clone());
    let observation = DeliveryObservation {
        head_branch: outcome
            .branch
            .clone()
            .unwrap_or_else(|| durable.head_branch.clone()),
        stage: outcome.stage.clone(),
        status: status.into(),
        wait_class: recovery_class,
        next_action: outcome.next_action.clone(),
        reached_ceiling: outcome.reached_state.clone(),
        expected_head_sha: expected_head_sha.clone(),
        canonical_pr_number: outcome.pr_number.map(|value| value as i64),
        canonical_pr_url: outcome.pr_url.clone(),
        canonical_head_sha: outcome.pr_number.map(|_| expected_head_sha),
        failure_signature: (outcome.final_state != "delivered" && outcome.final_state != "noop")
            .then(|| outcome.code.clone()),
        core_input,
    };
    delivery_run::record_delivery_observation(
        db,
        &durable.id,
        &durable.process,
        &observation,
        chrono::Utc::now().timestamp_millis(),
        outcome
            .retry_after_ms
            .unwrap_or(30_000)
            .saturating_add(60_000) as i64,
    )
    .await?;
    Ok(())
}

fn core_input_request_for_outcome(outcome: &delivery::DeliveryOutcome) -> Option<CoreInputRequest> {
    if outcome.recovery_class != delivery::RecoveryClass::CoreInputRequired {
        return None;
    }
    let mut missing = Vec::new();
    if let Some(capability) = outcome.capability_gap.as_deref() {
        missing.push(serde_json::json!({
            "kind": "capability",
            "key": capability,
        }));
    }
    if let Some(next_action) = outcome.next_action.as_deref() {
        missing.push(serde_json::json!({
            "kind": "required_input",
            "key": outcome.code,
            "resume_action": next_action,
        }));
    }
    if missing.is_empty() {
        return None;
    }
    Some(CoreInputRequest {
        request_key: outcome.code.clone(),
        inputs_json: serde_json::Value::Array(missing).to_string(),
        attempts_json: serde_json::json!({
            "alternatives_exhausted": true,
            "observed_stages": outcome.steps.iter().map(|step| &step.step).collect::<Vec<_>>(),
        })
        .to_string(),
        resume_stage: outcome.stage.clone(),
    })
}

/// Resume an expired recoverable wait after startup. The durable lease was
/// claimed before this function is called; only already-authorized autonomous
/// waits are eligible. `deliver` begins with local/remote reconciliation and is
/// idempotent against the canonical PR/head receipts.
pub(crate) async fn resume_claimed_delivery(
    db: sqlx::SqlitePool,
    settings: crate::config::settings::Settings,
    claimed: delivery_run::ClaimedRecovery,
    process: ProcessIdentity,
) -> Result<()> {
    if !should_resume_claimed_delivery(&claimed) {
        return Ok(());
    }
    let Some(requested_ceiling) = parse_ceiling(&claimed.requested_ceiling) else {
        return Err(crate::errors::AppError::Other(format!(
            "durable delivery run {} has an invalid requested ceiling",
            claimed.run_id
        )));
    };
    let cwd = std::path::PathBuf::from(&claimed.workspace_path);
    let opts = DeliverOpts {
        title: None,
        body: None,
        release_urgency: None,
        requested_ceiling: Some(requested_ceiling),
        extra_excludes: settings.delivery_exclude_globs.clone(),
        expect_branch: Some(claimed.head_branch.clone()),
    };
    let remote = delivery::resolve_delivery_remote(&cwd, &settings);
    let prepared = PreparedDurableRun {
        id: claimed.run_id,
        process,
        expected_head_sha: claimed.expected_head_sha,
        head_branch: claimed.head_branch,
    };
    loop {
        let outcome = delivery::deliver(
            &cwd,
            requested_ceiling,
            settings.delivery_merge_method,
            settings.delivery_ci_timeout_secs,
            &opts,
            remote.as_ref(),
            Some(&claimed.base_branch),
        )
        .await;
        persist_durable_outcome(&db, &prepared, &outcome).await?;
        if outcome.final_state != "waiting" {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            outcome.retry_after_ms.unwrap_or(30_000).clamp(1, 60_000),
        ))
        .await;
    }
}

pub(crate) fn should_resume_claimed_delivery(claimed: &delivery_run::ClaimedRecovery) -> bool {
    // Technical waits never need a second user decision. The explicit
    // authorization attached to the same objective is the only gate here;
    // autonomous_completion governs default business choices, not liveness.
    claimed.status == "waiting" && claimed.next_action_authorized
}

fn ceiling_label(ceiling: DeliveryCeiling) -> &'static str {
    match ceiling {
        DeliveryCeiling::Off => "off",
        DeliveryCeiling::PrOnly => "pr_only",
        DeliveryCeiling::ThroughCiGreen => "through_ci_green",
        DeliveryCeiling::ThroughMerge => "through_merge",
        DeliveryCeiling::ThroughRelease => "through_release",
    }
}

fn durable_repo_identity(repo: &delivery::RepoContext) -> String {
    let source = repo
        .remote_url
        .as_deref()
        .unwrap_or_else(|| repo.root.to_str().unwrap_or("local-repository"));
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn durable_run_id(source_identity: &str, repo_identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_identity.as_bytes());
    hasher.update([0]);
    hasher.update(repo_identity.as_bytes());
    format!("delivery-{:x}", hasher.finalize())
}

fn durable_change_set_digest(root: &std::path::Path, head: &str) -> Result<String> {
    let repository = git2::Repository::open(root).map_err(|error| {
        crate::errors::AppError::Other(format!("cannot inspect delivery worktree: {error}"))
    })?;
    let statuses = repository.statuses(None).map_err(|error| {
        crate::errors::AppError::Other(format!("cannot inspect delivery changes: {error}"))
    })?;
    let mut entries: Vec<_> = statuses
        .iter()
        .filter_map(|entry| {
            entry
                .path()
                .map(|path| (path.to_owned(), entry.status().bits()))
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    hasher.update(head.as_bytes());
    for (path, status) in entries {
        hasher.update(path.as_bytes());
        hasher.update(status.to_le_bytes());
        if let Ok(bytes) = std::fs::read(root.join(&path)) {
            hasher.update(bytes);
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn tool_output_for_outcome(outcome: &delivery::DeliveryOutcome) -> ToolOutput {
    if let Err(error) = outcome.validate_contract() {
        return ToolOutput::err(format!(
            "交付状态契约校验失败，未生成成功或阻断结论: {error}"
        ))
        .with_metadata(json!({
            "status": "error",
            "code": "delivery_contract_invalid",
            "recoverable": false,
            "recovery_class": "external_state_uncertain"
        }));
    }
    let report = render_report(outcome);
    let output = match outcome.final_state.as_str() {
        "waiting" => ToolOutput::waiting(report),
        "blocked" => ToolOutput::blocked(report),
        _ => ToolOutput::ok(report),
    };
    output.with_metadata(json!({
        "status": outcome.final_state,
        "stage": outcome.stage,
        "code": outcome.code,
        "recoverable": outcome.recoverable,
        "recovery_class": outcome.recovery_class,
        "decision_type": if outcome.recovery_class == crate::agent::delivery::RecoveryClass::CoreInputRequired { "core_input_required" } else { "system_owned" },
        "requires_user_continue": false,
        "retry_after_ms": outcome.retry_after_ms,
        "next_action": outcome.next_action,
        "requested_ceiling": outcome.requested_ceiling,
        "effective_ceiling": outcome.effective_ceiling,
        "reached_state": outcome.reached_state,
        "capability_gap": outcome.capability_gap,
        "branch": outcome.branch,
        "commit_sha": outcome.commit_sha,
        "pr_number": outcome.pr_number,
        "pr_url": outcome.pr_url,
    }))
}

async fn persist_delivery_ref(
    db: &sqlx::SqlitePool,
    session_id: &str,
    outcome: &delivery::DeliveryOutcome,
) -> Result<()> {
    let (Some(branch), Some(pr_number), Some(pr_url)) = (
        outcome.branch.as_deref(),
        outcome.pr_number,
        outcome.pr_url.as_deref(),
    ) else {
        return Ok(());
    };
    let updated_at = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO session_delivery_refs
            (session_id, branch, pr_number, pr_url, commit_sha, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(session_id) DO UPDATE SET
            branch=excluded.branch,
            pr_number=excluded.pr_number,
            pr_url=excluded.pr_url,
            commit_sha=excluded.commit_sha,
            updated_at=excluded.updated_at",
    )
    .bind(session_id)
    .bind(branch)
    .bind(pr_number as i64)
    .bind(pr_url)
    .bind(outcome.commit_sha.as_deref())
    .bind(updated_at)
    .execute(db)
    .await?;
    Ok(())
}

/// Render a compact, model-readable report. Never `is_error` for a "blocked"
/// terminal — blocked is an informative outcome (e.g. CI red / needs a token),
/// not a tool crash; the content explains it so the model reports rather than
/// blindly retries. Split from `execute` so the attribution contract on
/// blocked outcomes stays unit-testable.
fn render_report(outcome: &delivery::DeliveryOutcome) -> String {
    let mut out = String::new();
    out.push_str(&format!("交付结果: {}\n", outcome.final_state));
    if let Some(branch) = &outcome.branch {
        out.push_str(&format!("分支: {branch}\n"));
    }
    out.push_str(&format!(
        "请求边界: {} · 实际边界: {} · 已到达: {}\n",
        outcome.requested_ceiling, outcome.effective_ceiling, outcome.reached_state
    ));
    for s in &outcome.steps {
        let mark = match s.status.as_str() {
            "ok" => "✅",
            "skipped" => "⏭️",
            "blocked" => "⛔",
            _ => "•",
        };
        out.push_str(&format!("  {mark} {}: {}\n", s.step, s.detail));
    }
    if let Some(url) = &outcome.pr_url {
        out.push_str(&format!("PR: {url}\n"));
    }
    out.push_str(&format!("\n{}", outcome.summary));
    if outcome.final_state == "blocked" || outcome.final_state == "waiting" {
        out.push_str(
            "\n\n注意:本次交付没有达到请求边界；只能报告上面明确列出的已完成步骤。\
即使之后查询发现仓库出现了新的合并或发布,那也是其他执行器(并行 agent 或自动化流水线)\
完成的,不得归因为你本次的交付动作。",
        );
        if outcome.recovery_class == crate::agent::delivery::RecoveryClass::WaitRetryable {
            out.push_str(
                "\n\n这是等待中的交付状态，系统会在退避后自动核对同一 PR；用户无需回复『继续』。",
            );
        } else if outcome.recoverable {
            out.push_str(
                "\n\n这是可恢复的交付状态，不是最终总结边界。执行 metadata/正文中的 next_action，\
然后重新调用 deliver_changes 续接同一 PR；不得使用 --admin、force push 或删 required check。",
            );
        } else if outcome.recovery_class == crate::agent::delivery::RecoveryClass::CoreInputRequired
        {
            out.push_str(
                "\n\n系统已把无法推导的外部核心输入合并为一次请求；输入补齐后应自动续接原 run，\
不得要求用户再回复『继续』，也不得降低原交付边界。",
            );
        } else {
            out.push_str("如实报告缺失能力、实际到达层级和恢复动作即可。");
        }
    }
    out
}

fn parse_ceiling(s: &str) -> Option<DeliveryCeiling> {
    match s {
        "off" => Some(DeliveryCeiling::Off),
        "pr_only" => Some(DeliveryCeiling::PrOnly),
        "through_ci_green" => Some(DeliveryCeiling::ThroughCiGreen),
        "through_merge" => Some(DeliveryCeiling::ThroughMerge),
        "through_release" => Some(DeliveryCeiling::ThroughRelease),
        _ => None,
    }
}

fn parse_release_urgency(s: &str) -> Option<ReleaseUrgency> {
    match s {
        "immediate" => Some(ReleaseUrgency::Immediate),
        "hold" => Some(ReleaseUrgency::Hold),
        _ => None,
    }
}

/// `hold` is the pipeline's one real business gate: it stops the WHOLE batch —
/// other people's merges included — and only a human can clear it with
/// `allow_guarded_batch`. So only a human may set it.
///
/// On 2026-08-05 five commits carried `Release-Urgency: hold`, every one
/// authored by the delivery identity rather than the user. The schema presented
/// it as a neutral cautious choice, so the agent picked it, manufactured a
/// block only the user could undo, and then walked into it. `immediate` is not
/// symmetric — it accelerates, its rubric is objective, and a wrong call costs
/// one early release — so it stays agent-decidable.
///
/// An unmandated `hold` is DROPPED, never turned into an error: refusing the
/// call would replace a self-inflicted release block with a self-inflicted
/// delivery block. Delivery proceeds on the ordinary cadence and the report
/// says the signal was ignored.
fn release_urgency_from_args(args: &Value) -> std::result::Result<Option<ReleaseUrgency>, String> {
    let Some(raw) = args.get("release_urgency") else {
        return Ok(None);
    };
    let Some(value) = raw.as_str() else {
        return Err("deliver_changes.release_urgency 必须是 immediate 或 hold".into());
    };
    let urgency = parse_release_urgency(value).ok_or_else(|| {
        format!(
            "无效的 deliver_changes.release_urgency: {value}; 只允许 immediate 或 hold，\
未执行任何交付动作。"
        )
    })?;
    if urgency == ReleaseUrgency::Hold && !user_mandated_hold(args) {
        tracing::info!(
            "dropping agent-initiated Release-Urgency: hold — only the user may gate a batch"
        );
        return Ok(None);
    }
    Ok(Some(urgency))
}

/// Did the USER ask for the hold? The caller must quote their instruction, so
/// the mandate is auditable in the commit rather than asserted by a boolean.
fn user_mandated_hold(args: &Value) -> bool {
    args.get("hold_requested_by_user")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|reason| !reason.is_empty())
}

#[cfg(test)]
mod tests {

    // 2026-08-05: five commits carried `Release-Urgency: hold`, all authored by
    // the delivery identity "CodeFactory" — the user never asked for any of
    // them. `hold` is the one genuine business gate in the pipeline: it blocks
    // the WHOLE batch, including other people's merges, and only a human can
    // clear it with allow_guarded_batch. An agent setting it on its own
    // manufactures a block only the user can undo, then walks into it. The tool
    // description read like a neutral safe choice, which is how it happened.
    //
    // `immediate` is not symmetric: it accelerates, its rubric is objective,
    // and a wrong call costs one early release. It stays agent-decidable.

    #[test]
    fn hold_requires_a_user_mandate_and_never_stops_the_delivery_without_one() {
        let unmandated = json!({"release_urgency": "hold"});
        let decided = release_urgency_from_args(&unmandated)
            .expect("an unmandated hold must not fail the delivery");
        assert!(
            decided.is_none(),
            "hold without a user mandate must be dropped, not applied"
        );

        let mandated = json!({
            "release_urgency": "hold",
            "hold_requested_by_user": "用户要求先别发，等文档就绪",
        });
        assert!(
            matches!(
                release_urgency_from_args(&mandated).expect("a mandated hold is valid"),
                Some(ReleaseUrgency::Hold)
            ),
            "an explicit user mandate still allows hold"
        );
    }

    #[test]
    fn immediate_stays_agent_decidable() {
        assert!(matches!(
            release_urgency_from_args(&json!({"release_urgency": "immediate"}))
                .expect("immediate needs no mandate"),
            Some(ReleaseUrgency::Immediate)
        ));
    }

    use super::*;
    use crate::agent::delivery::{DeliveryOutcome, RecoveryClass, StepResult};

    fn outcome(final_state: &str) -> DeliveryOutcome {
        DeliveryOutcome {
            steps: vec![StepResult {
                step: "ci".into(),
                status: if final_state == "blocked" {
                    "blocked"
                } else if final_state == "waiting" {
                    "waiting"
                } else {
                    "ok"
                }
                .into(),
                detail: "detail".into(),
            }],
            branch: Some("feature/x".into()),
            commit_sha: None,
            pr_url: None,
            pr_number: None,
            final_state: final_state.into(),
            stage: "ci".into(),
            code: if final_state == "blocked" {
                "delivery_ci_blocked"
            } else if final_state == "waiting" {
                "delivery_ci_waiting"
            } else {
                "delivery_ceiling_reached"
            }
            .into(),
            recoverable: matches!(final_state, "blocked" | "waiting"),
            recovery_class: if final_state == "blocked" {
                RecoveryClass::AgentActionRequired
            } else if final_state == "waiting" {
                RecoveryClass::WaitRetryable
            } else {
                RecoveryClass::None
            },
            retry_after_ms: (final_state == "waiting").then_some(30_000),
            next_action: matches!(final_state, "blocked" | "waiting")
                .then(|| "retry same PR".into()),
            reached_state: "local".into(),
            requested_ceiling: "through_release".into(),
            effective_ceiling: "through_release".into(),
            capability_gap: None,
            release_receipt: None,
            summary: "summary".into(),
        }
    }

    #[test]
    fn blocked_report_bans_claiming_foreign_delivery() {
        // A blocked delivery must tell the model that any PR/merge/release it
        // later observes in the repo was produced by other executors — the
        // 2026-07-16 session claimed Codex's release as its own.
        let report = render_report(&outcome("blocked"));
        assert!(report.contains("不得归因"));
        assert!(report.contains("其他执行器"));
    }

    #[test]
    fn delivered_report_carries_no_attribution_warning() {
        let report = render_report(&outcome("delivered"));
        assert!(!report.contains("不得归因"));
    }

    #[test]
    fn release_urgency_parser_accepts_only_the_governed_values() {
        assert_eq!(
            parse_release_urgency("immediate"),
            Some(ReleaseUrgency::Immediate)
        );
        assert_eq!(parse_release_urgency("hold"), Some(ReleaseUrgency::Hold));
        assert_eq!(parse_release_urgency("soon"), None);
        assert!(
            release_urgency_from_args(&json!({"release_urgency": "soon"}))
                .unwrap_err()
                .contains("未执行任何交付动作")
        );
        assert!(release_urgency_from_args(&json!({"release_urgency": 1})).is_err());
        assert_eq!(release_urgency_from_args(&json!({})).unwrap(), None);
    }

    #[test]
    fn delivery_defaults_to_autonomous_completion_unless_explicitly_disabled() {
        assert!(autonomous_completion_from_args(&json!({})));
        assert!(autonomous_completion_from_args(&json!({
            "autonomous_completion": true
        })));
        assert!(!autonomous_completion_from_args(&json!({
            "autonomous_completion": false
        })));
    }

    #[test]
    fn business_blocker_maps_to_blocked_tool_status() {
        let output = tool_output_for_outcome(&outcome("blocked"));
        assert_eq!(output.status, ToolExecutionStatus::Blocked);
        assert!(!output.is_error, "blocked is not a tool crash");
        let metadata = output.metadata.expect("delivery metadata");
        assert_eq!(metadata["recoverable"], true);
        assert_eq!(metadata["requested_ceiling"], "through_release");
        assert_eq!(metadata["effective_ceiling"], "through_release");
        assert_eq!(metadata["reached_state"], "local");
    }

    #[test]
    fn retryable_wait_has_a_distinct_tool_status_and_matching_report() {
        let output = tool_output_for_outcome(&outcome("waiting"));
        assert_eq!(output.status, ToolExecutionStatus::Waiting);
        assert!(!output.is_error);
        assert!(output.content.contains("交付结果: waiting"));
        assert!(output.content.contains("用户无需回复『继续』"));
        let metadata = output.metadata.expect("delivery metadata");
        assert_eq!(metadata["status"], "waiting");
        assert_eq!(metadata["recovery_class"], "wait_retryable");
        assert_eq!(metadata["retry_after_ms"], 30_000);
    }

    #[test]
    fn contradictory_delivery_contract_fails_closed() {
        let mut invalid = outcome("delivered");
        invalid.recoverable = true;
        invalid.recovery_class = RecoveryClass::WaitRetryable;
        invalid.retry_after_ms = Some(30_000);

        let output = tool_output_for_outcome(&invalid);

        assert_eq!(output.status, ToolExecutionStatus::Error);
        assert!(output.is_error);
        assert_eq!(
            output.metadata.expect("contract error metadata")["code"],
            "delivery_contract_invalid"
        );

        let mut zero_delay_wait = outcome("waiting");
        zero_delay_wait.retry_after_ms = Some(0);
        let output = tool_output_for_outcome(&zero_delay_wait);
        assert_eq!(output.status, ToolExecutionStatus::Error);
        assert!(output.is_error);
    }

    #[tokio::test]
    async fn session_delivery_reference_is_durable_and_replaced_by_latest_pr() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE session_delivery_refs (
                session_id TEXT PRIMARY KEY, branch TEXT NOT NULL,
                pr_number INTEGER NOT NULL, pr_url TEXT NOT NULL,
                commit_sha TEXT, updated_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut first = outcome("delivered");
        first.pr_number = Some(7);
        first.pr_url = Some("https://github.com/acme/repo/pull/7".into());
        first.commit_sha = Some("head-7".into());
        persist_delivery_ref(&pool, "session-1", &first)
            .await
            .unwrap();

        let mut latest = first.clone();
        latest.branch = Some("feature/latest".into());
        latest.pr_number = Some(8);
        latest.pr_url = Some("https://github.com/acme/repo/pull/8".into());
        latest.commit_sha = Some("head-8".into());
        persist_delivery_ref(&pool, "session-1", &latest)
            .await
            .unwrap();

        let row: (String, i64, String) = sqlx::query_as(
            "SELECT branch, pr_number, commit_sha FROM session_delivery_refs WHERE session_id = ?",
        )
        .bind("session-1")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, ("feature/latest".into(), 8, "head-8".into()));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_delivery_refs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    /// Gating `gh --watch` only works if the sanctioned alternative is
    /// discoverable. `through_ci_green` already waits for CI with backoff, but
    /// nothing said so — so the model reached for `--watch` and burned the
    /// quota (2026-08-03/04). Deny without an alternative just moves the bypass.
    #[test]
    fn the_schema_points_at_the_managed_way_to_wait_for_ci() {
        let schema = definition().function.parameters.to_string();
        assert!(schema.contains("through_ci_green"));
        assert!(
            schema.contains("WAIT for CI"),
            "the ceiling doc must name waiting as its purpose"
        );
        assert!(
            schema.contains("--watch"),
            "and must name the bypass it replaces: {schema}"
        );
    }

    #[test]
    fn startup_resumes_only_authorized_waiting_without_requiring_autonomous_flag() {
        let claimed = |status: &str, authorized: bool| delivery_run::ClaimedRecovery {
            run_id: "run".into(),
            workspace_path: "/workspace".into(),
            repo_identity: "repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            requested_ceiling: "through_release".into(),
            autonomous_completion: false,
            canonical_pr_number: Some(1),
            canonical_head_sha: Some("abc".into()),
            stage: "ci".into(),
            status: status.into(),
            wait_class: Some("wait_retryable".into()),
            next_action: Some("observe_ci".into()),
            next_action_authorized: authorized,
            failure_signature: None,
            stage_attempt: 0,
            progress_revision: 1,
            action: delivery_run::RecoveryAction::ObserveOnly,
        };

        assert!(should_resume_claimed_delivery(&claimed("waiting", true)));
        assert!(!should_resume_claimed_delivery(&claimed("waiting", false)));
        assert!(!should_resume_claimed_delivery(&claimed(
            "core_input_required",
            true
        )));
        assert!(!should_resume_claimed_delivery(&claimed(
            "needs_business_decision",
            true
        )));
    }
}
