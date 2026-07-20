// SPDX-License-Identifier: Apache-2.0
pub mod anthropic_client;
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
pub mod user_context;
pub mod verification;
pub mod worktree;

pub use dispatch::decide_chat_mode;

use chrono::Utc;
use codefactory_agent_core::{
    build_budget_convergence_prompt, build_completion_ready_prompt,
    build_completion_recovery_prompt, classify_command, evaluate_budget_command_in_directory,
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
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;
use tokio::time::timeout;
use uuid::Uuid;

use crate::config::settings::{ApiStyle, PermissionPolicy, Settings};
use crate::errors::Result;
use crate::mcp::McpManager;
use crate::openrouter::types::*;
use crate::storage::Message;
use crate::tools::{self, ExecCtx};
use crate::PendingPermissionMap;

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
1. **Do not re-plan and do not re-ask.** Do NOT restate the plan, do NOT\n\
   reply with a fresh plan, and do NOT end with \"Ready to proceed?\" or any\n\
   \"should I…?\" confirmation. Approval was already given — re-confirming is\n\
   a contract violation. Your FIRST action this turn should be the tool call\n\
   that starts the approved work, not prose.\n\
\n\
2. **Produce the deliverable, not a proposal for it.** If the approval named\n\
   an output (\"output a PPT\", \"生成报告\", \"build the endpoint\"), produce that\n\
   artifact. Describing how you *would* produce it is a failure.\n\
\n\
3. **Failure is not a stopping condition.** When a tool errors / a test\n\
   fails / a build breaks: diagnose, fix, re-run. Iterate a few times before\n\
   surfacing anything. Don't bounce back to the user on the first snag.\n\
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
    app: AppHandle,
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

#[derive(Debug, Clone, Default)]
pub struct AgentExecutionContext {
    pub parent_session_id: Option<String>,
    pub task_id: Option<String>,
    pub knowledge_library_ids: Vec<String>,
}

fn knowledge_scope_for_tools(
    execution_context: Option<&AgentExecutionContext>,
) -> Option<Vec<String>> {
    execution_context.map(|context| context.knowledge_library_ids.clone())
}

impl AgentLoop {
    fn emit_transport_retry(
        app: &AppHandle,
        event_name: &str,
        notice: crate::http_util::RetryNotice,
    ) {
        app.emit(
            event_name,
            StreamEvent::TransportRetry {
                label: notice.label,
                attempt: notice.attempt as u32,
                max_attempts: notice.max_attempts as u32,
                delay_ms: notice.delay.as_millis() as u64,
                reason: notice.reason,
            },
        )
        .ok();
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
        Self {
            app,
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
            anonymous: false,
            cancel: None,
        }
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

    fn emit_cancelled_done(&self, event_name: &str) {
        tracing::info!("chat turn cancelled by user (session {})", self.session_id);
        self.app
            .emit(
                event_name,
                StreamEvent::Done {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            )
            .ok();
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
        let event_name = format!("stream:{}", self.session_id);
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
        for body in crate::commands::skills::enabled_skill_prompts(&self.app).await {
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
                self.run_openai(history, &tool_defs, &event_name, &system_prompt)
                    .await
            }
            ApiStyle::Anthropic => {
                self.run_anthropic(history, &tool_defs, &event_name, &system_prompt)
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
        event_name: &str,
        system_prompt: &str,
    ) -> Result<()> {
        let completion_instruction = history
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.clone())
            .unwrap_or_default();
        let mut messages = self.build_openai_messages(history, system_prompt);
        let hook_runner = if self.anonymous {
            hooks::HookRunner::disabled(self.app.clone())
        } else {
            let settings = self.settings.read().await;
            hooks::HookRunner::from_settings(&settings, self.app.clone())
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
        let max_iterations = self.mode.max_iterations();
        for iteration in 0..max_iterations {
            // Cooperative cancellation: if the user hit "stop" for this chat
            // turn, end the stream cleanly between rounds. Checked here (not
            // mid tool-call) so in-flight work isn't hard-killed. No-op unless
            // a cancel flag was attached (chat only) and has actually tripped.
            if self.is_cancelled() {
                self.emit_cancelled_done(event_name);
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
                self.app
                    .emit(
                        event_name,
                        StreamEvent::ContextCompressed {
                            elided_count: compression.elided_count,
                            tokens_freed: compression.tokens_freed,
                        },
                    )
                    .ok();
            }

            let active_tool_defs = active_tool_definitions(tool_defs, finalization_pending);
            let (text, tool_calls, usage, reasoning) = match self.api_style {
                ApiStyle::Chatgpt => {
                    self.call_chatgpt_model(&messages, active_tool_defs, event_name)
                        .await?
                }
                _ => {
                    self.call_openai_model(&messages, active_tool_defs, event_name)
                        .await?
                }
            };
            finalization_pending = false;

            if self.is_cancelled() {
                self.emit_cancelled_done(event_name);
                emitted_terminal = true;
                break;
            }

            // Emit real (provider-reported) context-usage right after each
            // round-trip so the UI bar tracks actual usage, not just our
            // estimate. The estimate is only used to *trigger* compression.
            if let Some(u) = &usage {
                self.app
                    .emit(
                        event_name,
                        StreamEvent::ContextUsage {
                            used_tokens: u.prompt_tokens,
                            limit_tokens: context_limit,
                            max_limit_tokens: max_context_limit,
                        },
                    )
                    .ok();
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
                let evidence = completion_gate.evidence();
                if let Some(prompt) =
                    completion_recovery_prompt(&evidence, completion_recovery_attempts, self.mode)
                {
                    completion_recovery_attempts += 1;
                    // Make the rejection visible instead of silently looping:
                    // collapse the rejected candidate in the UI, persist the
                    // injected instruction so rebuilt history stays faithful.
                    self.mark_rejected_candidate(assistant_message_id.as_deref())
                        .await?;
                    self.persist_gate_message(&prompt, "gate_recovery").await?;
                    self.app
                        .emit(
                            event_name,
                            StreamEvent::CompletionGateAction {
                                kind: "recovery".into(),
                                detail: evidence.blockers.join("; "),
                            },
                        )
                        .ok();
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
                // Always emit a terminal Done so the frontend's `streaming`
                // flag clears — even when the provider omitted usage on the
                // final turn. Previously Done was gated behind `usage`, so a
                // missing usage left the chat hung "running" forever.
                let (done_in, done_out) = usage
                    .as_ref()
                    .map(|u| (u.prompt_tokens, u.completion_tokens))
                    .unwrap_or((0, 0));
                self.app
                    .emit(
                        &event_name,
                        StreamEvent::Done {
                            input_tokens: done_in,
                            output_tokens: done_out,
                        },
                    )
                    .ok();
                emitted_terminal = true;
                // Cost is only recorded when the provider reported usage.
                if let Some(u) = &usage {
                    // Persist cost entry and notify frontend to refresh stats.
                    // Anonymous runs are NEVER billed into today/month stats.
                    if !self.anonymous {
                        let inp = u.prompt_tokens as i64;
                        let out = u.completion_tokens as i64;
                        if let Err(e) = crate::commands::costs::record_cost_entry(
                            &self.db,
                            &self.session_id,
                            &self.model_id,
                            &self.endpoint_name,
                            inp,
                            out,
                        )
                        .await
                        {
                            tracing::warn!("Failed to record cost entry: {e}");
                        } else {
                            self.app.emit("token-usage-recorded", &self.session_id).ok();
                        }
                    }
                }
                break;
            }

            let mut result_messages = Vec::new();
            let mut progress_prompt = None;

            for (tool_index, tc) in tool_calls.iter().enumerate() {
                if let Some(remaining) =
                    cancelled_tool_suffix(self.cancel.as_ref(), &tool_calls, tool_index)
                {
                    return self
                        .finish_cancelled_tool_batch(event_name, remaining)
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

                self.app
                    .emit(
                        &event_name,
                        StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args: args.clone(),
                        },
                    )
                    .ok();

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
                            match self.request_permission(&event_name, tc, args.clone()).await {
                                PermissionResponse::Allow => None,
                                PermissionResponse::Deny => Some(
                                    "Tool call denied by user. Please try a different approach."
                                        .to_string(),
                                ),
                                PermissionResponse::Cancelled => {
                                    return self
                                        .finish_cancelled_tool_batch(
                                            &event_name,
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
                    self.app
                        .emit(
                            &event_name,
                            StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
                            },
                        )
                        .ok();
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

                // Pre-tool hook: may cancel
                let pre_allowed = hook_runner
                    .fire(hooks::HookEvent::PreTool {
                        tool_name: tc.function.name.clone(),
                        args: args.clone(),
                    })
                    .await;
                if !pre_allowed {
                    let content = "Tool call cancelled by hook.".to_string();
                    self.record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    self.app
                        .emit(
                            &event_name,
                            StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
                            },
                        )
                        .ok();
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

                let ctx = ExecCtx {
                    cwd: self.cwd.clone(),
                    db: Some(self.db.clone()),
                    session_id: Some(self.audit_session_id()),
                    task_id: self
                        .execution_context
                        .as_ref()
                        .and_then(|ctx| ctx.task_id.clone()),
                    knowledge_library_ids: knowledge_scope_for_tools(
                        self.execution_context.as_ref(),
                    ),
                    settings: Some(self.settings.read().await.clone()),
                };

                let tool_start = std::time::Instant::now();
                // Check if this is an MCP tool
                let mcp_server = self.mcp_manager.find_tool_server(&tc.function.name).await;
                let output_result = if let Some(server_id) = mcp_server {
                    match self
                        .mcp_manager
                        .call_tool(&server_id, &tc.function.name, args)
                        .await
                    {
                        Ok(text) => Ok(tools::ToolOutput::ok(text)),
                        Err(e) => Ok(tools::ToolOutput::err(format!("MCP error: {e}"))),
                    }
                } else {
                    tools::dispatch(&tc.function.name, args, &ctx).await
                };
                let duration_ms = tool_start.elapsed().as_millis() as u64;
                let output = match output_result {
                    Ok(output) => output,
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
                        return Err(error);
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

                // Post-tool hook
                hook_runner
                    .fire(hooks::HookEvent::PostTool {
                        tool_name: tc.function.name.clone(),
                        result: output.content.chars().take(500).collect(),
                        duration_ms,
                    })
                    .await;

                self.app
                    .emit(
                        &event_name,
                        StreamEvent::ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: output.content.clone(),
                            is_error: output.is_error,
                            status: if output.is_error {
                                "error".into()
                            } else {
                                "done".into()
                            },
                        },
                    )
                    .ok();

                result_messages.push(ChatMessage {
                    role: "tool".into(),
                    content: MessageContent::Text(output.content),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(tc.function.name.clone()),
                    reasoning_content: None,
                });
            }

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
                self.app
                    .emit(
                        event_name,
                        StreamEvent::CompletionGateAction {
                            kind: "ready".into(),
                            detail: String::new(),
                        },
                    )
                    .ok();
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

        // Safety net: the loop only emits a terminal Done when the model
        // produces a tool-call-free turn. If it instead burns through the whole
        // iteration budget (a tool call every round — far more likely on
        // Execute turns), no terminal event was sent and the frontend would
        // hang "running" forever. Emit one now so the stream always closes.
        if !emitted_terminal {
            tracing::warn!(
                "agent loop hit the iteration ceiling ({}) without a terminal turn; emitting Done",
                self.mode.max_iterations(),
            );
            self.app
                .emit(
                    &event_name,
                    StreamEvent::Done {
                        input_tokens: 0,
                        output_tokens: 0,
                    },
                )
                .ok();
        }

        Ok(())
    }

    async fn request_permission(
        &self,
        event_name: &str,
        tc: &ToolCall,
        args: serde_json::Value,
    ) -> PermissionResponse {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.pending_permissions
            .lock()
            .await
            .insert(tc.id.clone(), sender);

        self.app
            .emit(
                event_name,
                StreamEvent::PermissionRequest {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    args,
                },
            )
            .ok();

        let allow =
            await_permission_response(receiver, self.cancel.as_ref(), Duration::from_secs(600))
                .await;
        self.pending_permissions.lock().await.remove(&tc.id);
        allow
    }

    async fn finish_cancelled_tool_batch(
        &self,
        event_name: &str,
        remaining: &[ToolCall],
    ) -> Result<()> {
        let contents =
            persist_cancelled_tool_batch(&self.db, &self.session_id, self.anonymous, remaining)
                .await?;
        for (index, (tc, content)) in remaining.iter().zip(contents).enumerate() {
            if index > 0 {
                let args = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                self.app
                    .emit(
                        event_name,
                        StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args,
                        },
                    )
                    .ok();
            }
            self.app
                .emit(
                    event_name,
                    StreamEvent::ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: content.clone(),
                        is_error: true,
                        status: "cancelled".into(),
                    },
                )
                .ok();
        }
        self.app
            .emit(
                event_name,
                StreamEvent::Done {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            )
            .ok();
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
    async fn call_chatgpt_model(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        event_name: &str,
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
            "tool_choice": if tools.is_empty() { "none" } else { "auto" },
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
            |notice| Self::emit_transport_retry(&self.app, event_name, notice),
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
                                    self.app
                                        .emit(
                                            event_name,
                                            StreamEvent::TextDelta {
                                                content: d.to_string(),
                                            },
                                        )
                                        .ok();
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
            self.app
                .emit(
                    event_name,
                    StreamEvent::TextDelta {
                        content: text_buf.clone(),
                    },
                )
                .ok();
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
        event_name: &str,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>, Option<String>)> {
        let finalization_response = tool_defs.is_empty();
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // Strip OpenRouter-style "vendor/" prefix when talking to a direct
        // provider API. Defensive against ids that linger from earlier
        // OpenRouter use after the user switches endpoint.
        let outbound_model =
            crate::config::settings::normalize_model_id(&self.model_id, &self.base_url);

        let (tools, tool_choice) = openai_tool_controls(tool_defs);
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
            |notice| Self::emit_transport_retry(&self.app, event_name, notice),
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
                    |notice| Self::emit_transport_retry(&self.app, event_name, notice),
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
                            self.app
                                .emit(&event_name, StreamEvent::TextDelta { content: t.clone() })
                                .ok();
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
            self.app
                .emit(
                    &event_name,
                    StreamEvent::TextDelta {
                        content: text_buf.clone(),
                    },
                )
                .ok();
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
            "INSERT INTO messages (id, session_id, role, content, input_tokens, output_tokens, tool_calls, reasoning_content, created_at) \
             VALUES (?,?,?,?,?,?,?,?,?)",
        )
        .bind(&msg_id)
        .bind(&self.session_id)
        .bind(role)
        .bind(persisted_content)
        .bind(input_tok)
        .bind(output_tok)
        .bind(tool_calls_json)
        .bind(persisted_reasoning)
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

        for m in repair_incomplete_tool_history(history) {
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

        for m in repair_incomplete_tool_history(history) {
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

    async fn run_anthropic(
        &mut self,
        history: Vec<Message>,
        tool_defs: &[ToolDefinition],
        event_name: &str,
        system_prompt: &str,
    ) -> Result<()> {
        let completion_instruction = history
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.clone())
            .unwrap_or_default();
        let mut messages = self.build_anthropic_messages(history);
        let hook_runner = if self.anonymous {
            hooks::HookRunner::disabled(self.app.clone())
        } else {
            let settings = self.settings.read().await;
            hooks::HookRunner::from_settings(&settings, self.app.clone())
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
        let max_iterations = self.mode.max_iterations();
        for iteration in 0..max_iterations {
            // Cooperative cancellation: if the user hit "stop" for this chat
            // turn, end the stream cleanly between rounds. Checked here (not
            // mid tool-call) so in-flight work isn't hard-killed. No-op unless
            // a cancel flag was attached (chat only) and has actually tripped.
            if self.is_cancelled() {
                self.emit_cancelled_done(event_name);
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
            let resp = anthropic_client::stream_anthropic(
                &self.http,
                &self.base_url,
                &self.api_key,
                &self.model_id,
                system_prompt,
                messages.clone(),
                active_tool_defs,
                self.cancel.as_ref(),
                &self.app,
                event_name,
            )
            .await?;
            finalization_pending = false;

            if resp.cancelled || self.is_cancelled() {
                self.emit_cancelled_done(event_name);
                emitted_terminal = true;
                break;
            }

            // Emit context usage if Anthropic reported it (it sets 0 when missing)
            if resp.input_tokens > 0 {
                self.app
                    .emit(
                        event_name,
                        StreamEvent::ContextUsage {
                            used_tokens: resp.input_tokens.max(0) as u32,
                            limit_tokens: context_limit,
                            max_limit_tokens: max_context_limit,
                        },
                    )
                    .ok();
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
                let evidence = completion_gate.evidence();
                if let Some(prompt) =
                    completion_recovery_prompt(&evidence, completion_recovery_attempts, self.mode)
                {
                    completion_recovery_attempts += 1;
                    // Make the rejection visible instead of silently looping:
                    // collapse the rejected candidate in the UI, persist the
                    // injected instruction so rebuilt history stays faithful.
                    self.mark_rejected_candidate(assistant_message_id.as_deref())
                        .await?;
                    self.persist_gate_message(&prompt, "gate_recovery").await?;
                    self.app
                        .emit(
                            event_name,
                            StreamEvent::CompletionGateAction {
                                kind: "recovery".into(),
                                detail: evidence.blockers.join("; "),
                            },
                        )
                        .ok();
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "text",
                            "text": prompt,
                        }],
                    }));
                    continue;
                }
                emitted_terminal = true;
                let inp = resp.input_tokens;
                let out = resp.output_tokens;
                self.app
                    .emit(
                        event_name,
                        StreamEvent::Done {
                            input_tokens: inp as u32,
                            output_tokens: out as u32,
                        },
                    )
                    .ok();
                // Persist cost entry — NEVER for anonymous runs (no billing,
                // no usage-stats trace). Mirrors the OpenAI-path guard.
                if !self.anonymous {
                    if let Err(e) = crate::commands::costs::record_cost_entry(
                        &self.db,
                        &self.session_id,
                        &self.model_id,
                        &self.endpoint_name,
                        inp,
                        out,
                    )
                    .await
                    {
                        tracing::warn!("Failed to record cost entry (anthropic): {e}");
                    } else {
                        self.app.emit("token-usage-recorded", &self.session_id).ok();
                    }
                }
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

            for (tool_index, tc) in tool_calls.iter().enumerate() {
                if let Some(remaining) =
                    cancelled_tool_suffix(self.cancel.as_ref(), &tool_calls, tool_index)
                {
                    return self
                        .finish_cancelled_tool_batch(event_name, remaining)
                        .await;
                }
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                let completion_args = args.clone();

                self.app
                    .emit(
                        event_name,
                        StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args: args.clone(),
                        },
                    )
                    .ok();

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
                            match self.request_permission(event_name, tc, args.clone()).await {
                                PermissionResponse::Allow => None,
                                PermissionResponse::Deny => Some(
                                    "Tool call denied by user. Please try a different approach."
                                        .to_string(),
                                ),
                                PermissionResponse::Cancelled => {
                                    return self
                                        .finish_cancelled_tool_batch(
                                            event_name,
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
                    self.app
                        .emit(
                            event_name,
                            StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
                            },
                        )
                        .ok();
                    tool_result_blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tc.id,
                        "content": content,
                    }));
                    continue;
                }

                // Pre-tool hook: may cancel
                let pre_allowed = hook_runner
                    .fire(hooks::HookEvent::PreTool {
                        tool_name: tc.function.name.clone(),
                        args: args.clone(),
                    })
                    .await;
                if !pre_allowed {
                    let content = "Tool call cancelled by hook.".to_string();
                    self.record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    self.app
                        .emit(
                            event_name,
                            StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
                            },
                        )
                        .ok();
                    tool_result_blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tc.id,
                        "content": content,
                    }));
                    continue;
                }

                let ctx = ExecCtx {
                    cwd: self.cwd.clone(),
                    db: Some(self.db.clone()),
                    session_id: Some(self.audit_session_id()),
                    task_id: self
                        .execution_context
                        .as_ref()
                        .and_then(|ctx| ctx.task_id.clone()),
                    knowledge_library_ids: knowledge_scope_for_tools(
                        self.execution_context.as_ref(),
                    ),
                    settings: Some(self.settings.read().await.clone()),
                };

                let tool_start = std::time::Instant::now();
                // Check if this is an MCP tool
                let mcp_server = self.mcp_manager.find_tool_server(&tc.function.name).await;
                let output_result = if let Some(server_id) = mcp_server {
                    match self
                        .mcp_manager
                        .call_tool(&server_id, &tc.function.name, args)
                        .await
                    {
                        Ok(text) => Ok(tools::ToolOutput::ok(text)),
                        Err(e) => Ok(tools::ToolOutput::err(format!("MCP error: {e}"))),
                    }
                } else {
                    tools::dispatch(&tc.function.name, args, &ctx).await
                };
                let duration_ms = tool_start.elapsed().as_millis() as u64;
                let output = match output_result {
                    Ok(output) => output,
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
                        return Err(error);
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

                // Post-tool hook
                hook_runner
                    .fire(hooks::HookEvent::PostTool {
                        tool_name: tc.function.name.clone(),
                        result: output.content.chars().take(500).collect(),
                        duration_ms,
                    })
                    .await;

                self.app
                    .emit(
                        event_name,
                        StreamEvent::ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: output.content.clone(),
                            is_error: output.is_error,
                            status: if output.is_error {
                                "error".into()
                            } else {
                                "done".into()
                            },
                        },
                    )
                    .ok();

                tool_result_blocks.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tc.id,
                    "content": output.content,
                }));
            }

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
                self.app
                    .emit(
                        event_name,
                        StreamEvent::CompletionGateAction {
                            kind: "ready".into(),
                            detail: String::new(),
                        },
                    )
                    .ok();
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

        // Safety net: see run_openai — if the loop exhausted its iteration
        // budget without a tool-call-free turn, no Done was emitted and the
        // frontend would hang "running" forever. Emit one so the stream closes.
        if !emitted_terminal {
            tracing::warn!(
                "agent loop hit the iteration ceiling ({}) without a terminal turn; emitting Done",
                self.mode.max_iterations(),
            );
            self.app
                .emit(
                    event_name,
                    StreamEvent::Done {
                        input_tokens: 0,
                        output_tokens: 0,
                    },
                )
                .ok();
        }

        Ok(())
    }
}

fn openai_tool_controls(
    tool_defs: &[ToolDefinition],
) -> (Option<Vec<ToolDefinition>>, serde_json::Value) {
    if tool_defs.is_empty() {
        (None, serde_json::json!("none"))
    } else {
        (Some(tool_defs.to_vec()), serde_json::json!("auto"))
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
        || matches!(tool_name, "write_file" | "edit_file")
    {
        ToolKind::Mutation
    } else {
        ToolKind::ReadOnly
    };
    (command, kind)
}

/// How many times one run may reject the model's tool-call-free final response
/// and inject a completion-recovery prompt. Interactive chat gets exactly one
/// nudge: the user is watching, and every rejected candidate is already
/// rendered, so a rejection loop reads as the assistant repeating the same
/// answer (2026-07-16 session: seven near-identical replies in 13 minutes).
/// Execute/Autonomous runs get a few more attempts before the gate yields.
fn completion_recovery_limit(mode: AgentMode) -> u32 {
    match mode {
        AgentMode::Interactive => 1,
        AgentMode::Execute | AgentMode::Autonomous => 3,
    }
}

fn completion_recovery_prompt(
    evidence: &CompletionEvidence,
    attempts: u32,
    mode: AgentMode,
) -> Option<String> {
    if attempts >= completion_recovery_limit(mode) {
        return None;
    }
    (!evidence.completed).then(|| build_completion_recovery_prompt(evidence))
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
        blocks.push(Block::new(mem, 1, 8200));
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
        "{}\n\n{}\n\n# Working Directory\n\
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
    fn autonomous_empty_knowledge_scope_remains_explicitly_empty() {
        let context = AgentExecutionContext {
            parent_session_id: Some("parent".into()),
            task_id: Some("task".into()),
            knowledge_library_ids: Vec::new(),
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
            },
            ReasoningEffort::Ultra,
        );
        settings.default_endpoint = "openrouter".into();

        assert_eq!(
            resolve_chatgpt_reasoning_effort(&settings, "chatgpt", "gpt-session-model", None),
            ReasoningEffort::Low
        );
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
    fn project_knowledge_blocks_cover_memory_readme_config_with_priorities() {
        let cwd = std::env::temp_dir().join(format!("codefactory-pkb-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(cwd.join(".codefactory")).unwrap();
        std::fs::write(
            cwd.join(".codefactory").join("memory.md"),
            "remember pnpm not npm",
        )
        .unwrap();
        std::fs::write(cwd.join("README.md"), "# MyProj\nhello world").unwrap();
        std::fs::write(cwd.join("Cargo.toml"), "[package]\nname = \"x\"").unwrap();

        let blocks = project_knowledge_blocks(&cwd);

        // memory, README, config — in that render order, with eviction
        // priorities memory(1) < README(4) < config(5).
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].priority, 1);
        assert!(blocks[0].content.contains("# Project Memory"));
        assert!(blocks[0].content.contains("remember pnpm not npm"));
        assert_eq!(blocks[1].priority, 4);
        assert!(blocks[1].content.contains("# Project README"));
        assert!(blocks[1].content.contains("hello world"));
        assert_eq!(blocks[2].priority, 5);
        assert!(blocks[2].content.contains("# Project Config"));
        assert!(blocks[2].content.contains("name = \"x\""));

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
    fn completion_recovery_prompt_respects_mode_rejection_limits() {
        // Interactive chat gets exactly one recovery nudge; a rejection loop
        // reads to the user as the assistant repeating the same answer.
        let unsatisfied = CompletionGate::new(true).evidence();
        assert!(!unsatisfied.completed);
        assert!(completion_recovery_prompt(&unsatisfied, 0, AgentMode::Interactive).is_some());
        assert!(completion_recovery_prompt(&unsatisfied, 1, AgentMode::Interactive).is_none());
        assert!(completion_recovery_prompt(&unsatisfied, 2, AgentMode::Execute).is_some());
        assert!(completion_recovery_prompt(&unsatisfied, 3, AgentMode::Execute).is_none());
        assert!(completion_recovery_prompt(&unsatisfied, 2, AgentMode::Autonomous).is_some());
        assert!(completion_recovery_prompt(&unsatisfied, 3, AgentMode::Autonomous).is_none());

        let satisfied = CompletionGate::new(false).evidence();
        assert!(satisfied.completed);
        assert!(completion_recovery_prompt(&satisfied, 0, AgentMode::Interactive).is_none());
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
        let (tools, tool_choice) = openai_tool_controls(&[]);

        assert!(tools.is_none());
        assert_eq!(tool_choice, serde_json::json!("none"));
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
