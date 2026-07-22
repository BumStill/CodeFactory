// SPDX-License-Identifier: Apache-2.0
//! `deliver_changes` agent tool — the single call that carries code work
//! through git delivery (commit → push → PR → CI → merge → release) up to the
//! user-configured [`DeliveryCeiling`]. This is the capability whose absence
//! made the agent stall at a green build, re-listing the missing PR instead of
//! opening one. The model invokes this instead of hand-running git in bash.

use serde_json::{json, Value};

use super::{ExecCtx, ToolOutput};
use crate::agent::delivery::{self, DeliverOpts};
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
                    "ceiling": {
                        "type": "string",
                        "enum": ["off", "pr_only", "through_ci_green", "through_merge", "through_release"],
                        "description": "Optional per-call ceiling. Clamped to at most the user's configured ceiling — a call can lower, never raise it."
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
    let requested_ceiling = args
        .get("ceiling")
        .and_then(Value::as_str)
        .and_then(parse_ceiling);

    let opts = DeliverOpts {
        title,
        body,
        requested_ceiling,
        extra_excludes: settings.delivery_exclude_globs.clone(),
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

    Ok(ToolOutput::ok(render_report(&outcome)))
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
            "\n\n注意:本次交付已在上述步骤被阻断,你在本轮没有完成后续的 PR/合并/发布。\
即使之后查询发现仓库出现了新的合并或发布,那也是其他执行器(并行 agent 或自动化流水线)\
完成的,不得归因为你本次的交付动作;如实报告阻断原因和已完成到哪一步即可。",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::delivery::{DeliveryOutcome, StepResult};

    fn outcome(final_state: &str) -> DeliveryOutcome {
        DeliveryOutcome {
            steps: vec![StepResult {
                step: "ci".into(),
                status: if final_state == "blocked" { "blocked" } else { "ok" }.into(),
                detail: "detail".into(),
            }],
            branch: Some("feature/x".into()),
            commit_sha: None,
            pr_url: None,
            pr_number: None,
            final_state: final_state.into(),
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
}
