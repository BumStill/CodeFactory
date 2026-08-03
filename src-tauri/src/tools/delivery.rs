// SPDX-License-Identifier: Apache-2.0
//! `deliver_changes` agent tool — the single call that carries code work
//! through git delivery (commit → push → PR → CI → merge → release) up to the
//! user-configured [`DeliveryCeiling`]. This is the capability whose absence
//! made the agent stall at a green build, re-listing the missing PR instead of
//! opening one. The model invokes this instead of hand-running git in bash.

use serde_json::{json, Value};

#[cfg(test)]
use super::ToolExecutionStatus;
use super::{ExecCtx, ToolOutput};
use crate::agent::delivery::{self, DeliverOpts, ReleaseUrgency};
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
                merge, and release. Call this ONCE after tests pass to carry code work to done; \
                do NOT hand-run git in bash and do NOT stop at a green build to describe a missing \
                PR. Idempotent: safe to call again to resume. Returns the steps taken, the PR URL, \
                and a terminal state (delivered / blocked / noop)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Optional PR/commit title. Defaults to a message derived from the branch + changed files." },
                    "body":  { "type": "string", "description": "Optional PR body." },
                    "release_urgency": {
                        "type": "string",
                        "enum": ["immediate", "hold"],
                        "description": "Optional release-cadence signal. `immediate` is preserved in the final commit and requests the express lane; `hold` is preserved and blocks release until the whole batch is explicitly reviewed."
                    },
                    "ceiling": {
                        "type": "string",
                        "enum": ["off", "pr_only", "through_ci_green", "through_merge", "through_release"],
                        "description": "Optional per-call ceiling. Clamped to at most the user's configured ceiling — a call can lower, never raise it."
                    },
                    "expect_branch": {
                        "type": "string",
                        "description": "Optional guard: the branch you believe you are delivering. This tool has no branch argument — it delivers whatever branch the working directory is on. When resuming a specific PR, pass its head branch here; if the working directory is somewhere else the call stops before touching anything instead of opening a second PR for unrelated work."
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

    let remote = delivery::resolve_delivery_remote(&ctx.cwd, &settings);

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

    Ok(tool_output_for_outcome(&outcome))
}

fn tool_output_for_outcome(outcome: &delivery::DeliveryOutcome) -> ToolOutput {
    let report = render_report(outcome);
    let output = if outcome.final_state == "blocked" {
        ToolOutput::blocked(report)
    } else {
        ToolOutput::ok(report)
    };
    output.with_metadata(json!({
        "status": outcome.final_state,
        "stage": outcome.stage,
        "code": outcome.code,
        "recoverable": outcome.recoverable,
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
    if outcome.final_state == "blocked" {
        out.push_str(
            "\n\n注意:本次交付没有达到请求边界；只能报告上面明确列出的已完成步骤。\
即使之后查询发现仓库出现了新的合并或发布,那也是其他执行器(并行 agent 或自动化流水线)\
完成的,不得归因为你本次的交付动作;如实报告缺失能力、实际到达层级和恢复动作即可。",
        );
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

fn release_urgency_from_args(args: &Value) -> std::result::Result<Option<ReleaseUrgency>, String> {
    let Some(raw) = args.get("release_urgency") else {
        return Ok(None);
    };
    let Some(value) = raw.as_str() else {
        return Err("deliver_changes.release_urgency 必须是 immediate 或 hold".into());
    };
    parse_release_urgency(value).map(Some).ok_or_else(|| {
        format!(
            "无效的 deliver_changes.release_urgency: {value}; 只允许 immediate 或 hold，\
未执行任何交付动作。"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::delivery::{DeliveryOutcome, StepResult};

    fn outcome(final_state: &str) -> DeliveryOutcome {
        DeliveryOutcome {
            steps: vec![StepResult {
                step: "ci".into(),
                status: if final_state == "blocked" {
                    "blocked"
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
            } else {
                "delivery_ceiling_reached"
            }
            .into(),
            recoverable: final_state == "blocked",
            next_action: None,
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
}
