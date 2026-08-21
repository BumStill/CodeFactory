// SPDX-License-Identifier: Apache-2.0
//! `deliver_changes` agent tool — the single call that carries code work
//! through git delivery (commit → push → PR → CI → merge → release) up to the
//! user-configured [`DeliveryCeiling`]. This is the capability whose absence
//! made the agent stall at a green build, re-listing the missing PR instead of
//! opening one. The model invokes this instead of hand-running git in bash.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::future::Future;

#[cfg(test)]
use super::ToolExecutionStatus;
use super::{ExecCtx, ToolOutput};
use crate::agent::delivery::{
    self, DeliverOpts, DeliveryIdentitySnapshot, DeliveryMutationIntentToken,
    DeliveryMutationPermit, DeliveryMutationPermitVerifier, DeliveryRemote, MergeObservation,
    OpenPrObservation, ReleaseUrgency,
};
use crate::agent::delivery_run::{
    self, CoreInputRequest, DeliveryIdentityRevision, DeliveryObservation, NewDeliveryRun,
    ProcessIdentity,
};
use crate::agent::objective::ObjectiveStore;
use crate::config::settings::DeliveryCeiling;
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

const RECOVERY_SUPERVISOR_POLL_MS: u64 = 15_000;
const DELIVERY_LEASE_HEARTBEAT_MS: u64 = 20_000;
const DELIVERY_LEASE_TTL_MS: i64 = 90_000;

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

    let mut opts = DeliverOpts {
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
        expected_identity: None,
        mutation_permit: None,
    };

    let mut durable = match prepare_durable_run(
        &args,
        ctx,
        settings.delivery_ceiling,
        requested_ceiling,
    )
    .await
    {
        Ok(durable) => durable,
        Err(error) if error.to_string().contains("系统身份冲突") => {
            return Ok(ToolOutput::waiting(
                "交付目标身份不唯一；系统将只读核对 objective/worktree 映射，未执行任何交付副作用。",
            )
            .with_metadata(json!({
                "status": "recovering",
                "delivery_state": "waiting",
                "stage": "identity",
                "code": "delivery_identity_incident",
                "recoverable": true,
                "recovery_class": "agent_action_required",
                "decision_type": "system_owned",
                "requires_user_continue": false,
                "retry_after_ms": 1_000,
                "next_action": "只读核对当前 objective 所属 worktree/branch，再用 expect_branch 续接；不得询问用户选择技术身份。",
                "requested_ceiling": requested_ceiling.map(ceiling_label).unwrap_or_else(|| ceiling_label(settings.delivery_ceiling)),
                "reached_state": "local"
            })));
        }
        Err(error) if error.to_string().contains("active invocation") => {
            return Ok(ToolOutput::waiting(
                "同一 objective 已有一条交付执行持有 worktree lease；当前调用已附着等待，未并发执行 stage/commit/push。",
            )
            .with_metadata(json!({
                "status": "recovering",
                "delivery_state": "waiting",
                "stage": "lease",
                "code": "delivery_operation_already_running",
                "recoverable": true,
                "recovery_class": "agent_action_required",
                "decision_type": "system_owned",
                "requires_user_continue": false,
                "retry_after_ms": 1_000,
                "next_action": "attach_existing_delivery_run",
                "requested_ceiling": requested_ceiling.map(ceiling_label).unwrap_or_else(|| ceiling_label(settings.delivery_ceiling)),
                "reached_state": "local"
            })));
        }
        Err(error) => return Err(error),
    };
    if opts.expect_branch.is_none() {
        opts.expect_branch = durable.as_ref().map(|run| run.head_branch.clone());
    }
    opts.expected_identity = durable.as_ref().map(PreparedDurableRun::identity_snapshot);
    opts.mutation_permit = durable
        .as_ref()
        .and_then(|run| ctx.db.as_ref().map(|db| run.mutation_permit(db)));

    let remote = delivery::resolve_delivery_remote(&ctx.cwd, &settings);

    loop {
        let delivery_future = delivery::deliver(
            &ctx.cwd,
            settings.delivery_ceiling,
            settings.delivery_merge_method,
            settings.delivery_ci_timeout_secs,
            &opts,
            remote.as_ref(),
            None,
        );
        let outcome = if let (Some(db), Some(durable)) = (ctx.db.as_ref(), durable.as_ref()) {
            await_delivery_with_lease_heartbeat(db, durable, delivery_future).await?
        } else {
            delivery_future.await
        };

        if let (Some(db), Some(session_id)) = (ctx.db.as_ref(), ctx.session_id.as_deref()) {
            persist_delivery_ref(db, session_id, &outcome).await?;
        }
        if let (Some(db), Some(durable)) = (ctx.db.as_ref(), durable.as_mut()) {
            persist_durable_outcome(db, durable, &outcome).await?;
            opts.expected_identity = Some(durable.identity_snapshot());
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
    claim_epoch: i64,
    objective_id: String,
    workspace_path: std::path::PathBuf,
    worktree_identity: String,
    repo_identity: String,
    change_set_digest: String,
    expected_head_sha: String,
    head_branch: String,
}

impl PreparedDurableRun {
    fn identity_snapshot(&self) -> DeliveryIdentitySnapshot {
        DeliveryIdentitySnapshot {
            repo_identity: self.repo_identity.clone(),
            worktree_identity: self.worktree_identity.clone(),
            head_sha: self.expected_head_sha.clone(),
            change_set_digest: self.change_set_digest.clone(),
        }
    }

    fn mutation_permit(&self, db: &sqlx::SqlitePool) -> DeliveryMutationPermit {
        DeliveryMutationPermit::new(std::sync::Arc::new(DurableDeliveryMutationPermit {
            db: db.clone(),
            run_id: self.id.clone(),
            process: self.process.clone(),
            claim_epoch: self.claim_epoch,
        }))
    }
}

struct DurableDeliveryMutationPermit {
    db: sqlx::SqlitePool,
    run_id: String,
    process: ProcessIdentity,
    claim_epoch: i64,
}

#[async_trait::async_trait]
impl DeliveryMutationPermitVerifier for DurableDeliveryMutationPermit {
    async fn verify(&self, rung: &str) -> std::result::Result<(), String> {
        match delivery_run::verify_delivery_mutation_permit(
            &self.db,
            &self.run_id,
            &self.process,
            self.claim_epoch,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "run {} owner {} epoch {} is no longer authorized for {rung}",
                self.run_id, self.process.instance_id, self.claim_epoch
            )),
            Err(error) => Err(format!(
                "run {} epoch {} permit could not be verified before {rung}: {error}",
                self.run_id, self.claim_epoch
            )),
        }
    }

    async fn begin_external_mutation(
        &self,
        rung: &str,
        operation_key: &str,
        evidence: &str,
    ) -> std::result::Result<Option<DeliveryMutationIntentToken>, String> {
        let intent_id = uuid::Uuid::new_v4().to_string();
        let evidence = normalize_mutation_evidence(evidence);
        match delivery_run::begin_delivery_mutation_intent(
            &self.db,
            &intent_id,
            &self.run_id,
            &self.process,
            self.claim_epoch,
            rung,
            operation_key,
            Some(&evidence),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => Ok(Some(DeliveryMutationIntentToken {
                id: intent_id,
                rung: rung.to_string(),
                operation_key: operation_key.to_string(),
            })),
            Ok(false) => Err(format!(
                "run {} owner {} epoch {} could not durably begin external mutation {rung}; the effect was not dispatched",
                self.run_id, self.process.instance_id, self.claim_epoch
            )),
            Err(error) => Err(format!(
                "run {} epoch {} failed to persist external mutation intent {rung}: {error}; the effect was not dispatched",
                self.run_id, self.claim_epoch
            )),
        }
    }

    async fn commit_external_mutation(
        &self,
        intent: &DeliveryMutationIntentToken,
        evidence: &str,
    ) -> std::result::Result<(), String> {
        let evidence = normalize_mutation_evidence(evidence);
        match delivery_run::resolve_delivery_mutation_intent_committed(
            &self.db,
            &intent.id,
            &self.process,
            self.claim_epoch,
            Some(&evidence),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "external mutation {} completed but its durable intent {} could not be committed",
                intent.rung, intent.id
            )),
            Err(error) => Err(format!(
                "external mutation {} completed but durable intent settlement failed: {error}",
                intent.rung
            )),
        }
    }

    async fn mark_external_mutation_unknown(
        &self,
        intent: &DeliveryMutationIntentToken,
        detail: &str,
    ) -> std::result::Result<(), String> {
        let evidence = json!({
            "detail_digest": format!("sha256:{:x}", Sha256::digest(detail.as_bytes())),
            "classification": "external_result_uncertain",
        })
        .to_string();
        match delivery_run::mark_delivery_mutation_intent_unknown(
            &self.db,
            &intent.id,
            &self.process,
            self.claim_epoch,
            Some(&evidence),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "durable intent {} could not be marked unknown",
                intent.id
            )),
            Err(error) => Err(format!(
                "durable intent {} unknown settlement failed: {error}",
                intent.id
            )),
        }
    }
}

fn normalize_mutation_evidence(evidence: &str) -> String {
    serde_json::from_str::<serde_json::Value>(evidence)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| {
            json!({
                "detail_digest": format!("sha256:{:x}", Sha256::digest(evidence.as_bytes())),
            })
            .to_string()
        })
}

async fn drive_delivery_with_lease_heartbeat<F, T, Tick, TickFuture, Clock>(
    db: &sqlx::SqlitePool,
    run_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    operation: F,
    mut next_tick: Tick,
    mut clock: Clock,
    lease_ttl_ms: i64,
) -> Result<T>
where
    F: Future<Output = T>,
    Tick: FnMut() -> TickFuture,
    TickFuture: Future<Output = ()>,
    Clock: FnMut() -> i64,
{
    let initial_now = clock();
    if !delivery_run::renew_delivery_lease(
        db,
        run_id,
        process,
        claim_epoch,
        initial_now,
        lease_ttl_ms,
    )
    .await?
    {
        return Err(crate::errors::AppError::Other(
            "delivery lease was lost before the in-flight operation started; no new delivery operation was polled"
                .into(),
        ));
    }
    let mut last_confirmed_expiry = initial_now.saturating_add(lease_ttl_ms);

    tokio::pin!(operation);
    loop {
        tokio::select! {
            result = operation.as_mut() => return Ok(result),
            _ = next_tick() => {
                let heartbeat_now = clock();
                match delivery_run::renew_delivery_lease(
                    db,
                    run_id,
                    process,
                    claim_epoch,
                    heartbeat_now,
                    lease_ttl_ms,
                ).await {
                    Ok(true) => {
                        last_confirmed_expiry = heartbeat_now.saturating_add(lease_ttl_ms);
                    }
                    Ok(false) => {
                        tracing::error!(
                            run_id,
                            owner = %process.instance_id,
                            claim_epoch,
                            "delivery lease heartbeat lost ownership; dropping the stale operation before it can cross another mutation rung"
                        );
                        return Err(crate::errors::AppError::Other(
                            "delivery lease ownership changed while an operation was in flight; external state is uncertain and requires observe-only reconciliation"
                                .into(),
                        ));
                    }
                    Err(error) => {
                        // A transient SQLite failure may be retried only while the
                        // last positively-confirmed lease is still live. Once that
                        // boundary is crossed, continuing would let a competitor
                        // claim the run while this future can still mutate.
                        if heartbeat_now >= last_confirmed_expiry {
                            tracing::error!(run_id, claim_epoch, %error, "delivery lease became uncertain at expiry; dropping the stale operation for observe-only reconciliation");
                            return Err(crate::errors::AppError::Other(
                                "delivery lease could not be renewed before its confirmed expiry; external state is uncertain and requires observe-only reconciliation"
                                    .into(),
                            ));
                        }
                        tracing::warn!(run_id, claim_epoch, %error, last_confirmed_expiry, "delivery lease heartbeat failed transiently before confirmed expiry");
                    }
                }
            }
        }
    }
}

async fn await_delivery_with_lease_heartbeat<F, T>(
    db: &sqlx::SqlitePool,
    durable: &PreparedDurableRun,
    operation: F,
) -> Result<T>
where
    F: Future<Output = T>,
{
    drive_delivery_with_lease_heartbeat(
        db,
        &durable.id,
        &durable.process,
        durable.claim_epoch,
        operation,
        || {
            tokio::time::sleep(std::time::Duration::from_millis(
                DELIVERY_LEASE_HEARTBEAT_MS,
            ))
        },
        || chrono::Utc::now().timestamp_millis(),
        DELIVERY_LEASE_TTL_MS,
    )
    .await
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
    let expected_branch = args
        .get("expect_branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let (objective_id, task_segment_id) = durable_objective_identity(db, ctx).await?;
    if ctx.task_id.is_none() && crate::agent::execution_workspace::is_git_repository(&ctx.cwd) {
        crate::agent::execution_workspace::verify_objective_workspace(db, &objective_id, &ctx.cwd)
            .await
            .map_err(|error| {
                crate::errors::AppError::Other(format!(
                "系统身份冲突: deliver_changes cwd is not the Objective managed workspace: {error}"
            ))
            })?;
    }
    let (repo, _) = delivery::resolve_delivery_repo(&ctx.cwd, None, expected_branch)
        .map_err(crate::errors::AppError::Other)?;
    let identity =
        delivery::capture_delivery_identity(&repo).map_err(crate::errors::AppError::Other)?;
    let repo_identity = identity.repo_identity;
    let expected_head_sha = identity.head_sha;
    let change_set_digest = identity.change_set_digest;
    let worktree_identity = identity.worktree_identity;
    let id = durable_run_id(&objective_id, &repo_identity);
    let process = foreground_delivery_process_identity();
    let selected_ceiling = requested_ceiling
        .map(|requested| configured_ceiling.clamp_request(requested))
        .unwrap_or(configured_ceiling);
    let head_branch = repo.branch.clone();
    let run = NewDeliveryRun {
        id: id.clone(),
        objective_id,
        run_kind: "deliver_changes".into(),
        session_id: ctx.session_id.clone(),
        root_turn_id: ctx.root_turn_id.clone(),
        task_segment_id,
        task_id: ctx.task_id.clone(),
        workspace_path: repo.root.to_string_lossy().into_owned(),
        worktree_identity,
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
    let claim_epoch = delivery_run::create_delivery_run(
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
        claim_epoch,
        objective_id: run.objective_id,
        workspace_path: repo.root,
        worktree_identity: run.worktree_identity,
        repo_identity: run.repo_identity,
        change_set_digest: run.change_set_digest,
        expected_head_sha,
        head_branch,
    }))
}

fn foreground_delivery_process_identity() -> ProcessIdentity {
    ProcessIdentity::new(
        format!(
            "{}:{}:delivery:{}",
            std::process::id(),
            crate::storage::db::current_process_start_token()
                .unwrap_or_else(|| "unknown-start".into()),
            uuid::Uuid::new_v4(),
        ),
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    )
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
    durable: &mut PreparedDurableRun,
    outcome: &delivery::DeliveryOutcome,
) -> Result<()> {
    let core_input = core_input_request_for_outcome(outcome);
    let status = match outcome.final_state.as_str() {
        "delivered" | "noop" => "awaiting_completion_arbitration",
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
    let identity_revision = if expected_head_sha != durable.expected_head_sha {
        let (observed_repo, _) = delivery::resolve_delivery_repo(
            &durable.workspace_path,
            None,
            Some(&durable.head_branch),
        )
        .map_err(crate::errors::AppError::Other)?;
        let observed_identity = delivery::capture_delivery_identity(&observed_repo)
            .map_err(crate::errors::AppError::Other)?;
        if observed_identity.head_sha != expected_head_sha {
            return Err(crate::errors::AppError::Other(format!(
                "delivery outcome head {} does not match observed workspace head {}",
                expected_head_sha, observed_identity.head_sha
            )));
        }
        build_delivery_identity_revision(
            durable,
            &observed_identity.repo_identity,
            &observed_identity.worktree_identity,
            &observed_identity.head_sha,
            &observed_identity.change_set_digest,
        )?
    } else {
        None
    };
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
        identity_revision: identity_revision.clone(),
    };
    delivery_run::record_delivery_observation(
        db,
        &durable.id,
        &durable.process,
        durable.claim_epoch,
        &observation,
        chrono::Utc::now().timestamp_millis(),
        outcome
            .retry_after_ms
            .unwrap_or(30_000)
            .saturating_add(60_000) as i64,
    )
    .await?;
    if let Some(revision) = identity_revision {
        durable.expected_head_sha = revision.next_expected_head_sha;
        durable.change_set_digest = revision.next_change_set_digest;
    }
    Ok(())
}

fn build_delivery_identity_revision(
    durable: &PreparedDurableRun,
    observed_repo_identity: &str,
    observed_worktree_identity: &str,
    observed_head_sha: &str,
    observed_change_set_digest: &str,
) -> Result<Option<DeliveryIdentityRevision>> {
    if observed_repo_identity != durable.repo_identity
        || observed_worktree_identity != durable.worktree_identity
    {
        return Err(crate::errors::AppError::Other(
            "delivery identity revision observed a different repo/worktree; refused before persistence"
                .into(),
        ));
    }
    if observed_head_sha == durable.expected_head_sha {
        return Ok(None);
    }
    if observed_head_sha.is_empty() || observed_change_set_digest.is_empty() {
        return Err(crate::errors::AppError::Other(
            "delivery identity revision requires a resolved head and change-set digest".into(),
        ));
    }

    let mut revision = DeliveryIdentityRevision {
        receipt_id: String::new(),
        objective_id: durable.objective_id.clone(),
        repo_identity: durable.repo_identity.clone(),
        worktree_identity: durable.worktree_identity.clone(),
        previous_expected_head_sha: durable.expected_head_sha.clone(),
        previous_change_set_digest: durable.change_set_digest.clone(),
        next_expected_head_sha: observed_head_sha.into(),
        next_change_set_digest: observed_change_set_digest.into(),
    };
    revision.receipt_id =
        delivery_run::delivery_identity_revision_receipt_id(&durable.id, &revision);
    Ok(Some(revision))
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

async fn reconcile_unresolved_delivery_mutation_intents<R: DeliveryRemote>(
    db: &sqlx::SqlitePool,
    claimed: &delivery_run::ClaimedRecovery,
    process: &ProcessIdentity,
    remote: Option<&R>,
    takeover: &mut delivery::DeliveryTakeoverObservation,
) -> Result<bool> {
    let intents =
        delivery_run::list_unresolved_delivery_mutation_intents(db, &claimed.run_id).await?;
    // A settled release intent still proves that durable DB begin happened.
    // In particular, the old owner can commit the provider result and crash
    // before upgrading the local receipt from `intent_release`. Looking only
    // at unresolved intents would then authorize an unsafe second dispatch if
    // the provider's read API is briefly eventually consistent.
    let mut release_intent_seen =
        delivery_run_has_release_mutation_intent(db, &claimed.run_id).await?;
    for intent in intents {
        let confirmation = match intent.rung.as_str() {
            "git_push" => {
                if takeover.remote_head_sha.as_deref() != Some(claimed.expected_head_sha.as_str()) {
                    return Err(crate::errors::AppError::Other(format!(
                        "unresolved git push {} has no positive matching remote-head observation",
                        intent.intent_id
                    )));
                }
                let expected_key = delivery::external_operation_key(
                    "git_push",
                    &[
                        &claimed.repo_identity,
                        &claimed.head_branch,
                        &claimed.expected_head_sha,
                    ],
                );
                if expected_key != intent.operation_key {
                    return Err(crate::errors::AppError::Other(
                        "unresolved git push operation identity does not match the durable run"
                            .into(),
                    ));
                }
                json!({
                    "confirmation": "remote_head_matches",
                    "remote_head_sha": claimed.expected_head_sha,
                })
            }
            "provider_pr_create" | "provider_pr_open_or_get" => {
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "unresolved PR creation has no read-only provider observer".into(),
                    )
                })?;
                let state = match remote
                    .observe_open_pr(&claimed.head_branch, &claimed.base_branch)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    OpenPrObservation::Open(state) => state,
                    OpenPrObservation::Absent | OpenPrObservation::Unsupported => {
                        return Err(crate::errors::AppError::Other(
                            "unresolved PR creation is not positively visible yet; absence is not replay authority"
                                .into(),
                        ))
                    }
                };
                if takeover.remote_head_sha.as_deref() != Some(claimed.expected_head_sha.as_str())
                    && state.head_sha.as_deref() != Some(claimed.expected_head_sha.as_str())
                {
                    return Err(crate::errors::AppError::Other(
                        "observed PR does not prove the persisted delivery head".into(),
                    ));
                }
                let expected_key = delivery::external_operation_key(
                    &intent.rung,
                    &[
                        &state.pr.title,
                        &state.pr.body,
                        &claimed.head_branch,
                        &claimed.base_branch,
                    ],
                );
                if expected_key != intent.operation_key {
                    return Err(crate::errors::AppError::Other(
                        "observed PR title/body/head/base does not match the unresolved operation"
                            .into(),
                    ));
                }
                if takeover.canonical_pr_number.is_some()
                    && takeover.canonical_pr_number != Some(state.pr.number)
                {
                    return Err(crate::errors::AppError::Other(
                        "observed PR conflicts with the durable canonical PR number".into(),
                    ));
                }
                if takeover.canonical_pr_url.is_some()
                    && takeover.canonical_pr_url.as_deref() != Some(state.pr.url.as_str())
                {
                    return Err(crate::errors::AppError::Other(
                        "observed PR conflicts with the durable canonical PR URL".into(),
                    ));
                }
                takeover.canonical_pr_number = Some(state.pr.number);
                takeover.canonical_pr_url = Some(state.pr.url.clone());
                takeover.canonical_head_sha = Some(claimed.expected_head_sha.clone());
                json!({
                    "confirmation": "open_pr_matches",
                    "pr_number": state.pr.number,
                    "pr_url": state.pr.url,
                    "head_sha": claimed.expected_head_sha,
                })
            }
            "provider_pr_body_update" => {
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "unresolved PR body update has no read-only provider observer".into(),
                    )
                })?;
                let state = match remote
                    .observe_open_pr(&claimed.head_branch, &claimed.base_branch)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    OpenPrObservation::Open(state) => state,
                    OpenPrObservation::Absent | OpenPrObservation::Unsupported => {
                        return Err(crate::errors::AppError::Other(
                            "unresolved PR body update is not positively observable".into(),
                        ))
                    }
                };
                let number_text = state.pr.number.to_string();
                let expected_key = delivery::external_operation_key(
                    "provider_pr_body_update",
                    &[&number_text, &state.pr.body],
                );
                if expected_key != intent.operation_key
                    || claimed.canonical_pr_number != Some(state.pr.number as i64)
                {
                    return Err(crate::errors::AppError::Other(
                        "observed PR body does not match the unresolved update".into(),
                    ));
                }
                json!({
                    "confirmation": "pr_body_matches",
                    "pr_number": state.pr.number,
                    "body_digest": delivery::external_operation_key("body", &[&state.pr.body]),
                })
            }
            "provider_pr_merge" => {
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "unresolved PR merge has no read-only provider observer".into(),
                    )
                })?;
                let evidence: serde_json::Value = intent
                    .evidence_json
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok())
                    .ok_or_else(|| {
                        crate::errors::AppError::Other(
                            "unresolved PR merge lacks its durable operation envelope".into(),
                        )
                    })?;
                let number = evidence
                    .get("pr_number")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| {
                        crate::errors::AppError::Other(
                            "unresolved PR merge lacks its PR number".into(),
                        )
                    })?;
                let method = evidence
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let expected_head = evidence
                    .get("expected_head")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let number_text = number.to_string();
                let expected_key = delivery::external_operation_key(
                    "provider_pr_merge",
                    &[&number_text, method, expected_head],
                );
                if expected_key != intent.operation_key
                    || expected_head != claimed.expected_head_sha
                    || claimed.canonical_pr_number != Some(number as i64)
                {
                    return Err(crate::errors::AppError::Other(
                        "unresolved PR merge identity does not match the durable run".into(),
                    ));
                }
                match remote
                    .observe_merge(number, expected_head)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    MergeObservation::Merged { merge_sha } => json!({
                        "confirmation": "merge_observed",
                        "pr_number": number,
                        "merge_sha": merge_sha,
                    }),
                    MergeObservation::OpenSameHead { auto_merge: true } => json!({
                        "confirmation": "auto_merge_observed",
                        "pr_number": number,
                        "head_sha": expected_head,
                    }),
                    _ => {
                        return Err(crate::errors::AppError::Other(
                            "unresolved PR merge has no positive merged/queued observation".into(),
                        ))
                    }
                }
            }
            "provider_release_trigger" => {
                release_intent_seen = true;
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "unresolved release trigger has no read-only release observer".into(),
                    )
                })?;
                // `unknown` settlement records uncertainty detail and may
                // replace the row's current evidence. The immutable started
                // event retains the original operation envelope, so takeover
                // resolves identity from that ledger rather than losing the
                // workflow/ref/head boundary after an in-flight failure.
                let started_evidence =
                    started_delivery_mutation_evidence(db, &claimed.run_id, &intent.intent_id)
                        .await?;
                let target: delivery::ReleaseDispatchTarget = intent
                    .evidence_json
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok())
                    .or_else(|| {
                        started_evidence
                            .as_deref()
                            .and_then(|value| serde_json::from_str(value).ok())
                    })
                    .ok_or_else(|| {
                        crate::errors::AppError::Other(
                            "unresolved release trigger lacks an exact workflow/ref/head envelope"
                                .into(),
                        )
                    })?;
                if target.git_ref != claimed.base_branch
                    || target.workflow.trim().is_empty()
                    || target.head_sha.trim().is_empty()
                    || target.operation_key() != intent.operation_key
                {
                    return Err(crate::errors::AppError::Other(
                        "unresolved release trigger identity does not match its durable workflow/ref/head operation"
                            .into(),
                    ));
                }
                match remote
                    .observe_release_dispatch(&target)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    delivery::ReleaseDispatchObservation::Triggered {
                        run_id,
                        status,
                        head_sha,
                        detail,
                    } if head_sha == target.head_sha => json!({
                        "confirmation": "release_observed",
                        "workflow": target.workflow,
                        "git_ref": target.git_ref,
                        "head_sha": head_sha,
                        "run_id": run_id,
                        "status": status,
                        "detail_digest": format!("sha256:{:x}", Sha256::digest(detail.as_bytes())),
                    }),
                    delivery::ReleaseDispatchObservation::Absent => {
                        return Err(crate::errors::AppError::Other(
                            "unresolved release POST may still be in flight; exact absence is observation-only and never replay authority"
                                .into(),
                        ));
                    }
                    delivery::ReleaseDispatchObservation::Triggered { head_sha, .. } => {
                        return Err(crate::errors::AppError::Other(format!(
                            "observed release dispatch head {head_sha} does not match durable {}",
                            target.head_sha
                        )));
                    }
                    delivery::ReleaseDispatchObservation::HeadMismatch { observed_heads } => {
                        return Err(crate::errors::AppError::Other(format!(
                            "release workflow/ref has only nonmatching heads [{}]",
                            observed_heads.join(", ")
                        )));
                    }
                    delivery::ReleaseDispatchObservation::Unsupported(detail) => {
                        return Err(crate::errors::AppError::Other(format!(
                            "unresolved release trigger has no exact read-only observer: {detail}"
                        )));
                    }
                }
            }
            _ => {
                return Err(crate::errors::AppError::Other(format!(
                "unresolved delivery mutation rung {} has no safe positive reconciliation adapter",
                intent.rung
            )))
            }
        };
        let evidence = json!({
            "rung": intent.rung,
            "operation_key": intent.operation_key,
            "observation": confirmation,
        })
        .to_string();
        if !delivery_run::mark_delivery_mutation_intent_reconciled_committed(
            db,
            &intent.intent_id,
            process,
            claimed.claim_epoch,
            Some(&evidence),
            chrono::Utc::now().timestamp_millis(),
        )
        .await?
        {
            return Err(crate::errors::AppError::Other(format!(
                "delivery mutation intent {} lost its takeover epoch before reconciliation",
                intent.intent_id
            )));
        }
    }
    Ok(release_intent_seen)
}

async fn delivery_run_has_release_mutation_intent(
    db: &sqlx::SqlitePool,
    run_id: &str,
) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_mutation_intents
         WHERE run_id=? AND rung='provider_release_trigger'",
    )
    .bind(run_id)
    .fetch_one(db)
    .await?;
    Ok(count > 0)
}

async fn started_delivery_mutation_evidence(
    db: &sqlx::SqlitePool,
    run_id: &str,
    intent_id: &str,
) -> Result<Option<String>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT detail_json FROM delivery_run_events
         WHERE run_id=? AND event_kind='mutation_intent_started'
         ORDER BY created_at, id",
    )
    .bind(run_id)
    .fetch_all(db)
    .await?;
    for detail in rows {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&detail) else {
            continue;
        };
        if value.get("intent_id").and_then(serde_json::Value::as_str) != Some(intent_id) {
            continue;
        }
        return Ok(value.get("evidence").map(serde_json::Value::to_string));
    }
    Ok(None)
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
    let persisted_identity = DeliveryIdentitySnapshot {
        repo_identity: claimed.repo_identity.clone(),
        worktree_identity: claimed.worktree_identity.clone(),
        head_sha: claimed.expected_head_sha.clone(),
        change_set_digest: claimed.change_set_digest.clone(),
    };
    let mut opts = DeliverOpts {
        title: None,
        body: None,
        release_urgency: None,
        requested_ceiling: Some(requested_ceiling),
        extra_excludes: settings.delivery_exclude_globs.clone(),
        expect_branch: Some(claimed.head_branch.clone()),
        expected_identity: Some(DeliveryIdentitySnapshot {
            repo_identity: claimed.repo_identity.clone(),
            worktree_identity: claimed.worktree_identity.clone(),
            head_sha: claimed.expected_head_sha.clone(),
            change_set_digest: claimed.change_set_digest.clone(),
        }),
        mutation_permit: None,
    };
    let remote = delivery::resolve_delivery_remote(&cwd, &settings);
    let mut prepared = PreparedDurableRun {
        id: claimed.run_id.clone(),
        process: process.clone(),
        claim_epoch: claimed.claim_epoch,
        objective_id: claimed.objective_id.clone(),
        workspace_path: cwd.clone(),
        worktree_identity: claimed.worktree_identity.clone(),
        repo_identity: claimed.repo_identity.clone(),
        change_set_digest: claimed.change_set_digest.clone(),
        expected_head_sha: claimed.expected_head_sha.clone(),
        head_branch: claimed.head_branch.clone(),
    };

    let mut takeover = match delivery::observe_delivery_takeover(
        &cwd,
        Some(&claimed.base_branch),
        &claimed.head_branch,
        &persisted_identity,
        claimed.canonical_pr_number.map(|number| number as u64),
        claimed.canonical_pr_url.as_deref(),
        remote.as_ref(),
    )
    .await
    {
        Ok(observation) => observation,
        Err(error) => {
            let failure = DeliveryObservation {
                head_branch: claimed.head_branch.clone(),
                stage: "takeover_reconciliation".into(),
                status: "platform_incident".into(),
                wait_class: Some("external_state_uncertain".into()),
                next_action: Some("observe_only_reconcile".into()),
                reached_ceiling: claimed.reached_ceiling.clone(),
                expected_head_sha: claimed.expected_head_sha.clone(),
                canonical_pr_number: claimed.canonical_pr_number,
                canonical_pr_url: claimed.canonical_pr_url.clone(),
                canonical_head_sha: claimed.canonical_head_sha.clone(),
                failure_signature: Some(format!("takeover_reconciliation:{error}")),
                core_input: None,
                identity_revision: None,
            };
            let _ = delivery_run::record_delivery_observation(
                &db,
                &claimed.run_id,
                &process,
                claimed.claim_epoch,
                &failure,
                chrono::Utc::now().timestamp_millis(),
                DELIVERY_LEASE_TTL_MS,
            )
            .await;
            return Err(crate::errors::AppError::Other(format!(
                "delivery takeover remains observe-only: {error}"
            )));
        }
    };

    let release_db_intent_seen = match reconcile_unresolved_delivery_mutation_intents(
        &db,
        &claimed,
        &process,
        remote.as_ref(),
        &mut takeover,
    )
    .await
    {
        Ok(seen) => seen,
        Err(error) => {
            let failure = DeliveryObservation {
                head_branch: claimed.head_branch.clone(),
                stage: "takeover_mutation_reconciliation".into(),
                status: "platform_incident".into(),
                wait_class: Some("external_state_uncertain".into()),
                next_action: Some("observe_only_reconcile".into()),
                reached_ceiling: claimed.reached_ceiling.clone(),
                expected_head_sha: claimed.expected_head_sha.clone(),
                canonical_pr_number: claimed.canonical_pr_number,
                canonical_pr_url: claimed.canonical_pr_url.clone(),
                canonical_head_sha: claimed.canonical_head_sha.clone(),
                failure_signature: Some(format!("mutation_intent_reconciliation:{error}")),
                core_input: None,
                identity_revision: None,
            };
            let _ = delivery_run::record_delivery_observation(
                &db,
                &claimed.run_id,
                &process,
                claimed.claim_epoch,
                &failure,
                chrono::Utc::now().timestamp_millis(),
                DELIVERY_LEASE_TTL_MS,
            )
            .await;
            return Err(crate::errors::AppError::Other(format!(
                "delivery takeover remains observe-only: {error}"
            )));
        }
    };

    if let Err(error) = delivery::reconcile_local_release_intent(
        &cwd,
        Some(&claimed.base_branch),
        &claimed.head_branch,
        &claimed.expected_head_sha,
        !release_db_intent_seen,
        remote.as_ref(),
    )
    .await
    {
        let failure = DeliveryObservation {
            head_branch: claimed.head_branch.clone(),
            stage: "takeover_release_reconciliation".into(),
            status: "platform_incident".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            reached_ceiling: claimed.reached_ceiling.clone(),
            expected_head_sha: claimed.expected_head_sha.clone(),
            canonical_pr_number: claimed.canonical_pr_number,
            canonical_pr_url: claimed.canonical_pr_url.clone(),
            canonical_head_sha: claimed.canonical_head_sha.clone(),
            failure_signature: Some(format!("release_intent_reconciliation:{error}")),
            core_input: None,
            identity_revision: None,
        };
        let _ = delivery_run::record_delivery_observation(
            &db,
            &claimed.run_id,
            &process,
            claimed.claim_epoch,
            &failure,
            chrono::Utc::now().timestamp_millis(),
            DELIVERY_LEASE_TTL_MS,
        )
        .await;
        return Err(crate::errors::AppError::Other(format!(
            "delivery takeover remains observe-only: {error}"
        )));
    }

    let identity_revision = build_delivery_identity_revision(
        &prepared,
        &takeover.identity.repo_identity,
        &takeover.identity.worktree_identity,
        &takeover.identity.head_sha,
        &takeover.identity.change_set_digest,
    )?;
    let canonical_head_sha = takeover
        .canonical_head_sha
        .clone()
        .filter(|head| head == &takeover.identity.head_sha);
    let reconciled_observation = DeliveryObservation {
        head_branch: claimed.head_branch.clone(),
        stage: claimed.stage.clone(),
        status: claimed.status.clone(),
        wait_class: claimed.wait_class.clone(),
        next_action: claimed.next_action.clone(),
        reached_ceiling: claimed.reached_ceiling.clone(),
        expected_head_sha: takeover.identity.head_sha.clone(),
        canonical_pr_number: takeover.canonical_pr_number.map(|number| number as i64),
        canonical_pr_url: takeover.canonical_pr_url.clone(),
        canonical_head_sha,
        failure_signature: claimed.failure_signature.clone(),
        core_input: None,
        identity_revision: identity_revision.clone(),
    };
    delivery_run::record_delivery_observation(
        &db,
        &claimed.run_id,
        &process,
        claimed.claim_epoch,
        &reconciled_observation,
        chrono::Utc::now().timestamp_millis(),
        DELIVERY_LEASE_TTL_MS,
    )
    .await?;
    if let Some(revision) = identity_revision {
        prepared.expected_head_sha = revision.next_expected_head_sha;
        prepared.change_set_digest = revision.next_change_set_digest;
    }
    if !delivery_run::mark_delivery_claim_reconciled(
        &db,
        &claimed.run_id,
        &process,
        claimed.claim_epoch,
        chrono::Utc::now().timestamp_millis(),
    )
    .await?
    {
        return Err(crate::errors::AppError::Other(
            "delivery takeover lost its epoch before reconciliation completed".into(),
        ));
    }
    if claimed.status == "awaiting_completion_arbitration" {
        ObjectiveStore::new(db.clone())
            .settle_reconciled_delivery_after_takeover(
                &claimed.run_id,
                claimed.claim_epoch,
                &process.instance_id,
            )
            .await
            .map_err(|error| {
                crate::errors::AppError::Other(format!(
                    "delivery takeover could not settle its durable completion evidence: {error}"
                ))
            })?;
        return Ok(());
    }
    opts.expected_identity = Some(prepared.identity_snapshot());
    opts.mutation_permit = Some(prepared.mutation_permit(&db));
    loop {
        let delivery_future = delivery::deliver(
            &cwd,
            requested_ceiling,
            settings.delivery_merge_method,
            settings.delivery_ci_timeout_secs,
            &opts,
            remote.as_ref(),
            Some(&claimed.base_branch),
        );
        let outcome = await_delivery_with_lease_heartbeat(&db, &prepared, delivery_future).await?;
        persist_durable_outcome(&db, &mut prepared, &outcome).await?;
        opts.expected_identity = Some(prepared.identity_snapshot());
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
    matches!(
        claimed.status.as_str(),
        "waiting"
            | "platform_incident"
            | "agent_action_required"
            | "failed_internal"
            | "awaiting_completion_arbitration"
    ) && claimed.next_action_authorized
}

pub(crate) fn recovery_supervisor_poll_interval() -> std::time::Duration {
    std::time::Duration::from_millis(RECOVERY_SUPERVISOR_POLL_MS)
}

/// Continuously claims expired system-owned delivery work. A tool future can
/// disappear without a process restart (cancelled task, UI teardown, panic),
/// so a startup-only sweep is insufficient. The CAS lease in
/// `plan_startup_recovery` prevents sibling processes from owning the same run.
pub(crate) fn spawn_delivery_recovery_supervisor(
    pool: sqlx::SqlitePool,
    settings: crate::config::settings::Settings,
    process: ProcessIdentity,
) {
    tauri::async_runtime::spawn(async move {
        tracing::info!(
            poll_ms = RECOVERY_SUPERVISOR_POLL_MS,
            "recovery supervisor: started"
        );
        loop {
            let now = chrono::Utc::now().timestamp_millis();
            match delivery_run::plan_startup_recovery(&pool, &process, now, 60_000).await {
                Ok(plan) => {
                    if !plan.fail_closed_identity_missing.is_empty() {
                        tracing::warn!(
                            "recovery supervisor: left {} run(s) unclaimed because stable identity was incomplete",
                            plan.fail_closed_identity_missing.len()
                        );
                    }
                    for claimed in plan.claimed {
                        if !should_resume_claimed_delivery(&claimed) {
                            continue;
                        }
                        let recovery_db = pool.clone();
                        let recovery_settings = settings.clone();
                        let recovery_process = process.clone();
                        tauri::async_runtime::spawn(async move {
                            let run_id = claimed.run_id.clone();
                            if let Err(error) = resume_claimed_delivery(
                                recovery_db,
                                recovery_settings,
                                claimed,
                                recovery_process,
                            )
                            .await
                            {
                                tracing::warn!(
                                    "recovery supervisor: durable delivery run {run_id} could not resume: {error}"
                                );
                            }
                        });
                    }
                }
                Err(error) => tracing::warn!(
                    "recovery supervisor: lease planning failed (will retry): {error}"
                ),
            }
            tokio::time::sleep(recovery_supervisor_poll_interval()).await;
        }
    });
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

async fn durable_objective_identity(
    db: &sqlx::SqlitePool,
    ctx: &ExecCtx,
) -> Result<(String, Option<String>)> {
    if let Some(task_id) = ctx.task_id.as_deref().filter(|value| !value.is_empty()) {
        let objective_id = sqlx::query_scalar::<_, Option<String>>(
            "SELECT objective_id FROM task_runs WHERE id=?",
        )
        .bind(task_id)
        .fetch_optional(db)
        .await?
        .flatten()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::errors::AppError::Other(format!(
                "deliver_changes refused task {task_id} without a unified objective identity; legacy reconciliation is required"
            ))
        })?;
        return Ok((objective_id, None));
    }
    let root_turn_id = ctx
        .root_turn_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "deliver_changes refused external mutation without a durable objective identity"
                    .into(),
            )
        })?;
    let (objective_id, task_segment_id) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT objective_id, task_segment_id FROM chat_turn_state WHERE root_turn_id=?",
    )
    .bind(root_turn_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| {
        crate::errors::AppError::Other(format!(
            "deliver_changes refused unknown root turn {root_turn_id} without a durable objective identity"
        ))
    })?;
    let objective_id = objective_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::errors::AppError::Other(format!(
                "deliver_changes refused root turn {root_turn_id} without a unified objective identity; legacy reconciliation is required"
            ))
        })?;
    Ok((objective_id, task_segment_id))
}

fn durable_run_id(source_identity: &str, repo_identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_identity.as_bytes());
    hasher.update([0]);
    hasher.update(repo_identity.as_bytes());
    format!("delivery-{:x}", hasher.finalize())
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
    let system_owned_recovery = outcome.final_state == "blocked"
        && outcome.recoverable
        && outcome.recovery_class == delivery::RecoveryClass::AgentActionRequired;
    let report = render_report(outcome);
    let output = match outcome.final_state.as_str() {
        "waiting" => ToolOutput::waiting(report),
        "blocked" if system_owned_recovery => ToolOutput::waiting(report),
        "blocked" => ToolOutput::blocked(report),
        _ => ToolOutput::ok(report),
    };
    output.with_metadata(json!({
        "status": if system_owned_recovery { "recovering" } else { outcome.final_state.as_str() },
        "delivery_state": outcome.final_state,
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
    let system_owned_recovery = outcome.final_state == "blocked"
        && outcome.recoverable
        && outcome.recovery_class == delivery::RecoveryClass::AgentActionRequired;
    if system_owned_recovery {
        out.push_str("交付状态: recovering\n");
    } else {
        out.push_str(&format!("交付结果: {}\n", outcome.final_state));
    }
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
            "blocked" if system_owned_recovery => "↻",
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
                "\n\n这是系统负责的恢复阶段，不是用户阻断或最终总结边界。系统将执行 next_action，\
然后续接同一 PR；用户无需回复『继续』，且不得使用 --admin、force push 或删 required check。",
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

    struct LiveOnlyReleaseObserver {
        dispatches: std::sync::atomic::AtomicUsize,
        release_observation: std::sync::Mutex<Option<delivery::ReleaseDispatchObservation>>,
        observed_targets: std::sync::Mutex<Vec<delivery::ReleaseDispatchTarget>>,
    }

    impl delivery::DeliveryRemote for LiveOnlyReleaseObserver {
        fn capabilities(&self) -> delivery::DeliveryCapabilities {
            delivery::DeliveryCapabilities {
                review: true,
                ci: true,
                merge: true,
                release: true,
                live: true,
            }
        }

        async fn open_or_get_pr(
            &self,
            _title: &str,
            _body: &str,
            _head: &str,
            _base: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<delivery::DeliveryPr, String> {
            unreachable!("release reconciliation is read-only")
        }

        async fn ci_status(&self, _sha: &str) -> std::result::Result<delivery::CiStatus, String> {
            unreachable!("release reconciliation does not inspect CI")
        }

        async fn merge_pr(
            &self,
            _number: u64,
            _method: crate::config::settings::MergeMethod,
            _commit_message: Option<&delivery::MergeCommitMessage>,
            _expected_head: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<delivery::MergeRequestResult, String> {
            unreachable!("release reconciliation must never merge")
        }

        async fn trigger_release(
            &self,
            _head_sha: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<String, String> {
            self.dispatches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("unexpected dispatch".into())
        }

        async fn observe_release_dispatch(
            &self,
            target: &delivery::ReleaseDispatchTarget,
        ) -> std::result::Result<delivery::ReleaseDispatchObservation, String> {
            self.observed_targets.lock().unwrap().push(target.clone());
            Ok(self
                .release_observation
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(delivery::ReleaseDispatchObservation::Absent))
        }

        async fn verify_live(
            &self,
            _sha: &str,
            _url: Option<&str>,
        ) -> std::result::Result<delivery::ObservationStatus, String> {
            Ok(delivery::ObservationStatus::Success(
                "generic live endpoint happened to match".into(),
            ))
        }
    }

    #[test]
    fn foreground_delivery_lease_owner_is_unique_per_invocation() {
        let first = foreground_delivery_process_identity();
        let second = foreground_delivery_process_identity();
        assert_ne!(first.instance_id, second.instance_id);
        assert!(first.instance_id.contains(":delivery:"));
        assert!(second.instance_id.contains(":delivery:"));
    }

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
    fn system_owned_recovery_never_projects_as_a_blocked_tool() {
        let output = tool_output_for_outcome(&outcome("blocked"));
        assert_eq!(output.status, ToolExecutionStatus::Waiting);
        assert!(!output.is_error, "system-owned recovery is active work");
        assert!(output.content.contains("交付状态: recovering"));
        assert!(!output.content.contains("交付结果: blocked"));
        let metadata = output.metadata.expect("delivery metadata");
        assert_eq!(metadata["status"], "recovering");
        assert_eq!(metadata["delivery_state"], "blocked");
        assert_eq!(metadata["recoverable"], true);
        assert_eq!(metadata["decision_type"], "system_owned");
        assert_eq!(metadata["requires_user_continue"], false);
        assert_eq!(metadata["requested_ceiling"], "through_release");
        assert_eq!(metadata["effective_ceiling"], "through_release");
        assert_eq!(metadata["reached_state"], "local");
    }

    #[test]
    fn genuine_core_input_still_projects_as_blocked() {
        let mut core_input = outcome("blocked");
        core_input.recoverable = false;
        core_input.recovery_class = RecoveryClass::CoreInputRequired;
        let output = tool_output_for_outcome(&core_input);
        assert_eq!(output.status, ToolExecutionStatus::Blocked);
        assert_eq!(
            output.metadata.expect("delivery metadata")["decision_type"],
            "core_input_required"
        );
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

    #[tokio::test]
    async fn incomplete_or_noop_delivery_never_projects_business_completed() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let process = ProcessIdentity::new("process", "1.79.1", "17901");
        let run = NewDeliveryRun {
            id: "arbiter-run".into(),
            objective_id: "objective-opaque-arbiter".into(),
            run_kind: "chat_delivery".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("objective".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:arbiter".into(),
            repo_identity: "repo".into(),
            base_branch: "main".into(),
            head_branch: "feature/x".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "preflight".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("deliver".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let claim_epoch = delivery_run::create_delivery_run(
            &pool,
            &run,
            &process,
            chrono::Utc::now().timestamp_millis(),
            DELIVERY_LEASE_TTL_MS,
        )
        .await
        .unwrap();
        let mut prepared = PreparedDurableRun {
            id: run.id,
            process,
            claim_epoch,
            objective_id: run.objective_id,
            workspace_path: run.workspace_path.into(),
            worktree_identity: run.worktree_identity,
            repo_identity: run.repo_identity,
            change_set_digest: run.change_set_digest,
            expected_head_sha: "abc".into(),
            head_branch: "feature/x".into(),
        };

        let mut incomplete = outcome("delivered");
        incomplete.requested_ceiling = "through_release".into();
        incomplete.reached_state = "local".into();
        persist_durable_outcome(&pool, &mut prepared, &incomplete)
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM delivery_runs WHERE id='arbiter-run'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "awaiting_completion_arbitration");

        let mut evidence_candidate = outcome("delivered");
        evidence_candidate.requested_ceiling = "through_release".into();
        evidence_candidate.reached_state = "live_verified".into();
        evidence_candidate.commit_sha = Some("abc".into());
        evidence_candidate.pr_number = Some(42);
        evidence_candidate.pr_url = Some("https://example.invalid/pr/42".into());
        persist_durable_outcome(&pool, &mut prepared, &evidence_candidate)
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM delivery_runs WHERE id='arbiter-run'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            status, "awaiting_completion_arbitration",
            "DeliveryRun may project evidence but only CompletionArbiter may complete the Objective"
        );

        let mut noop = outcome("noop");
        noop.requested_ceiling = "through_release".into();
        noop.reached_state = "local".into();
        persist_durable_outcome(&pool, &mut prepared, &noop)
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM delivery_runs WHERE id='arbiter-run'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "awaiting_completion_arbitration");
    }

    #[tokio::test]
    async fn external_state_uncertainty_remains_system_owned_in_durable_projection() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let process = ProcessIdentity::new("process", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "external-uncertainty-run".into(),
            objective_id: "objective-opaque-external-uncertainty".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:external-uncertainty".into(),
            repo_identity: "repo:external-uncertainty".into(),
            base_branch: "main".into(),
            head_branch: "feature/x".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "delivery".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let now = chrono::Utc::now().timestamp_millis();
        let claim_epoch = delivery_run::create_delivery_run(&pool, &run, &process, now, 60_000)
            .await
            .unwrap();
        let mut prepared = PreparedDurableRun {
            id: run.id,
            process,
            claim_epoch,
            objective_id: run.objective_id,
            workspace_path: run.workspace_path.into(),
            worktree_identity: run.worktree_identity,
            repo_identity: run.repo_identity,
            change_set_digest: run.change_set_digest,
            expected_head_sha: run.expected_head_sha,
            head_branch: run.head_branch,
        };
        let mut uncertain = outcome("waiting");
        uncertain.recovery_class = RecoveryClass::ExternalStateUncertain;
        uncertain.code = "delivery_mutation_uncertain".into();
        uncertain.next_action = Some("observe_only_reconcile".into());

        persist_durable_outcome(&pool, &mut prepared, &uncertain)
            .await
            .unwrap();
        let (status, wait_class, next_action): (String, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT status, wait_class, next_action FROM delivery_runs
                 WHERE id='external-uncertainty-run'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "waiting");
        assert_eq!(wait_class.as_deref(), Some("external_state_uncertain"));
        assert_eq!(next_action.as_deref(), Some("observe_only_reconcile"));
    }

    #[test]
    fn identity_revision_receipt_binds_stable_repo_and_worktree() {
        let prepared = PreparedDurableRun {
            id: "run".into(),
            process: ProcessIdentity::new("process", "1.79.2", "17902"),
            claim_epoch: 1,
            objective_id: "objective-opaque-revision".into(),
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:a".into(),
            repo_identity: "repo:a".into(),
            change_set_digest: "digest-before".into(),
            expected_head_sha: "aaa".into(),
            head_branch: "feature".into(),
        };

        assert!(build_delivery_identity_revision(
            &prepared,
            "repo:a",
            "worktree:a",
            "aaa",
            "digest-before",
        )
        .unwrap()
        .is_none());
        assert!(build_delivery_identity_revision(
            &prepared,
            "repo:b",
            "worktree:a",
            "bbb",
            "digest-after",
        )
        .is_err());

        let receipt = build_delivery_identity_revision(
            &prepared,
            "repo:a",
            "worktree:a",
            "bbb",
            "digest-after",
        )
        .unwrap()
        .expect("a new observed head requires a receipt");
        assert_eq!(receipt.objective_id, "objective-opaque-revision");
        assert_eq!(receipt.previous_expected_head_sha, "aaa");
        assert_eq!(receipt.next_expected_head_sha, "bbb");
        assert_eq!(receipt.next_change_set_digest, "digest-after");
        assert!(receipt.receipt_id.starts_with("sha256:"));
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
    fn startup_resumes_authorized_system_owned_states_without_requiring_autonomous_flag() {
        let claimed = |status: &str, authorized: bool| delivery_run::ClaimedRecovery {
            run_id: "run".into(),
            claim_epoch: 1,
            objective_id: "objective-opaque-supervisor".into(),
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:run".into(),
            repo_identity: "repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            requested_ceiling: "through_release".into(),
            autonomous_completion: false,
            canonical_pr_number: Some(1),
            canonical_pr_url: Some("https://example.invalid/pr/1".into()),
            canonical_head_sha: Some("abc".into()),
            reached_ceiling: "pr_open".into(),
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
        assert!(should_resume_claimed_delivery(&claimed(
            "platform_incident",
            true
        )));
        assert!(should_resume_claimed_delivery(&claimed(
            "agent_action_required",
            true
        )));
        assert!(should_resume_claimed_delivery(&claimed(
            "failed_internal",
            true
        )));
        assert!(should_resume_claimed_delivery(&claimed(
            "awaiting_completion_arbitration",
            true
        )));
        assert!(!should_resume_claimed_delivery(&claimed(
            "platform_incident",
            false
        )));
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

    #[test]
    fn recovery_supervisor_reclaims_within_product_slo() {
        assert!(recovery_supervisor_poll_interval() <= std::time::Duration::from_secs(30));
    }

    #[tokio::test]
    async fn in_flight_heartbeat_prevents_competing_recovery_across_original_lease() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-competitor", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "heartbeat-wrapper-run".into(),
            objective_id: "objective-opaque-heartbeat".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:heartbeat".into(),
            repo_identity: "repo:heartbeat".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "delivery".into(),
            status: "waiting".into(),
            wait_class: Some("wait_retryable".into()),
            next_action: Some("observe_ci".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let claim_epoch = delivery_run::create_delivery_run(&pool, &run, &owner, 100, 10)
            .await
            .unwrap();

        let ticks = std::sync::Arc::new(tokio::sync::Notify::new());
        let clock = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from([
            100_i64, 120, 140,
        ])));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
        let heartbeat_task = {
            let pool = pool.clone();
            let owner = owner.clone();
            let ticks_for_driver = ticks.clone();
            let clock_for_driver = clock.clone();
            let calls = calls.clone();
            tokio::spawn(async move {
                drive_delivery_with_lease_heartbeat(
                    &pool,
                    "heartbeat-wrapper-run",
                    &owner,
                    claim_epoch,
                    async move {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        finish_rx.await.unwrap();
                        "finished"
                    },
                    move || {
                        let ticks = ticks_for_driver.clone();
                        async move { ticks.notified().await }
                    },
                    move || {
                        clock_for_driver
                            .lock()
                            .unwrap()
                            .pop_front()
                            .expect("one timestamp per heartbeat")
                    },
                    30,
                )
                .await
            })
        };

        async fn wait_for_lease(pool: &sqlx::SqlitePool, expected: i64) {
            for _ in 0..100 {
                let observed: i64 = sqlx::query_scalar(
                    "SELECT lease_expires_at FROM delivery_runs WHERE id='heartbeat-wrapper-run'",
                )
                .fetch_one(pool)
                .await
                .unwrap();
                if observed == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("heartbeat did not advance lease to {expected}");
        }

        wait_for_lease(&pool, 130).await;
        ticks.notify_one();
        wait_for_lease(&pool, 150).await;
        let first_competing_claim =
            delivery_run::plan_startup_recovery(&pool, &competitor, 140, 30)
                .await
                .unwrap();
        assert!(first_competing_claim.claimed.is_empty());

        ticks.notify_one();
        wait_for_lease(&pool, 170).await;
        let second_competing_claim =
            delivery_run::plan_startup_recovery(&pool, &competitor, 160, 30)
                .await
                .unwrap();
        assert!(second_competing_claim.claimed.is_empty());

        finish_tx.send(()).unwrap();
        assert_eq!(heartbeat_task.await.unwrap().unwrap(), "finished");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let mut revision = DeliveryIdentityRevision {
            receipt_id: String::new(),
            objective_id: run.objective_id.clone(),
            repo_identity: "repo:heartbeat".into(),
            worktree_identity: "worktree:heartbeat".into(),
            previous_expected_head_sha: "abc".into(),
            previous_change_set_digest: "digest".into(),
            next_expected_head_sha: "def".into(),
            next_change_set_digest: "digest-after".into(),
        };
        revision.receipt_id =
            delivery_run::delivery_identity_revision_receipt_id("heartbeat-wrapper-run", &revision);
        let observation = DeliveryObservation {
            head_branch: "feature".into(),
            stage: "commit".into(),
            status: "waiting".into(),
            wait_class: Some("wait_retryable".into()),
            next_action: Some("push".into()),
            reached_ceiling: "committed".into(),
            expected_head_sha: "def".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            failure_signature: None,
            core_input: None,
            identity_revision: Some(revision),
        };
        assert!(delivery_run::record_delivery_observation(
            &pool,
            "heartbeat-wrapper-run",
            &owner,
            claim_epoch,
            &observation,
            165,
            30,
        )
        .await
        .unwrap());
        let persisted_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_identity_revisions
             WHERE run_id='heartbeat-wrapper-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(persisted_receipts, 1);
    }

    #[tokio::test]
    async fn lost_heartbeat_drops_old_future_and_takeover_stays_observe_only() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-competitor", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "heartbeat-loss-run".into(),
            objective_id: "objective-opaque-heartbeat-loss".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:heartbeat-loss".into(),
            repo_identity: "repo:heartbeat-loss".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "delivery".into(),
            status: "waiting".into(),
            wait_class: Some("wait_retryable".into()),
            next_action: Some("observe_ci".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let owner_epoch = delivery_run::create_delivery_run(&pool, &run, &owner, 100, 10)
            .await
            .unwrap();
        let ticks = std::sync::Arc::new(tokio::sync::Notify::new());
        let clock = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from([
            100_i64, 132,
        ])));
        let mutations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (allow_mutation_tx, allow_mutation_rx) = tokio::sync::oneshot::channel::<()>();
        let task = {
            let pool = pool.clone();
            let owner = owner.clone();
            let ticks_for_driver = ticks.clone();
            let clock_for_driver = clock.clone();
            let mutations = mutations.clone();
            tokio::spawn(async move {
                drive_delivery_with_lease_heartbeat(
                    &pool,
                    "heartbeat-loss-run",
                    &owner,
                    owner_epoch,
                    async move {
                        let _ = allow_mutation_rx.await;
                        mutations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    },
                    move || {
                        let ticks = ticks_for_driver.clone();
                        async move { ticks.notified().await }
                    },
                    move || {
                        clock_for_driver
                            .lock()
                            .unwrap()
                            .pop_front()
                            .expect("one timestamp per heartbeat")
                    },
                    30,
                )
                .await
            })
        };

        for _ in 0..100 {
            let expires: i64 = sqlx::query_scalar(
                "SELECT lease_expires_at FROM delivery_runs WHERE id='heartbeat-loss-run'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if expires == 130 {
                break;
            }
            tokio::task::yield_now().await;
        }
        let takeover = delivery_run::plan_startup_recovery(&pool, &competitor, 131, 30)
            .await
            .unwrap()
            .claimed
            .pop()
            .expect("competitor claims the expired epoch");
        ticks.notify_one();
        let error = task.await.unwrap().expect_err("old owner must be dropped");
        assert!(error.to_string().contains("external state is uncertain"));
        assert!(
            allow_mutation_tx.send(()).is_err(),
            "old future was dropped"
        );
        assert_eq!(mutations.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!delivery_run::verify_delivery_mutation_permit(
            &pool,
            "heartbeat-loss-run",
            &competitor,
            takeover.claim_epoch,
            132,
        )
        .await
        .unwrap());
        assert!(delivery_run::mark_delivery_claim_reconciled(
            &pool,
            "heartbeat-loss-run",
            &competitor,
            takeover.claim_epoch,
            132,
        )
        .await
        .unwrap());
        assert!(delivery_run::verify_delivery_mutation_permit(
            &pool,
            "heartbeat-loss-run",
            &competitor,
            takeover.claim_epoch,
            132,
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn takeover_requires_and_consumes_positive_push_observation_before_new_permit() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-competitor", "1.79.2", "17902");
        let now = chrono::Utc::now().timestamp_millis();
        let run = NewDeliveryRun {
            id: "push-takeover-run".into(),
            objective_id: "objective-opaque-push-takeover".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:push-takeover".into(),
            repo_identity: "repo:push-takeover".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "push".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 60_000)
            .await
            .unwrap();
        let operation_key = delivery::external_operation_key(
            "git_push",
            &[&run.repo_identity, &run.head_branch, &run.expected_head_sha],
        );
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "push-intent",
            &run.id,
            &owner,
            epoch,
            "git_push",
            &operation_key,
            Some(r#"{"commit_sha":"abc"}"#),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::mark_delivery_mutation_intent_unknown(
            &pool,
            "push-intent",
            &owner,
            epoch,
            Some(r#"{"classification":"timeout"}"#),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();

        let plan = delivery_run::plan_startup_recovery(&pool, &competitor, now + 3, 60_000)
            .await
            .unwrap();
        let claimed = plan.claimed.into_iter().next().unwrap();
        let mut absent = delivery::DeliveryTakeoverObservation {
            identity: DeliveryIdentitySnapshot {
                repo_identity: run.repo_identity.clone(),
                worktree_identity: run.worktree_identity.clone(),
                head_sha: run.expected_head_sha.clone(),
                change_set_digest: run.change_set_digest.clone(),
            },
            remote_head_sha: None,
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
        };
        assert!(reconcile_unresolved_delivery_mutation_intents(
            &pool,
            &claimed,
            &competitor,
            None::<&delivery::EitherRemote>,
            &mut absent,
        )
        .await
        .is_err());
        assert!(!delivery_run::mark_delivery_claim_reconciled(
            &pool,
            &run.id,
            &competitor,
            claimed.claim_epoch,
            now + 4,
        )
        .await
        .unwrap());

        absent.remote_head_sha = Some("abc".into());
        reconcile_unresolved_delivery_mutation_intents(
            &pool,
            &claimed,
            &competitor,
            None::<&delivery::EitherRemote>,
            &mut absent,
        )
        .await
        .unwrap();
        assert!(delivery_run::mark_delivery_claim_reconciled(
            &pool,
            &run.id,
            &competitor,
            claimed.claim_epoch,
            now + 5,
        )
        .await
        .unwrap());
        assert!(delivery_run::verify_delivery_mutation_permit(
            &pool,
            &run.id,
            &competitor,
            claimed.claim_epoch,
            now + 6,
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn release_mutation_intent_rejects_nonmatching_workflow_ref_head_even_if_live_is_green() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("release-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("release-competitor", "1.79.2", "17902");
        let now = chrono::Utc::now().timestamp_millis();
        let run = NewDeliveryRun {
            id: "release-takeover-run".into(),
            objective_id: "objective-opaque-release-takeover".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:release-takeover".into(),
            repo_identity: "repo:release-takeover".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "expected-head".into(),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some("expected-head".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "merged".into(),
            stage: "release".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 60_000)
            .await
            .unwrap();
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "release-intent",
            &run.id,
            &owner,
            epoch,
            "provider_release_trigger",
            "sha256:not-the-exact-operation",
            Some(
                r#"{"workflow":"other-release.yml","git_ref":"other","expected_head":"other-head"}"#,
            ),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::mark_delivery_mutation_intent_unknown(
            &pool,
            "release-intent",
            &owner,
            epoch,
            Some(r#"{"classification":"post_in_flight"}"#),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();
        let claimed = delivery_run::plan_startup_recovery(&pool, &competitor, now + 3, 60_000)
            .await
            .unwrap()
            .claimed
            .into_iter()
            .next()
            .unwrap();
        let mut takeover = delivery::DeliveryTakeoverObservation {
            identity: DeliveryIdentitySnapshot {
                repo_identity: run.repo_identity.clone(),
                worktree_identity: run.worktree_identity.clone(),
                head_sha: run.expected_head_sha.clone(),
                change_set_digest: run.change_set_digest.clone(),
            },
            remote_head_sha: Some(run.expected_head_sha.clone()),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some(run.expected_head_sha.clone()),
        };
        let remote = LiveOnlyReleaseObserver {
            dispatches: std::sync::atomic::AtomicUsize::new(0),
            release_observation: std::sync::Mutex::new(None),
            observed_targets: std::sync::Mutex::new(Vec::new()),
        };

        reconcile_unresolved_delivery_mutation_intents(
            &pool,
            &claimed,
            &competitor,
            Some(&remote),
            &mut takeover,
        )
        .await
        .expect_err("generic live evidence must not reconcile a different workflow/ref/head");

        assert_eq!(
            remote.dispatches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_mutation_intents WHERE intent_id='release-intent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "unknown");
    }

    #[tokio::test]
    async fn in_flight_release_post_reconciles_only_the_exact_workflow_ref_head_without_redispatch()
    {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("release-owner-exact", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("release-competitor-exact", "1.79.2", "17902");
        let now = chrono::Utc::now().timestamp_millis();
        let run = NewDeliveryRun {
            id: "release-takeover-exact-run".into(),
            objective_id: "objective-opaque-release-exact".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:release-exact".into(),
            repo_identity: "repo:release-exact".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "delivery-feature-head".into(),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some("delivery-feature-head".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "merged".into(),
            stage: "release".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 60_000)
            .await
            .unwrap();
        let target = delivery::ReleaseDispatchTarget {
            workflow: "auto-release.yml".into(),
            git_ref: run.base_branch.clone(),
            head_sha: "release-main-head".into(),
        };
        let envelope = serde_json::to_string(&target).unwrap();
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "release-exact-intent",
            &run.id,
            &owner,
            epoch,
            "provider_release_trigger",
            &target.operation_key(),
            Some(&envelope),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::mark_delivery_mutation_intent_unknown(
            &pool,
            "release-exact-intent",
            &owner,
            epoch,
            Some(r#"{"classification":"post_in_flight"}"#),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();
        let claimed = delivery_run::plan_startup_recovery(&pool, &competitor, now + 3, 60_000)
            .await
            .unwrap()
            .claimed
            .into_iter()
            .next()
            .unwrap();
        let remote = LiveOnlyReleaseObserver {
            dispatches: std::sync::atomic::AtomicUsize::new(0),
            release_observation: std::sync::Mutex::new(Some(
                delivery::ReleaseDispatchObservation::Triggered {
                    run_id: "workflow-run-42".into(),
                    status: "in_progress".into(),
                    head_sha: target.head_sha.clone(),
                    detail: "https://example.invalid/actions/runs/42".into(),
                },
            )),
            observed_targets: std::sync::Mutex::new(Vec::new()),
        };
        let mut takeover = delivery::DeliveryTakeoverObservation {
            identity: DeliveryIdentitySnapshot {
                repo_identity: run.repo_identity.clone(),
                worktree_identity: run.worktree_identity.clone(),
                head_sha: run.expected_head_sha.clone(),
                change_set_digest: run.change_set_digest.clone(),
            },
            remote_head_sha: Some(run.expected_head_sha.clone()),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some(run.expected_head_sha.clone()),
        };

        reconcile_unresolved_delivery_mutation_intents(
            &pool,
            &claimed,
            &competitor,
            Some(&remote),
            &mut takeover,
        )
        .await
        .unwrap();

        assert_eq!(*remote.observed_targets.lock().unwrap(), vec![target]);
        assert_eq!(
            remote.dispatches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_mutation_intents WHERE intent_id='release-exact-intent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "reconciled_committed");
    }

    #[tokio::test]
    async fn committed_release_intent_still_fences_local_absence_replay() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("release-owner-committed", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("release-competitor-committed", "1.79.2", "17902");
        let now = chrono::Utc::now().timestamp_millis();
        let run = NewDeliveryRun {
            id: "release-takeover-committed-run".into(),
            objective_id: "objective-opaque-release-committed".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:release-committed".into(),
            repo_identity: "repo:release-committed".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "delivery-feature-head".into(),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some("delivery-feature-head".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "merged".into(),
            stage: "release".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 60_000)
            .await
            .unwrap();
        let target = delivery::ReleaseDispatchTarget {
            workflow: "auto-release.yml".into(),
            git_ref: run.base_branch.clone(),
            head_sha: "release-main-head".into(),
        };
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "release-committed-intent",
            &run.id,
            &owner,
            epoch,
            "provider_release_trigger",
            &target.operation_key(),
            Some(&serde_json::to_string(&target).unwrap()),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::resolve_delivery_mutation_intent_committed(
            &pool,
            "release-committed-intent",
            &owner,
            epoch,
            Some(r#"{"dispatch":"accepted"}"#),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();
        let claimed = delivery_run::plan_startup_recovery(&pool, &competitor, now + 3, 60_000)
            .await
            .unwrap()
            .claimed
            .into_iter()
            .next()
            .unwrap();
        let mut takeover = delivery::DeliveryTakeoverObservation {
            identity: DeliveryIdentitySnapshot {
                repo_identity: run.repo_identity.clone(),
                worktree_identity: run.worktree_identity.clone(),
                head_sha: run.expected_head_sha.clone(),
                change_set_digest: run.change_set_digest.clone(),
            },
            remote_head_sha: Some(run.expected_head_sha.clone()),
            canonical_pr_number: Some(7),
            canonical_pr_url: run.canonical_pr_url.clone(),
            canonical_head_sha: run.canonical_head_sha.clone(),
        };

        assert!(
            reconcile_unresolved_delivery_mutation_intents(
                &pool,
                &claimed,
                &competitor,
                None::<&delivery::EitherRemote>,
                &mut takeover,
            )
            .await
            .unwrap(),
            "a committed release dispatch still proves DB begin occurred and must fence local absence replay"
        );
    }

    #[tokio::test]
    async fn contextual_root_turns_share_one_durable_objective() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_task_segments (
                id TEXT PRIMARY KEY,
                previous_segment_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
                root_turn_id TEXT PRIMARY KEY,
                task_segment_id TEXT,
                objective_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_task_segments(id, previous_segment_id) VALUES
             ('segment-a', NULL), ('segment-b', 'segment-a')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, task_segment_id, objective_id) VALUES
             ('turn-a', 'segment-a', 'objective-opaque-uuid'),
             ('turn-b', 'segment-b', 'objective-opaque-uuid')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut first = ExecCtx::new(std::path::PathBuf::from("/tmp"), Some(pool.clone()));
        first.root_turn_id = Some("turn-a".into());
        let mut continuation = ExecCtx::new(std::path::PathBuf::from("/tmp"), Some(pool.clone()));
        continuation.root_turn_id = Some("turn-b".into());

        let first_identity = durable_objective_identity(&pool, &first).await.unwrap();
        let continuation_identity = durable_objective_identity(&pool, &continuation)
            .await
            .unwrap();
        assert_eq!(first_identity.0, "objective-opaque-uuid");
        assert_eq!(continuation_identity.0, first_identity.0);
        assert_eq!(continuation_identity.1.as_deref(), Some("segment-b"));
    }

    #[tokio::test]
    async fn delivery_refuses_legacy_string_fallback_without_unified_objective_binding() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
                root_turn_id TEXT PRIMARY KEY,
                task_segment_id TEXT,
                objective_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, task_segment_id, objective_id)
             VALUES ('legacy-turn', 'legacy-segment', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut ctx = ExecCtx::new(std::path::PathBuf::from("/tmp"), Some(pool.clone()));
        ctx.root_turn_id = Some("legacy-turn".into());

        let error = durable_objective_identity(&pool, &ctx).await.unwrap_err();
        assert!(error.to_string().contains("unified objective identity"));
    }

    #[tokio::test]
    async fn task_delivery_uses_the_unified_objective_binding() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                objective_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_runs(id, objective_id)
             VALUES ('task-run', 'task-objective-opaque-uuid')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut ctx = ExecCtx::new(std::path::PathBuf::from("/tmp"), Some(pool.clone()));
        ctx.task_id = Some("task-run".into());

        let identity = durable_objective_identity(&pool, &ctx).await.unwrap();
        assert_eq!(identity, ("task-objective-opaque-uuid".into(), None));
    }
}
