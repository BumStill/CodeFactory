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
mod context_policy;
mod fact_checker;
mod lifecycle_hooks;
pub mod model_transport;
mod permission_gateway;
pub mod persistence;
pub mod scheduler;
pub mod sse_buffer;
pub mod subagent;
mod tool_backend;
pub mod user_context;
pub mod verification;
pub mod worktree;

pub use dispatch::decide_chat_mode;

#[cfg(test)]
use codefactory_agent_core::CompletionEvidence;
use futures_util::{Stream, StreamExt};
use reqwest::Client;
use sqlx::SqlitePool;
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
// `LifecycleHooks`/`NoOpHooks` are imported by name (used as a `dyn` type + built
// directly); `DesktopLifecycleHooks` lives in `lifecycle_hooks`.
use codefactory_agent_loop::services::{LifecycleHooks, NoOpHooks};
// After slice 4.7 BOTH provider loops live in `run_agent_loop`; the bin is a thin
// adapter, so most loop-body helpers (gate/prompt/tool/transport/protocol fns)
// are referenced only inside agent-loop now. The mode-policy AgentMode wrappers
// below still delegate to `policy::`, and several fns are exercised by bin unit
// tests that stayed here — those imports are `#[cfg(test)]`-gated.
use codefactory_agent_loop::policy::openai_tool_controls;
#[cfg(test)]
use codefactory_agent_loop::policy::{self, CompletionFinalization};
#[cfg(test)]
use codefactory_agent_loop::context::is_provider_overloaded;
#[cfg(test)]
use codefactory_agent_loop::run::cancelled_tool_suffix;
#[cfg(test)]
use codefactory_agent_loop::policy::{
    active_tool_definitions, completion_command_and_kind,
    completion_recovery_attempts_after_tool_batch, record_completion_outcome,
};
#[cfg(test)]
use codefactory_agent_loop::protocol::{
    is_vision_rejection, repair_openai_tool_protocol, strip_image_parts,
};
#[cfg(test)]
use codefactory_agent_core::{CompletionGate, ProgressTracker};
use codefactory_agent_loop::run::FinalizationPolicy;

/// Desktop `AgentMode` → the loop crate's `FinalizationPolicy`.
fn finalization_policy(mode: AgentMode) -> FinalizationPolicy {
    match mode {
        AgentMode::Interactive | AgentMode::Execute => FinalizationPolicy::ReleaseWithWarning,
        AgentMode::Autonomous => FinalizationPolicy::BlockOnIncomplete,
    }
}

/// Rejected-final-response recovery budget per run (was `completion_recovery_limit`).
fn recovery_limit_for(mode: AgentMode) -> u32 {
    match mode {
        AgentMode::Interactive | AgentMode::Execute => 3,
        AgentMode::Autonomous => 1,
    }
}

/// Chat modes auto-continue across segment checkpoints; only unattended
/// Autonomous runs apply the converging wall-budget tool policy.
fn wall_budget_applies(mode: AgentMode) -> bool {
    matches!(mode, AgentMode::Autonomous)
}

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

// `cancelled_tool_suffix` moved to `agent-loop::run` (keystone slice 4.6b) and is
// re-imported below so both provider loops keep the unqualified name.

/// Internal checkpoint cadence for INTERACTIVE chat. Reaching this count
/// produces a progress summary and begins another segment when the task is
/// still advancing; it is not a task-level execution limit.
const MAX_ITERATIONS_INTERACTIVE: usize = 30;

/// Iteration ceiling for AUTONOMOUS execution (subagents, approved
/// task runs). The whole point is to NOT bounce back to the user for
/// every micro-decision, so the budget is much larger. Most iterations
/// are tool round-trips, not LLM turns — they're cheap.
const MAX_ITERATIONS_AUTONOMOUS: usize = 200;

/// Internal checkpoint cadence for EXECUTE turns — the chat surface right
/// after the user approved a plan. It is less frequent than interactive chat
/// while still giving the user natural progress summaries and an opportunity
/// to interject between continuing segments.
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
    /// Selects segment checkpoint cadence and system prompt. Interactive for
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

/// Read the per-session ChatGPT reasoning-effort override
/// (`sessions.reasoning_effort`), or `None` when unset / no such session. The
/// loop calls this once per round (keystone slice 4.4d) so the transport reads
/// no DB; a mid-run change is picked up on the next round.
async fn fetch_session_reasoning_effort(db: &SqlitePool, session_id: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT reasoning_effort FROM sessions WHERE id = ?")
        .bind(session_id)
        .fetch_one(db)
        .await
        .ok()
        .flatten()
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
    /// - hooks are skipped entirely (`NoOpHooks` when `app` is absent);
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
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
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

    /// Desktop adapter for the shared `run_agent_loop` (keystone slice 4.6b,
    /// generalized to both providers in 4.7): build the per-turn inputs, the run
    /// config, and the capability services, then drive the ONE shared loop. The
    /// `AppHandle`-owning handles (`HookRunner`, `DesktopToolBackend`) are
    /// constructed ONLY here — inside this `run()`-reached fn the unit-test EXE
    /// dead-strips (#166) — and erased into `Arc<dyn …>` so `run_agent_loop`
    /// links no tauri. The three flags are the ONLY per-provider difference:
    /// OpenAI/ChatGPT = (compress, no-backoff, expand-window); Anthropic =
    /// (no-compress, backoff, flat-window). The transport itself dispatches on
    /// `self.api_style` inside `complete()`.
    async fn run_via_agent_loop(
        &mut self,
        history: Vec<Message>,
        tool_defs: &[ToolDefinition],
        system_prompt: &str,
        context_compression: bool,
        overload_backoff: bool,
        expand_context_window: bool,
    ) -> Result<()> {
        let hooks: std::sync::Arc<dyn LifecycleHooks> = match &self.app {
            None => std::sync::Arc::new(NoOpHooks),
            Some(app) => {
                let runner = if self.anonymous {
                    hooks::HookRunner::disabled(app.clone())
                } else {
                    let settings = self.settings.read().await;
                    hooks::HookRunner::from_settings(&settings, app.clone())
                };
                std::sync::Arc::new(lifecycle_hooks::DesktopLifecycleHooks { runner })
            }
        };
        let tool_backend = tool_backend::DesktopToolBackend {
            #[cfg(not(test))]
            app: self.app.clone(),
            db: self.db.clone(),
            mcp_manager: self.mcp_manager.clone(),
            settings: self.settings.clone(),
        };
        let completion_instruction = latest_user_instruction(&history);
        let fact_check_instruction = effective_fact_check_instruction(&history);
        let messages = self.build_openai_messages(history, system_prompt);
        let inputs = codefactory_agent_loop::run::LoopInputs {
            messages,
            system_prompt: system_prompt.to_string(),
            tool_defs: tool_defs.to_vec(),
            completion_instruction,
            fact_check_instruction,
            audit_session_id: self.audit_session_id(),
            knowledge_library_ids: knowledge_scope_for_tools(self.execution_context.as_ref()),
            cancel: self.cancel.clone(),
        };
        let config = codefactory_agent_loop::run::RunConfig {
            finalization: finalization_policy(self.mode),
            gate_benchmark: false,
            progress_window: 8,
            recovery_limit: recovery_limit_for(self.mode),
            max_iterations: self.mode.max_iterations(),
            wall_budget_applies: wall_budget_applies(self.mode),
            context_compression,
            overload_backoff,
            session_id: self.session_id.clone(),
            endpoint_name: self.endpoint_name.clone(),
            model_id: self.model_id.clone(),
            base_url: self.base_url.clone(),
            usage_run_id: self.usage_run_id.clone(),
            surface: self
                .execution_context
                .as_ref()
                .map_or(UsageSurface::Interactive, |c| c.usage_surface)
                .as_str()
                .to_string(),
            task_id: self.execution_context.as_ref().and_then(|c| c.task_id.clone()),
            anonymous: self.anonymous,
            is_chatgpt: self.api_style == ApiStyle::Chatgpt,
            cwd: self.cwd.clone(),
        };
        let svc = codefactory_agent_loop::run::LoopServices {
            transport: std::sync::Arc::new(self.model_transport()),
            tools: std::sync::Arc::new(tool_backend),
            persistence: std::sync::Arc::new(self.persistence()),
            events: self.events.clone(),
            budget: std::sync::Arc::new(codefactory_agent_loop::journal::NullBudget),
            // Today's token-based elision, unchanged (slice 4.8c seam).
            compactor: std::sync::Arc::new(codefactory_agent_loop::services::DefaultCompressor),
            permission: std::sync::Arc::new(self.permission_gateway()),
            hooks,
            context_policy: std::sync::Arc::new(self.context_policy(expand_context_window)),
            fact_checker: std::sync::Arc::new(fact_checker::DesktopFactChecker { mode: self.mode }),
        };
        // The desktop discards the returned RunOutcome (Done already emitted via
        // the sink); LoopError maps to AppError::Other verbatim (message-identical).
        codefactory_agent_loop::run::run_agent_loop(inputs, config, svc).await?;
        Ok(())
    }

    /// OpenAI/ChatGPT: compress history, no overload backoff, expandable window.
    async fn run_openai(
        &mut self,
        history: Vec<Message>,
        tool_defs: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<()> {
        self.run_via_agent_loop(history, tool_defs, system_prompt, true, false, true)
            .await
    }

    /// Anthropic (keystone slice 4.7): NO compression (it never elides), reactive
    /// overload backoff, flat `default_limit` context window. The transport's
    /// `complete()` converts canonical `ChatMessage` ↔ the Anthropic wire at the
    /// edge, so this drives the SAME shared loop as OpenAI.
    async fn run_anthropic(
        &mut self,
        history: Vec<Message>,
        tool_defs: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<()> {
        self.run_via_agent_loop(history, tool_defs, system_prompt, false, true, false)
            .await
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

    /// Build the desktop context policy for this run (keystone slice 4.6). Reads
    /// the live `Settings`/db each round through the trait; owns no `AppHandle`.
    /// Absorbs the old `resolve_round_reasoning_effort` (per-round freshness: a
    /// mid-run `sessions.reasoning_effort` change takes effect next round) plus
    /// the `supports_vision`/`context_window` reads.
    fn context_policy(&self, expand_context_window: bool) -> context_policy::DesktopContextPolicy {
        context_policy::DesktopContextPolicy {
            settings: self.settings.clone(),
            db: self.db.clone(),
            session_id: self.session_id.clone(),
            endpoint_name: self.endpoint_name.clone(),
            model_id: self.model_id.clone(),
            api_style: self.api_style.clone(),
            expand_context_window,
        }
    }

    /// Build the desktop permission gateway for this run (keystone slice 4.6).
    /// Clones only `Arc` handles — settings, the event sink, the shared
    /// pending-permission map, and the SAME cancel `Arc` — and owns no
    /// `AppHandle`. Reads the live policy and prompts the frontend on `Ask`.
    fn permission_gateway(&self) -> permission_gateway::DesktopPermissionGateway {
        permission_gateway::DesktopPermissionGateway {
            settings: self.settings.clone(),
            events: self.events.clone(),
            pending_permissions: self.pending_permissions.clone(),
            cancel: self.cancel.clone(),
        }
    }

    /// Build the desktop model transport for this run (keystone slice 4.5a).
    /// Clones only Arc/Client handles + small Strings — crucially the SAME
    /// cancel `Arc` (shared `AtomicBool`), and NO `AppHandle` (#166). The three
    /// run-loop call sites dispatch through this; the transport methods now live
    /// on [`model_transport::DesktopModelTransport`].
    fn model_transport(&self) -> model_transport::DesktopModelTransport {
        model_transport::DesktopModelTransport {
            http: self.http.clone(),
            events: self.events.clone(),
            model_id: self.model_id.clone(),
            session_id: self.session_id.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            api_style: self.api_style.clone(),
            cancel: self.cancel.clone(),
        }
    }

    /// Build the desktop persistence backend for this run. Cheap (clones the
    /// pool handle + session id); the inherent persist_* helpers below delegate
    /// to it so all message/trajectory writes — and the anonymous no-trace
    /// guard — live in one place ([`persistence::SqlitePersistence`]).
    fn persistence(&self) -> persistence::SqlitePersistence {
        persistence::SqlitePersistence {
            db: self.db.clone(),
            session_id: self.session_id.clone(),
            anonymous: self.anonymous,
        }
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

/// How many times one run may reject a tool-call-free final response and ask
/// the model to resolve concrete completion blockers. User-facing chat gets
/// three bounded, tool-required recovery opportunities: enough to survive an
/// incidental shell/precondition mistake without restoring the historical
/// unbounded near-duplicate loop. Autonomous attempts stay single-shot because
/// the scheduler can respawn them with a fresh evidence brief.

// Test-only since slice 4.7: both loops call `policy::` directly with the
// resolved scalars; these AgentMode wrappers stay to pin the mode→policy map.
#[cfg(test)]
fn completion_recovery_requires_tool(_mode: AgentMode) -> bool {
    true
}

// Test-only since slice 4.7: both loops call `policy::` directly with the
// resolved scalars; these AgentMode wrappers stay to pin the mode→policy map.
#[cfg(test)]
fn completion_finalization(
    evidence: &CompletionEvidence,
    attempts: u32,
    mode: AgentMode,
) -> CompletionFinalization {
    policy::completion_finalization(
        evidence,
        attempts,
        finalization_policy(mode),
        recovery_limit_for(mode),
    )
}

/// User-facing warning when a chat turn ends without complete verification.
/// Chinese, plain language, no gate terminology; the raw blocker list goes
/// to the log only.

// Test-only since slice 4.7: both loops call `policy::` directly with the
// resolved scalars; these AgentMode wrappers stay to pin the mode→policy map.
#[cfg(test)]
fn iteration_ceiling_terminal_event(evidence: &CompletionEvidence, mode: AgentMode) -> StreamEvent {
    policy::iteration_ceiling_terminal_event(evidence, finalization_policy(mode))
}

// Test-only since slice 4.7: both loops call `policy::` directly with the
// resolved scalars; these AgentMode wrappers stay to pin the mode→policy map.
#[cfg(test)]
fn completion_recovery_prompt(
    evidence: &CompletionEvidence,
    attempts: u32,
    mode: AgentMode,
) -> Option<String> {
    policy::completion_recovery_prompt(
        evidence,
        attempts,
        finalization_policy(mode),
        recovery_limit_for(mode),
    )
}

/// Placeholder inserted in place of an image part when the active model
// `IMAGE_STRIPPED_PLACEHOLDER`, `is_vision_rejection`, `strip_image_parts`,
// `strip_image_values` moved to `agent-loop::protocol` (keystone slice 4.6b),
// re-imported at the top so both loops + the bin unit tests keep the names.

/// History rows that may be replayed to the provider on a later user turn.
/// Completion-review controls are persisted for UI recovery and forensics,
/// but replaying them would inject obsolete framework instructions into a new
/// request. The current run already holds its gate prompts in memory.
fn replayable_history(history: Vec<Message>) -> Vec<Message> {
    history
        .into_iter()
        .filter(|m| {
            !matches!(
                m.completion_state.as_deref(),
                Some(
                    "turn_error"
                        | "turn_notice"
                        | "gate_recovery"
                        | "gate_ready"
                        | "rejected_candidate"
                        | "gate_blocked"
                        | "gate_warning"
                )
            )
        })
        .collect()
}

/// The active objective comes only from a real user-authored row. Older
/// releases persisted internal `turn_notice` controls with role=user; ignore
/// every completion-state row so those legacy notices cannot become the next
/// turn's completion instruction even before a database migration or reload.
fn latest_user_instruction(history: &[Message]) -> String {
    history
        .iter()
        .rev()
        .find(|message| message.role == "user" && message.completion_state.is_none())
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

/// Fact checking follows the effective objective, not just the latest row.
/// A short approval ("做吧", "继续", "go ahead") inherits the immediately
/// preceding assistant proposal, matching the same dispatch semantics that
/// moved the turn into Execute mode. Internal completion-state rows never
/// participate in that inheritance.
fn effective_fact_check_instruction(history: &[Message]) -> String {
    let Some((user_index, user_message)) = history
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| message.role == "user" && message.completion_state.is_none())
    else {
        return String::new();
    };

    if !dispatch::is_approval(&user_message.content) {
        return user_message.content.clone();
    }

    let previous_proposal = history[..user_index]
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && message.completion_state.is_none())
        .map(|message| message.content.trim())
        .filter(|content| !content.is_empty());

    match previous_proposal {
        Some(proposal) => format!("{proposal}\n\n用户批准：{}", user_message.content),
        None => user_message.content.clone(),
    }
}

/// Transient provider saturation worth a backoff retry instead of a dead
/// turn. Distinct from capacity (context) and capability (vision) errors.
// `is_provider_overloaded` moved to `agent-loop::context` (keystone slice 4.7);
// re-exported via `context::` (bin) so run_anthropic + the matcher test resolve it.

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

/// Candidate assertion units for fact checking.
///
/// Fact checks must never combine unrelated words from separate paragraphs,
/// quoted material, or fenced examples into a new claim. A field failure on
/// 2026-07-24 did exactly that: "请检查模型配置" near the top of a product
/// analysis plus "GitHub token 不可用" in a later Agent example was read as
/// "please configure a GitHub token", hijacking the answer into delivery.
fn strip_inline_non_assertive_spans(line: &str) -> String {
    let mut stripped = String::with_capacity(line.len());
    let mut quote_end = None;
    let chars = line.chars().collect::<Vec<_>>();
    for (index, ch) in chars.iter().copied().enumerate() {
        if let Some(end) = quote_end {
            if ch == end {
                quote_end = None;
            }
            continue;
        }
        quote_end = match ch {
            '`' => Some('`'),
            '"' => Some('"'),
            '\'' if (index == 0 || !chars[index - 1].is_alphanumeric())
                && chars[index + 1..].contains(&'\'') =>
            {
                Some('\'')
            }
            '“' => Some('”'),
            '‘' => Some('’'),
            '「' => Some('」'),
            '『' => Some('』'),
            '《' => Some('》'),
            _ => {
                stripped.push(ch);
                None
            }
        };
    }
    stripped
}

fn fact_claim_units(text: &str) -> Vec<String> {
    let mut units = Vec::new();
    let mut in_fenced_code = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fenced_code = !in_fenced_code;
            continue;
        }
        if in_fenced_code
            || trimmed.is_empty()
            || trimmed.starts_with('>')
            || (trimmed.starts_with('|') && trimmed.ends_with('|'))
        {
            continue;
        }
        let assertive = strip_inline_non_assertive_spans(trimmed);
        units.extend(
            assertive
                .split(|ch| matches!(ch, '。' | '！' | '？' | '!' | '?' | '；' | ';'))
                .map(str::trim)
                .filter(|unit| !unit.is_empty())
                .map(str::to_string),
        );
    }
    units
}

/// User intent keeps quoted UI labels and inline tool names because they may
/// be the action itself ("点击“创建 PR”", "调用 `deliver_changes`"). Candidate
/// fact parsing strips those spans instead, since a reply may merely quote an
/// error. Fenced examples and blockquotes remain excluded on both paths.
fn intent_units(text: &str) -> Vec<String> {
    let mut units = Vec::new();
    let mut in_fenced_code = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fenced_code = !in_fenced_code;
            continue;
        }
        if in_fenced_code || trimmed.is_empty() || trimmed.starts_with('>') {
            continue;
        }
        units.extend(
            trimmed
                .split(|ch| matches!(ch, '。' | '！' | '？' | '!' | '?' | '；' | ';'))
                .map(str::trim)
                .filter(|unit| !unit.is_empty())
                .map(str::to_string),
        );
    }
    units
}

fn is_example_or_hypothesis(unit: &str) -> bool {
    let lower = unit.to_lowercase();
    [
        "示例",
        "例如",
        "比如",
        "假设",
        "方案示意",
        "example",
        "for example",
        "e.g.",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Does this reply claim the delivery channel is unusable (cannot open a
/// PR / needs gh auth login / needs a token)? Each match must be local to one
/// assertion unit; successful-delivery reports and examples are exempt.
fn claims_delivery_blocked(text: &str) -> bool {
    let success_markers = [
        "已创建",
        "已通过 gh",
        "交付结果: delivered",
        "验证成功",
        "pr created",
    ];
    let inability = ["无法", "不能", "cannot", "can't", "unable"];
    let channel = ["创建 pr", "开 pr", "create the pr", "create a pr", "自动创建 pr"];
    fact_claim_units(text).into_iter().any(|unit| {
        if is_example_or_hypothesis(&unit) {
            return false;
        }
        let lower = unit.to_lowercase();
        if success_markers
            .iter()
            .any(|marker| lower.contains(&marker.to_lowercase()))
        {
            return false;
        }
        let inability_hit = inability.iter().any(|word| lower.contains(word))
            && channel.iter().any(|word| lower.contains(word));
        let setup_demand = (lower.contains("gh auth login")
            || (lower.contains("token")
                && (lower.contains("配置") || lower.contains("configure"))))
            && (lower.contains("请")
                || lower.contains("please")
                || lower.contains("完成认证"));
        inability_hit || setup_demand
    })
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

fn instruction_requests_delivery(instruction: &str) -> bool {
    let request_markers = [
        "请",
        "立即",
        "现在",
        "继续",
        "直接",
        "帮我",
        "开始",
        "完成",
        "然后",
        "并",
        "please",
        "go ahead",
        "continue",
        "then",
    ];
    let delivery_actions = [
        "deliver_changes",
        "提交并推送",
        "提交改动",
        "提交这些",
        "推送分支",
        "推送改动",
        "创建 pr",
        "开 pr",
        "开个 pr",
        "合并 pr",
        "发版",
        "发布版本",
        "完成交付",
        "继续交付",
        "交付改动",
        "交付当前",
        "commit and push",
        "push the branch",
        "push these changes",
        "create a pr",
        "open a pr",
        "open the pr",
        "merge the pr",
        "ship the changes",
    ];
    let analysis_markers = [
        "分析",
        "解释",
        "为什么",
        "是什么",
        "是否",
        "怎么",
        "如何",
        "评估",
        "review",
        "explain",
        "why",
        "how",
    ];
    let execution_bridges = [
        "立即",
        "直接",
        "然后",
        "再",
        "接着",
        "完成后",
        "分析并提交",
        "检查并提交",
        "修改并提交",
        "go ahead",
        "then",
    ];

    intent_units(instruction).into_iter().any(|unit| {
        if is_example_or_hypothesis(&unit) {
            return false;
        }
        let lower = unit.to_lowercase();
        let asks_analysis = analysis_markers
            .iter()
            .any(|marker| lower.contains(marker));
        if asks_analysis
            && !execution_bridges
                .iter()
                .any(|bridge| lower.contains(bridge))
        {
            return false;
        }
        delivery_actions
            .iter()
            .any(|action| lower.contains(action))
            && request_markers
                .iter()
                .any(|marker| lower.contains(marker))
    })
}

fn delivery_fact_check_applies(mode: AgentMode, instruction: &str) -> bool {
    !matches!(mode, AgentMode::Interactive) && instruction_requests_delivery(instruction)
}

fn delivery_fact_check_correction(
    text: &str,
    completion_instruction: &str,
    mode: AgentMode,
    gh_cli_available: bool,
) -> Option<String> {
    if !delivery_fact_check_applies(mode, completion_instruction)
        || !claims_delivery_blocked(text)
        || !gh_cli_available
    {
        return None;
    }
    Some(
        "事实纠偏:你刚声称交付通道不可用/需要配置,但本机 GitHub CLI 已登录且刚刚实测可用。\
这是基于过期上下文的错误判断。立即调用 deliver_changes 完成交付并报告其真实结果;\
不要再要求用户配置任何令牌或运行 gh auth login。"
            .to_string(),
    )
}

/// Fact-check a tool-call-free reply against live probes. Text detectors run
/// only for execution turns; an interactive analysis must finish as an answer,
/// never as a hidden correction loop. Probes run ONLY on a text match, so
/// ordinary execution turns pay nothing. Delivery corrections additionally
/// require a user instruction that explicitly asks for delivery.
fn fact_check_reply(
    text: &str,
    completion_instruction: &str,
    mode: AgentMode,
) -> Option<String> {
    if matches!(mode, AgentMode::Interactive) {
        return None;
    }
    if let Some(correction) = delivery_fact_check_correction(
        text,
        completion_instruction,
        mode,
        delivery::gh_cli_available(),
    ) {
        return Some(correction);
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
// Test-only since slice 4.7: both loops call `policy::` directly with the
// resolved scalars; these AgentMode wrappers stay to pin the mode→policy map.
#[cfg(test)]
fn completion_ready_applies(mode: AgentMode) -> bool {
    policy::completion_ready_applies(finalization_policy(mode))
}

// Test-only since slice 4.7: both loops call `policy::` directly with the
// resolved scalars; these AgentMode wrappers stay to pin the mode→policy map.
#[cfg(test)]
fn autonomous_budget_denial(
    mode: AgentMode,
    remaining_model_rounds: u32,
    evidence: &CompletionEvidence,
    tool_name: &str,
    args: &serde_json::Value,
    working_directory: &Path,
) -> Option<String> {
    // Desktop has no wall clock (`None`, slice 4.8c b3) and uses the DEFAULT
    // `format_budget_denial` wording — applying both here keeps these tests
    // pinning the exact user-facing string the loop produces.
    use codefactory_agent_loop::services::PermissionGateway as _;
    policy::autonomous_budget_denial(
        wall_budget_applies(mode),
        remaining_model_rounds,
        None,
        evidence,
        tool_name,
        args,
        working_directory,
    )
    .map(|denial| {
        codefactory_agent_loop::services::AllowAllPermissions
            .format_budget_denial(&denial.rule, &denial.reason)
    })
}

// `record_completion_outcome` moved to `agent-loop::policy` (keystone slice
// 4.6b), its `&tools::ToolOutput` param flattened to `(content, is_error)`.
// Re-imported below; both loops pass `&output.content, output.is_error`.

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

// `repair_openai_tool_protocol` moved to `agent-loop::protocol` (keystone slice
// 4.6b), re-imported at the top so both loops + the bin unit tests keep the name.

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
    /// Build the `ToolInvocationResult` the loop now feeds the gate with
    /// (slice 4.8c b2), applying the SAME classification the loop used inline
    /// before — so these #135/#136 gate tests pin identical behaviour.
    fn tool_result(
        tool_name: &str,
        args: &serde_json::Value,
        content: &str,
        is_error: bool,
    ) -> codefactory_agent_loop::tool::ToolInvocationResult {
        let (command, kind) =
            codefactory_agent_loop::policy::completion_command_and_kind(tool_name, args);
        codefactory_agent_loop::tool::ToolInvocationResult {
            content: content.to_string(),
            is_error,
            command,
            kind,
            return_code: None,
            stdout: String::new(),
            stderr: String::new(),
            error: None,
            next_working_directory: None,
            duration_ms: 0,
        }
    }
    use super::*;
    // `completion_command_and_kind` (and its `ToolKind` result) moved to
    // agent-loop in slice 4.6; this test still exercises it via the re-export.
    use codefactory_agent_core::ToolKind;

    #[tokio::test]
    async fn session_reasoning_effort_reflects_the_current_row() {
        // Freshness contract for slice 4.4d: the loop re-reads this per round via
        // resolve_round_reasoning_effort, so a mid-run change to
        // sessions.reasoning_effort is picked up on the NEXT round — hoisting the
        // read out of the transport must not freeze it at run start.
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY, reasoning_effort TEXT)")
            .execute(&db)
            .await
            .unwrap();
        // No row → None (unset override).
        assert_eq!(fetch_session_reasoning_effort(&db, "s1").await, None);
        sqlx::query("INSERT INTO sessions (id, reasoning_effort) VALUES ('s1', 'high')")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(
            fetch_session_reasoning_effort(&db, "s1").await.as_deref(),
            Some("high")
        );
        // Change mid-run → the next read reflects it, not the stale value.
        sqlx::query("UPDATE sessions SET reasoning_effort='low' WHERE id='s1'")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(
            fetch_session_reasoning_effort(&db, "s1").await.as_deref(),
            Some("low")
        );
    }

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
    fn internal_completion_artifacts_are_excluded_from_next_turn_provider_history() {
        let mut err = stored_message("user", "[回合错误] 400 no vision", None);
        err.completion_state = Some("turn_error".into());
        let mut recovery = stored_message("user", "The completion gate…", None);
        recovery.completion_state = Some("gate_recovery".into());
        let mut ready = stored_message("user", "Finalize now.", None);
        ready.completion_state = Some("gate_ready".into());
        let mut rejected = stored_message("assistant", "Done, but not verified.", None);
        rejected.completion_state = Some("rejected_candidate".into());
        let mut warning = stored_message("assistant", "Verification incomplete.", None);
        warning.completion_state = Some("gate_warning".into());

        let filtered = replayable_history(vec![
            stored_message("user", "hi", None),
            err,
            recovery,
            ready,
            rejected,
            warning,
            stored_message("assistant", "hello", None),
        ]);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].content, "hi");
        assert_eq!(filtered[1].content, "hello");
    }

    #[test]
    fn latest_user_instruction_ignores_legacy_user_role_turn_notices() {
        let real_user = stored_message(
            "user",
            "仔细分析待执行入口，不要扩展成无关的交付任务。",
            None,
        );
        let mut legacy_notice = stored_message(
            "user",
            "事实纠偏：立即调用 deliver_changes 完成交付。",
            None,
        );
        legacy_notice.completion_state = Some("turn_notice".into());

        assert_eq!(
            latest_user_instruction(&[real_user, legacy_notice]),
            "仔细分析待执行入口，不要扩展成无关的交付任务。"
        );
    }

    #[test]
    fn fact_check_instruction_inherits_an_approved_delivery_proposal() {
        let proposal = stored_message(
            "assistant",
            "方案已经准备好：提交并推送当前改动，然后创建 PR。是否开始实施？",
            None,
        );
        let approval = stored_message("user", "做吧", None);
        let inherited = effective_fact_check_instruction(&[proposal, approval]);

        assert!(instruction_requests_delivery(&inherited), "{inherited}");
    }

    #[test]
    fn fact_check_instruction_does_not_invent_delivery_for_plain_continuation() {
        let analysis = stored_message(
            "assistant",
            "待执行入口应该只在用户有可操作事项时出现。",
            None,
        );
        let continuation = stored_message("user", "继续", None);
        let inherited = effective_fact_check_instruction(&[analysis, continuation]);

        assert!(!instruction_requests_delivery(&inherited), "{inherited}");
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
            "t",
            &tool_result("write_file", &serde_json::json!({"path": "src/example.rs", "content": "fn main() {}"}), "written", false),
        );
        assert!(!gate.evidence().completed);

        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "t",
            &tool_result("bash", &serde_json::json!({"command": "cargo test"}), "test result: ok", false),
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
            "t",
            &tool_result("write_file", &serde_json::json!({"path": "src/app.rs", "content": "fixed"}), "written", false),
        );
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "t",
            &tool_result("bash", &serde_json::json!({
                "command": "status=0; grep -n stale src/app.rs || status=$?; test \"$status\" -le 1"
            }), "zsh:1: read-only variable: status", true),
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
            "t",
            &tool_result("bash", &serde_json::json!({"command": "cargo test"}), "test result: ok. 1 passed; 0 failed", false),
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
            "t",
            &tool_result("write_file", &serde_json::json!({"path": "result.txt", "content": "candidate"}), "written", false),
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
    fn unattended_final_stage_requires_repair_after_one_failure_diagnostic() {
        let mut gate = CompletionGate::new(true);
        let mut progress = ProgressTracker::new(8);
        let mut sequence = 0;
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "t",
            &tool_result("write_file", &serde_json::json!({"path": "src/worker.rs", "content": "candidate"}), "written", false),
        );
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "t",
            &tool_result("bash", &serde_json::json!({"command": "cargo test worker::tests::behavior"}), "assertion failed", true),
        );
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "t",
            &tool_result("read_file", &serde_json::json!({"path": "src/worker.rs"}), "candidate", false),
        );
        let evidence = gate.evidence();

        let denied = autonomous_budget_denial(
            AgentMode::Autonomous,
            8,
            &evidence,
            "read_file",
            &serde_json::json!({"path": "src/another_module.rs"}),
            Path::new("/workspace"),
        );
        assert!(denied
            .as_deref()
            .is_some_and(|message| message.contains("final-stage diagnostic read")));

        for mode in [AgentMode::Interactive, AgentMode::Execute] {
            assert!(autonomous_budget_denial(
                mode,
                8,
                &evidence,
                "read_file",
                &serde_json::json!({"path": "src/another_module.rs"}),
                Path::new("/workspace"),
            )
            .is_none());
        }
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
            "t",
            &tool_result("write_file", &serde_json::json!({"path": "tool", "content": "implementation"}), "written", false),
        );
        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "t",
            &tool_result("bash", &serde_json::json!({
                "command": "test \"$(./tool 3)\" = 9 && test \"$(./tool 5)\" = 25"
            }), "examples passed", false),
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
            "t",
            &tool_result("bash", &serde_json::json!({"command": "test \"$(./tool 7)\" = 49"}), "independent case passed", false),
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
            "t",
            &tool_result("bash", &serde_json::json!({
                "command": "set -e\nprintf '== agent injection ==\\n'; sed -n '445,500p' src-tauri/src/agent/mod.rs"
            }), "Err(e) => Ok(tools::ToolOutput::err(format!(\"MCP error: {e}\")))", false),
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
            "t",
            &tool_result("bash", &serde_json::json!({
                "command": "pnpm exec vitest run src/pages/Workspace/TaskCreator.test.tsx"
            }), "Test Files  2 passed (2)", false),
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
            "任务已委派，若长时间未开始请检查模型配置。\n\nAgent B：检查发布流程\n原因：GitHub token 不可用",
            "产品方案示例：无法创建 PR，请在设置中配置 GitHub token。",
            "界面现在展示错误文案：“无法创建 PR，请在设置中配置 GitHub token”。",
            "文案应为 `无法创建 PR，请在设置中配置 GitHub token`。",
            "界面展示 '无法创建 PR，请在设置中配置 GitHub token' 作为错误文案。",
            "| 场景 | 示例文案 |\n| --- | --- |\n| 未认证 | 无法创建 PR，请在设置中配置 GitHub token |",
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
            let correction = fact_check_reply(
                "docker 未安装,无法启用沙箱",
                "请运行 Docker 构建并修复失败。",
                AgentMode::Execute,
            );
            assert!(correction.is_some_and(|c| c.contains("docker")));
        }
        assert!(
            fact_check_reply(
                "一切正常,任务完成。",
                "请完成当前修改。",
                AgentMode::Execute,
            )
            .is_none()
        );
    }

    #[test]
    fn fact_check_cannot_redirect_an_interactive_analysis_turn() {
        let blocked_claim =
            "无法创建 PR：请在设置中配置 GitHub token，然后我才能继续。";
        for analysis_instruction in [
            "仔细分析一下待执行入口，我觉得应该做成后台任务结果或子 agent 展示。",
            "请分析提交并推送流程是否合理，不要实际执行。",
            "如何设计创建 PR 和发布版本的状态展示？",
        ] {
            assert!(
                fact_check_reply(
                    blocked_claim,
                    analysis_instruction,
                    AgentMode::Interactive,
                )
                .is_none(),
                "{analysis_instruction}"
            );
        }
        for unrelated_candidate in [
            "方案示例：docker 未安装时应展示环境错误。",
            "认证完成后回复继续，我再介绍后续产品流程。",
        ] {
            assert!(
                fact_check_reply(
                    unrelated_candidate,
                    "请分析待执行状态的产品表达。",
                    AgentMode::Interactive,
                )
                .is_none(),
                "{unrelated_candidate}"
            );
        }

        if delivery::gh_cli_available() {
            for delivery_instruction in [
                "请提交并推送当前改动，然后创建 PR。",
                "请分析失败原因，然后提交改动并创建 PR。",
            ] {
                assert!(
                    fact_check_reply(
                        blocked_claim,
                        delivery_instruction,
                        AgentMode::Execute,
                    )
                    .is_some_and(|correction| correction.contains("deliver_changes")),
                    "{delivery_instruction}"
                );
            }
        }
    }

    #[test]
    fn delivery_fact_check_positive_path_is_deterministic_without_local_gh() {
        for delivery_instruction in [
            "请提交并推送当前改动，然后创建 PR。",
            "请点击“创建 PR”并继续。",
            "请调用 `deliver_changes` 完成交付。",
        ] {
            let correction = delivery_fact_check_correction(
                "无法创建 PR：请在设置中配置 GitHub token，然后我才能继续。",
                delivery_instruction,
                AgentMode::Execute,
                true,
            );
            assert!(
                correction.is_some_and(|text| text.contains("deliver_changes")),
                "{delivery_instruction}"
            );
        }
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
    fn completion_recovery_attempt_count_is_monotonic_across_tool_batches() {
        assert_eq!(completion_recovery_attempts_after_tool_batch(1, false), 1);
        assert_eq!(completion_recovery_attempts_after_tool_batch(1, true), 1);
        assert_eq!(completion_recovery_attempts_after_tool_batch(2, true), 2);
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
            "t",
            &tool_result("bash", &serde_json::json!({
                "command": "nohup ./server >server.log 2>&1 & echo $! >server.pid"
            }), "started", false),
        );
        assert!(!gate.evidence().completed);

        record_completion_outcome(
            &mut gate,
            &mut progress,
            &mut sequence,
            Path::new("/workspace"),
            "t",
            &tool_result("bash", &serde_json::json!({
                "command": "timeout 10 curl --fail http://127.0.0.1:8080/health"
            }), "healthy", false),
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
            "t",
            &tool_result("edit_file", &serde_json::json!({
                "path": "/workspace/compatdemo/service.py",
                "old_string": "rt.old_value(value)",
                "new_string": "rt.new_value(value)"
            }), "Edited /workspace/compatdemo/service.py", false),
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
