// SPDX-License-Identifier: Apache-2.0
//! The completion-gate mode policy (keystone slice 4.6, sub-step 1).
//!
//! These are the PURE finalization/recovery/tool-control decisions the loop
//! makes each round. Moved verbatim out of `agent/mod.rs` and re-parameterized
//! by [`FinalizationPolicy`] + `recovery_limit` + `wall_budget_applies` instead
//! of the desktop `AgentMode`, so the shared loop can drive them with no
//! desktop coupling. The desktop keeps thin `AgentMode`-taking wrappers that map
//! to these, so its call sites and the #135/#136 gate tests are unchanged and
//! pin behaviour byte-for-byte.
//!
//! Mapping (desktop): Interactive/Execute → `ReleaseWithWarning`, recovery 3,
//! `wall_budget_applies=false`; Autonomous →
//! `BlockOnIncomplete`, recovery 1, wall budget on. The `Benchmark` arm serves
//! the sidecar (4.8) and is never produced on the desktop path.

use crate::run::FinalizationPolicy;
use crate::types::{StreamEvent, ToolDefinition};
use codefactory_agent_core::{
    build_completion_recovery_prompt, classify_command,
    evaluate_budget_command_with_time_in_directory, CompletionEvidence, CompletionGate,
    PolicyDecision, ProgressTracker, ToolKind, ToolOutcome,
};
use std::path::Path;

/// Record one completed tool call against the completion gate + progress tracker
/// and return the progress-nudge prompt (if any). Moved out of the bin loop
/// (keystone slice 4.6b) with its `tools::ToolOutput` param flattened to
/// `(content, is_error)` — the only two fields it ever read — so it carries no
/// bin type. Timestamps stay 0 and `return_code` still derives from `is_error`.
#[allow(clippy::too_many_arguments)]
pub fn record_completion_outcome(
    gate: &mut CompletionGate,
    progress: &mut ProgressTracker,
    sequence: &mut u64,
    working_directory: &Path,
    request_id: &str,
    result: &crate::tool::ToolInvocationResult,
) -> CompletionRecord {
    *sequence += 1;
    // The BACKEND supplies `command`/`kind` and the real shell streams (keystone
    // slice 4.8c b2). Previously this synthesized them from `(tool_name, args)`
    // via `completion_command_and_kind`, which only calls `classify_command`
    // when `tool_name == "bash"` — fine for the desktop (whose backend now
    // applies exactly that rule), but it silently classified EVERY eval-sidecar
    // call as `ReadOnly` (its tool is named `run_shell`), so the gate would
    // never have seen a `Mutation`.
    let outcome = ToolOutcome {
        request_id: request_id.to_string(),
        command: result.command.clone(),
        working_directory: Some(working_directory.to_string_lossy().into_owned()),
        kind: result.kind.clone(),
        sequence: *sequence,
        started_at_ms: 0,
        finished_at_ms: 0,
        return_code: result
            .return_code
            .or(Some(if result.is_error { 1 } else { 0 })),
        stdout: if result.stdout.is_empty() {
            result.content.clone()
        } else {
            result.stdout.clone()
        },
        stderr: result.stderr.clone(),
        error: result
            .error
            .clone()
            .or_else(|| result.is_error.then(|| result.content.clone())),
        semantic_failure: false,
    }
    .with_detected_semantic_failure();
    let succeeded = outcome.succeeded();
    gate.record(&outcome);
    CompletionRecord {
        progress_prompt: progress.record(&outcome),
        succeeded,
    }
}

#[derive(Debug)]
pub struct CompletionRecord {
    pub progress_prompt: Option<String>,
    pub succeeded: bool,
}

/// Cache only deterministic local test/build commands. Remote observations,
/// runtime probes, and broad shell assertions can change without a workspace
/// mutation and must always execute.
pub fn reusable_local_verification_key(
    command: &str,
    kind: &ToolKind,
    working_directory: &Path,
) -> Option<String> {
    if !matches!(kind, ToolKind::Verification) {
        return None;
    }
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = normalized.to_ascii_lowercase();
    let deterministic_local = [
        "pytest",
        "unittest",
        "vitest",
        "jest",
        "playwright test",
        "tsc --noemit",
        "tsc -p",
        "tsc -b",
        "cargo check",
        "cargo build",
        "cargo test",
        "npm run build",
        "npm run lint",
        "npm test",
        "pnpm build",
        "pnpm lint",
        "pnpm test",
        "yarn build",
        "yarn test",
        "bun test",
        "make check",
        "make test",
        "ctest",
        "go test",
        "mvn test",
        "gradle test",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let dynamic_observation = [
        "gh ",
        "github.com",
        "http://",
        "https://",
        "curl ",
        "wget ",
        "sleep ",
        "while ",
        "until ",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    (deterministic_local && !dynamic_observation).then(|| {
        format!(
            "{}\n{}",
            working_directory.to_string_lossy(),
            normalized.trim()
        )
    })
}

/// The four-way finalization decision for a tool-call-free "final" response.
#[derive(Debug, PartialEq, Eq)]
pub enum CompletionFinalization {
    Complete,
    Recover(String),
    /// Chat surfaces after recovery exhaustion: the reply is the best available
    /// answer — release it, but persist a human-readable warning. Never an Error,
    /// never internal-contract wording (2026-07-21 field report).
    ReleaseWithWarning(String),
    /// Unattended Autonomous runs only — the scheduler treats this as an
    /// incomplete attempt and respawns.
    Blocked(String),
}

pub fn completion_finalization(
    evidence: &CompletionEvidence,
    attempts: u32,
    policy: FinalizationPolicy,
    recovery_limit: u32,
) -> CompletionFinalization {
    if evidence.completed {
        return CompletionFinalization::Complete;
    }
    if attempts < recovery_limit {
        return CompletionFinalization::Recover(build_completion_recovery_prompt(evidence));
    }
    match policy {
        FinalizationPolicy::BlockOnIncomplete => {
            CompletionFinalization::Blocked(completion_blocked_message(evidence))
        }
        FinalizationPolicy::ReleaseWithWarning => {
            CompletionFinalization::ReleaseWithWarning(unverified_release_warning(evidence))
        }
        // The sidecar's 2-way completed/recovery branch: release the final text
        // with neither amber warning nor Error. Provisional (refined when the
        // sidecar joins in 4.8); never produced on the desktop path.
        FinalizationPolicy::Benchmark => CompletionFinalization::Complete,
    }
}

/// User-facing warning when a chat turn ends without complete verification.
/// Chinese, plain language, no gate terminology; the raw blocker list goes to
/// the log only.
pub fn unverified_release_warning(evidence: &CompletionEvidence) -> String {
    tracing::info!(
        "releasing chat turn with unverified blockers: {}",
        evidence.blockers.join("; ")
    );
    "⚠ 以上回复未经完整验证:本轮修改后仍有检查未复验(或失败未复跑)。\
结论可能不完整;回复「继续验证」可让我补齐。"
        .to_string()
}

pub fn completion_blocked_message(evidence: &CompletionEvidence) -> String {
    format!(
        "Completion blocked because required verification is still missing: {}",
        evidence.blockers.join("; ")
    )
}

pub fn iteration_ceiling_terminal_event(
    evidence: &CompletionEvidence,
    policy: FinalizationPolicy,
) -> StreamEvent {
    // Error only when the run must block on incompleteness (Autonomous); every
    // other policy ends the ceiling-hit turn with a plain Done.
    if evidence.completed || !matches!(policy, FinalizationPolicy::BlockOnIncomplete) {
        StreamEvent::Done {
            input_tokens: 0,
            output_tokens: 0,
        }
    } else {
        StreamEvent::Error {
            message: completion_blocked_message(evidence),
        }
    }
}

/// The per-segment round count remains an internal checkpoint cadence, never a
/// task-level completion boundary for chat. Two full segments without material
/// completion-evidence progress are enough to stop an accidental tool loop,
/// while a productive run automatically receives another segment.
pub const MAX_STALLED_CHAT_SEGMENTS: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentCheckpointDecision {
    /// Completion evidence is satisfied; the tools-disabled checkpoint response
    /// becomes the final user-facing answer.
    Complete,
    /// Persist the checkpoint summary and automatically open the next segment.
    Continue,
    /// Stop a demonstrably stalled loop with a visible, resumable notice.
    Pause(String),
    /// Non-chat policies retain their existing terminal ceiling semantics.
    Terminal,
}

pub fn segment_checkpoint_decision(
    evidence: &CompletionEvidence,
    policy: FinalizationPolicy,
    material_progress: bool,
    stalled_segments_before: u32,
) -> SegmentCheckpointDecision {
    if evidence.completed {
        return SegmentCheckpointDecision::Complete;
    }
    if !matches!(policy, FinalizationPolicy::ReleaseWithWarning) {
        return SegmentCheckpointDecision::Terminal;
    }
    if material_progress || stalled_segments_before + 1 < MAX_STALLED_CHAT_SEGMENTS {
        return SegmentCheckpointDecision::Continue;
    }
    SegmentCheckpointDecision::Pause(
        "连续两个执行段未取得可验证进展，已停止自动重试以避免原地循环。\
当前进度已保存；修正阻塞条件后回复「继续执行」即可从这里恢复。"
            .to_string(),
    )
}

/// Force one tools-disabled response at a segment checkpoint. It is a natural
/// assistant progress update, not an internal status card, and is persisted as
/// ordinary assistant history before the next segment starts.
pub fn segment_checkpoint_summary_prompt(evidence: &CompletionEvidence) -> String {
    let blockers = if evidence.blockers.is_empty() {
        "尚无结构化 blocker；请根据刚完成的工具结果判断下一步。".to_string()
    } else {
        evidence.blockers.join("; ")
    };
    format!(
        "这是内部连续执行检查点，不是任务轮次上限，也不是让用户接手。\
请用简洁自然的对话说明已经完成的具体进展、当前验证状态和紧接着要做的动作。\
当前结构化阻塞：{blockers}。\
除非验收条件确实已经满足，否则不得宣称任务完成、不得要求用户回复继续；\
明确说明你将自动继续执行。此轮禁用工具，只输出进度总结。"
    )
}

pub fn completion_recovery_prompt(
    evidence: &CompletionEvidence,
    attempts: u32,
    policy: FinalizationPolicy,
    recovery_limit: u32,
) -> Option<String> {
    match completion_finalization(evidence, attempts, policy, recovery_limit) {
        CompletionFinalization::Recover(prompt) => Some(prompt),
        CompletionFinalization::Complete
        | CompletionFinalization::ReleaseWithWarning(_)
        | CompletionFinalization::Blocked(_) => None,
    }
}

pub fn completion_recovery_attempts_after_tool_batch(
    attempts: u32,
    _material_evidence_progress: bool,
) -> u32 {
    // This is a total turn budget, not a "consecutive no progress" counter.
    // Material progress may clear stagnation heuristics, but it must not grant a
    // fresh set of rejected-final-response recovery rounds.
    attempts
}

/// Whether the completion-ready coverage-audit nudge applies. Autonomous +
/// Benchmark only (never the chat ReleaseWithWarning surface).
pub fn completion_ready_applies(policy: FinalizationPolicy) -> bool {
    !matches!(policy, FinalizationPolicy::ReleaseWithWarning)
}

pub fn openai_tool_controls(
    tool_defs: &[ToolDefinition],
    require_tool: bool,
) -> (Option<Vec<ToolDefinition>>, serde_json::Value) {
    if tool_defs.is_empty() {
        (None, serde_json::json!("none"))
    } else {
        (
            Some(tool_defs.to_vec()),
            serde_json::json!(if require_tool { "required" } else { "auto" }),
        )
    }
}

pub fn active_tool_definitions(
    tool_defs: &[ToolDefinition],
    finalization_pending: bool,
) -> &[ToolDefinition] {
    if finalization_pending {
        &[]
    } else {
        tool_defs
    }
}

pub fn completion_command_and_kind(
    tool_name: &str,
    args: &serde_json::Value,
) -> (String, ToolKind) {
    let command = args
        .get("command")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .or_else(|| {
            let pattern = args
                .get("pattern")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let path = args
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let base = match (pattern.is_empty(), path.is_empty()) {
                (false, false) => Some(format!("{tool_name} {pattern} {path}")),
                (false, true) => Some(format!("{tool_name} {pattern} .")),
                (true, false) => Some(format!("{tool_name} {path}")),
                (true, true) => None,
            }?;
            let glob = args
                .get("glob")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty());
            Some(match glob {
                Some(glob) => format!("{base} --glob {glob}"),
                None => base,
            })
        })
        .unwrap_or_else(|| tool_name.to_owned());
    let kind = if tool_name == "bash" {
        classify_command(&command, 300_000)
    } else if tool_name.starts_with("write_")
        || tool_name.starts_with("edit_")
        || matches!(tool_name, "write_file" | "edit_file" | "delegate_tasks")
    {
        ToolKind::Mutation
    } else {
        ToolKind::ReadOnly
    };
    (command, kind)
}

/// A completion-policy denial, kept STRUCTURED so each surface can word it its
/// own way (keystone slice 4.8c b4) — the desktop's user-facing sentence and the
/// eval sidecar's `policy denied command ({rule}): {reason}` are different
/// contracts. Formatting happens in `PermissionGateway::format_budget_denial`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetDenial {
    pub rule: String,
    pub reason: String,
}

#[allow(clippy::too_many_arguments)]
pub fn autonomous_budget_denial(
    wall_budget_applies: bool,
    remaining_model_rounds: u32,
    // `(remaining, total)` seconds — `None` on surfaces without a wall clock,
    // which is exactly what the old call passed (slice 4.8c b3).
    wall_time: Option<(u64, u64)>,
    evidence: &CompletionEvidence,
    // Pre-classified by the ToolBackend (slice 4.8c b5) — see
    // `ToolBackend::classify`. Deriving it here would re-introduce the
    // `tool_name == "bash"` trap for surfaces whose shell tool has another name.
    command: &str,
    kind: &ToolKind,
    working_directory: &Path,
) -> Option<BudgetDenial> {
    // Interactive chat (wall budget off) is not constrained by the round budget,
    // but deterministic completion invariants still apply to model tools.
    let effective_remaining = if !wall_budget_applies {
        u32::MAX
    } else {
        remaining_model_rounds
    };
    match evaluate_budget_command_with_time_in_directory(
        effective_remaining,
        wall_time,
        evidence,
        command,
        kind,
        working_directory.to_str(),
    ) {
        PolicyDecision::Allow => None,
        PolicyDecision::Deny { rule, reason } => Some(BudgetDenial { rule, reason }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_budget_denies_only_exhausted_read_only_calls() {
        // Fires only when the allowance is spent AND the call is ReadOnly.
        assert!(inspection_budget_denial(false, false, &ToolKind::ReadOnly).is_none());
        assert!(inspection_budget_denial(true, false, &ToolKind::Mutation).is_none());
        let d = inspection_budget_denial(true, false, &ToolKind::ReadOnly)
            .expect("exhausted + read-only denies");
        assert_eq!(d.rule, "inspection_budget");
        assert!(d.reason.contains("initial inspection is exhausted"));
        // The reason switches once a mutation has been seen.
        let d = inspection_budget_denial(true, true, &ToolKind::ReadOnly).unwrap();
        assert!(d.reason.contains("post-change inspection is exhausted"));
    }

    #[test]
    fn the_default_classifier_is_bash_only_which_is_why_backends_override_it() {
        // Pins the trap this seam exists for: the DEFAULT rule classifies a
        // shell tool named anything other than `bash` as ReadOnly. A surface
        // whose tool is `run_shell` MUST override `ToolBackend::classify`, or
        // every call reads ReadOnly and inspection_budget_denial fires on all
        // of them.
        let args = serde_json::json!({"command": "rm -rf build"});
        let (_, bash_kind) = completion_command_and_kind("bash", &args);
        let (_, other_kind) = completion_command_and_kind("run_shell", &args);
        assert!(
            !matches!(bash_kind, ToolKind::ReadOnly),
            "bash is classified"
        );
        assert!(
            matches!(other_kind, ToolKind::ReadOnly),
            "non-bash falls back to ReadOnly — the reason ToolBackend::classify is overridable"
        );
    }

    #[test]
    fn only_deterministic_local_checks_receive_reuse_keys() {
        let cwd = Path::new("/workspace");
        let cargo = reusable_local_verification_key(
            "  cargo   test -p codefactory-agent-loop ",
            &ToolKind::Verification,
            cwd,
        )
        .expect("local cargo test is repeatable");
        assert_eq!(cargo, "/workspace\ncargo test -p codefactory-agent-loop");
        assert!(
            reusable_local_verification_key(
                "gh pr checks 123 --watch",
                &ToolKind::Verification,
                cwd,
            )
            .is_none(),
            "remote observations change without a workspace mutation",
        );
        assert!(
            reusable_local_verification_key(
                "curl --max-time 2 http://localhost:3000/health",
                &ToolKind::FunctionalProbe { bounded: true },
                cwd,
            )
            .is_none(),
            "runtime/functional probes must always execute",
        );
        assert_ne!(
            reusable_local_verification_key(
                "pnpm test",
                &ToolKind::Verification,
                Path::new("/workspace-a"),
            ),
            reusable_local_verification_key(
                "pnpm test",
                &ToolKind::Verification,
                Path::new("/workspace-b"),
            ),
        );
    }

    fn evidence(completed: bool, blockers: &[&str]) -> CompletionEvidence {
        let mut e = CompletionEvidence::default();
        e.completed = completed;
        e.blockers = blockers.iter().map(|s| s.to_string()).collect();
        e
    }

    #[test]
    fn finalization_completes_when_evidence_is_done() {
        assert_eq!(
            completion_finalization(
                &evidence(true, &[]),
                0,
                FinalizationPolicy::ReleaseWithWarning,
                3
            ),
            CompletionFinalization::Complete
        );
    }

    #[test]
    fn finalization_recovers_under_the_limit_then_diverges_by_policy() {
        // Under the recovery limit → Recover regardless of policy.
        assert!(matches!(
            completion_finalization(
                &evidence(false, &["x"]),
                0,
                FinalizationPolicy::BlockOnIncomplete,
                1
            ),
            CompletionFinalization::Recover(_)
        ));
        // At the limit: BlockOnIncomplete → Blocked, ReleaseWithWarning → warning.
        assert!(matches!(
            completion_finalization(
                &evidence(false, &["x"]),
                1,
                FinalizationPolicy::BlockOnIncomplete,
                1
            ),
            CompletionFinalization::Blocked(_)
        ));
        assert!(matches!(
            completion_finalization(
                &evidence(false, &["x"]),
                3,
                FinalizationPolicy::ReleaseWithWarning,
                3
            ),
            CompletionFinalization::ReleaseWithWarning(_)
        ));
    }

    #[test]
    fn ceiling_errors_only_when_blocking_on_incomplete() {
        assert!(matches!(
            iteration_ceiling_terminal_event(
                &evidence(false, &["x"]),
                FinalizationPolicy::BlockOnIncomplete
            ),
            StreamEvent::Error { .. }
        ));
        assert!(matches!(
            iteration_ceiling_terminal_event(
                &evidence(false, &["x"]),
                FinalizationPolicy::ReleaseWithWarning
            ),
            StreamEvent::Done { .. }
        ));
    }

    #[test]
    fn chat_checkpoint_is_an_automatic_continuation_not_a_terminal_done() {
        let decision = segment_checkpoint_decision(
            &evidence(false, &["verification still pending"]),
            FinalizationPolicy::ReleaseWithWarning,
            true,
            0,
        );
        assert_eq!(decision, SegmentCheckpointDecision::Continue);

        let summary_prompt =
            segment_checkpoint_summary_prompt(&evidence(false, &["verification still pending"]));
        assert!(summary_prompt.contains("自动继续"));
        assert!(summary_prompt.contains("不得宣称任务完成"));
    }

    #[test]
    fn chat_checkpoint_only_pauses_after_repeated_no_progress() {
        let decision = segment_checkpoint_decision(
            &evidence(false, &["same blocker"]),
            FinalizationPolicy::ReleaseWithWarning,
            false,
            MAX_STALLED_CHAT_SEGMENTS - 1,
        );
        let SegmentCheckpointDecision::Pause(notice) = decision else {
            panic!("repeated stalled chat segments must persist a resumable pause");
        };
        assert!(notice.contains("连续"));
        assert!(notice.contains("进度已保存"));
        assert!(notice.contains("继续执行"));
    }

    #[test]
    fn ready_nudge_skips_the_release_with_warning_surface() {
        assert!(!completion_ready_applies(
            FinalizationPolicy::ReleaseWithWarning
        ));
        assert!(completion_ready_applies(
            FinalizationPolicy::BlockOnIncomplete
        ));
    }
}

/// The inspection-budget rule (keystone slice 4.8c b5): once a surface's
/// read-only allowance is spent, further READ-ONLY calls are denied so the model
/// stops inspecting and starts acting. Pure — the loop supplies the tracker
/// state, since `ProgressTracker` lives inside `run_agent_loop`.
///
/// `kind` MUST come from `ToolBackend::classify` — with the default
/// bash-only rule a `run_shell`-style tool classifies `ReadOnly` every time and
/// this would deny every call.
pub fn inspection_budget_denial(
    read_only_exhausted: bool,
    mutation_seen: bool,
    kind: &ToolKind,
) -> Option<BudgetDenial> {
    if !read_only_exhausted || !matches!(kind, ToolKind::ReadOnly) {
        return None;
    }
    Some(BudgetDenial {
        rule: "inspection_budget".to_owned(),
        reason: if mutation_seen {
            "post-change inspection is exhausted; make the smallest corrective edit, run a bounded functional verification, or batch a specifically justified read with that action"
                .to_owned()
        } else {
            "initial inspection is exhausted; batch any remaining reads with the first implementation or begin the smallest candidate implementation now"
                .to_owned()
        },
    })
}
