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
//! Mapping (desktop): Interactive/Execute → `ReleaseWithWarning`, recovery 1,
//! `wall_budget_applies=false`; Autonomous → `BlockOnIncomplete`, recovery 1,
//! wall budget on. The `Benchmark` arm serves the sidecar (4.8) and is never
//! produced on the desktop path.

use crate::run::{FinalizationPolicy, TurnCapability};
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
    if matches!(result.status, crate::tool::ToolExecutionStatus::Waiting) {
        return CompletionRecord {
            progress_prompt: None,
            succeeded: false,
        };
    }
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
        return_code: result.return_code.or(Some(match result.status {
            crate::tool::ToolExecutionStatus::Done => 0,
            crate::tool::ToolExecutionStatus::Waiting
            | crate::tool::ToolExecutionStatus::Blocked
            | crate::tool::ToolExecutionStatus::Error => 1,
        })),
        stdout: if result.stdout.is_empty() {
            result.content.clone()
        } else {
            result.stdout.clone()
        },
        stderr: result.stderr.clone(),
        error: result.error.clone().or_else(|| {
            matches!(
                result.status,
                crate::tool::ToolExecutionStatus::Blocked | crate::tool::ToolExecutionStatus::Error
            )
            .then(|| result.content.clone())
        }),
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

pub fn completion_recovery_attempts_after_steer(attempts: u32) -> u32 {
    // A steer may refine or authorize the objective, but it must never mint a
    // fresh set of recovery rounds for the same root turn.
    attempts
}

/// Only unattended execution enters an automatic tools-disabled finalization
/// round as soon as its evidence ledger is complete. Interactive/Execute may
/// still have later planned mutations, so they finalize when the model
/// actually attempts a tool-free answer.
///
/// Both unattended arms qualify: `BlockOnIncomplete` (desktop Autonomous) and
/// `Benchmark` (the eval sidecar, which has no human in the loop at all). #260
/// narrowed this to `BlockOnIncomplete` while rewording the rule as "unattended
/// execution" — that dropped the sidecar's tools-disabled finalization round
/// even though the sidecar is exactly the surface the rule describes.
pub fn completion_ready_applies(policy: FinalizationPolicy) -> bool {
    matches!(
        policy,
        FinalizationPolicy::BlockOnIncomplete | FinalizationPolicy::Benchmark
    )
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
) -> Vec<ToolDefinition> {
    if finalization_pending {
        Vec::new()
    } else {
        tool_defs.to_vec()
    }
}

pub fn active_tool_definitions_for_capability(
    tool_defs: &[ToolDefinition],
    finalization_pending: bool,
    capability: TurnCapability,
) -> Vec<ToolDefinition> {
    if finalization_pending {
        return Vec::new();
    }
    tool_defs
        .iter()
        .filter(|definition| tool_visible_for_capability(capability, &definition.function.name))
        .cloned()
        .map(|mut definition| {
            if matches!(capability, TurnCapability::ReviewOnly)
                && matches!(
                    definition.function.name.as_str(),
                    "write_file" | "edit_file"
                )
            {
                definition
                    .function
                    .description
                    .push_str(REVIEW_WRITE_SCOPE_NOTE);
            }
            definition
        })
        .collect()
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
    } else if tool_name == "browser_session" {
        match args.get("action").and_then(serde_json::Value::as_str) {
            Some("click" | "fill" | "press") => ToolKind::Mutation,
            Some("screenshot") => ToolKind::Mutation,
            Some("open" | "attach" | "snapshot" | "tabs" | "select_tab" | "close") => {
                ToolKind::RuntimeProbe
            }
            _ => ToolKind::Mutation,
        }
    } else if tool_name.starts_with("write_")
        || tool_name.starts_with("edit_")
        || matches!(
            tool_name,
            "write_file"
                | "edit_file"
                | "delegate_tasks"
                | "dispatch_parallel_tasks"
                | "deliver_changes"
                | "skill_create"
                | "skill_update"
                | "skill_delete"
        )
    {
        ToolKind::Mutation
    } else {
        ToolKind::ReadOnly
    };
    (command, kind)
}

fn is_delivery_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    [
        "git commit",
        "git push",
        "git tag",
        "gh pr create",
        "gh pr merge",
        "gh workflow run",
        "gh release create",
        "glab mr create",
        "glab mr merge",
        "npm publish",
        "pnpm publish",
        "cargo publish",
        "kubectl apply",
        "helm upgrade",
        "vercel --prod",
        "netlify deploy",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Directories whose text documents are planning artifacts rather than product
/// code. A WHITELIST on purpose: an unrecognized location fails closed, the same
/// way `is_review_safe_named_tool` fails closed on an unknown tool name.
const PLANNING_DOCUMENT_ROOTS: &[&str] = &[
    "docs/",
    "doc/",
    "design/",
    "designs/",
    "spec/",
    "specs/",
    "plan/",
    "plans/",
    "planning/",
    "rfc/",
    "rfcs/",
    "adr/",
    "adrs/",
    "notes/",
];

/// Prose extensions only. A `.rs`/`.json`/`.sh` under `docs/` is still product
/// material — build scripts and fixtures live in documentation trees too.
const PLANNING_DOCUMENT_EXTENSIONS: &[&str] = &[".md", ".markdown", ".txt"];

/// Files that steer an agent or front the repository. Editing these CHANGES
/// BEHAVIOUR — the one thing a review turn must not do — so they are excluded
/// even when they sit inside a documentation directory.
const BEHAVIOUR_BEARING_DOCUMENTS: &[&str] = &[
    "agents.md",
    "claude.md",
    "codex.md",
    "gemini.md",
    "readme.md",
    "contributing.md",
    ".cursorrules",
    "copilot-instructions.md",
];

/// Is this path a planning/design document — the legitimate output of a
/// review turn?
///
/// Tolerant about how the model spells the path (absolute or workspace-relative,
/// `/` or `\`, any case) and strict about what it may reach: a prose extension,
/// inside a documentation directory, that is not an agent-instruction file, with
/// no `..` traversal.
pub fn is_planning_document_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    if normalized.is_empty() || normalized.split('/').any(|segment| segment == "..") {
        return false;
    }
    if !PLANNING_DOCUMENT_EXTENSIONS
        .iter()
        .any(|extension| normalized.ends_with(extension))
    {
        return false;
    }
    let file_name = normalized.rsplit('/').next().unwrap_or_default();
    if BEHAVIOUR_BEARING_DOCUMENTS.contains(&file_name) {
        return false;
    }
    // Segment-anchored so `mydocs/a.md` and `src/docsystem/a.md` do not pass.
    PLANNING_DOCUMENT_ROOTS
        .iter()
        .any(|root| normalized.starts_with(root) || normalized.contains(&format!("/{root}")))
}

/// A write whose reach is fully pinned by one `path` argument, aimed at a
/// planning document. This is the ONLY mutation a review turn may perform.
///
/// Deliberately restricted to the two path-bounded write tools: a shell command
/// naming `docs/plan.md` can still touch anything else in the same line, so
/// `bash` mutations stay denied and the model is pushed onto `write_file`.
fn is_planning_document_write(tool_name: &str, args: &serde_json::Value) -> bool {
    if !matches!(tool_name, "write_file" | "edit_file") {
        return false;
    }
    args.get("path")
        .and_then(serde_json::Value::as_str)
        .is_some_and(is_planning_document_path)
}

/// A structural denial has to carry the allowed route, not just the refusal.
/// Without it the model retries the same write through a heredoc, then a patch,
/// then hands the blocker to the user (2026-07-30 field report).
fn review_only_denial(tool_name: &str) -> String {
    format!(
        "已跳过与当前显式只读意图冲突的 `{tool_name}` 变更动作；这不是账号权限、GitHub 权限或 high-risk 命令判定。\
继续完成当前只读目标，不要要求用户重复确认；后续用户意图变化时重新逐动作判断。\
需要把方案落盘时，用 `write_file` 或 `edit_file` 写 `docs/` 下的 Markdown 文档\
（例如 `docs/plans/<slug>.md`），不要用 shell、heredoc 或 patch 写文件。\
代码、配置、测试以及 AGENTS.md / CLAUDE.md / README.md 在当前显式只读约束下不修改。"
    )
}

/// Appended to the write tools' schema on a review turn, so the model learns the
/// bound from the tool description instead of from a denial after the fact.
const REVIEW_WRITE_SCOPE_NOTE: &str = " On this review-only turn the path MUST be a planning or design document inside a documentation directory (for example `docs/plans/<slug>.md`), with a `.md`, `.markdown`, or `.txt` extension. Writes to code, config, tests, or agent-instruction files such as AGENTS.md, CLAUDE.md and README.md are rejected until the user asks for implementation.";

fn is_review_safe_named_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file"
            | "glob"
            | "grep"
            | "kb_search"
            | "kb_get_chunk"
            | "read_pptx"
            | "read_xlsx"
            | "skill_list"
            | "skill_search"
            | "skill_fetch"
            | "bash"
            | "browser_session"
            // Offered so a planning turn can persist its own document. The
            // per-call path check in `capability_denial` is what keeps them
            // narrow; without them here the tool is invisible and the model
            // resorts to a shell heredoc that the gate then blocks anyway.
            | "write_file"
            | "edit_file"
    )
}

pub fn tool_visible_for_capability(capability: TurnCapability, tool_name: &str) -> bool {
    match capability {
        TurnCapability::ReviewOnly => is_review_safe_named_tool(tool_name),
        TurnCapability::Implement => tool_name != "deliver_changes",
        TurnCapability::Deliver => true,
    }
}

/// Structural intent gate, evaluated before the permission gateway.
///
/// `args` is the raw tool payload: the review arm needs the target `path` to
/// tell a planning document apart from a product change (see
/// [`is_planning_document_path`]). Deriving it from `command` would be a string
/// round-trip through `completion_command_and_kind`, and would break for any
/// backend that formats its command line differently.
pub fn capability_denial(
    capability: TurnCapability,
    tool_name: &str,
    command: &str,
    kind: &ToolKind,
    args: &serde_json::Value,
) -> Option<String> {
    match capability {
        TurnCapability::ReviewOnly => {
            // A planning document IS the deliverable of a review turn. Blocking
            // it forced the whole plan into the chat instead of onto disk.
            if is_planning_document_write(tool_name, args) {
                return None;
            }
            let mutating = matches!(kind, ToolKind::Mutation | ToolKind::BackgroundServiceStart);
            if mutating || !is_review_safe_named_tool(tool_name) {
                Some(review_only_denial(tool_name))
            } else {
                None
            }
        }
        TurnCapability::Implement => {
            if tool_name == "deliver_changes" || is_delivery_command(command) {
                Some(
                    "当前意图只授权本地实施，已跳过提交、推送、PR、合并或发布动作；需要交付时必须获得对应意图并调用受控交付工具。"
                        .into(),
                )
            } else {
                None
            }
        }
        TurnCapability::Deliver => {
            if tool_name == "bash" && is_delivery_command(command) {
                Some(
                    "已跳过裸 shell 交付命令；提交、推送、PR、合并和发布必须调用 `deliver_changes`，由它记录 CI 门禁、head SHA 与恢复回执。这不是账号或命令权限分类问题；不要重试同一条 gh/git 命令。"
                        .into(),
                )
            } else {
                None
            }
        }
    }
}

/// Convert a machine-classified, recoverable delivery stop into another
/// execution round. The ordinary blocked path intentionally finalizes, but a
/// delivery result with an explicit `next_action` is a state-machine edge, not
/// a terminal blocker. Two rounds are enough to repair metadata/BEHIND and to
/// return one real CI failure to implementation without creating an infinite
/// agent loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRecoveryAction {
    pub prompt: String,
    pub retry_after: std::time::Duration,
    pub counts_as_repair_attempt: bool,
}

pub fn recoverable_delivery_prompt(
    tool_name: &str,
    metadata: &serde_json::Value,
    attempts: u8,
) -> Option<DeliveryRecoveryAction> {
    const MAX_ATTEMPTS: u8 = 2;
    const MAX_RETRY_AFTER_MS: u64 = 60_000;
    let recovery_class = metadata
        .get("recovery_class")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("agent_action_required");
    let is_retryable_wait = recovery_class == "wait_retryable";
    if tool_name != "deliver_changes"
        || (!is_retryable_wait && attempts >= MAX_ATTEMPTS)
        || metadata
            .get("recoverable")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return None;
    }
    let next_action = metadata
        .get("next_action")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let code = metadata
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("delivery_recoverable");
    let retry_after = if is_retryable_wait {
        std::time::Duration::from_millis(
            metadata
                .get("retry_after_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30_000)
                .clamp(1, MAX_RETRY_AFTER_MS),
        )
    } else {
        std::time::Duration::ZERO
    };
    let prompt = if is_retryable_wait {
        format!(
            "受控交付仍在等待可恢复的远端状态 `{code}`。这不是终态、权限问题或需要用户再次推动的阻断。\
等待退避已经完成；现在重新调用 deliver_changes 核对远端事实并续接同一 PR。\
不要创建新 PR，不要重复已经完成的外部动作，也不得使用 --admin、force push 或门禁绕过。\n恢复动作：{next_action}"
        )
    } else {
        format!(
            "受控交付返回可恢复状态 `{code}`。这是第 {}/{} 次有界修复，不是终态，也不是权限问题。\
立即执行下面的恢复动作；若修改了代码或正文，完成验证后再次调用 deliver_changes 续接同一 PR。\
恢复过程必须保留 required checks，并继续使用受控交付；禁止 --admin、force push 和门禁绕过。\n恢复动作：{next_action}",
            attempts + 1,
            MAX_ATTEMPTS
        )
    };
    Some(DeliveryRecoveryAction {
        prompt,
        retry_after,
        counts_as_repair_attempt: !is_retryable_wait,
    })
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

    fn no_args() -> serde_json::Value {
        serde_json::json!({})
    }

    fn write_to(path: &str) -> serde_json::Value {
        serde_json::json!({"path": path, "content": "# plan\n"})
    }

    #[test]
    fn explicit_action_intent_is_enforced_before_tool_permission() {
        assert!(capability_denial(
            TurnCapability::ReviewOnly,
            "edit_file",
            "edit_file src/lib.rs",
            &ToolKind::Mutation,
            &write_to("src/lib.rs"),
        )
        .is_some());
        assert!(capability_denial(
            TurnCapability::ReviewOnly,
            "bash",
            "git status --short",
            &ToolKind::ReadOnly,
            &no_args(),
        )
        .is_none());
        assert!(capability_denial(
            TurnCapability::Implement,
            "deliver_changes",
            "deliver_changes",
            &ToolKind::Mutation,
            &no_args(),
        )
        .is_some());
        assert!(capability_denial(
            TurnCapability::Implement,
            "bash",
            "git push origin feat/x",
            &ToolKind::Mutation,
            &no_args(),
        )
        .is_some());
        assert!(capability_denial(
            TurnCapability::Deliver,
            "deliver_changes",
            "deliver_changes",
            &ToolKind::Mutation,
            &no_args(),
        )
        .is_none());
    }

    #[test]
    fn recoverable_delivery_blocker_returns_to_execution_with_a_bound() {
        let metadata = serde_json::json!({
            "status": "blocked",
            "code": "delivery_ci_failed",
            "recoverable": true,
            "next_action": "读取失败 check，修复后重新调用 deliver_changes",
            "pr_url": "https://example.test/pull/7"
        });

        let first = recoverable_delivery_prompt("deliver_changes", &metadata, 0)
            .expect("the first recovery must continue the delivery loop");
        assert!(first.prompt.contains("读取失败 check"));
        assert!(first.prompt.contains("deliver_changes"));
        assert!(!first.prompt.contains("只生成阻断总结"));
        assert!(first.counts_as_repair_attempt);

        let second = recoverable_delivery_prompt("deliver_changes", &metadata, 1)
            .expect("one retry may still need a second bounded recovery");
        assert!(second.prompt.contains("第 2/2 次"));

        assert!(recoverable_delivery_prompt("deliver_changes", &metadata, 2).is_none());
    }

    #[test]
    fn retryable_delivery_wait_is_not_turned_into_a_terminal_blocker_after_two_polls() {
        let metadata = serde_json::json!({
            "status": "waiting",
            "code": "delivery_merge_queued",
            "recovery_class": "wait_retryable",
            "recoverable": true,
            "retry_after_ms": 30_000,
            "next_action": "等待远端门禁产生新状态后续接同一 PR",
            "pr_url": "https://example.test/pull/7"
        });

        let third_poll = recoverable_delivery_prompt("deliver_changes", &metadata, 2)
            .expect("a remote waiting state is active work, not an exhausted repair attempt");
        assert!(third_poll.prompt.contains("等待远端门禁"));
        assert!(third_poll.prompt.contains("同一 PR"));
        assert!(!third_poll.prompt.contains("第 3/2 次"));
        assert!(!third_poll.counts_as_repair_attempt);
        assert_eq!(third_poll.retry_after, std::time::Duration::from_secs(30));
    }

    #[test]
    fn delivery_wait_does_not_mutate_generic_completion_evidence() {
        let mut gate = CompletionGate::default();
        let before = gate.evidence();
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        let result = crate::tool::ToolInvocationResult {
            content: "remote checks pending".into(),
            is_error: false,
            status: crate::tool::ToolExecutionStatus::Waiting,
            command: "deliver_changes".into(),
            kind: ToolKind::Mutation,
            return_code: None,
            stdout: String::new(),
            stderr: String::new(),
            error: None,
            metadata: None,
            next_working_directory: None,
            duration_ms: 30_000,
        };

        let record = record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/tmp"),
            "delivery-wait",
            &result,
        );

        assert_eq!(sequence, 0);
        assert!(!record.succeeded);
        assert_eq!(gate.evidence(), before);
    }

    #[test]
    fn non_delivery_or_nonrecoverable_blockers_still_finalize() {
        let blocked = serde_json::json!({
            "recoverable": false,
            "next_action": "ask a human"
        });
        assert!(recoverable_delivery_prompt("deliver_changes", &blocked, 0).is_none());

        let unrelated = serde_json::json!({
            "recoverable": true,
            "next_action": "retry"
        });
        assert!(recoverable_delivery_prompt("browser_session", &unrelated, 0).is_none());
    }

    #[test]
    fn review_denial_does_not_ask_the_user_to_repeat_authorization() {
        let denial = capability_denial(
            TurnCapability::ReviewOnly,
            "bash",
            "touch sentinel",
            &ToolKind::Mutation,
            &no_args(),
        )
        .expect("mutation must be denied");
        assert!(!denial.contains("请由用户明确要求"));
        assert!(denial.contains("不要要求用户重复确认"));
    }

    #[test]
    fn review_only_fails_closed_for_unknown_name_prefixed_tools() {
        for name in ["read_and_delete", "get_and_publish", "list_then_write"] {
            assert!(!tool_visible_for_capability(
                TurnCapability::ReviewOnly,
                name
            ));
            assert!(capability_denial(
                TurnCapability::ReviewOnly,
                name,
                name,
                &ToolKind::ReadOnly,
                &no_args(),
            )
            .is_some());
        }
    }

    // ── Planning documents are the deliverable of a review turn ─────────────
    //
    // The motivating field report (2026-07-30): the user asked for a design
    // ("不要改代码，先给方案"), the turn correctly became ReviewOnly, and then
    // every attempt to persist that design — `python3 - <<PY … write_text`,
    // `apply_patch` adding `docs/specs/feature-specs/*.md` — was structurally
    // denied. ReviewOnly conflated "do not change the product" with "do not
    // write anything at all", so a planning turn could not produce its own
    // artifact and dumped the whole plan into the chat instead.

    #[test]
    fn review_only_allows_the_planning_document_it_exists_to_produce() {
        for path in [
            "docs/specs/feature-specs/on-demand-embedded-browser-pane.md",
            "docs/design/session-execution-governance-ux-design.md",
            "docs/plans/intent-recognition-planning.md",
            "docs/long-tasks/keystone-headless-runner.md",
            "design/notes.txt",
            "specs/rfc-0001.markdown",
        ] {
            assert!(
                capability_denial(
                    TurnCapability::ReviewOnly,
                    "write_file",
                    &format!("write_file {path}"),
                    &ToolKind::Mutation,
                    &write_to(path),
                )
                .is_none(),
                "{path} is the planning artifact of a review turn, not a product change"
            );
        }
        // edit_file on an existing planning document is the same class.
        assert!(capability_denial(
            TurnCapability::ReviewOnly,
            "edit_file",
            "edit_file docs/specs/feature-specs/mvp-agent-client.md",
            &ToolKind::Mutation,
            &serde_json::json!({
                "path": "docs/specs/feature-specs/mvp-agent-client.md",
                "old_string": "a",
                "new_string": "b",
            }),
        )
        .is_none());
    }

    #[test]
    fn review_only_still_refuses_code_config_and_agent_instruction_writes() {
        for path in [
            // Product code and config are never a planning artifact …
            "src/main.rs",
            "src/agent/dispatch.rs",
            "package.json",
            "src-tauri/Cargo.toml",
            "docs/scripts/build.sh",
            "docs/config.json",
            "migrations/003_add_column.sql",
            // … nor is a test, even a Markdown-adjacent one …
            "tests/plan.spec.ts",
            // … nor are the files that steer the agent itself. Editing those
            // changes behaviour, which is exactly what a review turn must not do.
            "AGENTS.md",
            "CLAUDE.md",
            "README.md",
            "docs/AGENTS.md",
            ".cursorrules",
            // … and traversal out of the workspace fails closed.
            "docs/../../../etc/notes.md",
        ] {
            assert!(
                capability_denial(
                    TurnCapability::ReviewOnly,
                    "write_file",
                    &format!("write_file {path}"),
                    &ToolKind::Mutation,
                    &write_to(path),
                )
                .is_some(),
                "{path} must stay blocked until the user asks for implementation"
            );
        }
    }

    #[test]
    fn review_only_keeps_shell_writes_blocked_because_a_command_has_no_path_bound() {
        // The screenshot's exact escape hatches. A shell command can touch
        // anything, so it cannot be admitted by path — the model must be pushed
        // onto `write_file` instead.
        for command in [
            "python3 - <<'PY'\nPath('docs/plans/x.md').write_text('…')\nPY",
            "apply_patch <<'PATCH'\n*** Add File: docs/plans/x.md\nPATCH",
            "echo '# plan' > docs/plans/x.md",
            "cat > docs/plans/x.md",
        ] {
            assert!(
                capability_denial(
                    TurnCapability::ReviewOnly,
                    "bash",
                    command,
                    &ToolKind::Mutation,
                    &serde_json::json!({"command": command}),
                )
                .is_some(),
                "{command:?} is an unbounded mutation even though it names a docs path"
            );
        }
    }

    #[test]
    fn review_denial_names_the_route_instead_of_only_saying_no() {
        // A gate that only blocks makes the model retry the same write three
        // ways and then hand the blocker to the user. It must point at the
        // allowed route in the same breath.
        let denial = capability_denial(
            TurnCapability::ReviewOnly,
            "bash",
            "python3 - <<'PY'\nPath('docs/x.md').write_text('…')\nPY",
            &ToolKind::Mutation,
            &serde_json::json!({"command": "python3 -"}),
        )
        .expect("shell mutation must be denied");
        assert!(denial.contains("write_file"), "names the allowed tool");
        assert!(denial.contains("docs/"), "names the allowed location");
        assert!(denial.contains("不要要求用户重复确认"));
        assert!(!denial.contains("请由用户明确要求"));
        assert!(!denial.contains("本回合"));
        assert!(denial.contains("不是账号权限"));
    }

    #[test]
    fn deliver_intent_routes_raw_delivery_commands_to_the_governed_tool() {
        let denial = capability_denial(
            TurnCapability::Deliver,
            "bash",
            "gh pr merge 281 --squash --auto",
            &ToolKind::Mutation,
            &serde_json::json!({"command": "gh pr merge 281 --squash --auto"}),
        )
        .expect("raw delivery mutation must stay behind deliver_changes");
        assert!(denial.contains("deliver_changes"));
        assert!(!denial.contains("本回合"));
        assert!(!denial.contains("高风险"));
    }

    #[test]
    fn planning_document_detection_is_a_whitelist_and_fails_closed() {
        assert!(is_planning_document_path("docs/plans/a.md"));
        assert!(is_planning_document_path("DOCS/PLANS/A.MD"));
        assert!(is_planning_document_path(r"docs\plans\a.md"));
        assert!(is_planning_document_path(
            "/Users/leo/Projects/CodeFactory/docs/specs/a.md"
        ));
        // Right extension, wrong place — a whitelist, not "any Markdown".
        assert!(!is_planning_document_path("notes.md"));
        assert!(!is_planning_document_path("src/plan.md"));
        // Right place, wrong extension.
        assert!(!is_planning_document_path("docs/plans/a.rs"));
        assert!(!is_planning_document_path("docs/plans/a"));
        // Substring near-misses must not open the gate.
        assert!(!is_planning_document_path("src/docsystem/a.md"));
        assert!(!is_planning_document_path("mydocs/a.md"));
        assert!(!is_planning_document_path(""));
    }

    #[test]
    fn review_turns_expose_the_write_tools_scoped_to_documents() {
        for name in ["write_file", "edit_file"] {
            assert!(
                tool_visible_for_capability(TurnCapability::ReviewOnly, name),
                "{name} must be offered, or the model falls back to a shell heredoc"
            );
        }
        assert!(!tool_visible_for_capability(
            TurnCapability::ReviewOnly,
            "deliver_changes"
        ));

        let defs = vec![
            definition("write_file", "Create or overwrite a file."),
            definition("read_file", "Read a file."),
        ];
        let review =
            active_tool_definitions_for_capability(&defs, false, TurnCapability::ReviewOnly);
        let write = review
            .iter()
            .find(|d| d.function.name == "write_file")
            .expect("write_file stays available on a review turn");
        assert!(
            write.function.description.contains("docs/"),
            "the document-only bound belongs in the schema the model reads, \
             not only in the denial it gets afterwards: {}",
            write.function.description
        );
        // Unrelated tools and other capabilities keep their exact description.
        assert_eq!(
            review
                .iter()
                .find(|d| d.function.name == "read_file")
                .unwrap()
                .function
                .description,
            "Read a file."
        );
        let implement =
            active_tool_definitions_for_capability(&defs, false, TurnCapability::Implement);
        assert_eq!(
            implement
                .iter()
                .find(|d| d.function.name == "write_file")
                .unwrap()
                .function
                .description,
            "Create or overwrite a file."
        );
    }

    fn definition(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            r#type: "function".into(),
            function: crate::types::FunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }
    }

    #[test]
    fn steer_never_refills_the_turn_recovery_budget() {
        assert_eq!(completion_recovery_attempts_after_steer(0), 0);
        assert_eq!(completion_recovery_attempts_after_steer(1), 1);
    }

    #[test]
    fn delivery_and_parallel_tools_are_never_classified_as_read_only() {
        for name in [
            "deliver_changes",
            "delegate_tasks",
            "dispatch_parallel_tasks",
        ] {
            let (_, kind) = completion_command_and_kind(name, &serde_json::json!({}));
            assert_eq!(kind, ToolKind::Mutation, "{name}");
        }
    }

    #[test]
    fn browser_screenshot_is_a_workspace_mutation_but_observation_stays_read_only() {
        let (_, screenshot) = completion_command_and_kind(
            "browser_session",
            &serde_json::json!({"action":"screenshot","path":"proof/page.png"}),
        );
        assert_eq!(screenshot, ToolKind::Mutation);
        assert!(capability_denial(
            TurnCapability::ReviewOnly,
            "browser_session",
            "browser_session screenshot",
            &screenshot,
            &serde_json::json!({"action": "screenshot", "path": "proof/page.png"}),
        )
        .is_some());

        for action in ["open", "attach", "snapshot", "tabs", "select_tab", "close"] {
            let (_, kind) = completion_command_and_kind(
                "browser_session",
                &serde_json::json!({"action":action}),
            );
            assert_eq!(kind, ToolKind::RuntimeProbe, "{action}");
        }
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
    fn ready_nudge_is_reserved_for_unattended_execution() {
        assert!(!completion_ready_applies(
            FinalizationPolicy::ReleaseWithWarning
        ));
        assert!(completion_ready_applies(
            FinalizationPolicy::BlockOnIncomplete
        ));
        // The eval sidecar is the most unattended surface there is; leaving it
        // out is what broke the headless finalization round after #260.
        assert!(completion_ready_applies(FinalizationPolicy::Benchmark));
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
