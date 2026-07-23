// SPDX-License-Identifier: Apache-2.0
pub mod anthropic_client;
pub mod events;
pub mod attachments;
pub mod checkpoint;
pub mod context;
pub mod context_budget;
pub mod delivery;
pub mod dispatch;
pub mod hooks;
pub mod journal;
pub mod scheduler;
pub mod sse_buffer;
pub mod subagent;
mod tool_backend;
pub mod user_context;
pub mod verification;
pub mod worktree;

pub use dispatch::decide_chat_mode;

use chrono::Utc;
use codefactory_agent_core::{
    build_budget_convergence_prompt, build_completion_ready_prompt,
    build_completion_recovery_prompt, classify_command, completion_evidence_made_progress,
    evaluate_budget_command_in_directory, provider_rejects_required_tool_choice,
    sanitize_completion_summary, should_prompt_budget_convergence, CompletionEvidence,
    CompletionGate, PolicyDecision, ProgressTracker, ToolKind, ToolOutcome,
};
use futures_util::{Stream, StreamExt};
use reqwest::Client;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::RwLock;
use tokio::time::timeout;
use uuid::Uuid;

use crate::config::settings::{ApiStyle, PermissionPolicy, Settings};
use crate::errors::Result;
use crate::mcp::McpManager;
use crate::openrouter::types::*;
use crate::storage::Message;
use crate::tools::{self};
use crate::PendingPermissionMap;
// Brings `DesktopToolBackend::execute`/`list_schemas` into method scope; the
// concrete backend lives in `tool_backend`.
use codefactory_agent_loop::tool::ToolBackend as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionResponse {
    Allow,
    Deny,
    Cancelled,
}

#[derive(Debug, PartialEq, Eq)]
enum StreamPoll<T> {
    Item(Option<T>),
    Cancelled,
}

async fn wait_for_cancellation(cancel: Option<&Arc<AtomicBool>>) {
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn next_stream_item<S>(
    stream: &mut S,
    cancel: Option<&Arc<AtomicBool>>,
) -> StreamPoll<S::Item>
where
    S: Stream + Unpin,
{
    tokio::select! {
        item = stream.next() => StreamPoll::Item(item),
        _ = wait_for_cancellation(cancel), if cancel.is_some() => StreamPoll::Cancelled,
    }
}

async fn await_permission_response(
    receiver: tokio::sync::oneshot::Receiver<bool>,
    cancel: Option<&Arc<AtomicBool>>,
    max_wait: Duration,
) -> PermissionResponse {
    tokio::select! {
        response = timeout(max_wait, receiver) => match response {
            Ok(Ok(true)) => PermissionResponse::Allow,
            _ => PermissionResponse::Deny,
        },
        _ = wait_for_cancellation(cancel), if cancel.is_some() => PermissionResponse::Cancelled,
    }
}

fn cancelled_tool_suffix<'a>(
    cancel: Option<&Arc<AtomicBool>>,
    tool_calls: &'a [ToolCall],
    start: usize,
) -> Option<&'a [ToolCall]> {
    cancel
        .is_some_and(|flag| flag.load(Ordering::SeqCst))
        .then(|| &tool_calls[start..])
}

/// Tool-call iteration ceiling for INTERACTIVE chat. Conservative
/// because every iteration is a user-visible turn — letting it run too
/// long makes the chat feel stuck.
const MAX_ITERATIONS_INTERACTIVE: usize = 30;

/// Iteration ceiling for AUTONOMOUS execution (subagents, approved
/// task runs). The whole point is to NOT bounce back to the user for
/// every micro-decision, so the budget is much larger. Most iterations
/// are tool round-trips, not LLM turns — they're cheap.
const MAX_ITERATIONS_AUTONOMOUS: usize = 200;

/// Iteration ceiling for EXECUTE turns — the chat surface right after the
/// user approved a plan. Higher than interactive (the work was greenlit, so
/// don't bounce back early) but well under autonomous (the user is still in
/// the room and may interject between turns).
const MAX_ITERATIONS_EXECUTE: usize = 80;

/// How many distinct retry attempts the autonomous agent makes against
/// the SAME failure signature before being forced to try a different
/// approach. e.g. a `cargo test` that keeps failing the same way 5
/// times in a row means the current angle isn't working.
#[allow(dead_code)]
const MAX_RETRIES_PER_BLOCKER: usize = 5;

/// Distinct approaches the autonomous agent tries before giving up
/// and surfacing as a hard blocker.
#[allow(dead_code)]
const MAX_DISTINCT_APPROACHES: usize = 3;

const EXECUTION_COMPLETION_CONTRACT: &str =
    include_str!("../../../agent_contracts/execution_completion.md");

/// Selects which behavior contract the AgentLoop runs under.
///
/// - `Interactive`: chat panel. Plan-first, ask before non-trivial work,
///   conservative iteration budget. Lets the user steer in real time.
/// - `Autonomous`: subagents executing approved tasks. The user already
///   said GO; the agent MUST keep working, retry on failure, and only
///   stop when acceptance criteria pass OR a hard blocker is hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Interactive,
    /// Chat surface, but the user just approved a pending proposal. Same
    /// session and tool-permission safety net as Interactive, but the
    /// "plan-first / ask to proceed" contract is replaced by "carry out the
    /// approved work now, don't re-ask". Selected per-turn by the framework
    /// (see [`dispatch::decide_chat_mode`]) — never exposed as a user toggle.
    Execute,
    Autonomous,
}

impl AgentMode {
    pub fn max_iterations(&self) -> usize {
        match self {
            AgentMode::Interactive => MAX_ITERATIONS_INTERACTIVE,
            AgentMode::Execute => MAX_ITERATIONS_EXECUTE,
            AgentMode::Autonomous => MAX_ITERATIONS_AUTONOMOUS,
        }
    }

    pub fn system_prompt(&self) -> &'static str {
        match self {
            AgentMode::Interactive => SYSTEM_PROMPT,
            AgentMode::Execute => SYSTEM_PROMPT_EXECUTE,
            AgentMode::Autonomous => SYSTEM_PROMPT_AUTONOMOUS,
        }
    }
}

const SYSTEM_PROMPT: &str = "\
You are CodeFactory, an AI coding assistant running in the user's desktop app.\n\
You have tools to read/write files, search code, and execute shell commands.\n\
Work step by step. Read files before editing them. Prefer targeted edits over full rewrites.\n\
\n\
# Communicate as an engineer, not a build log\n\
Every plan and every summary you write is read by a human who wants to\n\
understand what's happening, not audit your filesystem. Lead with\n\
analysis, end with bookkeeping:\n\
\n\
- **What problem this solves** — one sentence in the user's words, not yours.\n\
- **How you'll approach it / how you approached it** — 2-3 sentences of\n\
  reasoning. Why this design? What did you consider and reject? Any risk?\n\
- **Outcome the user will see** — concrete behavioural change, error\n\
  removed, output now correct, etc.\n\
- **Files** — last, brief. A short list with one-clause purpose each.\n\
  Never a wall of paths with no context.\n\
\n\
Bad summary (do NOT do this):\n\
  \"Modified src/foo.rs, src/bar.rs, src/baz.rs. Added 3 tests.\"\n\
\n\
Good summary:\n\
  \"Fixed the duplicate-entry crash when importing CSVs with mixed line\n\
   endings. Root cause was the parser treating \\\\r\\\\n as two records;\n\
   normalised to \\\\n at read time and added a regression test for the\n\
   mixed case.\n\
   - src/csv/parse.rs — normalisation at the byte-stream boundary\n\
   - tests/csv_mixed_eol.rs — covers \\\\r\\\\n, \\\\n, and mixed inputs\"\n\
\n\
The same shape applies to plans before execution. Lead with the problem\n\
and approach; file lists come last and concise.\n\
\n\
# Narrate the work as you go\n\
Before each burst of tool calls, write ONE short sentence in the user's\n\
language saying what you are about to do and why. After any tool failure,\n\
the next text you produce must say what failed and how you are responding\n\
before you continue. Never leave a sequence of tool calls — especially\n\
failed ones — without a human-readable thread explaining it. If the user's\n\
latest message contained a question or claim, answer it in your first\n\
sentence before any tool call.\n\
\n\
# Plan-first for non-trivial work\n\
If the request involves more than ~3 files, introduces new behaviour,\n\
refactors across modules, or has any ambiguity in acceptance, reply\n\
first with a plan in the format above (problem → approach → acceptance\n\
→ files), then end with \"Ready to proceed?\" and wait for the user's\n\
go-ahead (\"yes\", \"ok\", \"做吧\", or a refinement). Skip this ceremony\n\
for one-line bugfixes, typos, and pure read-only investigation.\n\
\n\
# Don't re-confirm an approved plan\n\
If your previous message proposed a plan or suggestions and the user's\n\
reply approves it (\"yes\", \"ok\", \"做吧\", \"同意\", \"就这样\", or names a\n\
deliverable like \"output the PPT\"), do NOT reply with another plan and\n\
do NOT ask \"Ready to proceed?\" again — begin executing immediately. The\n\
user already greenlit it; re-confirming wastes their turn and breaks the\n\
engineering contract.\n\
\n\
# TDD execution loop\n\
Once the user approves the plan, execute in this exact order:\n\
1. Write the failing tests at the paths declared in the plan.\n\
2. Run the tests (use the project's standard runner: `cargo test`,\n\
   `pnpm test`, `pytest`, etc.) and confirm they fail for the reason\n\
   you expect.\n\
3. Write the implementation.\n\
4. Re-run the tests. If anything still fails, follow the discipline\n\
   section below.\n\
5. Run the full test suite once more to catch regressions.\n\
6. Report a summary using the analysis-first shape above.\n\
7. **Deliver (code changes in a git repo).** After the suite is green, call\n\
   the `deliver_changes` tool ONCE to carry the work through the user's\n\
   configured delivery ceiling (commit -> push -> PR -> CI -> merge ->\n\
   release). Do NOT hand-run git in bash, and do NOT stop at a green build to\n\
   describe a missing PR — invoking the tool IS how code work reaches done.\n\
   The tool stages only real source files (never local noise) and is\n\
   idempotent, so it is safe to call again to resume after any interruption.\n\
\n\
# Test-modification discipline (NON-NEGOTIABLE)\n\
A failing test is a *data point*, not a reason to edit the test. When\n\
a test fails, you MUST first diagnose:\n\
\n\
- **Implementation is wrong** → fix the implementation. Do not touch\n\
  the test file. This is the default assumption.\n\
- **Test is genuinely wrong** (wrong expected value, broken setup,\n\
  wrong assumption about the spec) → you may edit the test, but in\n\
  the SAME turn you must state explicitly why the test was incorrect:\n\
    \"Modifying tests/foo.test.ts: this test expected the error message\n\
     to be 'bad input' but the spec says 'invalid input'. Test was wrong.\"\n\
- **Unclear** → stop and ask the user. Never guess by editing the test\n\
  to make it pass.\n\
\n\
Editing a test purely because it failed, without a stated reason rooted\n\
in the spec, is a hard failure of the engineering contract.";

// ─────────────────────────────────────────────────────────────────────────────
// SYSTEM_PROMPT_AUTONOMOUS — used by subagents executing approved tasks.
// The user is NOT in the loop here. Defaults flip from "ask the user"
// to "keep going until acceptance passes or you're truly blocked".
// ─────────────────────────────────────────────────────────────────────────────

const SYSTEM_PROMPT_AUTONOMOUS: &str = "\
You are CodeFactory in AUTONOMOUS execution mode.\n\
\n\
**CONTEXT YOU MUST INTERNALIZE:**\n\
The user already approved a plan. They are NOT in this turn. Asking for\n\
confirmation, asking 'should I X?', or stopping to clarify is a contract\n\
violation. Every action you'd normally check first — JUST DO IT, then\n\
verify the outcome. The plan is the contract.\n\
\n\
**HARD RULES — non-negotiable:**\n\
\n\
1. **Never stop to ask 'should I proceed?'**\n\
   If the next step is in the plan, do it. If not, follow the plan's\n\
   spirit using your judgment. Only escalate for HARD blockers (see below).\n\
\n\
2. **Failure is not a stopping condition. Iterate.**\n\
   When a test fails / tool errors / compile breaks:\n\
     - DIAGNOSE the actual failure (read the error output carefully)\n\
     - FIX the implementation (not the test, unless test is genuinely wrong)\n\
     - RE-RUN\n\
   Up to 5 retries against the same blocker. After 5 same-signature\n\
   failures, change APPROACH (different file structure, different lib,\n\
   different algorithm). After 3 distinct approaches all fail, surface\n\
   as hard blocker.\n\
\n\
3. **Done means acceptance criteria pass, not 'code compiled'.**\n\
   The plan contains acceptance_criteria (real test commands, real\n\
   user-visible behaviors). Before declaring complete you MUST:\n\
     - Run the explicit verification commands in acceptance_criteria\n\
     - Confirm they pass with real output (not your guess)\n\
     - If they don't pass: that's failure mode #2 above — iterate\n\
\n\
4. **No 'I think this should work'. Verify.**\n\
   After every write_file / edit_file that touches code: run the\n\
   relevant test or compile. After every config change: verify the\n\
   loaded config matches what you intended.\n\
\n\
5. **HARD blockers — the ONLY valid reasons to stop:**\n\
     - Missing credential that requires a user action (API key, OAuth)\n\
     - Missing file the user must provide (their data, their license)\n\
     - Plan is logically contradictory and 3 approaches all reveal it\n\
   When you stop for a hard blocker: state it in ONE precise sentence,\n\
   tell the user EXACTLY what action unblocks you, then end.\n\
\n\
**PARALLEL FAN-OUT:**\n\
When the plan contains several INDEPENDENT sub-jobs (auditing many\n\
modules, migrating many files, researching directions) that do not edit\n\
the same files, call `dispatch_parallel_tasks` once with self-contained\n\
briefs instead of doing them serially — the scheduler runs them\n\
concurrently under the user's parallelism cap. Keep sequential steps of\n\
one change OUT of it.\n\
\n\
**HOW A TURN ENDS NATURALLY:**\n\
- All acceptance criteria verified pass -> declare done with the\n\
  one-line evidence per criterion (the command run + the result)\n\
- Hard blocker hit (see above) -> state it, end\n\
- Iteration budget exhausted (200 tool calls) -> report progress,\n\
  list what's left, end\n\
\n\
**OUTPUT WHEN DONE (mandatory shape):**\n\
\n\
  Acceptance: <criterion 1> — verified via `<command>` -> <result>\n\
  Acceptance: <criterion 2> — ...\n\
  Files: <short list>\n\
  Approach: <2-3 sentences on what you actually did and why>\n\
\n\
Anything less is a contract violation — the parent scheduler will\n\
catch it and respawn you with a 'previous attempt incomplete' brief.\n\
\n\
You can communicate as engineer-to-engineer in your reasoning, but\n\
prose without verification is wasted tokens. Verify, then summarize.";

// ─────────────────────────────────────────────────────────────────────────────
// SYSTEM_PROMPT_EXECUTE — one chat turn right after the user approves a
// pending proposal. Same chat session and tool-permission safety net as
// interactive, but the plan-first/ask contract is replaced with "carry out
// the approved work now". Selected per-turn by `dispatch::decide_chat_mode`;
// never exposed as a user-facing mode.
// ─────────────────────────────────────────────────────────────────────────────

const SYSTEM_PROMPT_EXECUTE: &str = "\
You are CodeFactory. In your previous message you proposed a plan or a set\n\
of suggestions, and the user just APPROVED it. Carry it out NOW.\n\
\n\
**HARD RULES — non-negotiable:**\n\
\n\
1. **Do not re-plan and do not re-ask — but DO speak.** Do NOT restate the\n\
   plan, do NOT reply with a fresh plan, and do NOT end with \"Ready to\n\
   proceed?\" or any \"should I…?\" confirmation — approval was already given.\n\
   Open with ONE short sentence in the user's language: directly answer\n\
   anything the user just said or asked, then say what you are starting now.\n\
   Then begin the tool calls. One orienting sentence is not re-planning;\n\
   silence is how the user ends up staring at a wall of unexplained tool\n\
   cards with no idea what is happening.\n\
\n\
2. **Produce the deliverable, not a proposal for it.** If the approval named\n\
   an output (\"output a PPT\", \"生成报告\", \"build the endpoint\"), produce that\n\
   artifact. Describing how you *would* produce it is a failure.\n\
\n\
3. **Failure is not a stopping condition — but it is a speaking condition.**\n\
   When a tool errors / a test fails / a build breaks: say in one short\n\
   sentence (user's language) what failed and what you are trying next, then\n\
   diagnose, fix, re-run. Iterate a few times before surfacing a blocker,\n\
   but never silently skip past a red tool result — an unexplained failure\n\
   card reads as \"something broke and the agent ignored it\".\n\
\n\
4. **Keep going until done or truly blocked.** Stop only for a HARD blocker:\n\
   a missing credential/file the user must provide, or a destructive,\n\
   irreversible action that genuinely needs their explicit OK. A tool\n\
   permission prompt is NOT a blocker — it's the normal safety check; let it\n\
   surface and continue once answered.\n\
\n\
5. **Verify, then report.** Before declaring done, confirm the work actually\n\
   happened (run it, read it back). Then summarize engineer-style: what you\n\
   did, the outcome the user sees, and a short list of files/deliverables.\n\
   Lead with the result, keep bookkeeping last.\n\
6. **Deliver code work.** For a code change in a git repo, once verified, call\n\
   the `deliver_changes` tool to carry it through the configured delivery\n\
   ceiling (commit -> push -> PR -> CI -> merge -> release). A green build\n\
   without delivery is not done; narrating the missing PR instead of opening\n\
   it is the exact failure this rule exists to prevent. The tool is idempotent\n\
   and commits only real source files.";

pub struct AgentLoop {
    /// None in a headless run (no Tauri frontend). Present for the desktop
    /// app. Slice 1's EventSink already carries the UI stream; this remains
    /// only for tools/hooks/skills that need the live app, all guarded.
    app: Option<AppHandle>,
    events: std::sync::Arc<dyn events::EventSink>,
    db: SqlitePool,
    session_id: String,
    endpoint_name: String,
    model_id: String,
    base_url: String,
    api_key: String,
    api_style: ApiStyle,
    cwd: PathBuf,
    http: Client,
    settings: Arc<RwLock<Settings>>,
    pending_permissions: PendingPermissionMap,
    mcp_manager: Arc<McpManager>,
    execution_context: Option<AgentExecutionContext>,
    /// Selects iteration ceiling and system prompt. Interactive for
    /// chat panel use, Autonomous for subagent / approved-task runs.
    mode: AgentMode,
    /// Stable for one AgentLoop execution; combined with the provider-round
    /// index to make usage persistence idempotent without collapsing genuine
    /// multi-round tool work.
    usage_run_id: String,
    /// Ephemeral "anonymous" run: when true, NOTHING is written to the DB
    /// (no user/assistant/tool messages, no cost entries). The conversation
    /// exists only in the frontend's memory and this run's model context, so
    /// a private/sensitive chat leaves no trace. Set via `.anonymous()`.
    anonymous: bool,
    /// User-requested cancellation for THIS chat turn. `None` for every
    /// non-chat construction (subagent / autonomous task runs), so those are
    /// completely unaffected. When set (via `.with_cancel()`), the run loop
    /// polls it between rounds and stops cleanly — it never interrupts an
    /// in-flight tool call, and never touches the task scheduler.
    cancel: Option<Arc<AtomicBool>>,
}

fn resolve_chatgpt_reasoning_effort(
    settings: &Settings,
    endpoint_name: &str,
    model_id: &str,
    session_effort: Option<&str>,
) -> crate::config::settings::ReasoningEffort {
    use crate::config::settings::ReasoningEffort;

    let requested = session_effort
        .and_then(ReasoningEffort::parse)
        .unwrap_or(settings.reasoning_effort);
    let requested = match requested {
        ReasoningEffort::Ultra => ReasoningEffort::Max,
        effort => effort,
    };
    let model = settings
        .endpoints
        .get(endpoint_name)
        .filter(|endpoint| matches!(endpoint.api_style, ApiStyle::Chatgpt))
        .and_then(|endpoint| {
            endpoint
                .custom_models
                .iter()
                .find(|model| model.id == model_id)
        });
    let Some(model) = model else {
        return requested;
    };
    let Some(supported) = model.supported_reasoning_efforts.as_deref() else {
        return requested;
    };

    if supported.contains(&requested) {
        return requested;
    }
    if let Some(default) = model.default_reasoning_effort {
        if supported.is_empty() || supported.contains(&default) {
            return default;
        }
    }
    if supported.contains(&ReasoningEffort::Medium) {
        return ReasoningEffort::Medium;
    }
    supported
        .first()
        .copied()
        .unwrap_or(ReasoningEffort::Medium)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UsageSurface {
    #[default]
    Interactive,
    Autonomous,
    Subagent,
    Eval,
}

impl UsageSurface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Autonomous => "autonomous",
            Self::Subagent => "subagent",
            Self::Eval => "eval",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AgentExecutionContext {
    pub parent_session_id: Option<String>,
    pub task_id: Option<String>,
    pub knowledge_library_ids: Vec<String>,
    pub usage_surface: UsageSurface,
}

fn knowledge_scope_for_tools(
    execution_context: Option<&AgentExecutionContext>,
) -> Option<Vec<String>> {
    execution_context.map(|context| context.knowledge_library_ids.clone())
}

impl AgentLoop {
    fn emit_transport_retry(events: &dyn events::EventSink, notice: crate::http_util::RetryNotice) {
        events.emit(StreamEvent::TransportRetry {
            label: notice.label,
            attempt: notice.attempt as u32,
            max_attempts: notice.max_attempts as u32,
            delay_ms: notice.delay.as_millis() as u64,
            reason: notice.reason,
        });
    }

    pub fn new(
        app: AppHandle,
        db: SqlitePool,
        session_id: String,
        endpoint_name: String,
        model_id: String,
        base_url: String,
        api_key: String,
        api_style: ApiStyle,
        cwd: PathBuf,
        settings: Arc<RwLock<Settings>>,
        pending_permissions: PendingPermissionMap,
        mcp_manager: Arc<McpManager>,
        execution_context: Option<AgentExecutionContext>,
    ) -> Self {
        // Default to interactive — chat call sites get the existing
        // behavior. Subagent + autonomous task runners call
        // `new_with_mode` explicitly.
        Self::new_with_mode(
            app,
            db,
            session_id,
            endpoint_name,
            model_id,
            base_url,
            api_key,
            api_style,
            cwd,
            settings,
            pending_permissions,
            mcp_manager,
            execution_context,
            AgentMode::Interactive,
        )
    }

    /// Construct an agent loop with an explicit execution mode.
    /// Autonomous mode is used by subagent runs where the user is no
    /// longer in the turn and the agent must keep working until done.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_mode(
        app: AppHandle,
        db: SqlitePool,
        session_id: String,
        endpoint_name: String,
        model_id: String,
        base_url: String,
        api_key: String,
        api_style: ApiStyle,
        cwd: PathBuf,
        settings: Arc<RwLock<Settings>>,
        pending_permissions: PendingPermissionMap,
        mcp_manager: Arc<McpManager>,
        execution_context: Option<AgentExecutionContext>,
        mode: AgentMode,
    ) -> Self {
        let events: std::sync::Arc<dyn events::EventSink> =
            std::sync::Arc::new(events::TauriEventSink::new(app.clone(), &session_id));
        Self {
            app: Some(app),
            events,
            db,
            session_id,
            endpoint_name,
            model_id,
            base_url,
            api_key,
            api_style,
            cwd,
            http: Client::new(),
            settings,
            pending_permissions,
            mcp_manager,
            execution_context,
            mode,
            usage_run_id: Uuid::new_v4().to_string(),
            anonymous: false,
            cancel: None,
        }
    }

    /// Build an agent loop with NO Tauri `AppHandle` — the headless seam
    /// (keystone slice 3). This is the SAME loop, the SAME completion gate,
    /// and the SAME tool surface as the desktop app; the only differences are
    /// wholly guarded:
    /// - events flow through the caller-supplied `events` sink (a
    ///   [`events::CollectingEventSink`] for eval, or a streaming JSONL sink
    ///   for a CLI) instead of the Tauri frontend;
    /// - hooks are skipped entirely (`hook_runner` is `None`);
    /// - skills come from the user skills dir only (no builtin/AppHandle);
    /// - `delegate_tasks` degrades (it needs live UI sessions);
    /// - usage-recorded UI pings are skipped.
    ///
    /// Every other tool (read/write/edit/glob/grep/bash/office/kb/delivery)
    /// runs exactly as in the app. The headless runner is a `not(test)` binary
    /// (see `--headless-smoke`), never the unit-test EXE.
    ///
    /// `#[cfg(not(test))]` is load-bearing, not cosmetic: this is the ONLY
    /// crate-reachable path that constructs an `AgentLoop`. Every other
    /// constructor is reached solely through private-module `#[tauri::command]`
    /// handlers that the unit-test EXE's link graph dead-strips. If this stayed
    /// in the test build, `run_headless_smoke_cli` (a crate-public root) would
    /// force `AgentLoop` construction — which links the Tauri `AppHandle`
    /// machinery — into `codefactory_lib-*.exe`, whose Windows loader then
    /// aborts with `STATUS_ENTRYPOINT_NOT_FOUND` (0xc0000139) before any test
    /// runs (hotfix #166). Gating it out of the test EXE costs nothing.
    #[cfg(not(test))]
    #[allow(clippy::too_many_arguments)]
    pub fn new_headless(
        events: std::sync::Arc<dyn events::EventSink>,
        db: SqlitePool,
        session_id: String,
        endpoint_name: String,
        model_id: String,
        base_url: String,
        api_key: String,
        api_style: ApiStyle,
        cwd: PathBuf,
        settings: Arc<RwLock<Settings>>,
        pending_permissions: PendingPermissionMap,
        mcp_manager: Arc<McpManager>,
        execution_context: Option<AgentExecutionContext>,
        mode: AgentMode,
    ) -> Self {
        Self {
            app: None,
            events,
            db,
            session_id,
            endpoint_name,
            model_id,
            base_url,
            api_key,
            api_style,
            cwd,
            http: Client::new(),
            settings,
            pending_permissions,
            mcp_manager,
            execution_context,
            mode,
            usage_run_id: Uuid::new_v4().to_string(),
            anonymous: false,
            cancel: None,
        }
    }

    /// Headless construction smoke (keystone slice 3). Release CI invokes this
    /// on the exact packaged executable — the same pattern as
    /// `--evolution-smoke` — to prove the REAL `AgentLoop` constructs and its
    /// full tool surface is reachable with NO `AppHandle`. That is precisely
    /// the Windows loader path #166 made fragile, and running it as a
    /// `not(test)` binary (never the unit-test EXE) is the safe place to prove
    /// it. Network-free: it builds the loop and validates the event-sink +
    /// tool wiring; it does NOT call a model. The live headless *turn* is
    /// slice 4's job.
    ///
    /// `#[cfg(not(test))]` because it constructs an `AgentLoop` (via
    /// [`Self::new_headless`]); see that method for why the test EXE must not
    /// link this path (#166).
    #[cfg(not(test))]
    pub async fn run_headless_smoke(output_path: &Path) -> Result<serde_json::Value> {
        let smoke_id = Uuid::new_v4().to_string();
        let root = std::env::temp_dir().join(format!("codefactory-headless-smoke-{smoke_id}"));
        let project = root.join("project");
        std::fs::create_dir_all(&project)?;
        let db_path = root.join("smoke.db");
        let db_url = format!("sqlite:{}", db_path.display());
        let pool = crate::storage::db::connect(&db_url).await?;

        let sink = Arc::new(events::CollectingEventSink::new());
        let sink_for_loop: Arc<dyn events::EventSink> = sink.clone();
        let tool_count = tools::all_definitions().len();

        // The load-bearing assertion: this constructs the real loop with
        // `app: None`. If the packaged binary's loader could not resolve the
        // headless path, this line would abort before returning.
        let _agent = AgentLoop::new_headless(
            sink_for_loop,
            pool.clone(),
            format!("headless-smoke-{smoke_id}"),
            "smoke-endpoint".to_string(),
            "smoke-model".to_string(),
            "http://127.0.0.1:0".to_string(), // never dialed — construction only
            "smoke-key".to_string(),
            ApiStyle::Openai,
            project.clone(),
            Arc::new(RwLock::new(Settings::default())),
            Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            Arc::new(McpManager::new()),
            None,
            AgentMode::Autonomous,
        );

        // The loop owns this exact `sink` Arc; emitting proves the sink handed
        // to the headless loop is a functioning `EventSink`. UFCS so the trait
        // method resolves without importing the trait into module scope.
        events::EventSink::emit(
            sink.as_ref(),
            StreamEvent::TextDelta {
                content: "headless-smoke".into(),
            },
        );
        let events_recorded = sink.events().len();

        let _ = std::fs::remove_dir_all(&root);

        if tool_count == 0 {
            return Err(crate::errors::AppError::Other(
                "headless smoke: tool surface is empty".into(),
            ));
        }
        if events_recorded != 1 {
            return Err(crate::errors::AppError::Other(
                "headless smoke: event sink did not record".into(),
            ));
        }

        let receipt = serde_json::json!({
            "ok": true,
            "smoke": "headless-construction",
            "tool_count": tool_count,
            "events_recorded": events_recorded,
            "app_handle": "none",
        });
        std::fs::write(
            output_path,
            serde_json::to_string_pretty(&receipt).unwrap_or_default(),
        )?;
        Ok(receipt)
    }

    async fn record_usage_event_for_round(&self, usage: &Usage, iteration: usize) {
        if self.anonymous || (usage.prompt_tokens == 0 && usage.completion_tokens == 0) {
            return;
        }
        let surface = self
            .execution_context
            .as_ref()
            .map_or(UsageSurface::Interactive, |context| context.usage_surface)
            .as_str();
        let local_endpoint = {
            let base = self.base_url.to_ascii_lowercase();
            base.contains("127.0.0.1")
                || base.contains("localhost")
                || base.contains("0.0.0.0")
                || base.starts_with("http://[::1]")
        };
        let (provider, actual_cost_usd, cost_source) = if self.api_style == ApiStyle::Chatgpt {
            ("chatgpt".to_string(), None, "subscription".to_string())
        } else if local_endpoint {
            (self.endpoint_name.clone(), None, "local".to_string())
        } else if let Some(cost) = usage
            .cost
            .filter(|value| value.is_finite() && *value >= 0.0)
        {
            let provider = if self.base_url.contains("openrouter.ai") {
                "openrouter".to_string()
            } else {
                self.endpoint_name.clone()
            };
            (provider, Some(cost), "provider_actual".to_string())
        } else {
            (self.endpoint_name.clone(), None, "unknown".to_string())
        };
        let event = crate::commands::costs::UsageEventInput {
            request_id: self.usage_request_id(iteration),
            session_id: self.session_id.clone(),
            task_id: self
                .execution_context
                .as_ref()
                .and_then(|context| context.task_id.clone()),
            surface: surface.to_string(),
            provider,
            endpoint: self.endpoint_name.clone(),
            model: self.model_id.clone(),
            input_tokens: usage.prompt_tokens as i64,
            output_tokens: usage.completion_tokens as i64,
            reasoning_tokens: usage
                .completion_tokens_details
                .as_ref()
                .map_or(0, |details| details.reasoning_tokens as i64),
            cached_tokens: usage
                .prompt_tokens_details
                .as_ref()
                .map_or(0, |details| details.cached_tokens as i64),
            actual_cost_usd,
            estimated_cost_usd: None,
            cost_source,
            created_at: None,
        };
        match crate::commands::costs::record_usage_event(&self.db, event).await {
            Ok(true) => {
                if let Some(app) = &self.app {
                    use tauri::Emitter;
                    app.emit("model-usage-recorded", &self.session_id).ok();
                }
                // Compatibility for the existing footer during the migration.
                if let Some(app) = &self.app {
                    use tauri::Emitter;
                    app.emit("token-usage-recorded", &self.session_id).ok();
                }
            }
            Ok(false) => {}
            Err(error) => tracing::warn!("failed to record request usage: {error}"),
        }
    }

    fn usage_request_id(&self, iteration: usize) -> String {
        format!("{}:{iteration}", self.usage_run_id)
    }

    /// Mark this loop as an anonymous/ephemeral run: disables ALL DB
    /// persistence (messages, cost entries). Chainable —
    /// `AgentLoop::new(..).anonymous()`. Used by `send_message_anonymous`.
    pub fn anonymous(mut self) -> Self {
        self.anonymous = true;
        self
    }

    /// Attach a cooperative cancellation flag for this chat turn. The run loop
    /// checks it at the top of each round and stops cleanly if set. Only the
    /// interactive chat path wires this (via the `cancel_chat` command); every
    /// other call site leaves it `None`, so their behavior is unchanged.
    pub fn with_cancel(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    fn emit_cancelled_done(&self) {
        tracing::info!("chat turn cancelled by user (session {})", self.session_id);
        self.events.emit(StreamEvent::Done {
                    input_tokens: 0,
                    output_tokens: 0,
                });
    }

    pub async fn run(&mut self, history: Vec<Message>) -> Result<()> {
        let mut tool_defs = tools::all_definitions();
        // Append MCP tools as additional tool definitions
        let mcp_tools = self.mcp_manager.list_all_tools().await;
        for mcp_tool in &mcp_tools {
            tool_defs.push(mcp_tool_to_definition(mcp_tool));
        }
        // Anonymous runs must leave NO DB trace. The knowledge tools
        // (kb_search / kb_get_chunk) write a `retrieval_events` audit row —
        // including the user's query text — keyed on the session, so withhold
        // them entirely from anonymous chats (KB access is off in no-trace mode).
        if self.anonymous {
            tool_defs
                .retain(|d| d.function.name != "kb_search" && d.function.name != "kb_get_chunk");
        }
        // Assemble the system prompt under ONE shared budget: the fixed base
        // persona (always kept), then project knowledge (memory/README/config),
        // enabled skills, and the user's preferences/learnings. Blocks render in
        // this order, but the budget is allocated by priority — so when context
        // is tight the least-important blocks (config, then README) yield first
        // while preferences and memory survive. Wiring prefs/learnings here is
        // what lets the chat the user actually talks to honor them (the
        // post-mortem loop captured them but only spec decomposition read them).
        // See `agent::context_budget`.
        let cwd_str = self.cwd.to_string_lossy();
        let mut blocks = project_knowledge_blocks(&self.cwd);
        let skill_bodies = match &self.app {
            Some(app) => crate::commands::skills::enabled_skill_prompts(app).await,
            None => crate::commands::skills::enabled_user_skill_prompts().await,
        };
        for body in skill_bodies {
            blocks.push(context_budget::Block::new(
                format!("---\n\n{body}"),
                2,
                4000,
            ));
        }
        let user_ctx = user_context::build_prefs_and_learnings(&self.db, &cwd_str).await;
        if !user_ctx.is_empty() {
            blocks.push(context_budget::Block::new(user_ctx, 0, 2500));
        }
        let mut system_prompt = context_budget::assemble(
            base_system_prompt(self.mode, &self.cwd),
            blocks,
            SYSTEM_PROMPT_BUDGET,
        );
        // Model-aware reinforcement for post-approval Execute turns (no-op for
        // high-compliance models and all non-Execute turns).
        system_prompt.push_str(compliance_booster(self.mode, &self.model_id));
        // Delivery-chain readiness: surface a broken chain (e.g. missing
        // GitHub token) in the model's FIRST reply instead of letting it be
        // discovered when deliver_changes blocks after the work is done.
        {
            let settings = self.settings.read().await;
            if let Some(note) = delivery::delivery_readiness_note(&self.cwd, &settings) {
                system_prompt.push_str(&note);
            }
        }
        let api_style = self.api_style.clone();

        match api_style {
            // ChatGPT shares the OpenAI orchestration loop (same ChatMessage
            // shape, tool loop, persistence, events). Only the per-round model
            // call differs — run_openai picks call_chatgpt_model when needed.
            ApiStyle::Openai | ApiStyle::Chatgpt => {
                self.run_openai(history, &tool_defs, &system_prompt)
                    .await
            }
            ApiStyle::Anthropic => {
                self.run_anthropic(history, &tool_defs, &system_prompt)
                    .await
            }
        }
    }

    fn audit_session_id(&self) -> String {
        self.execution_context
            .as_ref()
            .and_then(|ctx| ctx.parent_session_id.clone())
            .unwrap_or_else(|| self.session_id.clone())
    }

    async fn run_openai(
        &mut self,
        history: Vec<Message>,
        tool_defs: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<()> {
        let completion_instruction = history
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.clone())
            .unwrap_or_default();
        let mut messages = self.build_openai_messages(history, system_prompt);
        // Proactive capability match: strip images BEFORE the first request
        // when the model is KNOWN text-only, instead of burning a 400 round
        // trip every turn. The reactive strip-and-retry stays as the net for
        // unknown models and wrong guesses.
        {
            let settings = self.settings.read().await;
            if !context::model_supports_vision(&settings, &self.endpoint_name, &self.model_id) {
                let stripped = strip_image_parts(&mut messages);
                if stripped > 0 {
                    let notice = format!(
                        "当前模型不支持图片输入,已在发送前将历史中的 {stripped} 张图片替换为\
占位文本;切换到支持图片的模型可恢复图片理解。"
                    );
                    self.persist_gate_message_once("已在发送前", &notice, "turn_notice")
                        .await?;
                }
            }
        }
        // Headless runs have no frontend and no hooks. Building `Option<_>`
        // (None when `app` is absent) keeps `HookRunner` — which owns an
        // `AppHandle` — constructed ONLY here, inside `run()`, which the
        // unit-test EXE dead-strips. Never construct it in test code: an
        // AppHandle-owning struct instantiated in `codefactory_lib-*.exe`
        // trips the Windows loader (`STATUS_ENTRYPOINT_NOT_FOUND`, #166).
        let hook_runner: Option<hooks::HookRunner> = match &self.app {
            None => None,
            Some(app) => Some(if self.anonymous {
                hooks::HookRunner::disabled(app.clone())
            } else {
                let settings = self.settings.read().await;
                hooks::HookRunner::from_settings(&settings, app.clone())
            }),
        };

        // Did we emit a terminal Done/Error this run? Used to guarantee the
        // stream always closes even if the loop runs to its iteration ceiling.
        let mut emitted_terminal = false;
        let mut completion_gate =
            CompletionGate::new_for_instruction(false, &completion_instruction);
        let mut completion_sequence = 0_u64;
        let mut last_completion_nudge_sequence = None;
        let mut progress_tracker = ProgressTracker::new(8);
        let mut finalization_pending = false;
        let mut completion_recovery_attempts = 0_u32;
        let mut fact_check_used = false;
        let mut require_tool_next = false;
        let max_iterations = self.mode.max_iterations();
        for iteration in 0..max_iterations {
            // Cooperative cancellation: if the user hit "stop" for this chat
            // turn, end the stream cleanly between rounds. Checked here (not
            // mid tool-call) so in-flight work isn't hard-killed. No-op unless
            // a cancel flag was attached (chat only) and has actually tripped.
            if self.is_cancelled() {
                self.emit_cancelled_done();
                emitted_terminal = true;
                break;
            }
            // ── Context-window management ────────────────────────────────────
            // Estimate prompt tokens before sending. If we're over 75% of the
            // model's window, elide oversized tool results from the older
            // half. Notify the UI so the user knows what happened.
            let (context_limit, max_context_limit) = {
                let settings = self.settings.read().await;
                let window = context::resolve_context_window(
                    &settings,
                    &self.endpoint_name,
                    &self.model_id,
                    None,
                );
                let estimated = context::estimate_prompt_tokens(&messages, system_prompt);
                (window.select_limit(estimated), window.max_limit)
            };
            let compression = context::compress_if_needed(
                std::mem::take(&mut messages),
                system_prompt,
                context_limit,
            );
            // Storage repair is not enough: context compression can change the
            // final provider payload. Enforce the OpenAI tool-call protocol at
            // the last possible boundary before every model request.
            messages = repair_openai_tool_protocol(compression.messages);
            if compression.compressed {
                self.events.emit(StreamEvent::ContextCompressed {
                            elided_count: compression.elided_count,
                            tokens_freed: compression.tokens_freed,
                        });
            }

            let active_tool_defs = active_tool_definitions(tool_defs, finalization_pending);
            let required_tool_response = require_tool_next && !finalization_pending;
            let call_result = self
                .call_openai_transport(
                    &messages,
                    active_tool_defs,
                    required_tool_response,
                )
                .await;
            let (text, tool_calls, usage, reasoning) = match call_result {
                Ok(ok) => ok,
                // The active model rejects image input (e.g. the user switched
                // the session to a no-vision model with image attachments in
                // history). Strip images to placeholders and retry ONCE —
                // otherwise every「继续」replays the same history and dies the
                // same death (2026-07-21 field report).
                // Provider says the prompt is over the window: the resolved
                // window metadata or the token estimate was wrong. Emergency-
                // compress against a reduced budget and retry ONCE — a killed
                // turn on a replayable history dies identically on every
                //「继续」(2026-07-21: three context-window deaths in one day).
                Err(e) if context::is_context_overflow(&e.to_string()) => {
                    let emergency_limit = (context_limit / 5).max(1) * 4;
                    let compression = context::compress_if_needed(
                        std::mem::take(&mut messages),
                        system_prompt,
                        emergency_limit,
                    );
                    if !compression.compressed {
                        return Err(e);
                    }
                    messages = repair_openai_tool_protocol(compression.messages);
                    let notice = format!(
                        "上下文超出模型窗口,已压缩 {} 条历史(约释放 {} tokens)后重试。",
                        compression.elided_count, compression.tokens_freed
                    );
                    self.persist_gate_message(&notice, "turn_notice").await?;
                    self.events.emit(StreamEvent::CompletionGateAction {
                                kind: "turn_notice".into(),
                                detail: notice.clone(),
                            });
                    self.call_openai_transport(
                        &messages,
                        active_tool_defs,
                        required_tool_response,
                    )
                    .await?
                }
                Err(e) if is_vision_rejection(&e.to_string()) => {
                    let stripped = strip_image_parts(&mut messages);
                    if stripped == 0 {
                        return Err(e);
                    }
                    let notice = format!(
                        "已自动移除历史中的 {stripped} 张图片后重试:当前模型不支持图片输入。\
如需图片理解,请切换回支持图片的模型。"
                    );
                    self.persist_gate_message(&notice, "turn_notice").await?;
                    self.events.emit(StreamEvent::CompletionGateAction {
                                kind: "turn_notice".into(),
                                detail: notice.clone(),
                            });
                    self.call_openai_transport(
                        &messages,
                        active_tool_defs,
                        required_tool_response,
                    )
                    .await?
                }
                Err(e) => return Err(e),
            };
            finalization_pending = false;
            require_tool_next = false;

            // The provider request has completed. Persist already-consumed
            // Usage before honoring a cancellation that arrived in flight.
            let usage_request_id = self.usage_request_id(iteration);
            if let Some(round_usage) = usage.as_ref() {
                self.record_usage_event_for_round(round_usage, iteration)
                    .await;
            }

            if self.is_cancelled() {
                self.emit_cancelled_done();
                emitted_terminal = true;
                break;
            }

            // Emit real (provider-reported) context-usage right after each
            // round-trip so the UI bar tracks actual usage, not just our
            // estimate. The estimate is only used to *trigger* compression.
            if let Some(u) = &usage {
                self.events.emit(StreamEvent::ContextUsage {
                            used_tokens: u.prompt_tokens,
                            limit_tokens: context_limit,
                            max_limit_tokens: max_context_limit,
                        });
            }

            // Persist assistant turn — include tool_calls AND reasoning_content
            // so history reconstructs faithfully. Reasoning replay is required
            // by DeepSeek's reasoner family.
            let assistant_message_id =
                if !text.is_empty() || !tool_calls.is_empty() || reasoning.is_some() {
                    self.persist_message(
                        "assistant",
                        &text,
                        usage.as_ref(),
                        if tool_calls.is_empty() {
                            None
                        } else {
                            Some(&tool_calls)
                        },
                        reasoning.as_deref(),
                        Some(&usage_request_id),
                    )
                    .await?
                } else {
                    None
                };
            if let Some(message_id) = assistant_message_id.as_deref() {
                for tc in &tool_calls {
                    let args = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    crate::trajectory::record_tool_call_started(
                        &self.db,
                        &self.session_id,
                        message_id,
                        &tc.id,
                        &tc.function.name,
                        &args,
                    )
                    .await?;
                }
            }

            if tool_calls.is_empty() {
                // Systemic fact-check: a tool-call-free reply asserting a
                // machine-verifiable obstacle (delivery blocked / command
                // missing / waiting on a checkable condition) gets ONE live
                // probe-backed correction — facts over stale memory.
                if !fact_check_used {
                    if let Some(correction) = fact_check_reply(&text) {
                        fact_check_used = true;
                        self.persist_gate_message(&correction, "turn_notice").await?;
                        self.events.emit(StreamEvent::CompletionGateAction {
                                    kind: "turn_notice".into(),
                                    detail: correction.clone(),
                                });
                        messages.push(ChatMessage {
                            role: "user".into(),
                            content: MessageContent::Text(correction),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                            reasoning_content: None,
                        });
                        continue;
                    }
                }
                let evidence = completion_gate.evidence();
                match completion_finalization(&evidence, completion_recovery_attempts, self.mode) {
                    CompletionFinalization::Recover(prompt) => {
                        completion_recovery_attempts += 1;
                        require_tool_next = completion_recovery_requires_tool(self.mode);
                        // Make the rejection visible instead of silently looping:
                        // collapse the rejected candidate in the UI, persist the
                        // injected instruction so rebuilt history stays faithful.
                        self.mark_rejected_candidate(assistant_message_id.as_deref())
                            .await?;
                        self.persist_gate_message(&prompt, "gate_recovery").await?;
                        self.events.emit(StreamEvent::CompletionGateAction {
                                    kind: "recovery".into(),
                                    detail: evidence.blockers.join("; "),
                                });
                        messages.push(ChatMessage {
                            role: "user".into(),
                            content: MessageContent::Text(prompt),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                            reasoning_content: None,
                        });
                        continue;
                    }
                    CompletionFinalization::ReleaseWithWarning(warning) => {
                        // The reply stands — no folding, no Error. Persist a
                        // visible warning and fall through to the normal Done.
                        self.persist_gate_message(&warning, "gate_warning").await?;
                        self.events.emit(StreamEvent::CompletionGateAction {
                                    kind: "warning".into(),
                                    detail: warning.clone(),
                                });
                    }
                    CompletionFinalization::Blocked(message) => {
                        self.mark_rejected_candidate(assistant_message_id.as_deref())
                            .await?;
                        self.persist_gate_message(&message, "gate_blocked").await?;
                        self.events.emit(StreamEvent::Error { message });
                        emitted_terminal = true;
                        break;
                    }
                    CompletionFinalization::Complete => {}
                }
                // Always emit a terminal Done so the frontend's `streaming`
                // flag clears — even when the provider omitted usage on the
                // final turn. Previously Done was gated behind `usage`, so a
                // missing usage left the chat hung "running" forever.
                let (done_in, done_out) = usage
                    .as_ref()
                    .map(|u| (u.prompt_tokens, u.completion_tokens))
                    .unwrap_or((0, 0));
                self.events.emit(StreamEvent::Done {
                            input_tokens: done_in,
                            output_tokens: done_out,
                        });
                emitted_terminal = true;
                break;
            }

            let mut result_messages = Vec::new();
            let mut progress_prompt = None;
            let completion_evidence_before_tool_batch = completion_gate.evidence();

            for (tool_index, tc) in tool_calls.iter().enumerate() {
                if let Some(remaining) =
                    cancelled_tool_suffix(self.cancel.as_ref(), &tool_calls, tool_index)
                {
                    return self
                        .finish_cancelled_tool_batch(remaining)
                        .await;
                }
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                let completion_args = args.clone();

                // Extract bash command for finer-grained permission matching
                let bash_cmd = if tc.function.name == "bash" {
                    args.get("command")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };

                self.events.emit(StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args: args.clone(),
                        });

                let permission_policy = {
                    let settings = self.settings.read().await;
                    settings.permissions.clone()
                };

                let decision =
                    decide_permission(&permission_policy, &tc.function.name, bash_cmd.as_deref());

                let remaining = max_iterations.saturating_sub(iteration + 1) as u32;
                let completion_evidence = completion_gate.evidence();
                let denial_content = if let Some(content) = autonomous_budget_denial(
                    self.mode,
                    remaining,
                    &completion_evidence,
                    &tc.function.name,
                    &args,
                    &self.cwd,
                ) {
                    Some(content)
                } else {
                    match decision {
                        PermissionDecision::Allow => None,
                        PermissionDecision::Ask => {
                            match self.request_permission(tc, args.clone()).await {
                                PermissionResponse::Allow => None,
                                PermissionResponse::Deny => Some(
                                    "Tool call denied by user. Please try a different approach."
                                        .to_string(),
                                ),
                                PermissionResponse::Cancelled => {
                                    return self
                                        .finish_cancelled_tool_batch(
                                            &tool_calls[tool_index..],
                                        )
                                        .await;
                                }
                            }
                        }
                        PermissionDecision::Deny(reason) => {
                            tracing::warn!("Tool '{}' denied: {reason}", tc.function.name);
                            Some(format!(
                                "Tool call denied: {reason}. Please try a different approach."
                            ))
                        }
                    }
                };

                if let Some(content) = denial_content {
                    self.record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    self.events.emit(StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
                            });
                    result_messages.push(ChatMessage {
                        role: "tool".into(),
                        content: MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.function.name.clone()),
                        reasoning_content: None,
                    });
                    continue;
                }

                // Pre-tool hook: may cancel. Headless (no hooks) always allows.
                let pre_allowed = match &hook_runner {
                    Some(runner) => {
                        runner
                            .fire(hooks::HookEvent::PreTool {
                                tool_name: tc.function.name.clone(),
                                args: args.clone(),
                            })
                            .await
                    }
                    None => true,
                };
                if !pre_allowed {
                    let content = "Tool call cancelled by hook.".to_string();
                    self.record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    self.events.emit(StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
                            });
                    result_messages.push(ChatMessage {
                        role: "tool".into(),
                        content: MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.function.name.clone()),
                        reasoning_content: None,
                    });
                    continue;
                }

                // Tool execution now flows through the shared ToolBackend seam
                // (keystone slice 4.3): the desktop backend builds the ExecCtx
                // and runs MCP-first / native-dispatch. Timing stays in the loop
                // so it covers the fatal-error path exactly as before.
                let tool_backend = tool_backend::DesktopToolBackend {
                    #[cfg(not(test))]
                    app: self.app.clone(),
                    db: self.db.clone(),
                    mcp_manager: self.mcp_manager.clone(),
                    settings: self.settings.clone(),
                };
                let tool_ctx = codefactory_agent_loop::tool::ToolCtx {
                    working_directory: self.cwd.clone(),
                    session_id: Some(self.audit_session_id()),
                    task_id: self
                        .execution_context
                        .as_ref()
                        .and_then(|ctx| ctx.task_id.clone()),
                    knowledge_library_ids: knowledge_scope_for_tools(
                        self.execution_context.as_ref(),
                    ),
                    timeout_sec: None,
                };

                let tool_start = std::time::Instant::now();
                let exec_result = tool_backend.execute(tc, &args, &tool_ctx).await;
                let duration_ms = tool_start.elapsed().as_millis() as u64;
                let output = match exec_result {
                    Ok(result) => tools::ToolOutput {
                        content: result.content,
                        is_error: result.is_error,
                    },
                    Err(error) => {
                        let error_text = error.to_string();
                        self.record_tool_call_outcome(
                            tc,
                            "error",
                            None,
                            Some(&error_text),
                            duration_ms,
                        )
                        .await?;
                        return Err(crate::errors::AppError::Other(error_text));
                    }
                };
                self.record_tool_call_outcome(
                    tc,
                    if output.is_error { "error" } else { "done" },
                    if output.is_error {
                        None
                    } else {
                        Some(&output.content)
                    },
                    if output.is_error {
                        Some(&output.content)
                    } else {
                        None
                    },
                    duration_ms,
                )
                .await?;

                if let Some(prompt) = record_completion_outcome(
                    &mut completion_gate,
                    &mut progress_tracker,
                    &mut completion_sequence,
                    &self.cwd,
                    &tc.function.name,
                    &completion_args,
                    &output,
                ) {
                    progress_prompt = Some(prompt);
                }
                // Post-tool hook (skipped headless — no hooks).
                if let Some(runner) = &hook_runner {
                    runner
                        .fire(hooks::HookEvent::PostTool {
                            tool_name: tc.function.name.clone(),
                            result: output.content.chars().take(500).collect(),
                            duration_ms,
                        })
                        .await;
                }

                self.events.emit(StreamEvent::ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: output.content.clone(),
                            is_error: output.is_error,
                            status: if output.is_error {
                                "error".into()
                            } else {
                                "done".into()
                            },
                        });

                result_messages.push(ChatMessage {
                    role: "tool".into(),
                    content: MessageContent::Text(output.content),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(tc.function.name.clone()),
                    reasoning_content: None,
                });
            }

            completion_recovery_attempts = completion_recovery_attempts_after_tool_batch(
                completion_recovery_attempts,
                completion_evidence_made_progress(
                    &completion_evidence_before_tool_batch,
                    &completion_gate.evidence(),
                ),
            );

            messages.push(ChatMessage {
                role: "assistant".into(),
                content: MessageContent::Text(text),
                tool_calls: Some(tool_calls),
                tool_call_id: None,
                name: None,
                reasoning_content: reasoning,
            });
            messages.extend(result_messages);
            if let Some(prompt) = progress_prompt {
                messages.push(ChatMessage {
                    role: "user".into(),
                    content: MessageContent::Text(prompt),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
            }
            let evidence = completion_gate.evidence();
            if completion_ready_applies(self.mode)
                && evidence.completed
                && evidence.last_successful_verification_sequence != last_completion_nudge_sequence
            {
                last_completion_nudge_sequence = evidence.last_successful_verification_sequence;
                finalization_pending = true;
                self.persist_gate_message(build_completion_ready_prompt(), "gate_ready")
                    .await?;
                self.events.emit(StreamEvent::CompletionGateAction {
                            kind: "ready".into(),
                            detail: String::new(),
                        });
                messages.push(ChatMessage {
                    role: "user".into(),
                    content: MessageContent::Text(build_completion_ready_prompt().to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
            } else if self.mode != AgentMode::Interactive {
                let remaining = max_iterations.saturating_sub(iteration + 1);
                if should_prompt_budget_convergence(remaining as u32) {
                    messages.push(ChatMessage {
                        role: "user".into(),
                        content: MessageContent::Text(build_budget_convergence_prompt(
                            remaining as u32,
                            &evidence,
                        )),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        reasoning_content: None,
                    });
                }
            }
        }

        // Safety net: close the stream when every round used tools, but preserve
        // completion truth instead of converting an exhausted loop into success.
        if !emitted_terminal {
            let evidence = completion_gate.evidence();
            tracing::warn!(
                "agent loop hit the iteration ceiling ({}) without a terminal turn; completed={}",
                self.mode.max_iterations(),
                evidence.completed,
            );
            self.events.emit(iteration_ceiling_terminal_event(&evidence, self.mode));
        }

        Ok(())
    }

    async fn request_permission(
        &self,
        tc: &ToolCall,
        args: serde_json::Value,
    ) -> PermissionResponse {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.pending_permissions
            .lock()
            .await
            .insert(tc.id.clone(), sender);

        self.events.emit(StreamEvent::PermissionRequest {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    args,
                });
        {
            let settings = self.settings.read().await;
            crate::notify::send(
                &settings,
                crate::notify::NotifyEvent::PermissionWaiting,
                format!("工具 {} 正在等待你的批准", tc.function.name),
            );
        }

        let allow =
            await_permission_response(receiver, self.cancel.as_ref(), Duration::from_secs(600))
                .await;
        self.pending_permissions.lock().await.remove(&tc.id);
        allow
    }

    async fn finish_cancelled_tool_batch(
        &self,
        remaining: &[ToolCall],
    ) -> Result<()> {
        let contents =
            persist_cancelled_tool_batch(&self.db, &self.session_id, self.anonymous, remaining)
                .await?;
        for (index, (tc, content)) in remaining.iter().zip(contents).enumerate() {
            if index > 0 {
                let args = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                self.events.emit(StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args,
                        });
            }
            self.events.emit(StreamEvent::ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: content.clone(),
                        is_error: true,
                        status: "cancelled".into(),
                    });
        }
        self.events.emit(StreamEvent::Done {
                    input_tokens: 0,
                    output_tokens: 0,
                });
        Ok(())
    }

    /// Flatten a message body to plain text for protocol fields that cannot
    /// carry multimodal content (system instructions and tool output).
    fn content_to_text(content: &MessageContent) -> String {
        match content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Parts(parts) => serde_json::to_value(parts)
                .ok()
                .and_then(|v| {
                    v.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                })
                .unwrap_or_default(),
        }
    }

    /// Convert a user message from the Chat Completions-compatible shape used
    /// internally to ChatGPT Responses content items without dropping images.
    fn content_to_chatgpt_user_parts(content: &MessageContent) -> Vec<serde_json::Value> {
        match content {
            MessageContent::Text(text) => vec![serde_json::json!({
                "type": "input_text",
                "text": text,
            })],
            MessageContent::Parts(parts) => {
                let mut response_parts = Vec::with_capacity(parts.len());
                for part in parts {
                    if part.r#type == "image_url" {
                        if let Some(image_url) = part
                            .image_url
                            .as_ref()
                            .map(|image| image.url.as_str())
                            .filter(|url| !url.is_empty())
                        {
                            response_parts.push(serde_json::json!({
                                "type": "input_image",
                                "image_url": image_url,
                            }));
                        }
                    } else if let Some(text) = &part.text {
                        response_parts.push(serde_json::json!({
                            "type": "input_text",
                            "text": text,
                        }));
                    }
                }

                if response_parts.is_empty() {
                    response_parts.push(serde_json::json!({
                        "type": "input_text",
                        "text": Self::content_to_text(content),
                    }));
                }
                response_parts
            }
        }
    }

    /// Per-round model call against the ChatGPT backend **Responses API**
    /// (subscription). Self-resolves the OAuth access token (refreshing as
    /// needed) via `codex_auth`, so no API key is threaded through. Translates
    /// the OpenAI-shaped ChatMessage history into Responses `instructions` +
    /// `input` items, parses the Responses SSE stream, and returns the same
    /// (text, tool_calls, usage, reasoning) contract as call_openai_model.
    async fn call_openai_transport(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        require_tool: bool,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>, Option<String>)> {
        let first = match self.api_style {
            ApiStyle::Chatgpt => {
                self.call_chatgpt_model(messages, tool_defs, require_tool)
                    .await
            }
            _ => {
                self.call_openai_model(messages, tool_defs, require_tool)
                    .await
            }
        };
        let required_choice_unsupported = first.as_ref().err().is_some_and(|error| {
            require_tool && provider_rejects_required_tool_choice(&error.to_string())
        });
        if !required_choice_unsupported {
            return first;
        }
        match self.api_style {
            ApiStyle::Chatgpt => {
                self.call_chatgpt_model(messages, tool_defs, false)
                    .await
            }
            _ => {
                self.call_openai_model(messages, tool_defs, false)
                    .await
            }
        }
    }

    async fn call_chatgpt_model(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        require_tool: bool,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>, Option<String>)> {
        let finalization_response = tool_defs.is_empty();

        let (access_token, account_id) = crate::codex_auth::valid_access_token().await?;
        // The ChatGPT backend URL is fixed — use the canonical constant rather
        // than the endpoint's base_url so the request always lands correctly.
        let url = format!("{}/responses", crate::codex_auth::CHATGPT_BASE_URL);

        // ── ChatMessage history → Responses instructions + input items ──
        let mut instructions = String::new();
        let mut input: Vec<serde_json::Value> = Vec::new();
        for m in messages {
            match m.role.as_str() {
                "system" => instructions = Self::content_to_text(&m.content),
                "tool" => input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": Self::content_to_text(&m.content),
                })),
                "assistant" => {
                    let text = Self::content_to_text(&m.content);
                    if !text.is_empty() {
                        input.push(serde_json::json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}],
                        }));
                    }
                    if let Some(tcs) = &m.tool_calls {
                        for tc in tcs {
                            input.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": tc.id,
                                "name": tc.function.name,
                                "arguments": tc.function.arguments,
                            }));
                        }
                    }
                }
                _ => input.push(serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": Self::content_to_chatgpt_user_parts(&m.content),
                })),
            }
        }

        // Tools → Responses shape (function fields flattened, no "function" nest).
        let tools: Vec<serde_json::Value> = tool_defs
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": t.function.parameters,
                })
            })
            .collect();

        // Reasoning effort: a per-session override (sessions.reasoning_effort)
        // wins; otherwise the global Settings default (which itself defaults to
        // Medium for older configs).
        let session_effort: Option<String> = sqlx::query_scalar::<_, Option<String>>(
            "SELECT reasoning_effort FROM sessions WHERE id = ?",
        )
        .bind(&self.session_id)
        .fetch_one(&self.db)
        .await
        .ok()
        .flatten();
        let effort = {
            let settings = self.settings.read().await;
            resolve_chatgpt_reasoning_effort(
                &settings,
                &self.endpoint_name,
                &self.model_id,
                session_effort.as_deref(),
            )
            .as_str()
        };

        let mut body = serde_json::json!({
            "model": self.model_id,
            "instructions": instructions,
            "input": input,
            "tool_choice": if tools.is_empty() {
                "none"
            } else if require_tool {
                "required"
            } else {
                "auto"
            },
            "parallel_tool_calls": false,
            "store": false,
            "stream": true,
            "reasoning": { "effort": effort, "summary": "auto" },
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);
        }

        let response = crate::http_util::send_with_retry_and_notify(
            "ChatGPT Responses stream request",
            || {
                let mut request = self
                    .http
                    .post(&url)
                    .bearer_auth(&access_token)
                    .header("OpenAI-Beta", "responses=experimental")
                    .header("originator", "codex_cli_rs")
                    .header("session_id", &self.session_id)
                    .header("Accept", "text/event-stream")
                    .json(&body);
                if let Some(acct) = &account_id {
                    request = request.header("chatgpt-account-id", acct.as_str());
                }
                request
            },
            |notice| Self::emit_transport_retry(self.events.as_ref(), notice),
        )
        .await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(crate::errors::AppError::Other(format!(
                "ChatGPT 后端请求失败（{status}）：{text}"
            )));
        }

        // ── Parse the Responses SSE stream ──
        let mut byte_stream = response.bytes_stream();
        let mut text_buf = String::new();
        let mut reasoning_buf = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage: Option<Usage> = None;
        let mut byte_buffer: Vec<u8> = Vec::with_capacity(4096);
        let mut saw_terminal_marker = false;
        let mut malformed_data_lines = 0_usize;

        'sse: loop {
            let chunk = match next_stream_item(&mut byte_stream, self.cancel.as_ref()).await {
                StreamPoll::Item(Some(chunk)) => chunk,
                StreamPoll::Item(None) | StreamPoll::Cancelled => break,
            };
            let bytes = chunk?;
            byte_buffer.extend_from_slice(&bytes);
            while let Some(nl) = byte_buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = byte_buffer.drain(..=nl).collect();
                let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
                let line = line.trim_end_matches('\r');
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    saw_terminal_marker = true;
                    byte_buffer.clear();
                    break 'sse;
                }
                let Ok(ev) = serde_json::from_str::<serde_json::Value>(data) else {
                    malformed_data_lines += 1;
                    continue;
                };
                match ev.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                    "response.output_text.delta" => {
                        if let Some(d) = ev.get("delta").and_then(|v| v.as_str()) {
                            if !d.is_empty() {
                                if !finalization_response {
                                    self.events.emit(StreamEvent::TextDelta {
                                                content: d.to_string(),
                                            });
                                }
                                text_buf.push_str(d);
                            }
                        }
                    }
                    "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                        if let Some(d) = ev.get("delta").and_then(|v| v.as_str()) {
                            reasoning_buf.push_str(d);
                        }
                    }
                    "response.output_item.done" => {
                        if let Some(item) = ev.get("item") {
                            if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                                let call_id = item
                                    .get("call_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let name = item
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let arguments = item
                                    .get("arguments")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if !call_id.is_empty() && !name.is_empty() {
                                    tool_calls.push(ToolCall {
                                        id: call_id,
                                        r#type: "function".into(),
                                        function: FunctionCall { name, arguments },
                                    });
                                }
                            }
                        }
                    }
                    "response.completed" => {
                        saw_terminal_marker = true;
                        if let Some(u) = ev.get("response").and_then(|r| r.get("usage")) {
                            let inp =
                                u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            let out =
                                u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            usage = Some(Usage {
                                prompt_tokens: inp,
                                completion_tokens: out,
                                total_tokens: inp + out,
                                cost: u.get("cost").and_then(|v| v.as_f64()),
                                prompt_tokens_details: Some(
                                    crate::openrouter::types::PromptTokenDetails {
                                        cached_tokens: u
                                            .pointer("/input_tokens_details/cached_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                    },
                                ),
                                completion_tokens_details: Some(
                                    crate::openrouter::types::CompletionTokenDetails {
                                        reasoning_tokens: u
                                            .pointer("/output_tokens_details/reasoning_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                    },
                                ),
                            });
                        }
                    }
                    "response.failed" => {
                        let msg = ev
                            .get("response")
                            .and_then(|r| r.get("error"))
                            .and_then(|e| e.get("message"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("response.failed")
                            .to_string();
                        return Err(crate::errors::AppError::Other(format!(
                            "ChatGPT 后端返回错误：{msg}"
                        )));
                    }
                    _ => {}
                }
            }
        }

        if self.is_cancelled() {
            let reasoning = (!reasoning_buf.is_empty()).then_some(reasoning_buf);
            return Ok((text_buf, Vec::new(), usage, reasoning));
        }

        validate_openai_sse_completion(
            saw_terminal_marker,
            byte_buffer.len(),
            malformed_data_lines,
        )
        .map_err(crate::errors::AppError::Other)?;

        if finalization_response {
            text_buf = sanitize_completion_summary(&text_buf);
            tool_calls.clear();
            self.events.emit(StreamEvent::TextDelta {
                        content: text_buf.clone(),
                    });
        }
        let reasoning = if reasoning_buf.is_empty() {
            None
        } else {
            Some(reasoning_buf)
        };
        Ok((text_buf, tool_calls, usage, reasoning))
    }

    async fn call_openai_model(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        require_tool: bool,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>, Option<String>)> {
        let finalization_response = tool_defs.is_empty();
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // Strip OpenRouter-style "vendor/" prefix when talking to a direct
        // provider API. Defensive against ids that linger from earlier
        // OpenRouter use after the user switches endpoint.
        let outbound_model =
            crate::config::settings::normalize_model_id(&self.model_id, &self.base_url);

        let (tools, tool_choice) = openai_tool_controls(tool_defs, require_tool);
        let req = ChatRequest {
            model: outbound_model,
            messages: messages.to_vec(),
            tools,
            tool_choice: Some(tool_choice),
            stream: true,
            temperature: 0.2,
            max_tokens: 8192,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
        };

        // Send the request as-is — including `max_tokens` + `temperature`. We do
        // NOT pre-rewrite by model name: providers and proxies routinely serve
        // GPT-5-named models that accept the legacy fields just fine, and forcing
        // `max_completion_tokens` (plus dropping `temperature`) on them breaks
        // chat — the regression introduced by the name-based v1.19.2 attempt and
        // reported as "1.15 worked, recent builds don't". We adapt REACTIVELY
        // below, only when the server itself rejects `max_tokens`.
        let mut body = serde_json::to_value(&req)?;

        let mut response = crate::http_util::send_with_retry_and_notify(
            "OpenAI-compatible chat stream request",
            || {
                self.http
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .header("X-Title", "CodeFactory")
                    .json(&body)
            },
            |notice| Self::emit_transport_retry(self.events.as_ref(), notice),
        )
        .await?;

        // Reactive safety net for the GPT-5 / o-series `max_tokens` rejection.
        // The name-based adaptation above handles the common ids, but providers,
        // proxies and Azure deployments expose these models under names we can't
        // anticipate (`gpt5`, custom aliases, deployment ids). When the server
        // itself answers 400 "use 'max_completion_tokens' instead", honor it
        // once — regardless of model name — and resend. Makes the fix
        // name-independent so it can't silently miss a model.
        if response.status().as_u16() == 400 && body.get("max_tokens").is_some() {
            let err_text = response.text().await.unwrap_or_default();
            if err_text.contains("max_completion_tokens") {
                crate::config::settings::force_max_completion_tokens(&mut body);
                response = crate::http_util::send_with_retry_and_notify(
                    "OpenAI-compatible chat stream request after max_tokens adaptation",
                    || {
                        self.http
                            .post(&url)
                            .bearer_auth(&self.api_key)
                            .header("X-Title", "CodeFactory")
                            .json(&body)
                    },
                    |notice| Self::emit_transport_retry(self.events.as_ref(), notice),
                )
                .await?;
            } else {
                // A different 400 — surface the provider's real reason.
                return Err(crate::errors::AppError::Other(format!(
                    "HTTP 400 Bad Request: {err_text}"
                )));
            }
        }
        // Capture the response body on HTTP errors so the user sees the
        // provider's actual rejection reason (bad model id, unsupported
        // field, etc.) rather than just "HTTP 400".
        let response = crate::http_util::check_status(response).await?;

        let mut byte_stream = response.bytes_stream();
        let mut text_buf = String::new();
        let mut reasoning_buf = String::new();
        let mut tc_map: HashMap<u32, (String, String, String)> = HashMap::new();
        let mut usage: Option<Usage> = None;
        let mut saw_terminal_marker = false;
        let mut malformed_data_lines = 0_usize;

        // SSE line buffering — critical correctness fix.
        //
        // The previous implementation processed each TCP chunk as a self-
        // contained block of lines: `from_utf8_lossy(&bytes).lines()`.
        // When a single SSE event ("data: {...}\n") straddled two chunks,
        // chunk-1 ended with a truncated JSON line that failed to parse
        // and was silently skipped, and chunk-2 started mid-string also
        // failing — the entire event was dropped. The symptoms in
        // production: bash commands missing characters (`Select-Object`
        // becoming `Select-Obj`), file writes losing trailing content,
        // parallel tool-call arguments arriving as malformed JSON, all
        // diagnosed by the user as "the tool corrupted my command/file".
        //
        // The fix: keep a byte buffer across chunks. Cut lines only at
        // real `\n` boundaries. `\n` is a single ASCII byte, so partial
        // UTF-8 sequences never sit on a cut point and from_utf8_lossy
        // never sees an incomplete codepoint.
        let mut byte_buffer: Vec<u8> = Vec::with_capacity(4096);

        'sse: loop {
            let chunk = match next_stream_item(&mut byte_stream, self.cancel.as_ref()).await {
                StreamPoll::Item(Some(chunk)) => chunk,
                StreamPoll::Item(None) | StreamPoll::Cancelled => break,
            };
            let bytes = chunk?;
            byte_buffer.extend_from_slice(&bytes);

            while let Some(nl_pos) = byte_buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = byte_buffer.drain(..=nl_pos).collect();
                let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
                let line = line.trim_end_matches('\r'); // SSE may use CRLF

                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    saw_terminal_marker = true;
                    byte_buffer.clear();
                    break 'sse;
                }
                let Ok(sc) = serde_json::from_str::<StreamChunk>(data) else {
                    tracing::warn!("dropped malformed SSE data line (len={})", data.len());
                    malformed_data_lines += 1;
                    continue;
                };
                if let Some(u) = sc.usage {
                    usage = Some(u);
                }
                for choice in sc.choices {
                    if choice.finish_reason.is_some() {
                        saw_terminal_marker = true;
                    }
                    let delta = choice.delta;
                    if let Some(t) = delta.content.filter(|s| !s.is_empty()) {
                        if !finalization_response {
                            self.events.emit(StreamEvent::TextDelta { content: t.clone() });
                        }
                        text_buf.push_str(&t);
                    }
                    // DeepSeek reasoner family streams a separate reasoning_content
                    // field. Accumulate it for replay on subsequent turns. We don't
                    // stream it to the UI as TextDelta — keeping the chain-of-thought
                    // out of the visible chat is the right default; expose later via
                    // a "show reasoning" toggle if users want it.
                    if let Some(r) = delta.reasoning_content.filter(|s| !s.is_empty()) {
                        reasoning_buf.push_str(&r);
                    }
                    if let Some(tcs) = delta.tool_calls {
                        for tc in tcs {
                            let e = tc_map.entry(tc.index).or_default();
                            if let Some(id) = tc.id {
                                e.0 = id;
                            }
                            if let Some(f) = tc.function {
                                if let Some(n) = f.name {
                                    e.1 = n;
                                }
                                if let Some(a) = f.arguments {
                                    e.2.push_str(&a);
                                }
                            }
                        }
                    }
                }
            }
        }

        if self.is_cancelled() {
            let reasoning = (!reasoning_buf.is_empty()).then_some(reasoning_buf);
            return Ok((text_buf, Vec::new(), usage, reasoning));
        }

        validate_openai_sse_completion(
            saw_terminal_marker,
            byte_buffer.len(),
            malformed_data_lines,
        )
        .map_err(crate::errors::AppError::Other)?;

        let mut tool_calls: Vec<ToolCall> = tc_map
            .into_iter()
            .filter(|(_, (id, name, _))| !id.is_empty() && !name.is_empty())
            .map(|(_, (id, name, args))| ToolCall {
                id,
                r#type: "function".into(),
                function: FunctionCall {
                    name,
                    arguments: args,
                },
            })
            .collect();
        tool_calls.sort_by_key(|tc| tc.id.clone());

        if finalization_response {
            text_buf = sanitize_completion_summary(&text_buf);
            tool_calls.clear();
            self.events.emit(StreamEvent::TextDelta {
                        content: text_buf.clone(),
                    });
        }

        let reasoning = if reasoning_buf.is_empty() {
            None
        } else {
            Some(reasoning_buf)
        };
        Ok((text_buf, tool_calls, usage, reasoning))
    }

    async fn persist_message(
        &self,
        role: &str,
        content: &str,
        usage: Option<&Usage>,
        tool_calls: Option<&[ToolCall]>,
        reasoning_content: Option<&str>,
        usage_request_id: Option<&str>,
    ) -> Result<Option<String>> {
        // Anonymous runs never touch the DB — the assistant turn lives only in
        // the in-memory `messages` vec for the rest of this run.
        if self.anonymous {
            return Ok(None);
        }
        let msg_id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        let input_tok = usage.map(|u| u.prompt_tokens as i64);
        let output_tok = usage.map(|u| u.completion_tokens as i64);
        let persisted_content = crate::trajectory::redact_derived_message_for_storage(content);
        let persisted_reasoning =
            reasoning_content.map(crate::trajectory::redact_derived_message_for_storage);
        let tool_calls_json = tool_calls
            .filter(|tcs| !tcs.is_empty())
            .map(|tcs| crate::trajectory::redact_tool_calls_for_storage(tcs).unwrap_or_default());

        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, input_tokens, output_tokens, tool_calls, reasoning_content, usage_request_id, created_at) \
             VALUES (?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&msg_id)
        .bind(&self.session_id)
        .bind(role)
        .bind(persisted_content)
        .bind(input_tok)
        .bind(output_tok)
        .bind(tool_calls_json)
        .bind(persisted_reasoning)
        .bind(usage_request_id)
        .bind(now)
        .execute(&self.db)
        .await?;
        Ok(Some(msg_id))
    }

    /// Persist an injected completion-gate instruction as a user-role turn so
    /// rebuilt provider history matches what the model actually saw in this
    /// run, tagged via `completion_state` ("gate_recovery" | "gate_ready") so
    /// the UI renders it as a system notice instead of a user bubble.
    async fn persist_gate_message(&self, content: &str, state: &str) -> Result<()> {
        if self.anonymous {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, completion_state, created_at) \
             VALUES (?,?,?,?,?,?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&self.session_id)
        .bind("user")
        .bind(content)
        .bind(state)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Persist a gate/system notice at most once per session (matched by a
    /// stable `marker` substring) — proactive capability notices would
    /// otherwise repeat on every turn because history is rebuilt each run.
    async fn persist_gate_message_once(
        &self,
        marker: &str,
        content: &str,
        state: &str,
    ) -> Result<()> {
        if self.anonymous {
            return Ok(());
        }
        let existing: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM messages WHERE session_id = ? AND completion_state = ? \
             AND content LIKE ?",
        )
        .bind(&self.session_id)
        .bind(state)
        .bind(format!("%{marker}%"))
        .fetch_one(&self.db)
        .await?;
        if existing.0 > 0 {
            return Ok(());
        }
        self.persist_gate_message(content, state).await
    }

    /// Tag a persisted assistant reply that the completion gate rejected so
    /// the UI collapses it instead of rendering yet another full
    /// near-duplicate answer (2026-07-16 session: seven of them).
    async fn mark_rejected_candidate(&self, message_id: Option<&str>) -> Result<()> {
        let Some(message_id) = message_id else {
            return Ok(());
        };
        if self.anonymous {
            return Ok(());
        }
        sqlx::query("UPDATE messages SET completion_state='rejected_candidate' WHERE id=?")
            .bind(message_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    async fn record_tool_call_outcome(
        &self,
        tool_call: &ToolCall,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
        duration_ms: u64,
    ) -> Result<()> {
        if self.anonymous {
            return Ok(());
        }
        crate::trajectory::record_terminal_tool_outcome(
            &self.db,
            &self.session_id,
            &tool_call.id,
            status,
            result,
            error,
            duration_ms.min(i64::MAX as u64) as i64,
        )
        .await
    }

    fn build_openai_messages(
        &self,
        history: Vec<Message>,
        system_prompt: &str,
    ) -> Vec<ChatMessage> {
        let mut msgs = vec![ChatMessage {
            role: "system".into(),
            content: MessageContent::Text(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }];

        for m in repair_incomplete_tool_history(replayable_history(history)) {
            match m.role.as_str() {
                "tool" => {
                    // Content stored as: {"tool_call_id": "…", "content": "…"}
                    let (tool_call_id, content) = parse_tool_message_content(&m.content);
                    msgs.push(ChatMessage {
                        role: "tool".into(),
                        content: MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tool_call_id),
                        name: None,
                        reasoning_content: None,
                    });
                }
                "assistant" => {
                    // Restore tool_calls if they were persisted as JSON
                    let tool_calls: Option<Vec<ToolCall>> = m
                        .tool_calls
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok());
                    msgs.push(ChatMessage {
                        role: "assistant".into(),
                        content: MessageContent::Text(m.content),
                        tool_calls,
                        tool_call_id: None,
                        name: None,
                        // Replay reasoning_content verbatim so DeepSeek reasoner
                        // models don't 400 on the next turn.
                        reasoning_content: m.reasoning_content,
                    });
                }
                _ => {
                    // Convert markdown file:// image links → vision content
                    // parts when present, leaving plain text untouched.
                    // Missing files / unsupported types fall back to text
                    // silently (see attachments::extract_openai_parts).
                    let parts = attachments::extract_openai_parts(&m.content);
                    let content = if parts.is_empty() {
                        MessageContent::Text(m.content)
                    } else {
                        MessageContent::Parts(parts)
                    };
                    msgs.push(ChatMessage {
                        role: m.role,
                        content,
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        reasoning_content: None,
                    });
                }
            }
        }
        msgs
    }

    // ── Anthropic-specific helpers ────────────────────────────────────────────

    /// Build the Anthropic `messages` array from stored history.
    /// System prompt is passed separately; tool results use `user` role
    /// with `tool_result` content blocks.
    fn build_anthropic_messages(&self, history: Vec<Message>) -> Vec<serde_json::Value> {
        let mut msgs: Vec<serde_json::Value> = Vec::new();

        for m in repair_incomplete_tool_history(replayable_history(history)) {
            match m.role.as_str() {
                "tool" => {
                    let (tool_call_id, content) = parse_tool_message_content(&m.content);
                    msgs.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": content,
                        }]
                    }));
                }
                "assistant" => {
                    // Reconstruct content array: text + tool_use blocks
                    let tool_calls: Vec<ToolCall> = m
                        .tool_calls
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();

                    let mut content_blocks: Vec<serde_json::Value> = Vec::new();
                    if !m.content.is_empty() {
                        content_blocks.push(serde_json::json!({
                            "type": "text",
                            "text": m.content,
                        }));
                    }
                    for tc in &tool_calls {
                        let input: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(serde_json::json!({}));
                        content_blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input,
                        }));
                    }
                    if content_blocks.is_empty() {
                        content_blocks.push(serde_json::json!({ "type": "text", "text": "" }));
                    }
                    msgs.push(serde_json::json!({
                        "role": "assistant",
                        "content": content_blocks,
                    }));
                }
                "system" => {
                    // System is passed as a top-level param, skip inline.
                }
                _ => {
                    // user messages — convert markdown file:// image links
                    // to Anthropic vision content blocks when present.
                    let blocks = attachments::extract_anthropic_blocks(&m.content);
                    let content = if blocks.is_empty() {
                        serde_json::Value::String(m.content)
                    } else {
                        serde_json::Value::Array(blocks)
                    };
                    msgs.push(serde_json::json!({
                        "role": m.role,
                        "content": content,
                    }));
                }
            }
        }
        msgs
    }

    async fn call_anthropic_transport(
        &self,
        system_prompt: &str,
        messages: Vec<serde_json::Value>,
        tool_defs: &[ToolDefinition],
        require_tool: bool,
    ) -> Result<anthropic_client::AnthropicResponse> {
        let first = anthropic_client::stream_anthropic(
            &self.http,
            &self.base_url,
            &self.api_key,
            &self.model_id,
            system_prompt,
            messages.clone(),
            tool_defs,
            require_tool,
            self.cancel.as_ref(),
            self.events.as_ref(),
        )
        .await;
        let required_choice_unsupported = first.as_ref().err().is_some_and(|error| {
            require_tool && provider_rejects_required_tool_choice(&error.to_string())
        });
        if !required_choice_unsupported {
            return first;
        }
        anthropic_client::stream_anthropic(
            &self.http,
            &self.base_url,
            &self.api_key,
            &self.model_id,
            system_prompt,
            messages,
            tool_defs,
            false,
            self.cancel.as_ref(),
            self.events.as_ref(),
        )
        .await
    }

    async fn run_anthropic(
        &mut self,
        history: Vec<Message>,
        tool_defs: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<()> {
        let completion_instruction = history
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.clone())
            .unwrap_or_default();
        let mut messages = self.build_anthropic_messages(history);
        // Proactive capability match — see the OpenAI loop for rationale.
        {
            let settings = self.settings.read().await;
            if !context::model_supports_vision(&settings, &self.endpoint_name, &self.model_id) {
                let stripped = strip_image_values(&mut messages);
                if stripped > 0 {
                    let notice = format!(
                        "当前模型不支持图片输入,已在发送前将历史中的 {stripped} 张图片替换为\
占位文本;切换到支持图片的模型可恢复图片理解。"
                    );
                    self.persist_gate_message_once("已在发送前", &notice, "turn_notice")
                        .await?;
                }
            }
        }
        // Headless runs have no frontend and no hooks. Building `Option<_>`
        // (None when `app` is absent) keeps `HookRunner` — which owns an
        // `AppHandle` — constructed ONLY here, inside `run()`, which the
        // unit-test EXE dead-strips. Never construct it in test code: an
        // AppHandle-owning struct instantiated in `codefactory_lib-*.exe`
        // trips the Windows loader (`STATUS_ENTRYPOINT_NOT_FOUND`, #166).
        let hook_runner: Option<hooks::HookRunner> = match &self.app {
            None => None,
            Some(app) => Some(if self.anonymous {
                hooks::HookRunner::disabled(app.clone())
            } else {
                let settings = self.settings.read().await;
                hooks::HookRunner::from_settings(&settings, app.clone())
            }),
        };

        // Did we emit a terminal Done/Error this run? Used to guarantee the
        // stream always closes even if the loop runs to its iteration ceiling.
        let mut emitted_terminal = false;
        let mut completion_gate =
            CompletionGate::new_for_instruction(false, &completion_instruction);
        let mut completion_sequence = 0_u64;
        let mut last_completion_nudge_sequence = None;
        let mut progress_tracker = ProgressTracker::new(8);
        let mut finalization_pending = false;
        let mut completion_recovery_attempts = 0_u32;
        let mut fact_check_used = false;
        let mut require_tool_next = false;
        let max_iterations = self.mode.max_iterations();
        for iteration in 0..max_iterations {
            // Cooperative cancellation: if the user hit "stop" for this chat
            // turn, end the stream cleanly between rounds. Checked here (not
            // mid tool-call) so in-flight work isn't hard-killed. No-op unless
            // a cancel flag was attached (chat only) and has actually tripped.
            if self.is_cancelled() {
                self.emit_cancelled_done();
                emitted_terminal = true;
                break;
            }
            // We don't run elision compression on the Anthropic path because
            // its messages are serde_json::Value-shaped, not ChatMessage.
            // The OpenAI path is the primary one for now (OpenRouter,
            // DeepSeek, local models). We do still report context usage
            // when Anthropic returns input_tokens, so the UI bar stays
            // accurate. TODO: port elision to work on Value-shaped messages
            // for Anthropic users.
            let (context_limit, max_context_limit) = {
                let settings = self.settings.read().await;
                let window = context::resolve_context_window(
                    &settings,
                    &self.endpoint_name,
                    &self.model_id,
                    None,
                );
                (window.default_limit, window.max_limit)
            };

            let active_tool_defs = active_tool_definitions(tool_defs, finalization_pending);
            let required_tool_response = require_tool_next && !finalization_pending;
            let first_attempt = self
                .call_anthropic_transport(
                    system_prompt,
                    messages.clone(),
                    active_tool_defs,
                    required_tool_response,
                )
                .await;
            let resp = match first_attempt {
                Ok(resp) => resp,
                // Same no-vision degradation as the OpenAI loop: strip image
                // parts to placeholders and retry once instead of killing the
                // turn on every replay of an image-bearing history.
                // Transient provider saturation — see the OpenAI loop.
                Err(e) if is_provider_overloaded(&e.to_string()) => {
                    let notice = "模型服务过载,正在自动退避重试(最多 2 次)。".to_string();
                    self.persist_gate_message_once("自动退避重试", &notice, "turn_notice")
                        .await?;
                    let mut last_err = e;
                    let mut recovered = None;
                    for delay in [20u64, 40] {
                        if self.is_cancelled() {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                        match self
                            .call_anthropic_transport(
                                system_prompt,
                                messages.clone(),
                                active_tool_defs,
                                required_tool_response,
                            )
                            .await
                        {
                            Ok(ok) => {
                                recovered = Some(ok);
                                break;
                            }
                            Err(next) if is_provider_overloaded(&next.to_string()) => {
                                last_err = next;
                            }
                            Err(next) => return Err(next),
                        }
                    }
                    match recovered {
                        Some(resp) => resp,
                        None => return Err(last_err),
                    }
                }
                Err(e) if is_vision_rejection(&e.to_string()) => {
                    let stripped = strip_image_values(&mut messages);
                    if stripped == 0 {
                        return Err(e);
                    }
                    let notice = format!(
                        "已自动移除历史中的 {stripped} 张图片后重试:当前模型不支持图片输入。\
如需图片理解,请切换回支持图片的模型。"
                    );
                    self.persist_gate_message(&notice, "turn_notice").await?;
                    self.events.emit(StreamEvent::CompletionGateAction {
                                kind: "turn_notice".into(),
                                detail: notice.clone(),
                            });
                    self.call_anthropic_transport(
                        system_prompt,
                        messages.clone(),
                        active_tool_defs,
                        required_tool_response,
                    )
                    .await?
                }
                Err(e) => return Err(e),
            };
            finalization_pending = false;
            require_tool_next = false;

            let usage_request_id = self.usage_request_id(iteration);
            if resp.input_tokens > 0 || resp.output_tokens > 0 {
                let round_usage = Usage {
                    prompt_tokens: resp.input_tokens.max(0) as u32,
                    completion_tokens: resp.output_tokens.max(0) as u32,
                    total_tokens: resp.input_tokens.saturating_add(resp.output_tokens).max(0)
                        as u32,
                    cost: None,
                    prompt_tokens_details: None,
                    completion_tokens_details: None,
                };
                self.record_usage_event_for_round(&round_usage, iteration)
                    .await;
            }

            if resp.cancelled || self.is_cancelled() {
                self.emit_cancelled_done();
                emitted_terminal = true;
                break;
            }

            // Emit context usage if Anthropic reported it (it sets 0 when missing)
            if resp.input_tokens > 0 {
                self.events.emit(StreamEvent::ContextUsage {
                            used_tokens: resp.input_tokens.max(0) as u32,
                            limit_tokens: context_limit,
                            max_limit_tokens: max_context_limit,
                        });
            }

            let text = resp.text;
            let tool_calls = resp.tool_calls;

            // Persist assistant turn (Anthropic path — no separate reasoning
            // stream; Claude's extended thinking goes via the same `content`
            // for tool use turns)
            let assistant_message_id = if !text.is_empty() || !tool_calls.is_empty() {
                self.persist_message(
                    "assistant",
                    &text,
                    None,
                    if tool_calls.is_empty() {
                        None
                    } else {
                        Some(&tool_calls)
                    },
                    None,
                    Some(&usage_request_id),
                )
                .await?
            } else {
                None
            };
            if let Some(message_id) = assistant_message_id.as_deref() {
                for tc in &tool_calls {
                    let args = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    crate::trajectory::record_tool_call_started(
                        &self.db,
                        &self.session_id,
                        message_id,
                        &tc.id,
                        &tc.function.name,
                        &args,
                    )
                    .await?;
                }
            }

            if tool_calls.is_empty() {
                // Systemic fact-check — see the OpenAI loop for rationale.
                if !fact_check_used {
                    if let Some(correction) = fact_check_reply(&text) {
                        fact_check_used = true;
                        self.persist_gate_message(&correction, "turn_notice").await?;
                        self.events.emit(StreamEvent::CompletionGateAction {
                                    kind: "turn_notice".into(),
                                    detail: correction.clone(),
                                });
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": [{ "type": "text", "text": correction }],
                        }));
                        continue;
                    }
                }
                let evidence = completion_gate.evidence();
                match completion_finalization(&evidence, completion_recovery_attempts, self.mode) {
                    CompletionFinalization::Recover(prompt) => {
                        completion_recovery_attempts += 1;
                        require_tool_next = completion_recovery_requires_tool(self.mode);
                        // Make the rejection visible instead of silently looping:
                        // collapse the rejected candidate in the UI, persist the
                        // injected instruction so rebuilt history stays faithful.
                        self.mark_rejected_candidate(assistant_message_id.as_deref())
                            .await?;
                        self.persist_gate_message(&prompt, "gate_recovery").await?;
                        self.events.emit(StreamEvent::CompletionGateAction {
                                    kind: "recovery".into(),
                                    detail: evidence.blockers.join("; "),
                                });
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": [{
                                "type": "text",
                                "text": prompt,
                            }],
                        }));
                        continue;
                    }
                    CompletionFinalization::ReleaseWithWarning(warning) => {
                        // The reply stands — no folding, no Error. Persist a
                        // visible warning and fall through to the normal Done.
                        self.persist_gate_message(&warning, "gate_warning").await?;
                        self.events.emit(StreamEvent::CompletionGateAction {
                                    kind: "warning".into(),
                                    detail: warning.clone(),
                                });
                    }
                    CompletionFinalization::Blocked(message) => {
                        self.mark_rejected_candidate(assistant_message_id.as_deref())
                            .await?;
                        self.persist_gate_message(&message, "gate_blocked").await?;
                        self.events.emit(StreamEvent::Error { message });
                        emitted_terminal = true;
                        break;
                    }
                    CompletionFinalization::Complete => {}
                }
                emitted_terminal = true;
                let inp = resp.input_tokens;
                let out = resp.output_tokens;
                self.events.emit(StreamEvent::Done {
                            input_tokens: inp as u32,
                            output_tokens: out as u32,
                        });
                break;
            }

            // Build assistant message with tool_use blocks for Anthropic
            let mut assistant_content: Vec<serde_json::Value> = Vec::new();
            if !text.is_empty() {
                assistant_content.push(serde_json::json!({ "type": "text", "text": text }));
            }
            for tc in &tool_calls {
                let input: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::json!({}));
                assistant_content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.function.name,
                    "input": input,
                }));
            }
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": assistant_content,
            }));

            // Execute tools and collect tool_result blocks
            let mut tool_result_blocks: Vec<serde_json::Value> = Vec::new();
            let mut progress_prompt = None;
            let completion_evidence_before_tool_batch = completion_gate.evidence();

            for (tool_index, tc) in tool_calls.iter().enumerate() {
                if let Some(remaining) =
                    cancelled_tool_suffix(self.cancel.as_ref(), &tool_calls, tool_index)
                {
                    return self
                        .finish_cancelled_tool_batch(remaining)
                        .await;
                }
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                let completion_args = args.clone();

                self.events.emit(StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args: args.clone(),
                        });

                let bash_cmd = if tc.function.name == "bash" {
                    args.get("command")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };

                let permission_policy = {
                    let settings = self.settings.read().await;
                    settings.permissions.clone()
                };
                let decision =
                    decide_permission(&permission_policy, &tc.function.name, bash_cmd.as_deref());

                let remaining = max_iterations.saturating_sub(iteration + 1) as u32;
                let completion_evidence = completion_gate.evidence();
                let denial_content = if let Some(content) = autonomous_budget_denial(
                    self.mode,
                    remaining,
                    &completion_evidence,
                    &tc.function.name,
                    &args,
                    &self.cwd,
                ) {
                    Some(content)
                } else {
                    match decision {
                        PermissionDecision::Allow => None,
                        PermissionDecision::Ask => {
                            match self.request_permission(tc, args.clone()).await {
                                PermissionResponse::Allow => None,
                                PermissionResponse::Deny => Some(
                                    "Tool call denied by user. Please try a different approach."
                                        .to_string(),
                                ),
                                PermissionResponse::Cancelled => {
                                    return self
                                        .finish_cancelled_tool_batch(
                                            &tool_calls[tool_index..],
                                        )
                                        .await;
                                }
                            }
                        }
                        PermissionDecision::Deny(reason) => {
                            tracing::warn!("Tool '{}' denied: {reason}", tc.function.name);
                            Some(format!(
                                "Tool call denied: {reason}. Please try a different approach."
                            ))
                        }
                    }
                };

                if let Some(content) = denial_content {
                    self.record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    self.events.emit(StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
                            });
                    tool_result_blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tc.id,
                        "content": content,
                    }));
                    continue;
                }

                // Pre-tool hook: may cancel. Headless (no hooks) always allows.
                let pre_allowed = match &hook_runner {
                    Some(runner) => {
                        runner
                            .fire(hooks::HookEvent::PreTool {
                                tool_name: tc.function.name.clone(),
                                args: args.clone(),
                            })
                            .await
                    }
                    None => true,
                };
                if !pre_allowed {
                    let content = "Tool call cancelled by hook.".to_string();
                    self.record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    self.events.emit(StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
                            });
                    tool_result_blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tc.id,
                        "content": content,
                    }));
                    continue;
                }

                // Tool execution now flows through the shared ToolBackend seam
                // (keystone slice 4.3): the desktop backend builds the ExecCtx
                // and runs MCP-first / native-dispatch. Timing stays in the loop
                // so it covers the fatal-error path exactly as before.
                let tool_backend = tool_backend::DesktopToolBackend {
                    #[cfg(not(test))]
                    app: self.app.clone(),
                    db: self.db.clone(),
                    mcp_manager: self.mcp_manager.clone(),
                    settings: self.settings.clone(),
                };
                let tool_ctx = codefactory_agent_loop::tool::ToolCtx {
                    working_directory: self.cwd.clone(),
                    session_id: Some(self.audit_session_id()),
                    task_id: self
                        .execution_context
                        .as_ref()
                        .and_then(|ctx| ctx.task_id.clone()),
                    knowledge_library_ids: knowledge_scope_for_tools(
                        self.execution_context.as_ref(),
                    ),
                    timeout_sec: None,
                };

                let tool_start = std::time::Instant::now();
                let exec_result = tool_backend.execute(tc, &args, &tool_ctx).await;
                let duration_ms = tool_start.elapsed().as_millis() as u64;
                let output = match exec_result {
                    Ok(result) => tools::ToolOutput {
                        content: result.content,
                        is_error: result.is_error,
                    },
                    Err(error) => {
                        let error_text = error.to_string();
                        self.record_tool_call_outcome(
                            tc,
                            "error",
                            None,
                            Some(&error_text),
                            duration_ms,
                        )
                        .await?;
                        return Err(crate::errors::AppError::Other(error_text));
                    }
                };
                self.record_tool_call_outcome(
                    tc,
                    if output.is_error { "error" } else { "done" },
                    if output.is_error {
                        None
                    } else {
                        Some(&output.content)
                    },
                    if output.is_error {
                        Some(&output.content)
                    } else {
                        None
                    },
                    duration_ms,
                )
                .await?;

                if let Some(prompt) = record_completion_outcome(
                    &mut completion_gate,
                    &mut progress_tracker,
                    &mut completion_sequence,
                    &self.cwd,
                    &tc.function.name,
                    &completion_args,
                    &output,
                ) {
                    progress_prompt = Some(prompt);
                }
                // Post-tool hook (skipped headless — no hooks).
                if let Some(runner) = &hook_runner {
                    runner
                        .fire(hooks::HookEvent::PostTool {
                            tool_name: tc.function.name.clone(),
                            result: output.content.chars().take(500).collect(),
                            duration_ms,
                        })
                        .await;
                }

                self.events.emit(StreamEvent::ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: output.content.clone(),
                            is_error: output.is_error,
                            status: if output.is_error {
                                "error".into()
                            } else {
                                "done".into()
                            },
                        });

                tool_result_blocks.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tc.id,
                    "content": output.content,
                }));
            }

            completion_recovery_attempts = completion_recovery_attempts_after_tool_batch(
                completion_recovery_attempts,
                completion_evidence_made_progress(
                    &completion_evidence_before_tool_batch,
                    &completion_gate.evidence(),
                ),
            );

            // Append a single user message with all tool_result blocks
            if !tool_result_blocks.is_empty() {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": tool_result_blocks,
                }));
            }
            if let Some(prompt) = progress_prompt {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{"type": "text", "text": prompt}],
                }));
            }
            let evidence = completion_gate.evidence();
            if completion_ready_applies(self.mode)
                && evidence.completed
                && evidence.last_successful_verification_sequence != last_completion_nudge_sequence
            {
                last_completion_nudge_sequence = evidence.last_successful_verification_sequence;
                finalization_pending = true;
                self.persist_gate_message(build_completion_ready_prompt(), "gate_ready")
                    .await?;
                self.events.emit(StreamEvent::CompletionGateAction {
                            kind: "ready".into(),
                            detail: String::new(),
                        });
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": build_completion_ready_prompt(),
                    }],
                }));
            } else if self.mode != AgentMode::Interactive {
                let remaining = max_iterations.saturating_sub(iteration + 1);
                if should_prompt_budget_convergence(remaining as u32) {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "text",
                            "text": build_budget_convergence_prompt(remaining as u32, &evidence),
                        }],
                    }));
                }
            }
        }

        // Safety net: see run_openai. Close the stream without treating
        // incomplete completion evidence as success.
        if !emitted_terminal {
            let evidence = completion_gate.evidence();
            tracing::warn!(
                "agent loop hit the iteration ceiling ({}) without a terminal turn; completed={}",
                self.mode.max_iterations(),
                evidence.completed,
            );
            self.events.emit(iteration_ceiling_terminal_event(&evidence, self.mode));
        }

        Ok(())
    }
}

fn openai_tool_controls(
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

fn active_tool_definitions(
    tool_defs: &[ToolDefinition],
    finalization_pending: bool,
) -> &[ToolDefinition] {
    if finalization_pending {
        &[]
    } else {
        tool_defs
    }
}

fn validate_openai_sse_completion(
    saw_terminal_marker: bool,
    pending_bytes: usize,
    malformed_data_lines: usize,
) -> std::result::Result<(), String> {
    if malformed_data_lines > 0 {
        return Err(format!(
            "OpenAI-compatible stream contained {malformed_data_lines} malformed SSE data line(s)"
        ));
    }
    if pending_bytes > 0 {
        return Err(format!(
            "OpenAI-compatible stream ended with {pending_bytes} incomplete byte(s)"
        ));
    }
    if !saw_terminal_marker {
        return Err("OpenAI-compatible stream ended before [DONE] or finish_reason".to_owned());
    }
    Ok(())
}

fn completion_command_and_kind(tool_name: &str, args: &serde_json::Value) -> (String, ToolKind) {
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

/// How many times one run may reject a tool-call-free final response and ask
/// the model to resolve concrete completion blockers. User-facing chat gets
/// three bounded, tool-required recovery opportunities: enough to survive an
/// incidental shell/precondition mistake without restoring the historical
/// unbounded near-duplicate loop. Autonomous attempts stay single-shot because
/// the scheduler can respawn them with a fresh evidence brief.
fn completion_recovery_limit(mode: AgentMode) -> u32 {
    match mode {
        AgentMode::Interactive | AgentMode::Execute => 3,
        AgentMode::Autonomous => 1,
    }
}

fn completion_recovery_requires_tool(_mode: AgentMode) -> bool {
    true
}

fn completion_recovery_attempts_after_tool_batch(
    attempts: u32,
    material_evidence_progress: bool,
) -> u32 {
    if material_evidence_progress {
        0
    } else {
        attempts
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CompletionFinalization {
    Complete,
    Recover(String),
    /// Chat surfaces after recovery exhaustion: the model's reply is the best
    /// available answer — release it, but persist this human-readable warning
    /// so the user knows verification is incomplete. Never an Error, never
    /// internal-contract wording (2026-07-21 field report: an untranslated
    /// "Completion blocked…rerun every unresolved failed check" killed the
    /// turn and hid the reply).
    ReleaseWithWarning(String),
    /// Unattended Autonomous runs only — the scheduler treats this as an
    /// incomplete attempt and respawns.
    Blocked(String),
}

fn completion_finalization(
    evidence: &CompletionEvidence,
    attempts: u32,
    mode: AgentMode,
) -> CompletionFinalization {
    if evidence.completed {
        return CompletionFinalization::Complete;
    }
    if attempts < completion_recovery_limit(mode) {
        return CompletionFinalization::Recover(build_completion_recovery_prompt(evidence));
    }
    match mode {
        AgentMode::Autonomous => {
            CompletionFinalization::Blocked(completion_blocked_message(evidence))
        }
        AgentMode::Interactive | AgentMode::Execute => {
            CompletionFinalization::ReleaseWithWarning(unverified_release_warning(evidence))
        }
    }
}

/// User-facing warning when a chat turn ends without complete verification.
/// Chinese, plain language, no gate terminology; the raw blocker list goes
/// to the log only.
fn unverified_release_warning(evidence: &CompletionEvidence) -> String {
    tracing::info!(
        "releasing chat turn with unverified blockers: {}",
        evidence.blockers.join("; ")
    );
    "⚠ 以上回复未经完整验证:本轮修改后仍有检查未复验(或失败未复跑)。\
结论可能不完整;回复「继续验证」可让我补齐。"
        .to_string()
}

fn completion_blocked_message(evidence: &CompletionEvidence) -> String {
    format!(
        "Completion blocked because required verification is still missing: {}",
        evidence.blockers.join("; ")
    )
}

fn iteration_ceiling_terminal_event(evidence: &CompletionEvidence, mode: AgentMode) -> StreamEvent {
    if evidence.completed || !matches!(mode, AgentMode::Autonomous) {
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

fn completion_recovery_prompt(
    evidence: &CompletionEvidence,
    attempts: u32,
    mode: AgentMode,
) -> Option<String> {
    match completion_finalization(evidence, attempts, mode) {
        CompletionFinalization::Recover(prompt) => Some(prompt),
        CompletionFinalization::Complete
        | CompletionFinalization::ReleaseWithWarning(_)
        | CompletionFinalization::Blocked(_) => None,
    }
}

/// Placeholder inserted in place of an image part when the active model
/// rejects vision input. Visible to the model (so it knows an image existed)
/// and stable for the strip functions' idempotence checks.
const IMAGE_STRIPPED_PLACEHOLDER: &str = "[图片已省略:当前模型不支持图片输入]";

/// Does this provider error mean "the model can't accept image input"?
/// Deliberately narrow: capability wording only, never generic failures —
/// a false positive would silently drop the user's images on a transient
/// error, so unknown errors must stay unmatched and surface as-is.
fn is_vision_rejection(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    ["image", "vision", "multimodal"]
        .iter()
        .any(|needle| lower.contains(needle))
        && !lower.contains("rate limit")
}

/// Replace image parts in OpenAI-shaped messages with a text placeholder.
/// Returns how many were stripped (0 = nothing to do → do not retry again).
fn strip_image_parts(messages: &mut [ChatMessage]) -> usize {
    let mut stripped = 0;
    for message in messages.iter_mut() {
        if let MessageContent::Parts(parts) = &mut message.content {
            for part in parts.iter_mut() {
                if part.r#type == "image_url" {
                    part.r#type = "text".into();
                    part.text = Some(IMAGE_STRIPPED_PLACEHOLDER.to_string());
                    part.image_url = None;
                    stripped += 1;
                }
            }
        }
    }
    stripped
}

/// Same as [`strip_image_parts`] for the Anthropic-shaped JSON message array
/// (`type: "image"` blocks and any `image_url` compatibility parts).
fn strip_image_values(messages: &mut [serde_json::Value]) -> usize {
    let mut stripped = 0;
    for message in messages.iter_mut() {
        let Some(parts) = message.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        for part in parts.iter_mut() {
            let kind = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if kind == "image" || kind == "image_url" {
                *part = serde_json::json!({
                    "type": "text",
                    "text": IMAGE_STRIPPED_PLACEHOLDER,
                });
                stripped += 1;
            }
        }
    }
    stripped
}

/// History rows that may be replayed to the provider. Persisted turn-error
/// notices are for the user and forensics only — replaying them would inject
/// fake user turns the model never saw.
fn replayable_history(history: Vec<Message>) -> Vec<Message> {
    history
        .into_iter()
        .filter(|m| m.completion_state.as_deref() != Some("turn_error"))
        .collect()
}

/// Transient provider saturation worth a backoff retry instead of a dead
/// turn. Distinct from capacity (context) and capability (vision) errors.
fn is_provider_overloaded(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    ["overloaded", "try again later", "rate limit", "429", "503", "529"]
        .iter()
        .any(|w| lower.contains(w))
}

/// The self-recovery behavioral contract, appended to EVERY mode's system
/// prompt. Systemic answer to the field pattern where the agent reported
/// obstacles instead of attempting them ("无法自动创建 PR,请 gh auth
/// login" from stale memory, while a logged-in gh sat right there — and the
/// same shape for docker, tokens, and "回复继续" waits).
const SELF_RECOVERY_CONTRACT: &str = "\
# Self-recovery contract (all modes)\n\
1. **VERIFY BEFORE ASSERTING.** Never state an environment fact from memory —\n\
   tool presence, CLI login state, config existence, service reachability.\n\
   Run the cheap probe first (`which x`, `gh auth status`, read the file).\n\
   An assertion like \"X is not available\" without a probe in THIS turn is a\n\
   contract violation; the harness fact-checks such claims and will correct you.\n\
2. **TRY BEFORE ASKING.** On an obstacle, attempt at least two different\n\
   approaches yourself (narrate each in one short sentence). Ask the user only\n\
   after both fail, and include: what you verified, what you tried, the exact\n\
   errors, and the smallest action only the user can take.\n\
3. **POLL, DON'T PARK.** If the thing you are waiting for is machine-checkable\n\
   (auth completed, file created, CI finished, config saved), poll for it with\n\
   tools — never end the turn with \"完成后回复继续\" for a checkable condition.\n";

/// Does this reply claim the delivery channel is unusable (cannot open a
/// PR / needs gh auth login / needs a token)? Narrow: successful-delivery
/// reports are exempt.
fn claims_delivery_blocked(text: &str) -> bool {
    let lower = text.to_lowercase();
    let success_markers = [
        "已创建",
        "已通过 gh",
        "交付结果: delivered",
        "验证成功",
        "pr created",
    ];
    if success_markers.iter().any(|m| lower.contains(&m.to_lowercase())) {
        return false;
    }
    let inability = ["无法", "不能", "cannot", "can't", "unable"];
    let channel = ["创建 pr", "开 pr", "create the pr", "create a pr", "自动创建 pr"];
    let inability_hit = inability.iter().any(|w| lower.contains(w))
        && channel.iter().any(|w| lower.contains(w));
    let setup_demand = (lower.contains("gh auth login")
        || (lower.contains("token") && (lower.contains("配置") || lower.contains("configure"))))
        && (lower.contains("请") || lower.contains("please") || lower.contains("完成认证"));
    inability_hit || setup_demand
}

/// Does this reply claim a specific command/CLI is missing? Returns the
/// command name for a live probe. Narrow allowlist of tools we know how to
/// probe; positive statements ("已通过 docker…") are exempt.
fn claims_command_missing(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let missing_markers = [
        "未安装",
        "没有安装",
        "不存在",
        "找不到",
        "not installed",
        "command not found",
        "not found",
        "is missing",
    ];
    if !missing_markers.iter().any(|m| lower.contains(m)) {
        return None;
    }
    for cmd in ["docker", "gh", "git", "node", "pnpm", "npm", "cargo", "python3"] {
        if lower.contains(cmd) {
            // The marker must plausibly refer to the command, not random prose.
            return Some(cmd.to_string());
        }
    }
    None
}

/// Live probe: does `cmd` resolve on PATH?
fn command_exists(cmd: &str) -> bool {
    which_available(cmd)
}

fn which_available(cmd: &str) -> bool {
    std::process::Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Does this reply park on the user for a machine-checkable condition
/// ("完成后回复继续 / 配置完成后告诉我")? Genuine preference questions
/// ("回复 1 或 2") are not waits on checkable state.
fn claims_wait_for_user_on_checkable(text: &str) -> bool {
    let lower = text.to_lowercase();
    let wait_markers = [
        "完成后回复",
        "完成后告诉我",
        "配置好后回复",
        "配置完成后告诉",
        "reply \"continue\" once",
        "let me know once",
    ];
    wait_markers.iter().any(|m| lower.contains(&m.to_lowercase()))
}

/// Fact-check a tool-call-free reply against live probes. Text detectors run
/// first; probes run ONLY on a match, so ordinary turns pay nothing. Returns
/// the correction to inject, or None when no verified-false claim is found.
fn fact_check_reply(text: &str) -> Option<String> {
    if claims_delivery_blocked(text) && delivery::gh_cli_available() {
        return Some(
            "事实纠偏:你刚声称交付通道不可用/需要配置,但本机 GitHub CLI 已登录且刚刚实测可用。\
这是基于过期上下文的错误判断。立即调用 deliver_changes 完成交付并报告其真实结果;\
不要再要求用户配置任何令牌或运行 gh auth login。"
                .to_string(),
        );
    }
    if let Some(cmd) = claims_command_missing(text) {
        if command_exists(&cmd) {
            return Some(format!(
                "事实纠偏:你刚声称 `{cmd}` 不可用,但本机刚刚实测它存在于 PATH。\
不要凭记忆断言环境状态;直接使用 `{cmd}` 继续当前工作。"
            ));
        }
    }
    if claims_wait_for_user_on_checkable(text) {
        return Some(
            "事实纠偏:你在等用户口头确认一个机器可检测的条件。按自救契约第 3 条,\
用工具轮询该条件(例如探测认证状态/文件存在/CI 结论),满足后直接继续,\
不要把可自动检测的等待交给用户。"
                .to_string(),
        );
    }
    None
}

/// Whether this mode injects the completion-ready ("coverage audit") nudge at
/// all. The nudge freezes the toolset for the following round, forcing the
/// model to wrap up — an AUTONOMOUS-contract mechanism, where no user is
/// present and the scheduler respawns incomplete work. In interactive and
/// execute chat, `evidence.completed` only means "some mutation was verified"
/// (any mid-task `tsc`/`vitest` pass qualifies), NOT "the user's task is
/// done"; firing it mid-task forcibly ends the turn while the model is
/// announcing its next step, which reads as the assistant stalling. With the
/// user present, the user decides when the work is finished.
fn completion_ready_applies(mode: AgentMode) -> bool {
    matches!(mode, AgentMode::Autonomous)
}

fn autonomous_budget_denial(
    mode: AgentMode,
    remaining_model_rounds: u32,
    evidence: &CompletionEvidence,
    tool_name: &str,
    args: &serde_json::Value,
    working_directory: &Path,
) -> Option<String> {
    let (command, kind) = completion_command_and_kind(tool_name, args);
    // Interactive chat is not constrained by the autonomous round budget, but
    // deterministic completion invariants still apply to model-generated tools.
    let effective_remaining = if mode == AgentMode::Interactive {
        u32::MAX
    } else {
        remaining_model_rounds
    };
    match evaluate_budget_command_in_directory(
        effective_remaining,
        evidence,
        &command,
        &kind,
        working_directory.to_str(),
    ) {
        PolicyDecision::Allow => None,
        PolicyDecision::Deny { reason, .. } => Some(format!(
            "Tool call denied by completion policy: {reason}. Resolve the current completion blocker or finalize."
        )),
    }
}

fn record_completion_outcome(
    gate: &mut CompletionGate,
    progress: &mut ProgressTracker,
    sequence: &mut u64,
    working_directory: &Path,
    tool_name: &str,
    args: &serde_json::Value,
    output: &tools::ToolOutput,
) -> Option<String> {
    *sequence += 1;
    let (command, kind) = completion_command_and_kind(tool_name, args);
    let outcome = ToolOutcome {
        request_id: format!("desktop-tool-{sequence}"),
        command,
        working_directory: Some(working_directory.to_string_lossy().into_owned()),
        kind,
        sequence: *sequence,
        started_at_ms: 0,
        finished_at_ms: 0,
        return_code: Some(if output.is_error { 1 } else { 0 }),
        stdout: output.content.clone(),
        stderr: String::new(),
        error: output.is_error.then(|| output.content.clone()),
        semantic_failure: false,
    }
    .with_detected_semantic_failure();
    gate.record(&outcome);
    progress.record(&outcome)
}

/// Convert an MCP tool descriptor into the OpenAI-compatible ToolDefinition format.
fn mcp_tool_to_definition(tool: &crate::mcp::McpTool) -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}

/// Parse tool message content stored as `{"tool_call_id":"…","content":"…"}`.
/// Falls back gracefully if the JSON is malformed.
fn parse_tool_message_content(raw: &str) -> (String, String) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        let id = v
            .get("tool_call_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let content = v
            .get("content")
            .and_then(|x| x.as_str())
            .unwrap_or(raw)
            .to_string();
        return (id, content);
    }
    (String::new(), raw.to_string())
}

async fn persist_cancelled_tool_batch(
    db: &SqlitePool,
    session_id: &str,
    anonymous: bool,
    remaining: &[ToolCall],
) -> Result<Vec<String>> {
    let mut contents = Vec::with_capacity(remaining.len());
    for (index, tool_call) in remaining.iter().enumerate() {
        let content = if index == 0 {
            "Tool call cancelled by user."
        } else {
            "Tool call skipped because the batch was cancelled by user."
        }
        .to_string();
        if !anonymous {
            crate::trajectory::record_terminal_tool_outcome(
                db,
                session_id,
                &tool_call.id,
                "cancelled",
                None,
                Some(&content),
                0,
            )
            .await?;
        }
        contents.push(content);
    }
    Ok(contents)
}

fn repair_incomplete_tool_history(history: Vec<Message>) -> Vec<Message> {
    fn synthetic_tool_message(session_id: &str, tool_call_id: &str, created_at: i64) -> Message {
        Message {
            id: format!("recovered-tool-{tool_call_id}"),
            session_id: session_id.to_owned(),
            role: "tool".into(),
            content: serde_json::json!({
                "tool_call_id": tool_call_id,
                "content": "Tool result unavailable in persisted history; continue from current workspace state.",
            })
            .to_string(),
            model_id: None,
            input_tokens: None,
            output_tokens: None,
            tool_calls: None,
            reasoning_content: None,
            completion_state: None,
            created_at,
        }
    }

    let mut repaired = Vec::with_capacity(history.len());
    let mut pending_tool_calls: Vec<String> = Vec::new();
    let mut last_session_id = String::new();
    let mut last_created_at = 0;

    for message in history {
        last_session_id = message.session_id.clone();
        last_created_at = message.created_at;
        if message.role != "tool" && !pending_tool_calls.is_empty() {
            for tool_call_id in pending_tool_calls.drain(..) {
                repaired.push(synthetic_tool_message(
                    &message.session_id,
                    &tool_call_id,
                    message.created_at.saturating_sub(1),
                ));
            }
        }

        if message.role == "tool" {
            let (tool_call_id, _) = parse_tool_message_content(&message.content);
            if let Some(index) = pending_tool_calls
                .iter()
                .position(|pending| pending == &tool_call_id)
            {
                pending_tool_calls.remove(index);
                repaired.push(message);
            }
            continue;
        }

        if message.role == "assistant" {
            pending_tool_calls = message
                .tool_calls
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Vec<ToolCall>>(raw).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|tool_call| tool_call.id)
                .collect();
        }
        repaired.push(message);
    }

    for tool_call_id in pending_tool_calls {
        repaired.push(synthetic_tool_message(
            &last_session_id,
            &tool_call_id,
            last_created_at.saturating_add(1),
        ));
    }
    repaired
}

fn repair_openai_tool_protocol(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    fn synthetic_tool_message(tool_call_id: String) -> ChatMessage {
        ChatMessage {
            role: "tool".into(),
            content: MessageContent::Text(
                "Tool result unavailable in persisted history; continue from current workspace state."
                    .into(),
            ),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
            name: None,
            reasoning_content: None,
        }
    }

    fn append_missing_results(repaired: &mut Vec<ChatMessage>, pending: &mut Vec<String>) {
        repaired.extend(pending.drain(..).map(synthetic_tool_message));
    }

    let mut repaired = Vec::with_capacity(messages.len());
    let mut pending_tool_calls: Vec<String> = Vec::new();

    for mut message in messages {
        if message.role != "tool" && !pending_tool_calls.is_empty() {
            append_missing_results(&mut repaired, &mut pending_tool_calls);
        }

        if message.role == "tool" {
            let Some(tool_call_id) = message.tool_call_id.as_deref() else {
                continue;
            };
            let Some(index) = pending_tool_calls
                .iter()
                .position(|pending| pending == tool_call_id)
            else {
                continue;
            };
            pending_tool_calls.remove(index);
            repaired.push(message);
            continue;
        }

        if message.role == "assistant" {
            if let Some(tool_calls) = message.tool_calls.as_mut() {
                let mut seen_ids = HashSet::new();
                tool_calls.retain(|tool_call| {
                    !tool_call.id.trim().is_empty() && seen_ids.insert(tool_call.id.clone())
                });
                if tool_calls.is_empty() {
                    message.tool_calls = None;
                }
            }
            pending_tool_calls = message
                .tool_calls
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|tool_call| tool_call.id.clone())
                .collect();
        }
        repaired.push(message);
    }

    append_missing_results(&mut repaired, &mut pending_tool_calls);
    repaired
}

/// Read at most `max_chars` UTF-8 chars from a file, appending "…" if truncated.
fn read_file_capped(path: &Path, max_chars: usize) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    if raw.len() <= max_chars {
        Some(raw)
    } else {
        // Truncate at a char boundary
        let truncated: String = raw.chars().take(max_chars).collect();
        Some(format!("{truncated}\n[… truncated]"))
    }
}

/// Detect primary project type and return (label, config_file_path).
fn detect_project_config(cwd: &Path) -> Option<(&'static str, std::path::PathBuf)> {
    let candidates: &[(&str, &str)] = &[
        ("Rust (Cargo.toml)", "Cargo.toml"),
        ("Node.js (package.json)", "package.json"),
        ("Python (pyproject.toml)", "pyproject.toml"),
        ("Python (setup.py)", "setup.py"),
        ("Go (go.mod)", "go.mod"),
    ];
    for (label, file) in candidates {
        let path = cwd.join(file);
        if path.exists() {
            return Some((label, path));
        }
    }
    None
}

const REPOSITORY_INTENT_FILE_LIMIT: usize = 64;

/// Discover versioned product-intent documents without injecting every body
/// into the system prompt. The repository's own rules remain authoritative;
/// these common directories are only indexed when they already exist.
fn repository_intent_paths(cwd: &Path) -> Vec<String> {
    fn walk(cwd: &Path, dir: &Path, paths: &mut Vec<String>) {
        if paths.len() >= REPOSITORY_INTENT_FILE_LIMIT {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries = entries.flatten().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if paths.len() >= REPOSITORY_INTENT_FILE_LIMIT {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                walk(cwd, &path, paths);
            } else if file_type.is_file()
                && path.extension().and_then(|extension| extension.to_str()) == Some("md")
            {
                if let Ok(relative) = path.strip_prefix(cwd) {
                    paths.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }

    let mut paths = Vec::new();
    for relative in ["docs/specs", "docs/design"] {
        walk(cwd, &cwd.join(relative), &mut paths);
    }
    paths
}

// Scaffolding: standalone project-knowledge prompt builder, kept as the
// test-covered reference path (and referenced by docs in commands/memory.rs and
// agent/user_context.rs). The live loop (`AgentLoop::run`) currently inlines its
// own budgeted assembly, so this entry point is exercised only by tests for now.
// The `#[allow]` here also covers `build_system_prompt_for` + `PROJECT_CONTEXT_BUDGET`,
// which are reachable only through it.
#[allow(dead_code)]
fn build_system_prompt(cwd: &Path) -> String {
    build_system_prompt_for(AgentMode::Interactive, cwd)
}

/// Char ceiling for project-only knowledge (memory / README / config) in the
/// standalone prompt builder. The live loop uses a larger budget that also
/// covers skills + user context (see [`SYSTEM_PROMPT_BUDGET`]).
const PROJECT_CONTEXT_BUDGET: usize = 14_000;

/// Char ceiling for ALL injected knowledge — project + enabled skills + user
/// preferences/learnings — in the live agent loop. Roughly 6k tokens: a
/// protective cap so a big README plus several large skills can't silently
/// swallow the context window, not a limit the common case ever reaches.
const SYSTEM_PROMPT_BUDGET: usize = 24_000;

/// The project-derived knowledge blocks: memory (both supported files under one
/// heading), README, and the primary config file — each rendered in its
/// existing format. The caller fits them into a shared budget.
fn project_knowledge_blocks(cwd: &Path) -> Vec<context_budget::Block> {
    use context_budget::Block;
    let mut blocks = Vec::new();

    // Repository rules outrank assistant memory and are therefore allocated
    // context budget first. A capped ordinary file keeps the contract portable
    // across repositories without creating CodeFactory-owned project state.
    if let Some(content) = read_file_capped(&cwd.join("AGENTS.md"), 6000) {
        blocks.push(Block::new(
            format!("# Repository Authority (`AGENTS.md`)\n{content}"),
            0,
            6200,
        ));
    }

    let intent_paths = repository_intent_paths(cwd);
    if !intent_paths.is_empty() {
        let index = intent_paths
            .iter()
            .map(|path| format!("- `{path}`"))
            .collect::<Vec<_>>()
            .join("\n");
        blocks.push(Block::new(
            format!(
                "# Repository Intent Index\n\
                 These are ordinary versioned repository files, not Agent-owned records. \
                 Read only the relevant files before planning or implementation, and follow \
                 `AGENTS.md` when deciding where durable decisions belong.\n{index}"
            ),
            1,
            4600,
        ));
    }

    // Memory — `.codefactory/memory.md` (preferred, modern; matches the
    // .cursorrules / .claude/ family) + legacy `CODEFACTORY.md`, combined under
    // one heading. Each source keeps its prior 4000-char cap.
    let sources: [(&str, std::path::PathBuf); 2] = [
        (
            ".codefactory/memory.md",
            cwd.join(".codefactory").join("memory.md"),
        ),
        ("CODEFACTORY.md", cwd.join("CODEFACTORY.md")),
    ];
    let mut mem = String::new();
    for (label, path) in sources {
        if let Some(content) = read_file_capped(&path, 4000) {
            let content = content.trim();
            if content.is_empty() {
                continue;
            }
            if mem.is_empty() {
                mem.push_str("# Project Memory");
            }
            mem.push_str(&format!("\n\n## From `{label}`\n{content}"));
        }
    }
    if !mem.is_empty() {
        blocks.push(Block::new(mem, 2, 8200));
    }

    // README — first of the candidates that exists.
    for readme in &["README.md", "README.txt", "readme.md"] {
        if let Some(content) = read_file_capped(&cwd.join(readme), 3000) {
            blocks.push(Block::new(
                format!("# Project README ({readme})\n{content}"),
                4,
                3200,
            ));
            break;
        }
    }

    // Project config (Cargo.toml / package.json / etc.).
    if let Some((label, config_path)) = detect_project_config(cwd) {
        if let Some(content) = read_file_capped(&config_path, 2000) {
            blocks.push(Block::new(
                format!("# Project Config — {label}\n```\n{content}\n```"),
                5,
                2200,
            ));
        }
    }

    blocks
}

/// Same as `build_system_prompt` but parameterized on the agent mode. Project
/// knowledge only; the live loop ([`AgentLoop::run`]) assembles the full
/// budgeted prompt (which also folds in skills + user context).
fn build_system_prompt_for(mode: AgentMode, cwd: &Path) -> String {
    context_budget::assemble(
        base_system_prompt(mode, cwd),
        project_knowledge_blocks(cwd),
        PROJECT_CONTEXT_BUDGET,
    )
}

/// Fixed agent contract plus the exact project root for this session. Keeping
/// cwd in the non-evictable base prevents the model from guessing container
/// conventions such as `/workspace` before its first tool call.
fn base_system_prompt(mode: AgentMode, cwd: &Path) -> String {
    format!(
        "{}\n\n{}\n\n{SELF_RECOVERY_CONTRACT}\n# Repository-Owned Intent\n\
         Long-lived requirements, specifications, architecture decisions, and acceptance criteria belong to ordinary versioned files in the repository, not to Agent memory or an app-owned specification database. Before non-trivial planning or implementation, inspect `AGENTS.md`, the README, and relevant existing files such as `docs/specs` or `docs/design`; follow the repository's own convention rather than creating `.codefactory/specs`; plans and delegated task state belong to the current conversation; do not direct the user to a separate specification or planning screen. When a durable decision changes, edit the repository document through normal file tools so it appears in the diff and travels with Git.\n\n# Product Self-Repair Context\n\
         When the user reports behavior of the running product and the selected repository is that product's codebase, treat it as a product bug you can fix here. Inspect and fix it in this repository; do not stop at explaining the issue or asking the user to switch contexts.\n\n# Working Directory\n\
         The project root and default tool working directory is:\n{}\n\
         Use this exact path or paths relative to it. Do not assume `/workspace` or another container path.",
        mode.system_prompt(),
        EXECUTION_COMPLETION_CONTRACT,
        cwd.to_string_lossy()
    )
}

/// Extra, model-aware reinforcement of the [`AgentMode::Execute`] contract.
///
/// Instruction-following varies by model: stronger families infer "they said
/// go, so act" from the Execute prompt alone, while smaller / unknown models
/// keep latching onto plan-first habits and re-ask. Rather than expose a user
/// knob, the framework restates the contract harder — appended at the very end
/// of the system prompt, where weaker models weight it most — but only for
/// those models. Returns "" for high-compliance families (keeps their prompt
/// lean) and for any non-Execute turn.
fn compliance_booster(mode: AgentMode, model_id: &str) -> &'static str {
    if mode != AgentMode::Execute {
        return "";
    }
    let m = model_id.to_lowercase();
    let high = m.contains("opus")
        || m.contains("sonnet")
        || m.contains("gpt-5")
        || m.contains("o3")
        || m.contains("o1");
    if high {
        return "";
    }
    "\n\n# REMINDER — read before replying\n\
     The user ALREADY approved. Do NOT output a plan and do NOT ask to\n\
     confirm. Your first output this turn MUST be a tool call that starts the\n\
     approved work. If you reply with a plan or a question instead of acting,\n\
     you have failed this turn."
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PermissionDecision {
    Allow,
    Ask,
    Deny(String),
}

fn decide_permission(
    policy: &PermissionPolicy,
    tool_name: &str,
    cmd: Option<&str>,
) -> PermissionDecision {
    // Skill-management tools create *disabled* skills — nothing is injected into
    // the system prompt until the user enables it on the Skills page, which is
    // the real gate. So they never need a per-call permission prompt.
    if tool_name.starts_with("skill_") {
        return PermissionDecision::Allow;
    }
    if tool_name == "bash" {
        if let Some(command) = cmd {
            match crate::tools::shell_policy::classify_command(command) {
                crate::tools::shell_policy::ShellCommandPolicy::Deny { reason } => {
                    return PermissionDecision::Deny(format!(
                        "Denied by shell safety policy: {reason}"
                    ));
                }
                crate::tools::shell_policy::ShellCommandPolicy::Ask { .. } => {
                    return PermissionDecision::Ask;
                }
                crate::tools::shell_policy::ShellCommandPolicy::Allow { .. } => {}
            }
        }
    }

    let key = match cmd {
        Some(c) => format!("{}({})", tool_name, c),
        None => tool_name.to_string(),
    };

    for pattern in &policy.deny {
        if glob_match(pattern, &key) || glob_match(pattern, tool_name) {
            return PermissionDecision::Deny(format!("Denied by policy: matches '{pattern}'"));
        }
    }

    if policy.full_access {
        return PermissionDecision::Allow;
    }

    for pattern in &policy.allow {
        if glob_match(pattern, &key) || glob_match(pattern, tool_name) {
            return PermissionDecision::Allow;
        }
    }

    for pattern in &policy.ask {
        if glob_match(pattern, &key) || glob_match(pattern, tool_name) {
            return PermissionDecision::Ask;
        }
    }

    PermissionDecision::Ask
}

/// Simple glob matcher: supports `*` (any chars) and exact prefix matching like `bash(git *)`.
fn glob_match(pattern: &str, input: &str) -> bool {
    if pattern == input {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return input.starts_with(prefix);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_transport_retry_maps_notice_fields_through_the_event_sink() {
        // Keystone slice 1: the loop now emits through `EventSink`, so its
        // event output is testable without a Tauri AppHandle for the first
        // time. Verify the transport-retry mapping via a collecting sink.
        let sink = events::CollectingEventSink::new();
        AgentLoop::emit_transport_retry(
            &sink,
            crate::http_util::RetryNotice {
                label: "anthropic stream".into(),
                attempt: 2,
                max_attempts: 3,
                delay: std::time::Duration::from_millis(300),
                reason: "HTTP 503".into(),
            },
        );
        let evs = sink.events();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            StreamEvent::TransportRetry {
                attempt,
                max_attempts,
                delay_ms,
                reason,
                ..
            } => {
                assert_eq!((*attempt, *max_attempts, *delay_ms), (2, 3, 300));
                assert_eq!(reason, "HTTP 503");
            }
            other => panic!("expected TransportRetry, got {other:?}"),
        }
    }

    #[test]
    fn autonomous_empty_knowledge_scope_remains_explicitly_empty() {
        let context = AgentExecutionContext {
            parent_session_id: Some("parent".into()),
            task_id: Some("task".into()),
            knowledge_library_ids: Vec::new(),
            usage_surface: UsageSurface::Subagent,
        };

        assert_eq!(knowledge_scope_for_tools(Some(&context)), Some(Vec::new()));
        assert_eq!(knowledge_scope_for_tools(None), None);
    }

    #[tokio::test]
    async fn pending_permission_is_released_by_chat_cancellation() {
        let (_sender, receiver) = tokio::sync::oneshot::channel();
        let cancel = Arc::new(AtomicBool::new(true));

        assert_eq!(
            await_permission_response(receiver, Some(&cancel), Duration::from_secs(1)).await,
            PermissionResponse::Cancelled
        );
    }

    #[tokio::test]
    async fn pending_stream_is_released_by_chat_cancellation() {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut stream = futures_util::stream::pending::<u8>();

        assert!(matches!(
            next_stream_item(&mut stream, Some(&cancel)).await,
            StreamPoll::Cancelled
        ));
    }

    #[test]
    fn cancellation_selects_only_the_unstarted_tool_suffix() {
        let cancel = Arc::new(AtomicBool::new(true));
        let calls = vec![
            ToolCall {
                id: "call-finished".into(),
                r#type: "function".into(),
                function: FunctionCall {
                    name: "read_file".into(),
                    arguments: "{}".into(),
                },
            },
            ToolCall {
                id: "call-unstarted".into(),
                r#type: "function".into(),
                function: FunctionCall {
                    name: "write_file".into(),
                    arguments: "{}".into(),
                },
            },
        ];

        let remaining = cancelled_tool_suffix(Some(&cancel), &calls, 1).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "call-unstarted");
    }

    #[tokio::test]
    async fn cancellation_terminalizes_every_remaining_tool_call() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE tool_calls (
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL, tool_name TEXT NOT NULL,
                arguments TEXT NOT NULL DEFAULT '{}', result TEXT, status TEXT NOT NULL,
                error TEXT, duration_ms INTEGER, created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
                content TEXT NOT NULL, created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let calls = vec![
            ToolCall {
                id: "call-current".into(),
                r#type: "function".into(),
                function: crate::openrouter::types::FunctionCall {
                    name: "bash".into(),
                    arguments: r#"{"command":"sleep 10"}"#.into(),
                },
            },
            ToolCall {
                id: "call-later".into(),
                r#type: "function".into(),
                function: crate::openrouter::types::FunctionCall {
                    name: "write_file".into(),
                    arguments: "{}".into(),
                },
            },
        ];
        for call in &calls {
            crate::trajectory::record_tool_call_started(
                &pool,
                "session-1",
                "message-1",
                &call.id,
                &call.function.name,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        }

        let contents = persist_cancelled_tool_batch(&pool, "session-1", false, &calls)
            .await
            .unwrap();

        let cancelled: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tool_calls WHERE status = 'cancelled'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let replayed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages WHERE role = 'tool' AND content LIKE '%cancelled%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cancelled, 2);
        assert_eq!(replayed, 2);
        assert!(contents[1].contains("batch"));
    }

    fn settings_with_chatgpt_model(
        model: crate::config::settings::CustomModel,
        global: crate::config::settings::ReasoningEffort,
    ) -> Settings {
        let mut settings = Settings::default();
        settings.default_endpoint = "chatgpt".into();
        settings.default_model = model.id.clone();
        settings.reasoning_effort = global;
        settings.endpoints.insert(
            "chatgpt".into(),
            crate::config::settings::Endpoint {
                base_url: crate::codex_auth::CHATGPT_BASE_URL.into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                active_model: Some(model.id.clone()),
                custom_models: vec![model],
            },
        );
        settings
    }

    #[test]
    fn chatgpt_responses_user_content_preserves_text_and_image_parts_in_order() {
        let image_data_url = "data:image/png;base64,iVBORw0KGgo=";
        let content = MessageContent::Parts(vec![
            ContentPart {
                r#type: "text".into(),
                text: Some("先看截图".into()),
                image_url: None,
            },
            ContentPart {
                r#type: "image_url".into(),
                text: None,
                image_url: Some(ImageUrl {
                    url: image_data_url.into(),
                }),
            },
            ContentPart {
                r#type: "text".into(),
                text: Some("再回答问题".into()),
                image_url: None,
            },
        ]);

        assert_eq!(
            AgentLoop::content_to_chatgpt_user_parts(&content),
            vec![
                serde_json::json!({"type": "input_text", "text": "先看截图"}),
                serde_json::json!({"type": "input_image", "image_url": image_data_url}),
                serde_json::json!({"type": "input_text", "text": "再回答问题"}),
            ]
        );
    }

    #[test]
    fn chatgpt_responses_user_content_keeps_plain_text_shape() {
        assert_eq!(
            AgentLoop::content_to_chatgpt_user_parts(&MessageContent::Text("只发文字".into())),
            vec![serde_json::json!({"type": "input_text", "text": "只发文字"})]
        );
    }

    #[test]
    fn chatgpt_responses_user_content_does_not_emit_an_empty_image_url() {
        let content = MessageContent::Parts(vec![ContentPart {
            r#type: "image_url".into(),
            text: None,
            image_url: Some(ImageUrl { url: String::new() }),
        }]);

        assert_eq!(
            AgentLoop::content_to_chatgpt_user_parts(&content),
            vec![serde_json::json!({"type": "input_text", "text": ""})]
        );
    }

    #[test]
    fn chatgpt_responses_payload_contains_bytes_from_a_local_image_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("screen.png");
        let png_bytes = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(&image_path, png_bytes).unwrap();
        let markdown = format!(
            "识别这张图：\n![screen](file://{})\n只回答图中内容",
            image_path.display()
        );
        let content = MessageContent::Parts(attachments::extract_openai_parts(&markdown));

        let response_parts = AgentLoop::content_to_chatgpt_user_parts(&content);

        assert_eq!(response_parts.len(), 3);
        assert_eq!(response_parts[0]["type"], "input_text");
        assert_eq!(response_parts[1]["type"], "input_image");
        assert!(response_parts[1]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,iVBORw0KGgo"));
        assert_eq!(response_parts[2]["type"], "input_text");
    }

    #[test]
    fn chatgpt_effort_maps_legacy_ultra_override_to_transport_max() {
        use crate::config::settings::{CustomModel, ReasoningEffort};

        let settings = settings_with_chatgpt_model(
            CustomModel {
                id: "gpt-5.6-sol".into(),
                name: None,
                context_length: Some(272000),
                max_context_length: Some(272000),
                effective_context_window_percent: Some(95),
                default_reasoning_effort: Some(ReasoningEffort::Low),
                supported_reasoning_efforts: Some(vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::Max,
                ]),
            supports_vision: None,
            },
            ReasoningEffort::High,
        );

        assert_eq!(
            resolve_chatgpt_reasoning_effort(&settings, "chatgpt", "gpt-5.6-sol", Some("ultra")),
            ReasoningEffort::Max
        );
    }

    #[test]
    fn chatgpt_effort_maps_legacy_ultra_without_capability_metadata() {
        use crate::config::settings::{CustomModel, ReasoningEffort};

        let settings = settings_with_chatgpt_model(
            CustomModel {
                id: "legacy-model".into(),
                name: None,
                context_length: None,
                max_context_length: None,
                effective_context_window_percent: None,
                default_reasoning_effort: None,
                supported_reasoning_efforts: None,
                supports_vision: None,
            },
            ReasoningEffort::High,
        );

        assert_eq!(
            resolve_chatgpt_reasoning_effort(&settings, "chatgpt", "legacy-model", Some("ultra")),
            ReasoningEffort::Max
        );
    }

    #[test]
    fn chatgpt_effort_falls_back_to_model_default_when_requested_is_unsupported() {
        use crate::config::settings::{CustomModel, ReasoningEffort};

        let settings = settings_with_chatgpt_model(
            CustomModel {
                id: "gpt-5.5".into(),
                name: None,
                context_length: Some(272000),
                max_context_length: Some(272000),
                effective_context_window_percent: Some(95),
                default_reasoning_effort: Some(ReasoningEffort::Low),
                supported_reasoning_efforts: Some(vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                ]),
            supports_vision: None,
            },
            ReasoningEffort::Ultra,
        );

        assert_eq!(
            resolve_chatgpt_reasoning_effort(&settings, "chatgpt", "gpt-5.5", None),
            ReasoningEffort::Low
        );
    }

    #[test]
    fn chatgpt_effort_falls_back_to_medium_without_model_default() {
        use crate::config::settings::{CustomModel, ReasoningEffort};

        let settings = settings_with_chatgpt_model(
            CustomModel {
                id: "future-model".into(),
                name: None,
                context_length: None,
                max_context_length: None,
                effective_context_window_percent: None,
                default_reasoning_effort: None,
                supported_reasoning_efforts: Some(vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ]),
            supports_vision: None,
            },
            ReasoningEffort::Max,
        );

        assert_eq!(
            resolve_chatgpt_reasoning_effort(&settings, "chatgpt", "future-model", None),
            ReasoningEffort::Medium
        );
    }

    #[test]
    fn chatgpt_effort_preserves_legacy_behavior_without_capability_metadata() {
        use crate::config::settings::{CustomModel, ReasoningEffort};

        let settings = settings_with_chatgpt_model(
            CustomModel {
                id: "legacy-model".into(),
                name: None,
                context_length: None,
                max_context_length: None,
                effective_context_window_percent: None,
                default_reasoning_effort: None,
                supported_reasoning_efforts: None,
                supports_vision: None,
            },
            ReasoningEffort::High,
        );

        assert_eq!(
            resolve_chatgpt_reasoning_effort(&settings, "chatgpt", "legacy-model", Some("xhigh")),
            ReasoningEffort::XHigh
        );
    }

    #[test]
    fn chatgpt_effort_finds_session_model_after_default_endpoint_switches() {
        use crate::config::settings::{CustomModel, ReasoningEffort};

        let mut settings = settings_with_chatgpt_model(
            CustomModel {
                id: "gpt-session-model".into(),
                name: None,
                context_length: None,
                max_context_length: None,
                effective_context_window_percent: None,
                default_reasoning_effort: Some(ReasoningEffort::Low),
                supported_reasoning_efforts: Some(vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ]),
            supports_vision: None,
            },
            ReasoningEffort::Ultra,
        );
        settings.default_endpoint = "openrouter".into();

        assert_eq!(
            resolve_chatgpt_reasoning_effort(&settings, "chatgpt", "gpt-session-model", None),
            ReasoningEffort::Low
        );
    }

    #[test]
    fn vision_rejection_detector_matches_capability_errors_only() {
        // 2026-07-21 field report: switching the session to DeepSeek (no
        // vision) made every turn die on the replayed image attachment.
        for err in [
            "400 This model does not support image input",
            "invalid content type: image_url is not supported",
            "Multimodal content is not enabled for this model",
            "unsupported message content type Vision",
        ] {
            assert!(is_vision_rejection(err), "{err}");
        }
        for err in [
            "429 rate limit exceeded",
            "500 Internal Server Error",
            "context length exceeded",
        ] {
            assert!(!is_vision_rejection(err), "{err}");
        }
    }

    #[test]
    fn strip_image_parts_replaces_images_with_placeholders() {
        let mut messages = vec![
            ChatMessage {
                role: "user".into(),
                content: MessageContent::Parts(vec![
                    ContentPart {
                        r#type: "text".into(),
                        text: Some("看下这个截图".into()),
                        image_url: None,
                    },
                    ContentPart {
                        r#type: "image_url".into(),
                        text: None,
                        image_url: Some(ImageUrl {
                            url: "data:image/png;base64,AAAA".into(),
                        }),
                    },
                ]),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: MessageContent::Text("ok".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
        ];
        let stripped = strip_image_parts(&mut messages);
        assert_eq!(stripped, 1);
        match &messages[0].content {
            MessageContent::Parts(parts) => {
                assert!(parts.iter().all(|p| p.r#type == "text"));
                let joined: String = parts
                    .iter()
                    .filter_map(|p| p.text.clone())
                    .collect::<Vec<_>>()
                    .join(" ");
                assert!(joined.contains("看下这个截图"));
                assert!(joined.contains("图片已省略"));
            }
            MessageContent::Text(_) => panic!("parts must stay parts"),
        }
        // Idempotent: second pass strips nothing.
        assert_eq!(strip_image_parts(&mut messages), 0);
    }

    #[test]
    fn strip_image_values_handles_anthropic_shaped_content() {
        let mut messages = vec![serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "截图如下" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,BBBB" } },
            ],
        })];
        let stripped = strip_image_values(&mut messages);
        assert_eq!(stripped, 2);
        let parts = messages[0]["content"].as_array().unwrap();
        assert!(parts.iter().all(|p| p["type"] == "text"));
        assert_eq!(strip_image_values(&mut messages), 0);
    }

    #[test]
    fn turn_error_records_are_excluded_from_provider_history() {
        // A persisted turn-error notice is for the USER (and forensics); it
        // must never be replayed to the model as a fake user turn.
        let mut err = stored_message("user", "[回合错误] 400 no vision", None);
        err.completion_state = Some("turn_error".into());
        let history = vec![
            stored_message("user", "hi", None),
            err,
            stored_message("assistant", "hello", None),
        ];
        let filtered = replayable_history(history);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|m| !m.content.contains("回合错误")));
        // Gate artifacts (user-role turns the model really saw) stay in.
        let mut gate = stored_message("user", "The completion gate…", None);
        gate.completion_state = Some("gate_recovery".into());
        assert_eq!(replayable_history(vec![gate]).len(), 1);
    }

    fn stored_message(role: &str, content: &str, tool_calls: Option<String>) -> Message {
        Message {
            id: Uuid::new_v4().to_string(),
            session_id: "session-1".into(),
            role: role.into(),
            content: content.into(),
            model_id: None,
            input_tokens: None,
            output_tokens: None,
            tool_calls,
            reasoning_content: None,
            completion_state: None,
            created_at: 1,
        }
    }

    fn provider_message(
        role: &str,
        tool_calls: Option<Vec<ToolCall>>,
        tool_call_id: Option<&str>,
    ) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Text(String::new()),
            tool_calls,
            tool_call_id: tool_call_id.map(str::to_owned),
            name: None,
            reasoning_content: None,
        }
    }

    fn provider_tool_call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            r#type: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        }
    }

    // ── AgentMode contract tests ─────────────────────────────────────────
    //
    // The whole reason AgentMode exists is to flip two switches:
    //   1. iteration budget — autonomous runs need 6x interactive
    //   2. system prompt — autonomous tells the model "don't ask, iterate"
    // These tests guard against accidentally regressing either by reverting
    // a constant or losing the autonomous prompt branch.

    #[test]
    fn current_repository_is_treated_as_the_product_when_the_user_reports_app_behavior() {
        for mode in [
            AgentMode::Interactive,
            AgentMode::Execute,
            AgentMode::Autonomous,
        ] {
            let prompt = base_system_prompt(mode, Path::new("/projects/CodeFactory"));
            assert!(prompt.contains("the running product"));
            assert!(prompt.contains("fix it in this repository"));
            assert!(prompt.contains("do not stop at explaining"));
        }
    }

    #[test]
    fn repository_intent_belongs_to_git_while_plans_belong_to_the_session() {
        for mode in [
            AgentMode::Interactive,
            AgentMode::Execute,
            AgentMode::Autonomous,
        ] {
            let prompt = base_system_prompt(mode, Path::new("/projects/CodeFactory"));
            assert!(prompt.contains("# Repository-Owned Intent"));
            assert!(prompt.contains("AGENTS.md"));
            assert!(prompt.contains("docs/specs"));
            assert!(prompt.contains("docs/design"));
            assert!(prompt.contains("plans and delegated task state belong to the current conversation"));
            assert!(prompt.contains("do not direct the user to a separate specification or planning screen"));
        }
    }

    #[test]
    fn agent_mode_iteration_budgets_differ_significantly() {
        let interactive = AgentMode::Interactive.max_iterations();
        let autonomous = AgentMode::Autonomous.max_iterations();
        // Autonomous must be MUCH larger — the goal is letting subagents
        // run end-to-end without the 30-turn ceiling that caused tasks
        // to abort mid-implementation in v1.0.x.
        assert!(
            autonomous >= interactive * 4,
            "autonomous budget ({autonomous}) must be at least 4× interactive ({interactive})"
        );
        assert!(
            autonomous >= 100,
            "autonomous budget ({autonomous}) too small for real task work"
        );
    }

    #[test]
    fn agent_mode_autonomous_prompt_forbids_asking() {
        let prompt = AgentMode::Autonomous.system_prompt();
        // The whole spec for autonomous mode: the model must NOT stop to ask.
        // If someone weakens these phrases, the v1.0 'stops every 30 seconds'
        // bug returns silently.
        assert!(
            prompt.contains("AUTONOMOUS"),
            "autonomous prompt must self-identify as such"
        );
        assert!(
            prompt.contains("Never stop to ask"),
            "autonomous prompt must explicitly forbid 'should I proceed?'"
        );
        assert!(
            prompt.contains("Failure is not a stopping condition"),
            "autonomous prompt must mandate failure-iteration"
        );
        assert!(
            prompt.contains("acceptance criteria"),
            "autonomous prompt must reference acceptance criteria"
        );
    }

    #[test]
    fn agent_mode_interactive_prompt_unchanged() {
        // Interactive mode keeps the existing user-facing contract:
        // plan-first, ask before non-trivial work.
        let prompt = AgentMode::Interactive.system_prompt();
        assert!(
            prompt.contains("Plan-first"),
            "interactive prompt must keep plan-first guidance"
        );
    }

    fn policy(allow: &[&str], ask: &[&str], deny: &[&str]) -> PermissionPolicy {
        PermissionPolicy {
            allow: allow.iter().map(|v| v.to_string()).collect(),
            ask: ask.iter().map(|v| v.to_string()).collect(),
            deny: deny.iter().map(|v| v.to_string()).collect(),
            full_access: false,
        }
    }

    #[test]
    fn ask_policy_requires_user_decision() {
        let policy = policy(&["read_file"], &["bash"], &[]);
        assert_eq!(
            decide_permission(&policy, "bash", Some("pnpm build")),
            PermissionDecision::Ask
        );
    }

    #[test]
    fn full_access_honors_configured_deny_rules() {
        let mut policy = policy(&[], &["bash"], &["bash"]);
        policy.full_access = true;
        assert_eq!(
            decide_permission(&policy, "bash", Some("pnpm build")),
            PermissionDecision::Deny("Denied by policy: matches 'bash'".into())
        );
    }

    #[test]
    fn full_access_allows_unmatched_tools_without_asking() {
        let mut policy = policy(&[], &["bash"], &[]);
        policy.full_access = true;
        assert_eq!(
            decide_permission(&policy, "write_file", None),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn full_access_requires_ask_for_high_risk_shell_commands() {
        let mut policy = policy(&[], &[], &[]);
        policy.full_access = true;
        assert_eq!(
            decide_permission(&policy, "bash", Some("Remove-Item -Recurse -Force .\\dist")),
            PermissionDecision::Ask
        );
    }

    #[test]
    fn hard_denied_shell_commands_are_denied_before_full_access() {
        let mut policy = policy(&[], &[], &[]);
        policy.full_access = true;
        assert_eq!(
            decide_permission(&policy, "bash", Some("shutdown /s /t 0")),
            PermissionDecision::Deny(
                "Denied by shell safety policy: matches permanent deny 'shutdown'".into()
            )
        );
    }

    #[test]
    fn project_memory_from_codefactory_md_is_appended_to_system_prompt() {
        let cwd = std::env::temp_dir().join(format!(
            "codefactory-project-memory-test-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("CODEFACTORY.md"), "Use the repo-local memory.").unwrap();

        let prompt = build_system_prompt(&cwd);

        assert!(prompt.starts_with(SYSTEM_PROMPT));
        assert!(prompt.contains("CODEFACTORY.md"));
        assert!(prompt.contains("Use the repo-local memory."));

        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn system_prompt_names_the_exact_project_working_directory() {
        let cwd = std::env::temp_dir().join(format!(
            "codefactory-working-directory-test-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&cwd).unwrap();

        let prompt = build_system_prompt(&cwd);

        assert!(prompt.contains("# Working Directory"));
        assert!(prompt.contains(&cwd.to_string_lossy().to_string()));
        assert!(prompt.contains("Do not assume `/workspace`"));

        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn project_knowledge_blocks_prioritize_repo_authority_before_memory() {
        let cwd = std::env::temp_dir().join(format!("codefactory-pkb-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(cwd.join(".codefactory")).unwrap();
        std::fs::create_dir_all(cwd.join("docs/specs/feature-specs")).unwrap();
        std::fs::create_dir_all(cwd.join("docs/design")).unwrap();
        std::fs::write(cwd.join("AGENTS.md"), "# Repository rules\nUse pnpm.").unwrap();
        std::fs::write(
            cwd.join("docs/specs/feature-specs/login.md"),
            "# Login contract",
        )
        .unwrap();
        std::fs::write(cwd.join("docs/design/auth.md"), "# Auth design").unwrap();
        std::fs::write(
            cwd.join(".codefactory").join("memory.md"),
            "remember pnpm not npm",
        )
        .unwrap();
        std::fs::write(cwd.join("README.md"), "# MyProj\nhello world").unwrap();
        std::fs::write(cwd.join("Cargo.toml"), "[package]\nname = \"x\"").unwrap();

        let blocks = project_knowledge_blocks(&cwd);

        assert_eq!(blocks.len(), 5);
        assert_eq!(blocks[0].priority, 0);
        assert!(blocks[0].content.contains("# Repository Authority"));
        assert!(blocks[0].content.contains("Use pnpm."));
        assert_eq!(blocks[1].priority, 1);
        assert!(blocks[1].content.contains("# Repository Intent Index"));
        assert!(blocks[1].content.contains("docs/specs/feature-specs/login.md"));
        assert!(blocks[1].content.contains("docs/design/auth.md"));
        assert!(!blocks[1].content.contains(".codefactory/specs"));
        assert_eq!(blocks[2].priority, 2);
        assert!(blocks[2].content.contains("# Project Memory"));
        assert!(blocks[2].content.contains("remember pnpm not npm"));
        assert_eq!(blocks[3].priority, 4);
        assert!(blocks[3].content.contains("# Project README"));
        assert!(blocks[3].content.contains("hello world"));
        assert_eq!(blocks[4].priority, 5);
        assert!(blocks[4].content.contains("# Project Config"));
        assert!(blocks[4].content.contains("name = \"x\""));

        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn desktop_completion_gate_requires_verification_after_a_write() {
        let mut gate = CompletionGate::default();
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "write_file",
            &serde_json::json!({"path": "src/example.rs", "content": "fn main() {}"}),
            &tools::ToolOutput::ok("written"),
        );
        assert!(!gate.evidence().completed);

        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({"command": "cargo test"}),
            &tools::ToolOutput::ok("test result: ok"),
        );
        assert!(gate.evidence().completed);
    }

    #[test]
    fn desktop_completion_recovers_from_shell_setup_error_without_ending_the_session() {
        let mut gate = CompletionGate::default();
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;

        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "write_file",
            &serde_json::json!({"path": "src/app.rs", "content": "fixed"}),
            &tools::ToolOutput::ok("written"),
        );
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({
                "command": "status=0; grep -n stale src/app.rs || status=$?; test \"$status\" -le 1"
            }),
            &tools::ToolOutput::err("zsh:1: read-only variable: status"),
        );

        let failed = gate.evidence();
        assert!(failed.failed_verification_fingerprint.is_none());
        assert!(matches!(
            completion_finalization(&failed, 0, AgentMode::Interactive),
            CompletionFinalization::Recover(_)
        ));

        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({"command": "cargo test"}),
            &tools::ToolOutput::ok("test result: ok. 1 passed; 0 failed"),
        );

        let recovered = gate.evidence();
        assert!(recovered.completed, "blockers: {:?}", recovered.blockers);
        assert_eq!(
            completion_finalization(&recovered, 1, AgentMode::Interactive),
            CompletionFinalization::Complete
        );
    }

    #[test]
    fn autonomous_desktop_convergence_requires_verification_before_another_edit() {
        let mut gate = CompletionGate::new_for_instruction(
            false,
            "Repair the CLI. Running ./tool 6 should output 42.",
        );
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "write_file",
            &serde_json::json!({"path": "result.txt", "content": "candidate"}),
            &tools::ToolOutput::ok("written"),
        );
        let evidence = gate.evidence();

        let denied = autonomous_budget_denial(
            AgentMode::Autonomous,
            16,
            &evidence,
            "write_file",
            &serde_json::json!({"path": "result.txt", "content": "another candidate"}),
            Path::new("/workspace"),
        );
        assert!(denied
            .as_deref()
            .is_some_and(|message| message.contains("machine-checked verification")));

        assert!(autonomous_budget_denial(
            AgentMode::Autonomous,
            16,
            &evidence,
            "bash",
            &serde_json::json!({"command": "actual=$(./tool 6); test \"$actual\" = 42"}),
            Path::new("/workspace"),
        )
        .is_none());
        assert!(autonomous_budget_denial(
            AgentMode::Interactive,
            16,
            &evidence,
            "write_file",
            &serde_json::json!({"path": "result.txt", "content": "another candidate"}),
            Path::new("/workspace"),
        )
        .is_none());
    }

    #[test]
    fn desktop_final_stage_requires_repair_after_one_failure_diagnostic() {
        let mut gate = CompletionGate::new(true);
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "write_file",
            &serde_json::json!({"path": "src/worker.rs", "content": "candidate"}),
            &tools::ToolOutput::ok("written"),
        );
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({"command": "cargo test worker::tests::behavior"}),
            &tools::ToolOutput::err("assertion failed"),
        );
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "read_file",
            &serde_json::json!({"path": "src/worker.rs"}),
            &tools::ToolOutput::ok("candidate"),
        );
        let evidence = gate.evidence();

        for mode in [AgentMode::Autonomous, AgentMode::Execute] {
            let denied = autonomous_budget_denial(
                mode,
                8,
                &evidence,
                "read_file",
                &serde_json::json!({"path": "src/another_module.rs"}),
                Path::new("/workspace"),
            );
            assert!(denied
                .as_deref()
                .is_some_and(|message| message.contains("final-stage diagnostic read")));
        }
        assert!(autonomous_budget_denial(
            AgentMode::Interactive,
            8,
            &evidence,
            "read_file",
            &serde_json::json!({"path": "src/another_module.rs"}),
            Path::new("/workspace"),
        )
        .is_none());
    }

    #[test]
    fn desktop_completion_route_requires_non_example_behavior_evidence() {
        let mut gate = CompletionGate::new_for_instruction(
            false,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "write_file",
            &serde_json::json!({"path": "tool", "content": "implementation"}),
            &tools::ToolOutput::ok("written"),
        );
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({
                "command": "test \"$(./tool 3)\" = 9 && test \"$(./tool 5)\" = 25"
            }),
            &tools::ToolOutput::ok("examples passed"),
        );

        let smoke = gate.evidence();
        assert!(smoke.verification_diversity_required);
        assert_eq!(smoke.last_example_only_verification_sequence, Some(2));
        assert!(!smoke.completed);

        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({"command": "test \"$(./tool 7)\" = 49"}),
            &tools::ToolOutput::ok("independent case passed"),
        );
        let completed = gate.evidence();
        assert_eq!(completed.last_independent_verification_sequence, Some(3));
        assert!(completed.completed, "blockers: {:?}", completed.blockers);
    }

    #[test]
    fn desktop_completion_gate_ignores_read_only_investigation_noise() {
        // Regression for the 2026-07-16 session: printf section headers plus
        // source output containing the literal "error:" tripped the gate and
        // caused a reject/re-answer loop on a pure analysis request.
        let mut gate = CompletionGate::default();
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({
                "command": "set -e\nprintf '== agent injection ==\\n'; sed -n '445,500p' src-tauri/src/agent/mod.rs"
            }),
            &tools::ToolOutput::ok(
                "Err(e) => Ok(tools::ToolOutput::err(format!(\"MCP error: {e}\")))",
            ),
        );
        let evidence = gate.evidence();
        assert!(
            evidence.completed,
            "read-only investigation must not trip the gate: {:?}",
            evidence.blockers
        );

        // And the standard frontend verification commands satisfy the gate.
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({
                "command": "pnpm exec vitest run src/pages/Workspace/TaskCreator.test.tsx"
            }),
            &tools::ToolOutput::ok("Test Files  2 passed (2)"),
        );
        let evidence = gate.evidence();
        assert!(evidence.completed, "blockers: {:?}", evidence.blockers);
    }

    #[test]
    fn provider_overload_is_detected_for_backoff_retry() {
        // Week-audit finding: "Our servers are currently overloaded. Please
        // try again later." killed the turn outright — a transient condition
        // that deserves backoff, not death.
        for err in [
            "ChatGPT 后端返回错误:Our servers are currently overloaded. Please try again later.",
            "429 rate limit exceeded",
            "HTTP 503 Service Unavailable",
            "upstream 529 overloaded",
        ] {
            assert!(is_provider_overloaded(err), "{err}");
        }
        for err in [
            "Your input exceeds the context window of this model.",
            "This model does not support image input",
            "400 invalid request",
        ] {
            assert!(!is_provider_overloaded(err), "{err}");
        }
    }

    #[test]
    fn self_recovery_contract_is_present_in_every_chat_mode() {
        // Systemic fix for the "agent doesn't loop on obstacles" defect: the
        // behavioral contract must bind ALL modes — verify-before-asserting
        // environment state, try at least two approaches before asking, and
        // never park on "回复继续" for conditions a tool can poll.
        for mode in [AgentMode::Interactive, AgentMode::Execute, AgentMode::Autonomous] {
            let prompt = base_system_prompt(mode, Path::new("/workspace"));
            assert!(
                prompt.contains("VERIFY BEFORE ASSERTING"),
                "{mode:?} missing verify-before-asserting"
            );
            assert!(
                prompt.contains("TRY BEFORE ASKING"),
                "{mode:?} missing try-twice contract"
            );
            assert!(
                prompt.contains("POLL, DON'T PARK"),
                "{mode:?} missing poll-not-park contract"
            );
        }
    }

    #[test]
    fn delivery_blocked_claims_are_detected_narrowly() {
        for text in [
            "热修复已推送,但当前阻塞在无法自动创建 PR。请任选其一完成认证:1. 终端运行 gh auth login",
            "无法创建 PR:请在设置 → 远程仓库配置 GitHub token",
            "I cannot create the PR automatically; please run gh auth login first.",
        ] {
            assert!(claims_delivery_blocked(text), "{text}");
        }
        for text in [
            "PR #166 已创建,GitHub CLI 认证验证成功",
            "已通过 gh 触发发布工作流",
            "交付结果: delivered",
        ] {
            assert!(!claims_delivery_blocked(text), "{text}");
        }
    }

    #[test]
    fn command_missing_claims_are_detected_and_verified() {
        assert_eq!(
            claims_command_missing("docker 未安装,无法启用沙箱"),
            Some("docker".to_string())
        );
        assert_eq!(
            claims_command_missing("gh: command not found — please install the GitHub CLI"),
            Some("gh".to_string())
        );
        assert_eq!(claims_command_missing("已通过 docker 启动容器"), None);
        assert_eq!(claims_command_missing("正常回复,与命令无关"), None);
        assert!(command_exists("git"));
        assert!(!command_exists("definitely-not-a-real-binary-xyz"));
    }

    #[test]
    fn wait_for_user_on_checkable_conditions_is_detected() {
        assert!(claims_wait_for_user_on_checkable(
            "完成后回复“继续”,我会立即创建 PR 并等待 Windows CI 终态。"
        ));
        assert!(claims_wait_for_user_on_checkable(
            "配置完成后告诉我,我再继续交付。"
        ));
        assert!(!claims_wait_for_user_on_checkable(
            "你希望优先做哪一项?回复 1 或 2。"
        ));
        assert!(!claims_wait_for_user_on_checkable("任务已完成。"));
    }

    #[test]
    fn fact_check_reply_corrects_only_verified_false_claims() {
        if command_exists("docker") {
            let correction = fact_check_reply("docker 未安装,无法启用沙箱");
            assert!(correction.is_some_and(|c| c.contains("docker")));
        }
        assert!(fact_check_reply("一切正常,任务完成。").is_none());
    }

    #[test]
    fn completion_recovery_prompt_respects_mode_rejection_limits() {
        // User-facing chat gets several tool-backed repair opportunities. The
        // limit still prevents the historical unbounded near-duplicate loop,
        // while one incidental verifier/precondition mistake no longer ends
        // an otherwise active product-fix session.
        let unsatisfied = CompletionGate::new(true).evidence();
        assert!(!unsatisfied.completed);
        for attempts in 0..3 {
            assert!(
                completion_recovery_prompt(&unsatisfied, attempts, AgentMode::Interactive,)
                    .is_some()
            );
            assert!(
                completion_recovery_prompt(&unsatisfied, attempts, AgentMode::Execute).is_some()
            );
        }
        assert!(completion_recovery_prompt(&unsatisfied, 3, AgentMode::Interactive).is_none());
        assert!(completion_recovery_prompt(&unsatisfied, 3, AgentMode::Execute).is_none());
        assert!(completion_recovery_prompt(&unsatisfied, 0, AgentMode::Autonomous).is_some());
        assert!(completion_recovery_prompt(&unsatisfied, 1, AgentMode::Autonomous).is_none());
        assert!(completion_recovery_requires_tool(AgentMode::Interactive));
        assert!(completion_recovery_requires_tool(AgentMode::Execute));
        assert!(completion_recovery_requires_tool(AgentMode::Autonomous));

        let satisfied = CompletionGate::new(false).evidence();
        assert!(satisfied.completed);
        assert!(completion_recovery_prompt(&satisfied, 0, AgentMode::Interactive).is_none());
    }

    #[test]
    fn denied_tool_batch_does_not_reset_completion_recovery() {
        assert_eq!(completion_recovery_attempts_after_tool_batch(1, false), 1);
        assert_eq!(completion_recovery_attempts_after_tool_batch(1, true), 0);
    }

    #[test]
    fn exhausted_recovery_releases_with_warning_in_chat_and_blocks_autonomous() {
        // Updated from `exhausted_recovery_is_a_blocked_terminal_not_success`,
        // which pinned the behavior the 2026-07-21 field report complained
        // about: in Interactive/Execute chat, exhausting recovery FOLDED the
        // model's final reply and killed the turn with an untranslated
        // internal-contract Error ("Completion blocked because required
        // verification is still missing: rerun every unresolved failed
        // check…"). With the user present, the reply is the best available
        // answer: release it WITH a visible warning. Hard-blocking remains
        // correct only for unattended Autonomous runs (the scheduler
        // respawns those).
        let unsatisfied = CompletionGate::new(true).evidence();
        assert!(matches!(
            completion_finalization(&unsatisfied, 1, AgentMode::Interactive),
            CompletionFinalization::Recover(_)
        ));
        assert!(matches!(
            completion_finalization(&unsatisfied, 3, AgentMode::Interactive),
            CompletionFinalization::ReleaseWithWarning(_)
        ));
        assert!(matches!(
            completion_finalization(&unsatisfied, 3, AgentMode::Execute),
            CompletionFinalization::ReleaseWithWarning(_)
        ));
        if let CompletionFinalization::ReleaseWithWarning(warning) =
            completion_finalization(&unsatisfied, 3, AgentMode::Interactive)
        {
            // Human-readable Chinese, no internal-contract terminology.
            assert!(warning.contains("未经完整验证"));
            assert!(!warning.contains("Completion blocked"));
        }
        assert!(matches!(
            completion_finalization(&unsatisfied, 3, AgentMode::Autonomous),
            CompletionFinalization::Blocked(_)
        ));

        let satisfied = CompletionGate::new(false).evidence();
        assert!(matches!(
            completion_finalization(&satisfied, 99, AgentMode::Interactive),
            CompletionFinalization::Complete
        ));

        // Iteration ceiling: chat surfaces end with Done (the reply stands,
        // warning persisted separately); only Autonomous errors out.
        assert!(matches!(
            iteration_ceiling_terminal_event(&unsatisfied, AgentMode::Interactive),
            StreamEvent::Done { .. }
        ));
        assert!(matches!(
            iteration_ceiling_terminal_event(&unsatisfied, AgentMode::Autonomous),
            StreamEvent::Error { .. }
        ));
        assert!(matches!(
            iteration_ceiling_terminal_event(&satisfied, AgentMode::Interactive),
            StreamEvent::Done { .. }
        ));
    }

    #[test]
    fn prompts_demand_narration_around_tool_bursts_and_failures() {
        // 2026-07-20 field report: the user said "你不支持 GitHub 的 cli",
        // got zero text back, then watched ~15 unexplained tool cards run
        // (several red). Execute's old rule 1 said the first action should
        // be "the tool call …, not prose" — which models obeyed literally.
        // The contract is: no re-planning, but DO acknowledge and narrate.
        assert!(!SYSTEM_PROMPT_EXECUTE.contains("not prose"));
        assert!(SYSTEM_PROMPT_EXECUTE.contains("DO speak"));
        assert!(SYSTEM_PROMPT_EXECUTE.contains("directly answer"));
        assert!(SYSTEM_PROMPT_EXECUTE.contains("anything the user just said"));
        assert!(SYSTEM_PROMPT_EXECUTE.contains("it is a speaking condition"));
        assert!(SYSTEM_PROMPT_EXECUTE.contains("never silently skip past a red tool result"));

        // Interactive prompt carries the same narration discipline.
        assert!(SYSTEM_PROMPT.contains("# Narrate the work as you go"));
        assert!(SYSTEM_PROMPT.contains("what failed and how you are responding"));
    }

    #[test]
    fn completion_ready_nudge_is_autonomous_only() {
        // The ready ("coverage audit") nudge freezes the toolset for the next
        // round to force wrap-up. evidence.completed only means "some mutation
        // got verified" — in a long interactive TDD session that fires on any
        // mid-task tsc/vitest pass and forcibly stops the turn while the model
        // is announcing its next step (2026-07-17 session: agent stopped with
        // a "还不能结束,仍缺少证据" essay after 6 minutes). With the user
        // present, THEY decide when the work is done; the nudge is an
        // autonomous-contract mechanism only.
        assert!(!completion_ready_applies(AgentMode::Interactive));
        assert!(!completion_ready_applies(AgentMode::Execute));
        assert!(completion_ready_applies(AgentMode::Autonomous));
    }

    #[test]
    fn desktop_completion_gate_rejects_unprobed_background_service() {
        let mut gate = CompletionGate::default();
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({
                "command": "nohup ./server >server.log 2>&1 & echo $! >server.pid"
            }),
            &tools::ToolOutput::ok("started"),
        );
        assert!(!gate.evidence().completed);

        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "bash",
            &serde_json::json!({
                "command": "timeout 10 curl --fail http://127.0.0.1:8080/health"
            }),
            &tools::ToolOutput::ok("healthy"),
        );
        assert!(gate.evidence().completed);
    }

    #[test]
    fn desktop_compatibility_invariants_apply_to_interactive_and_autonomous_modes() {
        let evidence = CompletionEvidence {
            required_source_scan_extensions: vec![".py".to_owned()],
            blockers: vec![
                "source compatibility work requires a clean repository-wide residual scan"
                    .to_owned(),
            ],
            ..CompletionEvidence::default()
        };
        let unrelated = serde_json::json!({"command": "pytest tests/test_unrelated.py"});
        assert!(autonomous_budget_denial(
            AgentMode::Autonomous,
            8,
            &evidence,
            "bash",
            &unrelated,
            Path::new("/workspace"),
        )
        .is_some());

        let fragile_scan = serde_json::json!({
            "command": "cd /workspace && result=$(grep -r --include='*.py' 'old_value' pkg/ tests/ 2>&1); rc=$?; if [ $rc -gt 1 ]; then exit $rc; elif [ $rc -eq 0 ]; then exit 1; else echo 'CLEAN: no old_value references'; fi"
        });
        let denial = autonomous_budget_denial(
            AgentMode::Interactive,
            64,
            &evidence,
            "bash",
            &fragile_scan,
            Path::new("/workspace"),
        )
        .expect("interactive mode must reject a fragile compatibility scan");
        assert!(denial.contains("temporary results file"));
        assert!(denial.contains("test ! -s"));

        let robust_scan = serde_json::json!({
            "command": "cd /workspace && results=$(mktemp); errors=$(mktemp); grep -r --include='*.py' 'old_value' pkg/ tests/ >\"$results\" 2>\"$errors\"; rc=$?; if [ \"$rc\" -gt 1 ]; then cat \"$errors\" >&2; exit \"$rc\"; fi; test ! -s \"$results\""
        });
        assert!(autonomous_budget_denial(
            AgentMode::Interactive,
            64,
            &evidence,
            "bash",
            &robust_scan,
            Path::new("/workspace"),
        )
        .is_none());

        let source_edit = serde_json::json!({
            "path": "pkg/fast.pyx",
            "old_text": "np.int",
            "new_text": "np.int64"
        });
        assert!(autonomous_budget_denial(
            AgentMode::Autonomous,
            8,
            &evidence,
            "edit_file",
            &source_edit,
            Path::new("/workspace"),
        )
        .is_none());

        assert!(autonomous_budget_denial(
            AgentMode::Interactive,
            0,
            &CompletionEvidence::default(),
            "bash",
            &unrelated,
            Path::new("/workspace"),
        )
        .is_none());
    }

    #[test]
    fn interactive_finalization_recovers_when_completion_evidence_is_incomplete() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 1,
            blockers: vec![
                "source compatibility work requires a clean repository-wide residual scan"
                    .to_owned(),
            ],
            ..CompletionEvidence::default()
        };

        let prompt = completion_recovery_prompt(&evidence, 0, AgentMode::Interactive)
            .expect("interactive finalization must continue after a mutation with blockers");
        assert!(prompt.contains("source compatibility"));
        assert!(completion_recovery_prompt(
            &CompletionEvidence {
                completed: true,
                ..CompletionEvidence::default()
            },
            0,
            AgentMode::Interactive,
        )
        .is_none());
    }

    #[test]
    fn completion_tool_evidence_preserves_grep_pattern_and_path() {
        let (command, kind) = completion_command_and_kind(
            "grep",
            &serde_json::json!({
                "pattern": "^(import|from)",
                "path": "src",
                "glob": "*.py"
            }),
        );

        assert_eq!(kind, ToolKind::ReadOnly);
        assert!(command.contains("^(import|from)"));
        assert!(command.contains("src"));
        assert!(command.contains("--glob *.py"));

        let (root_command, _) =
            completion_command_and_kind("grep", &serde_json::json!({"pattern": "^(import|from)"}));
        assert_eq!(root_command, "grep ^(import|from) .");
    }

    #[test]
    fn persisted_history_repairs_missing_tool_results_before_the_next_user_turn() {
        let tool_calls = serde_json::json!([{
            "id": "call-denied",
            "type": "function",
            "function": {"name": "bash", "arguments": "{}"}
        }])
        .to_string();
        let repaired = repair_incomplete_tool_history(vec![
            stored_message("assistant", "", Some(tool_calls)),
            stored_message("user", "continue", None),
        ]);

        assert_eq!(repaired.len(), 3);
        assert_eq!(repaired[1].role, "tool");
        let (tool_call_id, content) = parse_tool_message_content(&repaired[1].content);
        assert_eq!(tool_call_id, "call-denied");
        assert!(content.contains("unavailable in persisted history"));
        assert_eq!(repaired[2].role, "user");
    }

    #[test]
    fn provider_payload_repairs_missing_tool_result_before_user_message() {
        let repaired = repair_openai_tool_protocol(vec![
            provider_message(
                "assistant",
                Some(vec![provider_tool_call("call-missing")]),
                None,
            ),
            provider_message("user", None, None),
        ]);

        assert_eq!(repaired.len(), 3);
        assert_eq!(repaired[1].role, "tool");
        assert_eq!(repaired[1].tool_call_id.as_deref(), Some("call-missing"));
        assert_eq!(repaired[2].role, "user");
    }

    #[test]
    fn provider_payload_repairs_only_missing_result_for_multi_tool_assistant() {
        let repaired = repair_openai_tool_protocol(vec![
            provider_message(
                "assistant",
                Some(vec![
                    provider_tool_call("call-a"),
                    provider_tool_call("call-b"),
                ]),
                None,
            ),
            provider_message("tool", None, Some("call-a")),
            provider_message("user", None, None),
        ]);

        let tool_ids: Vec<_> = repaired
            .iter()
            .filter(|message| message.role == "tool")
            .filter_map(|message| message.tool_call_id.as_deref())
            .collect();
        assert_eq!(tool_ids, vec!["call-a", "call-b"]);
        assert_eq!(
            repaired.last().map(|message| message.role.as_str()),
            Some("user")
        );
    }

    #[test]
    fn provider_payload_drops_orphan_tool_result() {
        let repaired = repair_openai_tool_protocol(vec![
            provider_message("system", None, None),
            provider_message("tool", None, Some("call-orphan")),
            provider_message("user", None, None),
        ]);

        assert_eq!(repaired.len(), 2);
        assert!(repaired.iter().all(|message| message.role != "tool"));
    }

    #[test]
    fn provider_payload_drops_empty_and_duplicate_tool_call_ids() {
        let repaired = repair_openai_tool_protocol(vec![
            provider_message(
                "assistant",
                Some(vec![
                    provider_tool_call(""),
                    provider_tool_call("call-a"),
                    provider_tool_call("call-a"),
                ]),
                None,
            ),
            provider_message("user", None, None),
        ]);

        let assistant_calls = repaired[0].tool_calls.as_deref().unwrap();
        assert_eq!(assistant_calls.len(), 1);
        assert_eq!(assistant_calls[0].id, "call-a");
        assert_eq!(
            repaired
                .iter()
                .filter(|message| message.role == "tool")
                .count(),
            1
        );
    }

    #[test]
    fn desktop_edit_path_activates_interactive_compatibility_scan_gate() {
        let mut gate = CompletionGate::new_for_instruction(
            false,
            "修复 runtime 升级后已移除 API 的源码兼容问题。",
        );
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "edit_file",
            &serde_json::json!({
                "path": "/workspace/compatdemo/service.py",
                "old_string": "rt.old_value(value)",
                "new_string": "rt.new_value(value)"
            }),
            &tools::ToolOutput::ok("Edited /workspace/compatdemo/service.py"),
        );

        let evidence = gate.evidence();
        assert_eq!(evidence.required_source_scan_extensions, vec![".py"]);
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("clean repository-wide residual scan")));

        let fragile_scan = serde_json::json!({
            "command": "cd /workspace && result=$(grep -r --include='*.py' 'old_value' compatdemo/ tests/ 2>&1); rc=$?; if [ $rc -gt 1 ]; then exit $rc; elif [ $rc -eq 0 ]; then exit 1; else echo 'CLEAN: no old_value references'; fi"
        });
        assert!(autonomous_budget_denial(
            AgentMode::Interactive,
            64,
            &evidence,
            "bash",
            &fragile_scan,
            Path::new("/workspace"),
        )
        .is_some());
    }

    #[test]
    fn completion_finalization_disables_openai_tools() {
        let (tools, tool_choice) = openai_tool_controls(&[], false);

        assert!(tools.is_none());
        assert_eq!(tool_choice, serde_json::json!("none"));
    }

    #[test]
    fn completion_recovery_requires_an_openai_tool_call() {
        let definitions = tools::all_definitions();

        let (tools, tool_choice) = openai_tool_controls(&definitions, true);
        assert_eq!(tools.as_ref().map(Vec::len), Some(definitions.len()));
        assert_eq!(tool_choice, serde_json::json!("required"));

        let (_, normal_choice) = openai_tool_controls(&definitions, false);
        assert_eq!(normal_choice, serde_json::json!("auto"));
    }

    #[test]
    fn completion_finalization_selects_an_empty_tool_surface() {
        let definitions = tools::all_definitions();

        assert!(active_tool_definitions(&definitions, true).is_empty());
        assert_eq!(
            active_tool_definitions(&definitions, false).len(),
            definitions.len()
        );
    }

    #[test]
    fn incomplete_openai_sse_stream_is_rejected() {
        let error = validate_openai_sse_completion(false, 0, 0).unwrap_err();
        assert!(error.contains("[DONE]"));
    }

    #[test]
    fn malformed_or_partial_openai_sse_stream_is_rejected() {
        assert!(validate_openai_sse_completion(true, 0, 1).is_err());
        assert!(validate_openai_sse_completion(true, 8, 0).is_err());
    }
}
