// SPDX-License-Identifier: Apache-2.0
//! Loop-level config + outcome (keystone slice 4.2).
//!
//! [`RunConfig`] carries the knobs that differ per surface but must NOT be
//! hardcoded per loop copy: the finalization policy, the gate's benchmark flag,
//! the progress-tracker window, and whether a wall budget applies. [`RunOutcome`]
//! is what `run_agent_loop` RETURNS — the terminal `finished` contract
//! (`final_text` + serialized `CompletionEvidence` + usage) is a typed return
//! value, NOT an `EventSink` event, so the sidecar's `finished` JSONL and its
//! contract-hash handshake never pollute the desktop `StreamEvent` UI stream.
//!
//! Provisional: nothing consumes these yet (the loop body lands in slice 4.6).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use codefactory_agent_core::CompletionEvidence;

use crate::events::EventSink;
use crate::journal::{PersistError, Persistence, RecoveryAttemptRow, UsageRow};
use crate::services::PermissionOutcome;
use crate::tool::ToolError;
use crate::transport::TransportError;
use crate::types::{ToolCall, Usage};

/// The suffix of `tool_calls` from `start` onward when the run is cancelled,
/// else `None`. The load is `SeqCst` — cooperative cancellation depends on it,
/// and index-0 vs index-N of the returned slice drives the cancelled-vs-skipped
/// message split. Shared by both provider loops (keystone slice 4.6b).
pub fn cancelled_tool_suffix<'a>(
    cancel: Option<&Arc<AtomicBool>>,
    tool_calls: &'a [ToolCall],
    start: usize,
) -> Option<&'a [ToolCall]> {
    cancel
        .is_some_and(|flag| flag.load(Ordering::SeqCst))
        .then(|| &tool_calls[start..])
}

/// Whether the run's shared cancel flag is set (`SeqCst`). Cooperative
/// cancellation depends on the ordering + the flag being the SAME `Arc` shared
/// with the transport and permission gateway.
pub fn is_cancelled(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

/// The stable request id for a round's usage row: `"{usage_run_id}:{iteration}"`.
/// ONE formula source ties the assistant-message row and the usage row so the
/// INSERT-OR-IGNORE idempotency (retry/resume) keys identically.
pub fn usage_request_id(usage_run_id: &str, iteration: usize) -> String {
    format!("{usage_run_id}:{iteration}")
}

/// The per-run identity a usage row needs, pre-derived by the surface so the
/// shared usage recorder carries no bin types (`ApiStyle`/`UsageSurface` stay in
/// the bin — `is_chatgpt`/`surface` arrive as a bool/String).
#[derive(Debug, Clone)]
pub struct UsageIdentity {
    pub session_id: String,
    pub endpoint_name: String,
    pub model_id: String,
    pub base_url: String,
    pub usage_run_id: String,
    pub surface: String,
    pub task_id: Option<String>,
    pub anonymous: bool,
    pub is_chatgpt: bool,
}

fn usage_identity_for_route(
    base: &UsageIdentity,
    route: Option<&crate::transport::EffectiveRoute>,
) -> UsageIdentity {
    let Some(route) = route else {
        return base.clone();
    };
    UsageIdentity {
        endpoint_name: route.endpoint_name.clone(),
        model_id: route.model_id.clone(),
        base_url: route.base_url.clone(),
        is_chatgpt: route.is_chatgpt,
        ..base.clone()
    }
}

async fn persist_route_change(
    persistence: &dyn Persistence,
    events: &dyn EventSink,
    route_change: Option<&crate::transport::RouteChange>,
) -> Result<(), LoopError> {
    let Some(change) = route_change else {
        return Ok(());
    };
    persistence
        .persist_gate_message(&change.notice, "turn_notice")
        .await?;
    events.emit(crate::types::StreamEvent::CompletionGateAction {
        kind: "turn_notice".into(),
        detail: change.notice.clone(),
    });
    Ok(())
}

/// Assemble + persist one round's usage row, and fire the cost-UI ping ONLY on a
/// newly-written row. Anonymous runs and (0,0)-token rounds are skipped BEFORE
/// assembly. Cost/provider derivation: ChatGPT → subscription, local endpoint →
/// local, a finite provider cost → provider_actual, else unknown. Moved out of
/// the bin loop (keystone slice 4.6b) so both the desktop method and the shared
/// loop share ONE implementation.
pub async fn record_usage_event_for_round(
    persistence: &dyn Persistence,
    events: &dyn EventSink,
    identity: &UsageIdentity,
    usage: &Usage,
    iteration: usize,
) {
    if identity.anonymous || (usage.prompt_tokens == 0 && usage.completion_tokens == 0) {
        return;
    }
    let local_endpoint = {
        let base = identity.base_url.to_ascii_lowercase();
        base.contains("127.0.0.1")
            || base.contains("localhost")
            || base.contains("0.0.0.0")
            || base.starts_with("http://[::1]")
    };
    let (provider, actual_cost_usd, cost_source) = if identity.is_chatgpt {
        ("chatgpt".to_string(), None, "subscription".to_string())
    } else if local_endpoint {
        (identity.endpoint_name.clone(), None, "local".to_string())
    } else if let Some(cost) = usage
        .cost
        .filter(|value| value.is_finite() && *value >= 0.0)
    {
        let provider = if identity.base_url.contains("openrouter.ai") {
            "openrouter".to_string()
        } else {
            identity.endpoint_name.clone()
        };
        (provider, Some(cost), "provider_actual".to_string())
    } else {
        (identity.endpoint_name.clone(), None, "unknown".to_string())
    };
    let row = UsageRow {
        request_id: usage_request_id(&identity.usage_run_id, iteration),
        session_id: &identity.session_id,
        task_id: identity.task_id.clone(),
        surface: &identity.surface,
        provider,
        endpoint: &identity.endpoint_name,
        model: &identity.model_id,
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
        cost_source,
    };
    match persistence.record_usage(row).await {
        Ok(true) => events.usage_recorded(&identity.session_id),
        Ok(false) => {}
        Err(error) => tracing::warn!("failed to record request usage: {error}"),
    }
}

/// The error `run_agent_loop` returns (keystone slice 4.6). Every arm's
/// `Display` is the underlying error verbatim, so a desktop adapter can map it
/// to `AppError::Other(e.to_string())` byte-for-byte, and the loop's
/// context-overflow / vision greps (which read a `TransportError`'s verbatim
/// `Display`) still work through the `Transport` arm. The loop body switches its
/// transport calls onto `complete()` in slice 4.6 sub-step 7; `run_agent_loop`
/// starts returning this in sub-step 8.
#[derive(Debug)]
pub enum LoopError {
    Transport(TransportError),
    Persist(PersistError),
    Tool(ToolError),
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoopError::Transport(e) => write!(f, "{e}"),
            LoopError::Persist(e) => write!(f, "{e}"),
            LoopError::Tool(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoopError {}

impl From<TransportError> for LoopError {
    fn from(e: TransportError) -> Self {
        LoopError::Transport(e)
    }
}

impl From<PersistError> for LoopError {
    fn from(e: PersistError) -> Self {
        LoopError::Persist(e)
    }
}

impl From<ToolError> for LoopError {
    fn from(e: ToolError) -> Self {
        LoopError::Tool(e)
    }
}

/// How the loop finalizes a turn. Desktop maps `AgentMode`; the sidecar adds a
/// `Benchmark` arm that must reproduce its 2-way completed/recovery branch
/// byte-for-byte (hardest-problem #2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizationPolicy {
    /// Chat surface: the provisional text may be displayed, but unmet evidence
    /// remains a system-owned `failed_internal` business outcome.
    ReleaseWithWarning,
    /// Autonomous/subagent: block + Error on unmet evidence, scheduler respawns.
    BlockOnIncomplete,
    /// Terminal-Bench sidecar: 2-way completed/recovery (no release-with-warning).
    Benchmark,
}

/// Current intent envelope used while evaluating each requested action.
/// Surfaces re-evaluate it for every user message and steer; it is not a fixed
/// lifetime lock for a root turn. Permissions may narrow an allowed action but
/// cannot widen explicit user constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnCapability {
    ReviewOnly,
    Implement,
    Deliver,
}

/// Per-run configuration that the surface supplies; keeps divergent constants
/// (gate benchmark flag, tracker window, recovery limit, wall budget) explicit
/// instead of forked per copy. The usage-attribution identity + flags live here
/// too so the loop names no bin enum (`ApiStyle`/`UsageSurface` are pre-derived
/// to `is_chatgpt`/`surface`).
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub finalization: FinalizationPolicy,
    pub turn_capability: TurnCapability,
    /// `CompletionGate` benchmark mode: false (desktop) / true (sidecar).
    pub gate_benchmark: bool,
    /// `ProgressTracker` window: 8 (desktop) / 4 (sidecar).
    pub progress_window: usize,
    /// Max recovery attempts before the finalization policy applies.
    pub recovery_limit: u32,
    /// Iteration ceiling for this run.
    pub max_iterations: usize,
    /// Whether the autonomous round budget constrains tools this run
    /// (Interactive = false).
    pub wall_budget_applies: bool,
    /// Whether to compress history + emergency-recompress on overflow before
    /// each send (OpenAI/ChatGPT=true; Anthropic=false — it never elides).
    pub context_compression: bool,
    /// Whether to reactively back off + retry on a transient provider overload
    /// (Anthropic=true; OpenAI=false).
    pub overload_backoff: bool,
    /// Two bounded delays after the first and second replay-safe overload.
    /// A third overload always returns control to durable Provider recovery;
    /// surfaces may inject zero delays in deterministic tests.
    pub overload_retry_delays: [std::time::Duration; 2],
    /// Deny further READ-ONLY calls once the read-only allowance is spent
    /// (keystone slice 4.8c b5). The eval sidecar enables it; the desktop does
    /// not, so its behaviour is unchanged.
    pub inspection_budget: bool,
    /// Push the model's REJECTED draft into history before the recovery prompt
    /// (keystone slice 4.8c b12). The sidecar does; the desktop does not — its
    /// UI already collapses the rejected candidate.
    pub replay_rejected_draft: bool,
    /// Periodic coalesced activity refresh while one backend tool future is
    /// still pending. `None` disables heartbeats for non-interactive surfaces.
    pub tool_heartbeat_interval: Option<std::time::Duration>,
    /// When a pending tool becomes a user-visible long wait.
    pub long_tool_wait_threshold: std::time::Duration,
    /// One-shot root-turn convergence signal after this many received tool
    /// calls. `None` disables the signal.
    pub tool_amplification_threshold: Option<usize>,
    // Usage-attribution identity + working dir (all constant for the run).
    pub session_id: String,
    pub endpoint_name: String,
    pub model_id: String,
    pub base_url: String,
    pub usage_run_id: String,
    pub surface: String,
    pub task_id: Option<String>,
    pub anonymous: bool,
    pub is_chatgpt: bool,
    pub cwd: std::path::PathBuf,
}

/// The per-turn data the surface pre-builds before the loop: the assembled
/// history+system, the tool schema, the resolved instructions, and the shared
/// cancel flag. `run_agent_loop` never sees `storage::Message` or attachments.
pub struct LoopInputs {
    pub messages: Vec<crate::types::ChatMessage>,
    pub system_prompt: String,
    pub tool_defs: Vec<crate::types::ToolDefinition>,
    pub completion_instruction: String,
    pub fact_check_instruction: String,
    pub audit_session_id: String,
    pub root_turn_id: Option<String>,
    pub mutation_permit: Option<crate::tool::MutationPermit>,
    pub knowledge_library_ids: Option<Vec<String>>,
    pub cancel: Option<Arc<AtomicBool>>,
}

/// The eight capability seams + fact-checker the loop drives, as trait objects.
/// Every `AppHandle`-owning concrete impl is built by the bin adapter and erased
/// here, so `run_agent_loop` links no `tauri` (#166).
pub struct LoopServices {
    pub transport: Arc<dyn crate::transport::ModelTransport>,
    pub tools: Arc<dyn crate::tool::ToolBackend>,
    pub persistence: Arc<dyn Persistence>,
    pub events: Arc<dyn EventSink>,
    pub budget: Arc<dyn crate::journal::Budget>,
    /// How the prompt is kept inside the context budget. Desktop supplies
    /// `DefaultCompressor` (token-based elision, today's behaviour); the eval
    /// sidecar supplies its char-budget digest so its scores don't move.
    pub compactor: Arc<dyn crate::services::ContextCompactor>,
    /// Revalidated immediately before every compactor invocation. Context
    /// recovery supplies a durable owner/revision/cursor fence; ordinary runs
    /// and the headless evaluator explicitly allow their own compaction.
    pub context_compaction_gate: Arc<dyn crate::services::ContextCompactionGate>,
    pub permission: Arc<dyn crate::services::PermissionGateway>,
    pub hooks: Arc<dyn crate::services::LifecycleHooks>,
    pub context_policy: Arc<dyn crate::services::ContextPolicy>,
    pub fact_checker: Arc<dyn crate::services::FactChecker>,
    /// Mid-run user input, drained at each round boundary. Surfaces with no
    /// interactive user supply [`crate::services::NoSteering`].
    pub steer: Arc<dyn crate::services::SteerInbox>,
}

#[allow(clippy::too_many_arguments)]
async fn publish_turn_activity(
    persistence: &dyn Persistence,
    events: &dyn EventSink,
    root_turn_id: Option<&str>,
    phase: &str,
    status: &str,
    activity_kind: &str,
    activity_label: &str,
    waiting_reason: Option<&str>,
    terminal_reason: Option<&str>,
) -> Result<(), LoopError> {
    let Some(root_turn_id) = root_turn_id else {
        return Ok(());
    };
    let update = crate::journal::TurnActivityUpdate {
        root_turn_id: root_turn_id.to_string(),
        phase: phase.to_string(),
        status: status.to_string(),
        recent_activity_kind: activity_kind.to_string(),
        recent_activity_label: activity_label.to_string(),
        waiting_reason: waiting_reason.map(str::to_string),
        terminal_reason: terminal_reason.map(str::to_string),
    };
    let revision = persistence.update_turn_activity(&update).await?;
    let updated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    events.emit(crate::types::StreamEvent::TurnActivityUpdated {
        root_turn_id: update.root_turn_id,
        revision,
        phase: update.phase,
        status: update.status,
        recent_activity_kind: update.recent_activity_kind,
        recent_activity_label: update.recent_activity_label,
        waiting_reason: update.waiting_reason,
        updated_at,
        terminal_reason: update.terminal_reason,
        objective_id: None,
        objective_status: None,
        recovery_owner: None,
        next_observation_at: None,
        last_progress_at: None,
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn settle_context_recovery(
    persistence: &dyn Persistence,
    events: &dyn EventSink,
    root_turn_id: Option<&str>,
    outcome: crate::context::ContextRecoveryOutcome,
    attempt_index: i64,
    output_started: bool,
    side_effect_started: bool,
    gate: &codefactory_agent_core::CompletionGate,
    tokens: (u64, u64),
    final_text: &str,
) -> Result<RunOutcome, LoopError> {
    if let Some(root_turn_id) = root_turn_id {
        persistence
            .record_recovery_attempt(&RecoveryAttemptRow {
                root_turn_id: root_turn_id.to_string(),
                domain: "context".into(),
                attempt_index,
                failure_code: outcome.failure_code().into(),
                failure_class: "context_capacity".into(),
                output_started,
                side_effect_started,
                terminal_decision: "waiting_system".into(),
            })
            .await?;
    }
    publish_turn_activity(
        persistence,
        events,
        root_turn_id,
        "recovering",
        "waiting",
        "context_recovery_waiting",
        "系统正在按当前模型窗口重新整理上下文",
        Some("已保留当前目标与进度，等待安全续接"),
        Some(outcome.terminal_reason()),
    )
    .await?;
    let notice = "上下文窗口仍不足，当前目标与进度已保留并转入系统恢复队列。";
    if persistence
        .persist_gate_message_once(outcome.failure_code(), notice, "turn_notice")
        .await?
    {
        events.emit(crate::types::StreamEvent::CompletionGateAction {
            kind: "turn_notice".into(),
            detail: notice.into(),
        });
    }
    // `Done` closes this transport segment only. The desktop's later
    // `TurnSettled` projection carries the durable WaitingSystem Objective.
    events.emit(crate::types::StreamEvent::Done {
        input_tokens: 0,
        output_tokens: 0,
    });
    Ok(run_outcome_for_terminal(
        gate,
        StopReason::PlatformIncident,
        tokens,
        final_text,
    ))
}

fn sanitized_tool_activity(name: &str) -> (&'static str, &'static str, &'static str) {
    match name {
        "read_file" | "glob" | "grep" | "kb_search" | "kb_get_chunk" => {
            ("working", "正在检查相关信息", "信息检查")
        }
        "write_file" | "edit_file" => ("working", "正在修改工作区", "工作区修改"),
        "bash" => ("working", "正在执行命令", "命令"),
        "deliver_changes" => ("delivering", "正在执行交付", "交付任务"),
        "delegate_tasks" | "dispatch_parallel_tasks" => ("working", "正在执行子任务", "子任务"),
        "browser_session" => ("working", "正在检查浏览器页面", "浏览器检查"),
        _ => ("working", "正在执行工具", "工具"),
    }
}

fn structured_delivery_terminal_summary(metadata: &serde_json::Value) -> Option<String> {
    if metadata.get("status").and_then(serde_json::Value::as_str) != Some("blocked")
        || metadata
            .get("recoverable")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
    {
        return None;
    }
    let field = |name: &str, fallback: &'static str| {
        metadata
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback)
    };
    let next_action = field("next_action", "需要先核对阻断原因后再安全续接。");
    Some(format!(
        "交付未达到请求边界 `{}`。实际到达 `{}`，停止在 `{}`（`{}`）。下一步：{}",
        field("requested_ceiling", "unknown"),
        field("reached_state", "unknown"),
        field("stage", "unknown"),
        field("code", "delivery_blocked"),
        next_action.trim(),
    ))
}

fn approximate_wait_label(elapsed: std::time::Duration) -> String {
    if elapsed < std::time::Duration::from_secs(60) {
        format!("{} 秒", elapsed.as_secs().max(1))
    } else {
        let minutes = elapsed.as_secs() / 60;
        format!("{minutes} 分钟")
    }
}

/// Finish a tool batch that was cancelled mid-flight: persist each remaining
/// call as cancelled and emit the terminal `Done{0,0}` exactly once. The crate
/// twin of the bin's inherent method (keystone slice 4.6b).
pub async fn finish_cancelled_tool_batch(
    persistence: &dyn Persistence,
    events: &dyn EventSink,
    remaining: &[ToolCall],
) -> Result<(), LoopError> {
    let contents = persistence.persist_cancelled_tool_batch(remaining).await?;
    for (index, (tc, content)) in remaining.iter().zip(contents).enumerate() {
        if index > 0 {
            let args = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
            events.emit(crate::types::StreamEvent::ToolCallStart {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                args,
            });
        }
        events.emit(crate::types::StreamEvent::ToolResult {
            tool_call_id: tc.id.clone(),
            content: content.clone(),
            is_error: true,
            status: "cancelled".into(),
            metadata: None,
        });
    }
    events.emit(crate::types::StreamEvent::Done {
        input_tokens: 0,
        output_tokens: 0,
    });
    Ok(())
}

/// The loop's terminal result, returned (not emitted). The sidecar writes its
/// `finished` JSONL from this plus the shared contract hash; the desktop
/// ignores it (it already emitted `Done` via `TauriEventSink`).
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub final_text: String,
    pub completion_evidence: CompletionEvidence,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Why the run ended (keystone slice 4.8c). The desktop ignores it — it has
    /// already emitted the terminal `StreamEvent`; the eval sidecar needs it to
    /// pick its `finished` text (e.g. a budget-exhaustion message vs the last
    /// model reply).
    pub stop_reason: StopReason,
}

/// Why `run_agent_loop` returned. Purely informational for the desktop; the
/// sidecar branches its terminal `finished` payload on it (keystone 4.8c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// A tool-call-free reply passed the completion gate.
    Finished,
    /// The current objective is not complete, but a system-owned continuation
    /// can still resume from the persisted evidence.
    Incomplete,
    /// Bounded technical recovery was exhausted. This is owned by the system,
    /// never a request for the user to say "continue".
    FailedInternal,
    /// The execution platform itself is unavailable and must remediate the run.
    PlatformIncident,
    /// The gate blocked the run (`BlockOnIncomplete` with unmet evidence).
    Blocked,
    /// The shared cancel flag tripped.
    Cancelled,
    /// The segment/iteration ceiling was reached without a terminal reply.
    IterationCeiling,
    /// `Budget::may_continue` refused another round (wall-clock reserve).
    BudgetExhausted,
}

/// The single shared agent loop (keystone slice 4.6b). Drives one chat turn to a
/// terminal entirely through the [`LoopServices`] trait objects — no `Settings`,
/// no DB, no `AppHandle` — so the desktop app and the sidecar run the SAME body.
/// The desktop adapter builds the inputs/config/services and discards the
/// returned [`RunOutcome`] (it already emitted `Done` via the sink).
///
/// The whole call is one `tracing` span (`agent_loop`) carrying the
/// usage-attribution identity, so every `tracing::info!`/`warn!` emitted
/// anywhere in this function — or in anything it calls synchronously, like
/// `LoopServices` implementations — is attributable to one turn without
/// manually stitching session/task ids across log lines.
#[tracing::instrument(
    name = "agent_loop",
    skip(inputs, config, svc),
    fields(
        session_id = %config.session_id,
        task_id = config.task_id.as_deref().unwrap_or(""),
        surface = %config.surface,
        endpoint = %config.endpoint_name,
        model = %config.model_id,
    )
)]
pub async fn run_agent_loop(
    inputs: LoopInputs,
    config: RunConfig,
    svc: LoopServices,
) -> Result<RunOutcome, LoopError> {
    let LoopInputs {
        mut messages,
        system_prompt,
        tool_defs,
        completion_instruction,
        fact_check_instruction,
        audit_session_id,
        root_turn_id,
        mutation_permit,
        knowledge_library_ids,
        cancel,
    } = inputs;
    let RunConfig {
        finalization,
        turn_capability: initial_turn_capability,
        recovery_limit,
        max_iterations,
        wall_budget_applies: wall_budget,
        context_compression,
        overload_backoff,
        overload_retry_delays,
        inspection_budget,
        replay_rejected_draft,
        tool_heartbeat_interval,
        long_tool_wait_threshold,
        tool_amplification_threshold,
        session_id,
        endpoint_name,
        model_id,
        base_url,
        usage_run_id,
        surface,
        task_id,
        anonymous,
        is_chatgpt,
        mut cwd,
        gate_benchmark,
        progress_window,
    } = config;
    let mut turn_capability = initial_turn_capability;
    let LoopServices {
        transport,
        tools: tool_backend,
        persistence,
        events,
        budget,
        compactor,
        context_compaction_gate,
        permission,
        hooks,
        context_policy,
        fact_checker,
        steer,
    } = svc;
    let usage_identity = UsageIdentity {
        session_id: session_id.clone(),
        endpoint_name,
        model_id,
        base_url,
        usage_run_id: usage_run_id.clone(),
        surface,
        task_id: task_id.clone(),
        anonymous,
        is_chatgpt,
    };
    let system_prompt = system_prompt.as_str();
    let tool_defs = tool_defs.as_slice();
    // Images are user input, not optional context. Never silently replace them
    // with placeholders: keep the original history intact so the user can
    // switch to a vision-capable model and retry.
    if !context_policy.supports_vision().await {
        let image_count = crate::protocol::image_part_count(&messages);
        if image_count > 0 {
            return Err(TransportError::Fatal(format!(
                "IMAGE_INPUT_UNSUPPORTED: 当前模型不支持图片输入，本次请求未发送，{image_count} 张图片均已保留。请切换到支持图片的模型后重试"
            ))
            .into());
        }
    }

    // Did we emit a terminal Done/Error this run? Used to guarantee the
    // stream always closes after completion, cancellation, or a visible
    // recoverable stop.
    let mut emitted_terminal = false;
    let mut terminal_stop_reason = None;
    let mut completion_gate = codefactory_agent_core::CompletionGate::new_for_instruction(
        gate_benchmark,
        &completion_instruction,
    );
    let mut completion_sequence = 0_u64;
    let mut last_completion_nudge_sequence = None;
    let mut successful_local_verifications = BTreeSet::new();
    let mut progress_tracker = codefactory_agent_core::ProgressTracker::new(progress_window as u32);
    let mut finalization_pending = false;
    let mut blocker_summary_pending = false;
    let mut blocker_terminal_reason: Option<String> = None;
    let mut completion_summary_retry_used = false;
    let mut completion_recovery_attempts = 0_u32;
    // The last delivery failure we saw, as `{code}|{stage}|{reached}|{sha}`.
    // Repair is judged by whether this CHANGES, not by how many tries have
    // happened — a count cannot tell "fixing it" from "spinning".
    let mut delivery_failure_signature: Option<String> = None;
    let mut structural_denial_seen = false;
    let mut fact_check_used = false;
    let mut require_tool_next = false;
    let mut model_round_index = 0_usize;
    let mut stalled_chat_segments = 0_u32;
    let mut root_tool_call_count = match root_turn_id.as_deref() {
        Some(root_turn_id) => persistence.root_turn_tool_call_count(root_turn_id).await?,
        None => 0,
    };
    let mut amplification_signal_emitted = false;
    let mut amplification_prompt_pending = false;
    // Run-level totals + the last model reply, carried into `RunOutcome`
    // (keystone slice 4.8c). The desktop discards them; the sidecar builds its
    // terminal `finished` payload from them.
    let mut total_input_tokens = 0_u64;
    let mut total_output_tokens = 0_u64;
    let mut last_final_text = String::new();
    // Sticky across all provider rounds in this run. `last_final_text` only
    // tracks a terminal prose candidate; earlier assistant text paired with a
    // tool call is still output and makes a blind Context replay unsafe.
    let mut output_started_in_run = false;
    publish_turn_activity(
        persistence.as_ref(),
        events.as_ref(),
        root_turn_id.as_deref(),
        "working",
        "active",
        "turn_running",
        "正在执行任务",
        None,
        None,
    )
    .await?;
    loop {
        let segment_start_evidence = completion_gate.evidence();
        for segment_iteration in 0..max_iterations {
            // `max_iterations` is a segment checkpoint cadence on chat
            // surfaces, not a task-level ceiling. Keep a global round index so
            // auto-continuation never reuses a usage id.
            let iteration = model_round_index;
            model_round_index = model_round_index.saturating_add(1);
            // Cooperative cancellation: if the user hit "stop" for this chat
            // turn, end the stream cleanly between rounds. Checked here (not
            // mid tool-call) so in-flight work isn't hard-killed. No-op unless
            // a cancel flag was attached (chat only) and has actually tripped.
            if is_cancelled(cancel.as_ref()) {
                tracing::info!("chat turn cancelled by user (session {session_id})");
                events.emit(crate::types::StreamEvent::Done {
                    input_tokens: 0,
                    output_tokens: 0,
                });
                emitted_terminal = true;
                terminal_stop_reason = Some(StopReason::Cancelled);
                break;
            }
            // ── Mid-run steering ─────────────────────────────────────────────
            // Same boundary, same discipline as cancellation above: the user's
            // correction reaches the model before its next request, and no
            // in-flight tool call is ever interrupted to deliver it.
            for steer_text in steer.drain().await {
                if let Some(next_capability) = steer.capability_override(&steer_text) {
                    if next_capability != turn_capability {
                        structural_denial_seen = false;
                    }
                    turn_capability = next_capability;
                    let label = match next_capability {
                        TurnCapability::ReviewOnly => "已更新动作意图：仅执行分析与读取",
                        TurnCapability::Implement => "已更新动作意图：允许本地实施",
                        TurnCapability::Deliver => "已更新动作意图：允许受控交付",
                    };
                    publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        "working",
                        "active",
                        "intent_changed",
                        label,
                        None,
                        None,
                    )
                    .await?;
                }
                let message_id = persistence
                    .persist_message(
                        "user",
                        &steer_text,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await?;
                messages.push(crate::types::ChatMessage {
                    role: "user".into(),
                    content: crate::types::MessageContent::Text(steer_text.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
                // The objective just changed, so the completion gate is now
                // judging against a goal this turn has not attempted yet.
                // Without this reset a turn that already spent 2 of its 3
                // recoveries would give the newly-stated goal almost no budget
                // and release it with an "unverified" warning it never earned.
                //
                // The gate itself is NOT rebuilt from the new instruction: its
                // requirements were derived once, but the evidence it has
                // accumulated (mutations made, verifications run) describes
                // work that actually happened and must not be discarded. Its
                // core rule — a successful verification later than the last
                // mutation — holds under any objective.
                completion_recovery_attempts =
                    crate::policy::completion_recovery_attempts_after_steer(
                        completion_recovery_attempts,
                    );
                events.emit(crate::types::StreamEvent::SteerApplied {
                    message_id,
                    content: steer_text,
                });
            }
            // Resolve the exact tool schema that this round will send before
            // estimating the prompt. Tool definitions are part of the provider
            // input budget, so calculating the budget first can cause an avoidable
            // over-context request followed by a compression retry.
            let active_tool_defs = crate::policy::active_tool_definitions_for_capability(
                tool_defs,
                finalization_pending,
                turn_capability,
            );
            // ── Context-window management ────────────────────────────────────
            // Estimate prompt tokens before sending. If we're over 75% of the
            // model's window, elide oversized tool results from the older
            // half. Notify the UI so the user knows what happened.
            let estimated = crate::context::estimate_prompt_tokens_with_tools(
                &messages,
                system_prompt,
                &active_tool_defs,
            );
            let (context_limit, max_context_limit) = context_policy.context_window(estimated).await;
            // Compression is OpenAI/ChatGPT-only (slice 4.7): the Anthropic path
            // never elides history, so with `context_compression=false` the
            // history passes through untouched (no mem::take/repair/event) — we
            // still resolve the window above for the ContextUsage denominator.
            if context_compression {
                context_compaction_gate
                    .authorize_compaction()
                    .await
                    .map_err(|reason| {
                        TransportError::Fatal(format!("CONTEXT_RECOVERY_FENCED: {reason}"))
                    })?;
                // Delegated to the ContextCompactor seam (slice 4.8c) so each
                // surface keeps its own budget discipline: desktop = token-based
                // elision (DefaultCompressor, byte-identical to before),
                // sidecar = its destructive char-budget digest.
                let compaction = crate::services::compact_with_measurement(
                    compactor.as_ref(),
                    std::mem::take(&mut messages),
                    system_prompt,
                    context_limit,
                    &active_tool_defs,
                );
                let did_shrink = compaction.shrank();
                let tokens_freed = compaction.tokens_freed();
                let elided_count = compaction.elided_count;
                messages = compaction.messages;
                if did_shrink {
                    events.emit(crate::types::StreamEvent::ContextCompressed {
                        elided_count,
                        tokens_freed,
                    });
                }
            }

            let summary_only_round = finalization_pending;
            let blocker_summary_round = summary_only_round && blocker_summary_pending;
            let active_tool_defs = crate::policy::active_tool_definitions_for_capability(
                tool_defs,
                finalization_pending,
                turn_capability,
            );
            let required_tool_response = require_tool_next && !finalization_pending;
            // Resolve reasoning effort ONCE per round via ContextPolicy (slice
            // 4.6): it re-reads db+settings each round (freshness) and returns ""
            // for non-ChatGPT styles, so the transport reads no DB. Held in
            // `round_options` so the two reactive retries below reuse this round's
            // value.
            let round_options = crate::transport::RoundOptions {
                require_tool: required_tool_response,
                tool_outcomes_so_far: completion_sequence as usize,
                reasoning_effort: context_policy.round_reasoning_effort().await,
            };
            let call_result = transport
                .complete(&messages, &active_tool_defs, &round_options)
                .await;
            let crate::transport::ModelResponse {
                text,
                tool_calls,
                usage,
                reasoning,
                effective_route,
                route_change,
            } = match call_result {
                Ok(ok) => ok,
                // Provider says the prompt is over the window: the resolved
                // window metadata or the token estimate was wrong. Emergency-
                // compress against a reduced budget and retry ONCE — a killed
                // turn on a replayable history dies identically on every
                //「继续」(2026-07-21: three context-window deaths in one day).
                Err(e)
                    if context_compression
                        && crate::context::is_context_overflow(&e.to_string()) =>
                {
                    let emergency_limit = (context_limit / 5).max(1) * 4;
                    context_compaction_gate
                        .authorize_compaction()
                        .await
                        .map_err(|reason| {
                            TransportError::Fatal(format!("CONTEXT_RECOVERY_FENCED: {reason}"))
                        })?;
                    let compression = crate::services::compact_with_measurement(
                        compactor.as_ref(),
                        std::mem::take(&mut messages),
                        system_prompt,
                        emergency_limit,
                        &active_tool_defs,
                    );
                    let did_shrink = compression.shrank();
                    let tokens_freed = compression.tokens_freed();
                    let elided_count = compression.elided_count;
                    messages = compression.messages;
                    let evidence = completion_gate.evidence();
                    let output_started = output_started_in_run;
                    let side_effect_started = evidence.last_mutation_sequence.is_some();
                    if !did_shrink {
                        return settle_context_recovery(
                            persistence.as_ref(),
                            events.as_ref(),
                            root_turn_id.as_deref(),
                            crate::context::ContextRecoveryOutcome::CompactionExhausted,
                            1,
                            output_started,
                            side_effect_started,
                            &completion_gate,
                            (total_input_tokens, total_output_tokens),
                            &last_final_text,
                        )
                        .await;
                    }
                    if let Some(root_turn_id) = root_turn_id.as_deref() {
                        persistence
                            .record_recovery_attempt(&RecoveryAttemptRow {
                                root_turn_id: root_turn_id.to_string(),
                                domain: "context".into(),
                                attempt_index: 1,
                                failure_code: "CONTEXT_OVERFLOW".into(),
                                failure_class: "context_capacity".into(),
                                output_started,
                                side_effect_started,
                                terminal_decision: "continue".into(),
                            })
                            .await?;
                    }
                    events.emit(crate::types::StreamEvent::ContextCompressed {
                        elided_count,
                        tokens_freed,
                    });
                    let notice = format!(
                        "上下文超出模型窗口，系统已进一步压缩 {} 条历史（约释放 {} tokens）并继续处理。",
                        elided_count, tokens_freed
                    );
                    persistence
                        .persist_gate_message(&notice, "turn_notice")
                        .await?;
                    events.emit(crate::types::StreamEvent::CompletionGateAction {
                        kind: "turn_notice".into(),
                        detail: notice.clone(),
                    });
                    match transport
                        .complete(&messages, &active_tool_defs, &round_options)
                        .await
                    {
                        Ok(response) => response,
                        Err(next) if crate::context::is_context_overflow(&next.to_string()) => {
                            return settle_context_recovery(
                                persistence.as_ref(),
                                events.as_ref(),
                                root_turn_id.as_deref(),
                                crate::context::ContextRecoveryOutcome::OverflowAfterCompaction,
                                2,
                                output_started,
                                side_effect_started,
                                &completion_gate,
                                (total_input_tokens, total_output_tokens),
                                &last_final_text,
                            )
                            .await;
                        }
                        Err(next) => return Err(next.into()),
                    }
                }
                Err(e) if crate::context::is_context_overflow(&e.to_string()) => {
                    let evidence = completion_gate.evidence();
                    return settle_context_recovery(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        crate::context::ContextRecoveryOutcome::CompressionUnavailable,
                        1,
                        output_started_in_run,
                        evidence.last_mutation_sequence.is_some(),
                        &completion_gate,
                        (total_input_tokens, total_output_tokens),
                        &last_final_text,
                    )
                    .await;
                }
                // Transient provider saturation (529/overloaded, 503, rate
                // limit): keep the authorized root turn alive with bounded
                // backoff. Technical retry-budget exhaustion is not a user
                // decision; only explicit cancellation exits this remediation.
                Err(e)
                    if overload_backoff
                        && crate::context::is_provider_overloaded(&e.to_string())
                        && completion_gate.evidence().outcome_count == 0 =>
                {
                    let notice = "模型服务过载，正在自动退避恢复；无需回复“继续”。".to_string();
                    persistence
                        .persist_gate_message_once("自动退避重试", &notice, "turn_notice")
                        .await?;
                    let mut last_err = e;
                    let mut completed_failures = 1i64;
                    loop {
                        if is_cancelled(cancel.as_ref()) {
                            return Err(last_err.into());
                        }
                        if let Some(root_turn_id) = root_turn_id.as_deref() {
                            persistence
                                .record_recovery_attempt(&RecoveryAttemptRow {
                                    root_turn_id: root_turn_id.to_string(),
                                    domain: "provider".into(),
                                    attempt_index: completed_failures,
                                    failure_code: "PROVIDER_OVERLOADED".into(),
                                    failure_class: "transient_provider".into(),
                                    output_started: false,
                                    side_effect_started: false,
                                    terminal_decision: if completed_failures >= 3 {
                                        "waiting_system".into()
                                    } else {
                                        "continue".into()
                                    },
                                })
                                .await?;
                        }
                        if completed_failures >= 3 {
                            return Err(TransportError::Retryable(format!(
                                "PROVIDER_OVERLOAD_BUDGET_EXHAUSTED: {last_err}"
                            ))
                            .into());
                        }
                        let delay = overload_retry_delays[(completed_failures - 1) as usize];
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        match transport
                            .complete(&messages, &active_tool_defs, &round_options)
                            .await
                        {
                            Ok(ok) => {
                                break ok;
                            }
                            Err(next)
                                if crate::context::is_provider_overloaded(&next.to_string()) =>
                            {
                                last_err = next;
                                completed_failures += 1;
                            }
                            Err(next) => return Err(next.into()),
                        }
                    }
                }
                Err(e) if crate::protocol::is_vision_rejection(&e.to_string()) => {
                    return Err(TransportError::Fatal(format!(
                        "IMAGE_INPUT_UNSUPPORTED: 当前模型拒绝了图片输入，本次请求已停止且不会移除图片。请切换到支持图片的模型后重试。原始错误：{e}"
                    ))
                    .into());
                }
                Err(e) => return Err(e.into()),
            };
            output_started_in_run |= !text.is_empty();
            finalization_pending = false;
            blocker_summary_pending = false;
            require_tool_next = false;
            persist_route_change(persistence.as_ref(), events.as_ref(), route_change.as_ref())
                .await?;

            // The provider request has completed. Persist already-consumed
            // Usage before honoring a cancellation that arrived in flight.
            let usage_request_id = usage_request_id(&usage_run_id, iteration);
            if let Some(round_usage) = usage.as_ref() {
                // Run-level totals for RunOutcome (slice 4.8c) — accumulated
                // alongside the per-round persistence, so a cancelled or
                // ceiling-terminated run still reports what it consumed.
                total_input_tokens =
                    total_input_tokens.saturating_add(round_usage.prompt_tokens as u64);
                total_output_tokens =
                    total_output_tokens.saturating_add(round_usage.completion_tokens as u64);
                record_usage_event_for_round(
                    persistence.as_ref(),
                    events.as_ref(),
                    &usage_identity_for_route(&usage_identity, effective_route.as_ref()),
                    round_usage,
                    iteration,
                )
                .await;
            }
            if is_cancelled(cancel.as_ref()) {
                tracing::info!("chat turn cancelled by user (session {session_id})");
                events.emit(crate::types::StreamEvent::Done {
                    input_tokens: 0,
                    output_tokens: 0,
                });
                emitted_terminal = true;
                break;
            }

            // Emit real (provider-reported) context-usage right after each
            // round-trip so the UI bar tracks actual usage, not just our
            // estimate. The estimate is only used to *trigger* compression.
            if let Some(u) = &usage {
                events.emit(crate::types::StreamEvent::ContextUsage {
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
                    persistence
                        .persist_message(
                            "assistant",
                            &text,
                            usage.as_ref().map(|u| u.prompt_tokens as i64),
                            usage.as_ref().map(|u| u.completion_tokens as i64),
                            if tool_calls.is_empty() {
                                None
                            } else {
                                Some(&tool_calls)
                            },
                            reasoning.as_deref(),
                            effective_route
                                .as_ref()
                                .map(|route| route.endpoint_name.as_str()),
                            effective_route
                                .as_ref()
                                .map(|route| route.model_id.as_str()),
                            Some(&usage_request_id),
                        )
                        .await?
                } else {
                    None
                };
            if let Some(message_id) = assistant_message_id.as_deref() {
                for tc in &tool_calls {
                    persistence.record_tool_call_started(message_id, tc).await?;
                }
            }

            if summary_only_round && !tool_calls.is_empty() {
                let content = "最终总结阶段不会执行新的工具调用；请直接使用已有验证结果完成总结。";
                let mut cancelled_results = Vec::with_capacity(tool_calls.len());
                for tc in &tool_calls {
                    persistence
                        .record_tool_call_outcome(tc, "denied", None, Some(content), 0)
                        .await?;
                    events.emit(crate::types::StreamEvent::ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: content.into(),
                        is_error: true,
                        status: "denied".into(),
                        metadata: None,
                    });
                    cancelled_results.push(crate::types::ChatMessage {
                        role: "tool".into(),
                        content: crate::types::MessageContent::Text(content.into()),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.function.name.clone()),
                        reasoning_content: None,
                    });
                }
                if completion_summary_retry_used {
                    let notice = "最终总结连续返回了未执行的工具请求，已停止以避免重复验证；现有验证结果保持有效。".to_string();
                    persistence
                        .persist_gate_message(&notice, "turn_notice")
                        .await?;
                    events.emit(crate::types::StreamEvent::Error {
                        message: notice.clone(),
                    });
                    return Ok(run_outcome_for_terminal(
                        &completion_gate,
                        StopReason::Blocked,
                        (total_input_tokens, total_output_tokens),
                        &last_final_text,
                    ));
                }
                completion_summary_retry_used = true;
                messages.push(crate::types::ChatMessage {
                    role: "assistant".into(),
                    content: crate::types::MessageContent::Text(text),
                    tool_calls: Some(tool_calls),
                    tool_call_id: None,
                    name: None,
                    reasoning_content: reasoning,
                });
                messages.extend(cancelled_results);
                messages.push(crate::types::ChatMessage {
                    role: "user".into(),
                    content: crate::types::MessageContent::Text(
                        codefactory_agent_core::build_completion_summary_prompt().into(),
                    ),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
                finalization_pending = true;
                blocker_summary_pending = blocker_summary_round;
                continue;
            }

            if tool_calls.is_empty() {
                if blocker_summary_round {
                    last_final_text = text.clone();
                    publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        "finalizing",
                        "blocked",
                        "blocked",
                        "任务已在明确边界停止",
                        None,
                        blocker_terminal_reason.as_deref().or(Some("tool_blocked")),
                    )
                    .await?;
                    let (done_in, done_out) = usage
                        .as_ref()
                        .map(|u| (u.prompt_tokens, u.completion_tokens))
                        .unwrap_or((0, 0));
                    events.emit(crate::types::StreamEvent::Done {
                        input_tokens: done_in,
                        output_tokens: done_out,
                    });
                    emitted_terminal = true;
                    terminal_stop_reason = Some(StopReason::Blocked);
                    break;
                }
                if structural_denial_seen {
                    last_final_text = text.clone();
                    publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        "finalizing",
                        "blocked",
                        "blocked",
                        "任务因结构边界停止",
                        None,
                        Some("capability_denied"),
                    )
                    .await?;
                    let (done_in, done_out) = usage
                        .as_ref()
                        .map(|u| (u.prompt_tokens, u.completion_tokens))
                        .unwrap_or((0, 0));
                    events.emit(crate::types::StreamEvent::Done {
                        input_tokens: done_in,
                        output_tokens: done_out,
                    });
                    emitted_terminal = true;
                    terminal_stop_reason = Some(StopReason::Blocked);
                    break;
                }
                // Systemic fact-check: a tool-call-free reply asserting a
                // machine-verifiable obstacle (delivery blocked / command
                // missing / waiting on a checkable condition) gets ONE live
                // probe-backed correction — facts over stale memory.
                if !summary_only_round && !fact_check_used {
                    if let Some(correction) =
                        fact_checker.fact_check(&text, &fact_check_instruction)
                    {
                        fact_check_used = true;
                        persistence
                            .persist_gate_message(&correction, "turn_notice")
                            .await?;
                        events.emit(crate::types::StreamEvent::CompletionGateAction {
                            kind: "turn_notice".into(),
                            detail: correction.clone(),
                        });
                        messages.push(crate::types::ChatMessage {
                            role: "user".into(),
                            content: crate::types::MessageContent::Text(correction),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                            reasoning_content: None,
                        });
                        continue;
                    }
                }
                let evidence = completion_gate.evidence();
                match crate::policy::completion_finalization(
                    &evidence,
                    completion_recovery_attempts,
                    finalization,
                    recovery_limit,
                ) {
                    crate::policy::CompletionFinalization::Recover(prompt) => {
                        // This round ends here and the loop goes around again.
                        // No tool_request went out, so a surface that must put
                        // usage on the wire every round emits it now (b14) —
                        // the tool-batch path calls this at its own end.
                        events.round_ended().await;
                        completion_recovery_attempts += 1;
                        require_tool_next = true;
                        // Make the rejection visible instead of silently looping:
                        // collapse the rejected candidate in the UI, persist the
                        // injected instruction so rebuilt history stays faithful.
                        persistence
                            .mark_rejected_candidate(assistant_message_id.as_deref())
                            .await?;
                        persistence
                            .persist_gate_message(&prompt, "gate_recovery")
                            .await?;
                        events.emit(crate::types::StreamEvent::CompletionGateAction {
                            kind: "recovery".into(),
                            detail: evidence.blockers.join("; "),
                        });
                        // b12: some surfaces let the model see its own rejected
                        // draft before the correction, so the next round has the
                        // context it is being asked to fix. Desktop keeps the
                        // draft out of history (the UI already collapsed it).
                        if replay_rejected_draft && !text.is_empty() {
                            messages.push(crate::types::ChatMessage {
                                role: "assistant".into(),
                                content: crate::types::MessageContent::Text(text.clone()),
                                tool_calls: None,
                                tool_call_id: None,
                                name: None,
                                reasoning_content: None,
                            });
                        }
                        messages.push(crate::types::ChatMessage {
                            role: "user".into(),
                            content: crate::types::MessageContent::Text(prompt),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                            reasoning_content: None,
                        });
                        continue;
                    }
                    crate::policy::CompletionFinalization::ReleaseWithWarning(warning) => {
                        // The text may stand as a provisional transport payload,
                        // but exhausted technical recovery is a system-owned
                        // failure, never business completion or a user gate.
                        persistence
                            .persist_gate_message(&warning, "gate_warning")
                            .await?;
                        events.emit(crate::types::StreamEvent::CompletionGateAction {
                            kind: "warning".into(),
                            detail: warning.clone(),
                        });
                        terminal_stop_reason = Some(StopReason::FailedInternal);
                    }
                    crate::policy::CompletionFinalization::Blocked(message) => {
                        persistence
                            .mark_rejected_candidate(assistant_message_id.as_deref())
                            .await?;
                        persistence
                            .persist_gate_message(&message, "gate_blocked")
                            .await?;
                        publish_turn_activity(
                            persistence.as_ref(),
                            events.as_ref(),
                            root_turn_id.as_deref(),
                            "finalizing",
                            "failed_internal",
                            "failed_internal",
                            "系统未能完成任务",
                            None,
                            Some("completion_recovery_exhausted"),
                        )
                        .await?;
                        events.emit(crate::types::StreamEvent::Error { message });
                        emitted_terminal = true;
                        terminal_stop_reason = Some(StopReason::FailedInternal);
                        break;
                    }
                    crate::policy::CompletionFinalization::Complete => {
                        terminal_stop_reason = Some(StopReason::Finished);
                    }
                }
                last_final_text = text.clone();
                if matches!(terminal_stop_reason, Some(StopReason::FailedInternal)) {
                    publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        "finalizing",
                        "failed_internal",
                        "failed_internal",
                        "系统未能完成任务",
                        None,
                        Some("completion_recovery_exhausted"),
                    )
                    .await?;
                } else {
                    publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        "finalizing",
                        "completed",
                        "completed",
                        "任务已完成",
                        None,
                        None,
                    )
                    .await?;
                }
                // Always emit a terminal Done so the frontend's `streaming`
                // flag clears — even when the provider omitted usage on the
                // final turn. Previously Done was gated behind `usage`, so a
                // missing usage left the chat hung "running" forever.
                let (done_in, done_out) = usage
                    .as_ref()
                    .map(|u| (u.prompt_tokens, u.completion_tokens))
                    .unwrap_or((0, 0));
                events.emit(crate::types::StreamEvent::Done {
                    input_tokens: done_in,
                    output_tokens: done_out,
                });
                emitted_terminal = true;
                break;
            }

            let mut result_messages = Vec::new();
            let mut progress_prompt = None;
            let mut blocked_tool_result = false;
            let mut delivery_recovery_action = None;
            let mut structured_delivery_blocker = None;
            let completion_evidence_before_tool_batch = completion_gate.evidence();

            for (tool_index, tc) in tool_calls.iter().enumerate() {
                root_tool_call_count = root_tool_call_count.saturating_add(1);
                if !amplification_signal_emitted
                    && tool_amplification_threshold
                        .filter(|threshold| *threshold > 0)
                        .is_some_and(|threshold| root_tool_call_count >= threshold)
                {
                    let threshold = tool_amplification_threshold.unwrap_or(root_tool_call_count);
                    let notice = "本回合工具调用较多，系统已要求复用已有证据并收敛剩余步骤。";
                    let marker = format!(
                        "tool_amplification:{}:{threshold}",
                        root_turn_id.as_deref().unwrap_or("anonymous")
                    );
                    let newly_persisted = persistence
                        .persist_gate_message_once(&marker, notice, "turn_notice")
                        .await?;
                    amplification_signal_emitted = true;
                    if anonymous || newly_persisted {
                        amplification_prompt_pending = true;
                        events.emit(crate::types::StreamEvent::CompletionGateAction {
                            kind: "warning".into(),
                            detail: notice.into(),
                        });
                    }
                }
                if let Some(remaining) =
                    cancelled_tool_suffix(cancel.as_ref(), &tool_calls, tool_index)
                {
                    finish_cancelled_tool_batch(persistence.as_ref(), events.as_ref(), remaining)
                        .await?;
                    return Ok(run_outcome_for_terminal(
                        &completion_gate,
                        StopReason::Cancelled,
                        (total_input_tokens, total_output_tokens),
                        &last_final_text,
                    ));
                }
                // A wall-clock surface stops BETWEEN calls of one batch: the
                // reserve pays for the closing answer, so the rest of the batch
                // is abandoned rather than run past it. Desktop's default never
                // trips this.
                if !budget.may_start_tool() {
                    return Ok(run_outcome_for_terminal(
                        &completion_gate,
                        StopReason::BudgetExhausted,
                        (total_input_tokens, total_output_tokens),
                        &last_final_text,
                    ));
                }
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();

                // Extract bash command for finer-grained permission matching
                let bash_cmd = if tc.function.name == "bash" {
                    args.get("command")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };

                events.emit(crate::types::StreamEvent::ToolCallStart {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    args: args.clone(),
                });
                let (activity_phase, activity_label, activity_subject) =
                    sanitized_tool_activity(&tc.function.name);
                publish_turn_activity(
                    persistence.as_ref(),
                    events.as_ref(),
                    root_turn_id.as_deref(),
                    activity_phase,
                    "active",
                    "tool",
                    activity_label,
                    None,
                    None,
                )
                .await?;

                let remaining = max_iterations.saturating_sub(segment_iteration + 1) as u32;
                let completion_evidence = completion_gate.evidence();
                // Classify ONCE, via the backend (slice 4.8c b5): the default
                // rule is bash-only, so a surface whose shell tool has another
                // name must override it or every call reads as ReadOnly.
                let (classified_command, classified_kind) = tool_backend.classify(tc, &args);
                let reusable_verification_key = crate::policy::reusable_local_verification_key(
                    &classified_command,
                    &classified_kind,
                    &cwd,
                );
                if reusable_verification_key
                    .as_ref()
                    .is_some_and(|key| successful_local_verifications.contains(key))
                {
                    let content = format!(
                        "已复用当前 workspace 中相同命令的成功验证结果，未重复执行：{}",
                        classified_command
                    );
                    persistence
                        .record_tool_call_outcome(tc, "done", Some(&content), None, 0)
                        .await?;
                    events.emit(crate::types::StreamEvent::ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: content.clone(),
                        is_error: false,
                        status: "done".into(),
                        metadata: None,
                    });
                    result_messages.push(crate::types::ChatMessage {
                        role: "tool".into(),
                        content: crate::types::MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.function.name.clone()),
                        reasoning_content: None,
                    });
                    continue;
                }
                // Inspection budget first (b5), then the completion-policy
                // budget — matching the sidecar's ordering. Both are off on the
                // desktop unless the surface opts in.
                let capability_denial = crate::policy::capability_denial(
                    turn_capability,
                    &tc.function.name,
                    &classified_command,
                    &classified_kind,
                    &args,
                );
                let capability_denied = capability_denial.is_some();
                let inspection_denial = if inspection_budget {
                    crate::policy::inspection_budget_denial(
                        progress_tracker.read_only_exhausted(),
                        progress_tracker.mutation_seen(),
                        &classified_kind,
                    )
                } else {
                    None
                };
                let mut permission_denial_duration_ms = 0_u64;
                let mut permission_denial_stops_chain = false;
                let mut permission_denial_terminal_reason: Option<&'static str> = None;
                let mut permission_denial_waits_system = false;
                let denial_content = if let Some(content) = capability_denial {
                    Some(content)
                } else if let Some(denial) = inspection_denial.or_else(|| {
                    crate::policy::autonomous_budget_denial(
                        wall_budget,
                        remaining,
                        // The Budget owns the run's clock (slice 4.8c b3); desktop
                        // has none and keeps the default `None`, which makes the
                        // evaluator behave exactly as before.
                        budget.wall_time(),
                        &completion_evidence,
                        &classified_command,
                        &classified_kind,
                        &cwd,
                    )
                }) {
                    // Wording is the surface's (b4): desktop keeps its sentence,
                    // the sidecar its `policy denied command (rule): reason`.
                    Some(permission.format_budget_denial(&denial.rule, &denial.reason))
                } else {
                    match permission.authorize(tc, &args, bash_cmd.as_deref()).await {
                        PermissionOutcome::Allow => None,
                        PermissionOutcome::Deny(denial) => {
                            permission_denial_duration_ms = denial.duration_ms;
                            permission_denial_stops_chain = denial.reason.stops_tool_chain();
                            permission_denial_waits_system =
                                denial.reason.is_system_owned_interruption();
                            permission_denial_terminal_reason =
                                Some(denial.reason.terminal_reason());
                            Some(denial.content)
                        }
                        PermissionOutcome::Cancelled => {
                            finish_cancelled_tool_batch(
                                persistence.as_ref(),
                                events.as_ref(),
                                &tool_calls[tool_index..],
                            )
                            .await?;
                            return Ok(run_outcome_for_terminal(
                                &completion_gate,
                                StopReason::Cancelled,
                                (total_input_tokens, total_output_tokens),
                                &last_final_text,
                            ));
                        }
                    }
                };

                if let Some(content) = denial_content {
                    if capability_denied {
                        structural_denial_seen = true;
                    }
                    if permission_denial_stops_chain && !permission_denial_waits_system {
                        blocked_tool_result = true;
                        if blocker_terminal_reason.is_none() {
                            blocker_terminal_reason =
                                permission_denial_terminal_reason.map(str::to_owned);
                        }
                    }
                    if permission_denial_waits_system {
                        blocker_terminal_reason =
                            permission_denial_terminal_reason.map(str::to_owned);
                    }
                    persistence
                        .record_tool_call_outcome(
                            tc,
                            "denied",
                            None,
                            Some(&content),
                            permission_denial_duration_ms,
                        )
                        .await?;
                    events.emit(crate::types::StreamEvent::ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: content.clone(),
                        is_error: true,
                        status: "denied".into(),
                        metadata: None,
                    });
                    result_messages.push(crate::types::ChatMessage {
                        role: "tool".into(),
                        content: crate::types::MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.function.name.clone()),
                        reasoning_content: None,
                    });
                    if permission_denial_stops_chain {
                        let remaining_calls = &tool_calls[tool_index + 1..];
                        let cancelled = persistence
                            .persist_cancelled_tool_batch(remaining_calls)
                            .await?;
                        for (remaining_call, cancelled_content) in
                            remaining_calls.iter().zip(cancelled)
                        {
                            let remaining_args =
                                serde_json::from_str(&remaining_call.function.arguments)
                                    .unwrap_or_default();
                            events.emit(crate::types::StreamEvent::ToolCallStart {
                                id: remaining_call.id.clone(),
                                name: remaining_call.function.name.clone(),
                                args: remaining_args,
                            });
                            events.emit(crate::types::StreamEvent::ToolResult {
                                tool_call_id: remaining_call.id.clone(),
                                content: cancelled_content.clone(),
                                is_error: true,
                                status: "cancelled".into(),
                                metadata: None,
                            });
                            result_messages.push(crate::types::ChatMessage {
                                role: "tool".into(),
                                content: crate::types::MessageContent::Text(cancelled_content),
                                tool_calls: None,
                                tool_call_id: Some(remaining_call.id.clone()),
                                name: Some(remaining_call.function.name.clone()),
                                reasoning_content: None,
                            });
                        }
                        break;
                    }
                    continue;
                }

                // Pre-tool hook: may cancel. Headless (no hooks) always allows.
                let pre_allowed = hooks.pre_tool(&tc.function.name, &args).await;
                if !pre_allowed {
                    let content = "Tool call cancelled by hook.".to_string();
                    persistence
                        .record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    events.emit(crate::types::StreamEvent::ToolResult {
                        tool_call_id: tc.id.clone(),
                        content: content.clone(),
                        is_error: true,
                        status: "denied".into(),
                        metadata: None,
                    });
                    result_messages.push(crate::types::ChatMessage {
                        role: "tool".into(),
                        content: crate::types::MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.function.name.clone()),
                        reasoning_content: None,
                    });
                    continue;
                }
                if matches!(classified_kind, codefactory_agent_core::ToolKind::Mutation) {
                    successful_local_verifications.clear();
                }

                // Tool execution flows through the shared ToolBackend seam
                // (keystone slice 4.3): the desktop backend builds the ExecCtx
                // and runs MCP-first / native-dispatch. Timing stays in the loop
                // so it covers the fatal-error path exactly as before. The backend
                // is the run-scoped `tool_backend` hoisted above (slice 4.6b).
                let tool_ctx = crate::tool::ToolCtx {
                    working_directory: cwd.clone(),
                    session_id: Some(audit_session_id.clone()),
                    root_turn_id: root_turn_id.clone(),
                    task_id: task_id.clone(),
                    trajectory_session_id: Some(session_id.clone()),
                    mutation_permit: mutation_permit.clone(),
                    knowledge_library_ids: knowledge_library_ids.clone(),
                    timeout_sec: None,
                };

                let tool_start = std::time::Instant::now();
                let mut execution = Box::pin(tool_backend.execute(tc, &args, &tool_ctx));
                let mut heartbeat_emitted = false;
                let exec_result = if let Some(interval) =
                    tool_heartbeat_interval.filter(|interval| !interval.is_zero())
                {
                    loop {
                        match tokio::time::timeout(interval, execution.as_mut()).await {
                            Ok(result) => break result,
                            Err(_) => {
                                if cancel
                                    .as_ref()
                                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                                {
                                    finish_cancelled_tool_batch(
                                        persistence.as_ref(),
                                        events.as_ref(),
                                        &tool_calls[tool_index..],
                                    )
                                    .await?;
                                    return Ok(run_outcome_for_terminal(
                                        &completion_gate,
                                        StopReason::Cancelled,
                                        (total_input_tokens, total_output_tokens),
                                        &last_final_text,
                                    ));
                                }
                                heartbeat_emitted = true;
                                let elapsed = tool_start.elapsed();
                                let elapsed_label = approximate_wait_label(elapsed);
                                let heartbeat_label =
                                    format!("{activity_subject}仍在运行（约 {elapsed_label}）");
                                let waiting_reason =
                                    (elapsed >= long_tool_wait_threshold).then(|| {
                                        format!("{activity_subject}已连续运行约 {elapsed_label}")
                                    });
                                if let Err(error) = publish_turn_activity(
                                    persistence.as_ref(),
                                    events.as_ref(),
                                    root_turn_id.as_deref(),
                                    activity_phase,
                                    "active",
                                    "tool_wait",
                                    &heartbeat_label,
                                    waiting_reason.as_deref(),
                                    None,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        error = %error,
                                        "tool heartbeat activity update failed; tool execution continues"
                                    );
                                }
                            }
                        }
                    }
                } else {
                    execution.await
                };
                let duration_ms = tool_start.elapsed().as_millis() as u64;
                let output = match exec_result {
                    Ok(result) => result,
                    Err(error) => {
                        let error_text = error.to_string();
                        persistence
                            .record_tool_call_outcome(
                                tc,
                                "error",
                                None,
                                Some(&error_text),
                                duration_ms,
                            )
                            .await?;
                        if heartbeat_emitted {
                            if let Err(activity_error) = publish_turn_activity(
                                persistence.as_ref(),
                                events.as_ref(),
                                root_turn_id.as_deref(),
                                activity_phase,
                                "blocked",
                                "tool_finished",
                                "长时工具已因错误停止",
                                None,
                                Some("tool_error"),
                            )
                            .await
                            {
                                tracing::warn!(
                                    error = %activity_error,
                                    "tool terminal activity update failed after fatal outcome persistence"
                                );
                            }
                        }
                        return Err(LoopError::Tool(crate::tool::ToolError {
                            message: error_text,
                        }));
                    }
                };
                let mut system_owned_tool_wait_reason = None;
                if matches!(
                    output.status,
                    crate::tool::ToolExecutionStatus::Waiting
                        | crate::tool::ToolExecutionStatus::Blocked
                ) {
                    if let Some(action) = output.metadata.as_ref().and_then(|metadata| {
                        crate::policy::recoverable_delivery_prompt(
                            &tc.function.name,
                            metadata,
                            delivery_failure_signature.as_deref(),
                        )
                    }) {
                        if action.counts_as_repair_attempt {
                            delivery_failure_signature = Some(action.signature.clone());
                        }
                        delivery_recovery_action = Some(action);
                    } else if tc.function.name != "deliver_changes"
                        && output.metadata.as_ref().is_some_and(|metadata| {
                            metadata
                                .get("system_owned")
                                .and_then(serde_json::Value::as_bool)
                                == Some(true)
                        })
                    {
                        system_owned_tool_wait_reason = Some(
                            output
                                .metadata
                                .as_ref()
                                .and_then(|metadata| metadata.get("code"))
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("tool_system_recovery")
                                .to_string(),
                        );
                    } else {
                        blocked_tool_result = true;
                        if output.metadata.as_ref().is_some_and(|metadata| {
                            metadata.get("code").and_then(serde_json::Value::as_str)
                                == Some("browser_pairing_required")
                        }) {
                            blocker_terminal_reason = Some("browser_pairing_required".into());
                        }
                        if tc.function.name == "deliver_changes" {
                            structured_delivery_blocker = output
                                .metadata
                                .as_ref()
                                .and_then(structured_delivery_terminal_summary);
                        }
                    }
                }
                persistence
                    .record_tool_call_outcome(
                        tc,
                        match output.status {
                            crate::tool::ToolExecutionStatus::Done => "done",
                            crate::tool::ToolExecutionStatus::Waiting => "waiting",
                            crate::tool::ToolExecutionStatus::Blocked => "blocked",
                            crate::tool::ToolExecutionStatus::Error => "error",
                        },
                        (!matches!(output.status, crate::tool::ToolExecutionStatus::Error))
                            .then_some(output.content.as_str()),
                        matches!(output.status, crate::tool::ToolExecutionStatus::Error)
                            .then_some(output.content.as_str()),
                        duration_ms,
                    )
                    .await?;
                if let Some(metadata) = output.metadata.as_ref() {
                    persistence.record_tool_call_metadata(tc, metadata).await?;
                }

                // b6: the backend may report where the shell ended up (the
                // sidecar's Harbor container tracks `cd`). Absolute paths only —
                // a relative one would silently re-root the run.
                if let Some(next) = output
                    .next_working_directory
                    .as_deref()
                    .filter(|p| std::path::Path::new(p).is_absolute())
                {
                    cwd = std::path::PathBuf::from(next);
                }
                let completion_record = crate::policy::record_completion_outcome(
                    &mut completion_gate,
                    &mut progress_tracker,
                    &mut completion_sequence,
                    &cwd,
                    &tc.id,
                    &output,
                );
                if matches!(output.kind, codefactory_agent_core::ToolKind::Mutation) {
                    successful_local_verifications.clear();
                }
                if let Some(prompt) = completion_record.progress_prompt {
                    progress_prompt = Some(prompt);
                }
                if completion_record.succeeded
                    && matches!(output.kind, codefactory_agent_core::ToolKind::Verification)
                {
                    if let Some(key) = reusable_verification_key {
                        successful_local_verifications.insert(key);
                    }
                }
                // Post-tool hook (skipped headless — no hooks).
                let post_result: String = output.content.chars().take(500).collect();
                hooks
                    .post_tool(&tc.function.name, &post_result, duration_ms)
                    .await;

                events.emit(crate::types::StreamEvent::ToolResult {
                    tool_call_id: tc.id.clone(),
                    content: output.content.clone(),
                    is_error: output.is_error,
                    status: match output.status {
                        crate::tool::ToolExecutionStatus::Done => "done",
                        crate::tool::ToolExecutionStatus::Waiting => "waiting",
                        crate::tool::ToolExecutionStatus::Blocked => "blocked",
                        crate::tool::ToolExecutionStatus::Error => "error",
                    }
                    .into(),
                    metadata: output.metadata.clone(),
                });
                if heartbeat_emitted {
                    let (status, label, terminal_reason) = match output.status {
                        crate::tool::ToolExecutionStatus::Done => {
                            ("active", "长时工具已完成，正在继续处理", None)
                        }
                        crate::tool::ToolExecutionStatus::Waiting => {
                            ("active", "远端交付仍在等待，系统将自动续接", None)
                        }
                        crate::tool::ToolExecutionStatus::Blocked => {
                            ("blocked", "长时工具已在明确边界停止", Some("tool_blocked"))
                        }
                        crate::tool::ToolExecutionStatus::Error => {
                            ("active", "长时工具执行失败，正在尝试恢复", None)
                        }
                    };
                    if let Err(error) = publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        activity_phase,
                        status,
                        if matches!(output.status, crate::tool::ToolExecutionStatus::Error) {
                            "tool_failed"
                        } else {
                            "tool_finished"
                        },
                        label,
                        None,
                        terminal_reason,
                    )
                    .await
                    {
                        tracing::warn!(
                            error = %error,
                            "tool terminal activity update failed after outcome persistence"
                        );
                    }
                }

                if let Some(reason) = system_owned_tool_wait_reason.as_deref() {
                    publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        "recovering",
                        "waiting",
                        "tool_recovery_waiting",
                        "外部状态尚未确认，系统正在只读对账",
                        Some("等待安全观察器确认副作用状态"),
                        Some(reason),
                    )
                    .await?;
                    finish_cancelled_tool_batch(
                        persistence.as_ref(),
                        events.as_ref(),
                        &tool_calls[tool_index + 1..],
                    )
                    .await?;
                    return Ok(run_outcome_for_terminal(
                        &completion_gate,
                        StopReason::PlatformIncident,
                        (total_input_tokens, total_output_tokens),
                        &last_final_text,
                    ));
                }

                result_messages.push(crate::types::ChatMessage {
                    role: "tool".into(),
                    content: crate::types::MessageContent::Text(output.content),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(tc.function.name.clone()),
                    reasoning_content: None,
                });
            }

            // The round (and its tool batch) is done — let the surface close it
            // out; the sidecar emits a usage_snapshot when no tool_request went
            // out this round (b14).
            events.round_ended().await;
            completion_recovery_attempts =
                crate::policy::completion_recovery_attempts_after_tool_batch(
                    completion_recovery_attempts,
                    codefactory_agent_core::completion_evidence_made_progress(
                        &completion_evidence_before_tool_batch,
                        &completion_gate.evidence(),
                    ),
                );

            messages.push(crate::types::ChatMessage {
                role: "assistant".into(),
                content: crate::types::MessageContent::Text(text),
                tool_calls: Some(tool_calls),
                tool_call_id: None,
                name: None,
                reasoning_content: reasoning,
            });
            messages.extend(result_messages);
            if blocker_terminal_reason.as_deref().is_some_and(|reason| {
                matches!(reason, "permission_timed_out" | "permission_channel_closed")
            }) && !blocked_tool_result
            {
                publish_turn_activity(
                    persistence.as_ref(),
                    events.as_ref(),
                    root_turn_id.as_deref(),
                    "recovering",
                    "waiting",
                    "permission_waiting",
                    "授权通道暂时中断，系统将自动续接",
                    Some("等待安全授权通道恢复"),
                    blocker_terminal_reason.as_deref(),
                )
                .await?;
                events.emit(crate::types::StreamEvent::Done {
                    input_tokens: total_input_tokens.min(u32::MAX as u64) as u32,
                    output_tokens: total_output_tokens.min(u32::MAX as u64) as u32,
                });
                return Ok(run_outcome_for_terminal(
                    &completion_gate,
                    StopReason::PlatformIncident,
                    (total_input_tokens, total_output_tokens),
                    &last_final_text,
                ));
            }
            if let Some(summary) = structured_delivery_blocker {
                persistence
                    .persist_gate_message(&summary, "gate_blocked")
                    .await?;
                events.emit(crate::types::StreamEvent::CompletionGateAction {
                    kind: "turn_notice".into(),
                    detail: summary.clone(),
                });
                publish_turn_activity(
                    persistence.as_ref(),
                    events.as_ref(),
                    root_turn_id.as_deref(),
                    "finalizing",
                    "blocked",
                    "blocked",
                    "交付已在明确边界停止",
                    None,
                    Some("delivery_blocked"),
                )
                .await?;
                events.emit(crate::types::StreamEvent::Done {
                    input_tokens: 0,
                    output_tokens: 0,
                });
                return Ok(run_outcome_for_terminal(
                    &completion_gate,
                    StopReason::Blocked,
                    (total_input_tokens, total_output_tokens),
                    &summary,
                ));
            }
            if !blocked_tool_result {
                if let Some(action) = delivery_recovery_action {
                    finalization_pending = false;
                    blocker_summary_pending = false;
                    let retry_after = action.retry_after;
                    publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        "recovering",
                        "active",
                        "delivery_recovery",
                        "正在自动修复交付失败并续跑",
                        None,
                        None,
                    )
                    .await?;
                    if !retry_after.is_zero() {
                        let deadline = tokio::time::Instant::now() + retry_after;
                        loop {
                            if cancel
                                .as_ref()
                                .is_some_and(|flag| flag.load(Ordering::SeqCst))
                            {
                                finish_cancelled_tool_batch(
                                    persistence.as_ref(),
                                    events.as_ref(),
                                    &[],
                                )
                                .await?;
                                return Ok(run_outcome_for_terminal(
                                    &completion_gate,
                                    StopReason::Cancelled,
                                    (total_input_tokens, total_output_tokens),
                                    &last_final_text,
                                ));
                            }
                            let now = tokio::time::Instant::now();
                            if now >= deadline {
                                break;
                            }
                            tokio::time::sleep(
                                (deadline - now).min(std::time::Duration::from_secs(1)),
                            )
                            .await;
                        }
                    }
                    messages.push(crate::types::ChatMessage {
                        role: "user".into(),
                        content: crate::types::MessageContent::Text(action.prompt),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        reasoning_content: None,
                    });
                    continue;
                }
            }
            if blocked_tool_result {
                finalization_pending = true;
                blocker_summary_pending = true;
                publish_turn_activity(
                    persistence.as_ref(),
                    events.as_ref(),
                    root_turn_id.as_deref(),
                    "finalizing",
                    "blocked",
                    "blocked",
                    "正在整理阻断结果",
                    None,
                    blocker_terminal_reason.as_deref().or(Some("tool_blocked")),
                )
                .await?;
                messages.push(crate::types::ChatMessage {
                    role: "user".into(),
                    content: crate::types::MessageContent::Text(
                        // Reached only when recovery is genuinely exhausted:
                        // the same failure repeated on an unchanged head, or a
                        // blocker that needs the user. Everything technical is
                        // routed through `recoverable_delivery_prompt` above and
                        // never gets here, so "don't retry" no longer lands on
                        // work that could still have continued.
                        "工具链已在明确边界停止:同一失败在未改变的 head 上重复出现,或需要用户提供只有用户能给的输入。\
不要重试同一动作、绕过授权或把其他来源冒充等价证据;请只用已有事实生成一次简洁阻断总结,\
说明停在哪一步、已完成什么、以及需要用户做的那一件具体事。".into(),
                    ),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
                continue;
            }
            if let Some(prompt) = progress_prompt {
                messages.push(crate::types::ChatMessage {
                    role: "user".into(),
                    content: crate::types::MessageContent::Text(prompt),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
            }
            let evidence = completion_gate.evidence();
            if amplification_prompt_pending {
                amplification_prompt_pending = false;
                if !evidence.completed && !finalization_pending && !structural_denial_seen {
                    let threshold = tool_amplification_threshold.unwrap_or(root_tool_call_count);
                    let convergence_scope = match turn_capability {
                        TurnCapability::ReviewOnly => "只保留尚未完成且能改变结论的最少只读检查",
                        TurnCapability::Implement => {
                            "只执行尚未完成且能改变结果的最少实施或验证步骤"
                        }
                        TurnCapability::Deliver => "只执行尚未完成且能改变交付结果的最少步骤",
                    };
                    messages.push(crate::types::ChatMessage {
                        role: "user".into(),
                        content: crate::types::MessageContent::Text(format!(
                            "本回合已累计 {threshold} 次工具调用。请在下一轮主动收敛：复用已有证据，停止重复探测，{convergence_scope}；若存在外部阻断，请直接说明，不要轮询等待。"
                        )),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        reasoning_content: None,
                    });
                }
            }
            if crate::policy::completion_ready_applies(finalization)
                && evidence.completed
                && evidence.last_successful_verification_sequence != last_completion_nudge_sequence
            {
                last_completion_nudge_sequence = evidence.last_successful_verification_sequence;
                finalization_pending = true;
                let ready_prompt = codefactory_agent_core::build_completion_summary_prompt();
                persistence
                    .persist_gate_message(ready_prompt, "gate_ready")
                    .await?;
                events.emit(crate::types::StreamEvent::CompletionGateAction {
                    kind: "ready".into(),
                    detail: String::new(),
                });
                publish_turn_activity(
                    persistence.as_ref(),
                    events.as_ref(),
                    root_turn_id.as_deref(),
                    "finalizing",
                    "active",
                    "finalizing",
                    "正在形成最终结果",
                    None,
                    None,
                )
                .await?;
                messages.push(crate::types::ChatMessage {
                    role: "user".into(),
                    content: crate::types::MessageContent::Text(ready_prompt.to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
            // `required_tool_response` is this round's own tool_choice: when it
            // is set, the batch we just finished IS the forced recovery. Making
            // that batch re-arm the flag would lock the turn into back-to-back
            // `required` rounds — the model could never speak again until the
            // evidence ledger closed. One forced tool, then a normal `auto`
            // round, whether the recovery tool succeeded, failed, or was denied.
            } else if !matches!(turn_capability, TurnCapability::ReviewOnly)
                && !evidence.completed
                && !required_tool_response
            {
                require_tool_next = true;
                messages.push(crate::types::ChatMessage {
                    role: "user".into(),
                    content: crate::types::MessageContent::Text(
                        codefactory_agent_core::build_completion_recovery_prompt(&evidence),
                    ),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
                events.emit(crate::types::StreamEvent::CompletionGateAction {
                    kind: "evidence_needed".into(),
                    detail: evidence.blockers.join("; "),
                });
                publish_turn_activity(
                    persistence.as_ref(),
                    events.as_ref(),
                    root_turn_id.as_deref(),
                    "recovering",
                    "active",
                    "verification",
                    "正在补充缺失验证",
                    Some("验证证据不足"),
                    None,
                )
                .await?;
            } else if wall_budget {
                let remaining = max_iterations.saturating_sub(segment_iteration + 1);
                if codefactory_agent_core::should_prompt_budget_convergence(remaining as u32) {
                    messages.push(crate::types::ChatMessage {
                        role: "user".into(),
                        content: crate::types::MessageContent::Text(
                            codefactory_agent_core::build_budget_convergence_prompt(
                                remaining as u32,
                                &evidence,
                            ),
                        ),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        reasoning_content: None,
                    });
                }
                // b11: surfaces with a wall clock also converge on TIME, not just
                // on remaining rounds. Desktop has no clock (`wall_time()` is
                // None) so this never fires there.
                if let Some((remaining_secs, total_secs)) = budget.wall_time() {
                    if codefactory_agent_core::should_prompt_time_convergence(
                        remaining_secs,
                        total_secs,
                    ) {
                        messages.push(crate::types::ChatMessage {
                            role: "user".into(),
                            content: crate::types::MessageContent::Text(
                                codefactory_agent_core::build_time_convergence_prompt(
                                    remaining_secs,
                                    &evidence,
                                ),
                            ),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                            reasoning_content: None,
                        });
                    }
                }
            }
        }

        if emitted_terminal {
            break;
        }

        let evidence = completion_gate.evidence();
        let material_progress = codefactory_agent_core::completion_evidence_made_progress(
            &segment_start_evidence,
            &evidence,
        );
        let checkpoint_decision = crate::policy::segment_checkpoint_decision(
            &evidence,
            finalization,
            material_progress,
            stalled_chat_segments,
        );
        tracing::warn!(
            "agent loop reached segment checkpoint after {} rounds; total_rounds={}; completed={}; material_progress={}; decision={:?}",
            max_iterations,
            model_round_index,
            evidence.completed,
            material_progress,
            checkpoint_decision,
        );

        if matches!(
            checkpoint_decision,
            crate::policy::SegmentCheckpointDecision::Terminal
        ) {
            publish_turn_activity(
                persistence.as_ref(),
                events.as_ref(),
                root_turn_id.as_deref(),
                "finalizing",
                "incomplete",
                "incomplete",
                "任务尚未达到完成条件",
                None,
                Some("completion_evidence_incomplete"),
            )
            .await?;
            events.emit(crate::policy::iteration_ceiling_terminal_event(
                &evidence,
                finalization,
            ));
            terminal_stop_reason = Some(StopReason::Incomplete);
            break;
        }

        // Every chat segment ends with one tools-disabled assistant update.
        // The response is streamed naturally and persisted as ordinary
        // assistant history; productive segments then continue automatically.
        let checkpoint_prompt = crate::policy::segment_checkpoint_summary_prompt(&evidence);
        messages.push(crate::types::ChatMessage {
            role: "user".into(),
            content: crate::types::MessageContent::Text(checkpoint_prompt),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        });
        let checkpoint_options = crate::transport::RoundOptions {
            require_tool: false,
            tool_outcomes_so_far: completion_sequence as usize,
            reasoning_effort: context_policy.round_reasoning_effort().await,
        };
        let checkpoint_round = model_round_index;
        model_round_index = model_round_index.saturating_add(1);
        let crate::transport::ModelResponse {
            text: checkpoint_text,
            tool_calls: checkpoint_tool_calls,
            usage: checkpoint_usage,
            reasoning: checkpoint_reasoning,
            effective_route: checkpoint_effective_route,
            route_change: checkpoint_route_change,
        } = transport
            .complete(&messages, &[], &checkpoint_options)
            .await?;
        persist_route_change(
            persistence.as_ref(),
            events.as_ref(),
            checkpoint_route_change.as_ref(),
        )
        .await?;
        let checkpoint_usage_id = usage_request_id(&usage_run_id, checkpoint_round);
        if let Some(round_usage) = checkpoint_usage.as_ref() {
            record_usage_event_for_round(
                persistence.as_ref(),
                events.as_ref(),
                &usage_identity_for_route(&usage_identity, checkpoint_effective_route.as_ref()),
                round_usage,
                checkpoint_round,
            )
            .await;
        }
        if !checkpoint_tool_calls.is_empty() {
            let notice = format!(
                "连续执行检查点返回了 {} 个未执行的工具请求。为避免会话记录与实际文件状态不一致，\
本轮已安全停止，当前进度和失败证据已保存，并已转入系统内部修复。",
                checkpoint_tool_calls.len()
            );
            persistence
                .persist_gate_message(&notice, "turn_notice")
                .await?;
            events.emit(crate::types::StreamEvent::CompletionGateAction {
                kind: "turn_notice".into(),
                detail: notice.clone(),
            });
            publish_turn_activity(
                persistence.as_ref(),
                events.as_ref(),
                root_turn_id.as_deref(),
                "finalizing",
                "failed_internal",
                "failed_internal",
                "系统检查点协议异常",
                None,
                Some("checkpoint_protocol_violation"),
            )
            .await?;
            events.emit(crate::types::StreamEvent::Error { message: notice });
            return Ok(run_outcome_for_terminal(
                &completion_gate,
                StopReason::FailedInternal,
                (total_input_tokens, total_output_tokens),
                &last_final_text,
            ));
        }
        persistence
            .persist_message(
                "assistant",
                &checkpoint_text,
                checkpoint_usage
                    .as_ref()
                    .map(|usage| usage.prompt_tokens as i64),
                checkpoint_usage
                    .as_ref()
                    .map(|usage| usage.completion_tokens as i64),
                None,
                checkpoint_reasoning.as_deref(),
                checkpoint_effective_route
                    .as_ref()
                    .map(|route| route.endpoint_name.as_str()),
                checkpoint_effective_route
                    .as_ref()
                    .map(|route| route.model_id.as_str()),
                Some(&checkpoint_usage_id),
            )
            .await?;
        messages.push(crate::types::ChatMessage {
            role: "assistant".into(),
            content: crate::types::MessageContent::Text(checkpoint_text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: checkpoint_reasoning,
        });

        match checkpoint_decision {
            crate::policy::SegmentCheckpointDecision::Complete => {
                events.emit(crate::types::StreamEvent::Done {
                    input_tokens: checkpoint_usage
                        .as_ref()
                        .map_or(0, |usage| usage.prompt_tokens),
                    output_tokens: checkpoint_usage
                        .as_ref()
                        .map_or(0, |usage| usage.completion_tokens),
                });
                break;
            }
            crate::policy::SegmentCheckpointDecision::Continue => {
                stalled_chat_segments = if material_progress {
                    0
                } else {
                    stalled_chat_segments.saturating_add(1)
                };
                if !budget.may_continue(model_round_index) {
                    let notice = "执行环境已到安全停止点，当前进度已保存。\
系统将保留原目标并转入平台恢复队列；本轮不标记为完成。";
                    persistence
                        .persist_gate_message(notice, "turn_notice")
                        .await?;
                    events.emit(crate::types::StreamEvent::CompletionGateAction {
                        kind: "turn_notice".into(),
                        detail: notice.into(),
                    });
                    publish_turn_activity(
                        persistence.as_ref(),
                        events.as_ref(),
                        root_turn_id.as_deref(),
                        "waiting",
                        "platform_incident",
                        "platform_incident",
                        "执行平台暂不可用",
                        Some("等待系统恢复执行环境"),
                        Some("execution_budget_exhausted"),
                    )
                    .await?;
                    events.emit(crate::types::StreamEvent::Done {
                        input_tokens: 0,
                        output_tokens: 0,
                    });
                    return Ok(run_outcome_for_terminal(
                        &completion_gate,
                        StopReason::PlatformIncident,
                        (total_input_tokens, total_output_tokens),
                        &last_final_text,
                    ));
                }
                messages.push(crate::types::ChatMessage {
                    role: "user".into(),
                    content: crate::types::MessageContent::Text(
                        "继续执行原任务。依据刚才的检查点直接采取下一步，不要重新规划，\
不要询问用户是否继续；只有完成、真实不可恢复阻塞、新增授权或用户停止时才能结束。"
                            .into(),
                    ),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
            }
            crate::policy::SegmentCheckpointDecision::Pause(notice) => {
                persistence
                    .persist_gate_message(&notice, "turn_notice")
                    .await?;
                events.emit(crate::types::StreamEvent::CompletionGateAction {
                    kind: "turn_notice".into(),
                    detail: notice,
                });
                publish_turn_activity(
                    persistence.as_ref(),
                    events.as_ref(),
                    root_turn_id.as_deref(),
                    "finalizing",
                    "failed_internal",
                    "failed_internal",
                    "系统未能取得可验证进展",
                    None,
                    Some("stalled_recovery_exhausted"),
                )
                .await?;
                events.emit(crate::types::StreamEvent::Done {
                    input_tokens: 0,
                    output_tokens: 0,
                });
                return Ok(run_outcome_for_terminal(
                    &completion_gate,
                    StopReason::FailedInternal,
                    (total_input_tokens, total_output_tokens),
                    &last_final_text,
                ));
            }
            crate::policy::SegmentCheckpointDecision::Terminal => {
                unreachable!("non-chat terminal checkpoint was handled before finalization")
            }
        }
    }

    // The loop fell out of its segments: either a terminal reply already
    // emitted (emitted_terminal) or the ceiling/budget stopped it.
    let stop_reason = terminal_stop_reason.unwrap_or(if emitted_terminal {
        StopReason::Finished
    } else {
        StopReason::IterationCeiling
    });
    Ok(run_outcome_for_terminal(
        &completion_gate,
        stop_reason,
        (total_input_tokens, total_output_tokens),
        &last_final_text,
    ))
}

/// The `RunOutcome` for a terminal. The desktop adapter discards it (it already
/// emitted the terminal `StreamEvent`), so only the completion evidence is
/// filled; the sidecar slice will populate `final_text`/tokens.
fn run_outcome_for_terminal(
    gate: &codefactory_agent_core::CompletionGate,
    stop_reason: StopReason,
    tokens: (u64, u64),
    final_text: &str,
) -> RunOutcome {
    RunOutcome {
        final_text: final_text.to_string(),
        completion_evidence: gate.evidence(),
        input_tokens: tokens.0,
        output_tokens: tokens.1,
        stop_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    use crate::events::CollectingEventSink;
    use crate::journal::{NullBudget, PersistResult};
    use crate::services::{
        AllowAllPermissions, CompactionOutcome, ContextCompactionGate, ContextCompactor,
        NoOpFactChecker, NoOpHooks, PermissionDenial, PermissionDenialReason, PermissionGateway,
    };
    use crate::tool::{ToolBackend, ToolCtx, ToolInvocationResult};
    use crate::transport::{ModelResponse, ModelTransport, RoundOptions};
    use crate::types::{
        ChatMessage, ContentPart, FunctionCall, FunctionDefinition, ImageUrl, MessageContent,
        StreamEvent, ToolDefinition,
    };
    use codefactory_agent_core::ToolKind;

    struct ScriptedTransport {
        responses: Mutex<VecDeque<Result<ModelResponse, TransportError>>>,
        calls: Mutex<Vec<usize>>,
        requests: Mutex<Vec<Vec<ChatMessage>>>,
        advertised_tools: Mutex<Vec<Vec<ToolDefinition>>>,
    }

    impl ScriptedTransport {
        fn new(responses: Vec<ModelResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(Ok).collect()),
                calls: Mutex::new(Vec::new()),
                requests: Mutex::new(Vec::new()),
                advertised_tools: Mutex::new(Vec::new()),
            }
        }

        fn advertised_tool_counts(&self) -> Vec<usize> {
            self.calls.lock().expect("calls").clone()
        }

        /// The exact tool schemas handed to the provider each round — the only
        /// way to prove which tools the model could actually see, and what their
        /// descriptions told it.
        fn advertised_tools(&self) -> Vec<Vec<ToolDefinition>> {
            self.advertised_tools.lock().expect("tools").clone()
        }

        /// The exact message list handed to the provider each round — the only
        /// way to prove what the model actually saw.
        fn requests(&self) -> Vec<Vec<ChatMessage>> {
            self.requests.lock().expect("requests").clone()
        }

        fn from_results(responses: Vec<Result<ModelResponse, TransportError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: Mutex::new(Vec::new()),
                requests: Mutex::new(Vec::new()),
                advertised_tools: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModelTransport for ScriptedTransport {
        async fn complete(
            &self,
            messages: &[ChatMessage],
            tools: &[ToolDefinition],
            _opts: &RoundOptions,
        ) -> Result<ModelResponse, TransportError> {
            self.calls.lock().expect("calls").push(tools.len());
            self.advertised_tools
                .lock()
                .expect("tools")
                .push(tools.to_vec());
            self.requests
                .lock()
                .expect("requests")
                .push(messages.to_vec());
            self.responses
                .lock()
                .expect("responses")
                .pop_front()
                .unwrap_or_else(|| Err(TransportError::Fatal("script exhausted".into())))
        }
    }

    #[derive(Default)]
    struct RecordingPersistence {
        messages: Mutex<Vec<(String, String, Option<String>)>>,
        notices: Mutex<Vec<(String, String)>>,
        notice_markers: Mutex<BTreeSet<String>>,
        usage_ids: Mutex<Vec<String>>,
        tool_call_count: AtomicUsize,
        activity_fail_kind: Mutex<Option<String>>,
        recovery_attempts: Mutex<Vec<RecoveryAttemptRow>>,
        activities: Mutex<Vec<crate::journal::TurnActivityUpdate>>,
    }

    #[async_trait::async_trait]
    impl Persistence for RecordingPersistence {
        async fn record_recovery_attempt(&self, attempt: &RecoveryAttemptRow) -> PersistResult<()> {
            self.recovery_attempts
                .lock()
                .expect("recovery attempts")
                .push(attempt.clone());
            Ok(())
        }

        async fn update_turn_activity(
            &self,
            update: &crate::journal::TurnActivityUpdate,
        ) -> PersistResult<i64> {
            let should_fail = {
                let mut kind = self.activity_fail_kind.lock().expect("activity fail kind");
                if kind.as_deref() == Some(update.recent_activity_kind.as_str()) {
                    kind.take();
                    true
                } else {
                    false
                }
            };
            if should_fail {
                return Err(crate::journal::PersistError {
                    message: "injected activity failure".into(),
                });
            }
            self.activities
                .lock()
                .expect("activities")
                .push(update.clone());
            Ok(1)
        }

        async fn persist_message(
            &self,
            role: &str,
            content: &str,
            _input_tokens: Option<i64>,
            _output_tokens: Option<i64>,
            _tool_calls: Option<&[ToolCall]>,
            _reasoning_content: Option<&str>,
            _endpoint_id: Option<&str>,
            _model_id: Option<&str>,
            usage_request_id: Option<&str>,
        ) -> PersistResult<Option<String>> {
            let id = format!("m{}", self.messages.lock().expect("messages").len());
            self.messages.lock().expect("messages").push((
                role.into(),
                content.into(),
                usage_request_id.map(str::to_owned),
            ));
            Ok(Some(id))
        }

        async fn persist_gate_message(&self, content: &str, state: &str) -> PersistResult<()> {
            self.notices
                .lock()
                .expect("notices")
                .push((state.into(), content.into()));
            Ok(())
        }

        async fn persist_gate_message_once(
            &self,
            marker: &str,
            content: &str,
            state: &str,
        ) -> PersistResult<bool> {
            if !self
                .notice_markers
                .lock()
                .expect("notice markers")
                .insert(marker.into())
            {
                return Ok(false);
            }
            self.persist_gate_message(content, state)
                .await
                .map(|()| true)
        }

        async fn root_turn_tool_call_count(&self, _root_turn_id: &str) -> PersistResult<usize> {
            Ok(self.tool_call_count.load(Ordering::SeqCst))
        }

        async fn mark_rejected_candidate(&self, _message_id: Option<&str>) -> PersistResult<()> {
            Ok(())
        }

        async fn record_tool_call_started(
            &self,
            _message_id: &str,
            _tool_call: &ToolCall,
        ) -> PersistResult<()> {
            self.tool_call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn record_tool_call_outcome(
            &self,
            _tool_call: &ToolCall,
            _status: &str,
            _result: Option<&str>,
            _error: Option<&str>,
            _duration_ms: u64,
        ) -> PersistResult<()> {
            Ok(())
        }

        async fn persist_cancelled_tool_batch(
            &self,
            remaining: &[ToolCall],
        ) -> PersistResult<Vec<String>> {
            Ok(remaining.iter().map(|_| "cancelled".into()).collect())
        }

        async fn record_usage(&self, row: UsageRow<'_>) -> PersistResult<bool> {
            self.usage_ids
                .lock()
                .expect("usage ids")
                .push(row.request_id);
            Ok(true)
        }
    }

    struct ScriptedTools;

    #[async_trait::async_trait]
    impl ToolBackend for ScriptedTools {
        async fn list_schemas(&self) -> Vec<ToolDefinition> {
            vec![tool_definition()]
        }

        async fn execute(
            &self,
            call: &ToolCall,
            _args: &serde_json::Value,
            _ctx: &ToolCtx,
        ) -> Result<ToolInvocationResult, ToolError> {
            let verification = call.function.name == "bash";
            Ok(ToolInvocationResult {
                content: if verification {
                    "test result: ok. 1 passed; 0 failed".into()
                } else {
                    "updated src/lib.rs".into()
                },
                is_error: false,
                status: crate::tool::ToolExecutionStatus::Done,
                command: call.function.name.clone(),
                kind: if verification {
                    ToolKind::Verification
                } else {
                    ToolKind::Mutation
                },
                return_code: Some(0),
                stdout: if verification {
                    "test result: ok".into()
                } else {
                    String::new()
                },
                stderr: String::new(),
                error: None,
                metadata: None,
                next_working_directory: None,
                duration_ms: 1,
            })
        }
    }

    struct SlowTools {
        delay: std::time::Duration,
    }

    struct BlockedDeliveryTools;

    #[async_trait::async_trait]
    impl ToolBackend for BlockedDeliveryTools {
        async fn list_schemas(&self) -> Vec<ToolDefinition> {
            vec![tool_definition()]
        }

        async fn execute(
            &self,
            _call: &ToolCall,
            _args: &serde_json::Value,
            _ctx: &ToolCtx,
        ) -> Result<ToolInvocationResult, ToolError> {
            Ok(ToolInvocationResult {
                content: "delivery requires an external signing identity".into(),
                is_error: false,
                status: crate::tool::ToolExecutionStatus::Blocked,
                command: "deliver_changes".into(),
                kind: ToolKind::Mutation,
                return_code: Some(1),
                stdout: String::new(),
                stderr: String::new(),
                error: None,
                metadata: Some(serde_json::json!({
                    "status": "blocked",
                    "stage": "release_signing",
                    "code": "delivery_signing_identity_required",
                    "recoverable": false,
                    "recovery_class": "core_input_required",
                    "requested_ceiling": "through_release",
                    "reached_state": "release_triggered",
                    "next_action": "provide the external signing identity once"
                })),
                next_working_directory: None,
                duration_ms: 1,
            })
        }
    }

    #[async_trait::async_trait]
    impl ToolBackend for SlowTools {
        async fn list_schemas(&self) -> Vec<ToolDefinition> {
            vec![tool_definition()]
        }

        async fn execute(
            &self,
            call: &ToolCall,
            _args: &serde_json::Value,
            _ctx: &ToolCtx,
        ) -> Result<ToolInvocationResult, ToolError> {
            tokio::time::sleep(self.delay).await;
            Ok(ToolInvocationResult {
                content: "long tool finished".into(),
                is_error: false,
                status: crate::tool::ToolExecutionStatus::Done,
                command: call.function.name.clone(),
                kind: ToolKind::Verification,
                return_code: Some(0),
                stdout: "ok".into(),
                stderr: String::new(),
                error: None,
                metadata: None,
                next_working_directory: None,
                duration_ms: self.delay.as_millis() as u64,
            })
        }
    }

    struct SlowErrorTools {
        delay: std::time::Duration,
    }

    #[derive(Default)]
    struct SystemOwnedWaitingTools {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ToolBackend for SystemOwnedWaitingTools {
        async fn list_schemas(&self) -> Vec<ToolDefinition> {
            vec![tool_definition()]
        }

        async fn execute(
            &self,
            call: &ToolCall,
            _args: &serde_json::Value,
            _ctx: &ToolCtx,
        ) -> Result<ToolInvocationResult, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolInvocationResult {
                content: "外部状态尚未确认，系统将只读对账。".into(),
                is_error: false,
                status: crate::tool::ToolExecutionStatus::Waiting,
                command: call.function.name.clone(),
                kind: ToolKind::Mutation,
                return_code: None,
                stdout: String::new(),
                stderr: String::new(),
                error: None,
                metadata: Some(serde_json::json!({
                    "code": "external_state_uncertain",
                    "recoverable": true,
                    "system_owned": true,
                    "next_action": "observe_only_reconcile",
                })),
                next_working_directory: None,
                duration_ms: 1,
            })
        }
    }

    #[async_trait::async_trait]
    impl ToolBackend for SlowErrorTools {
        async fn list_schemas(&self) -> Vec<ToolDefinition> {
            vec![tool_definition()]
        }

        async fn execute(
            &self,
            call: &ToolCall,
            _args: &serde_json::Value,
            _ctx: &ToolCtx,
        ) -> Result<ToolInvocationResult, ToolError> {
            tokio::time::sleep(self.delay).await;
            Ok(ToolInvocationResult {
                content: "command failed and can be repaired".into(),
                is_error: true,
                status: crate::tool::ToolExecutionStatus::Error,
                command: call.function.name.clone(),
                kind: ToolKind::Verification,
                return_code: Some(1),
                stdout: String::new(),
                stderr: "failed".into(),
                error: Some("failed".into()),
                metadata: None,
                next_working_directory: None,
                duration_ms: self.delay.as_millis() as u64,
            })
        }
    }

    struct FixedDenialPermission {
        calls: AtomicUsize,
        reason: PermissionDenialReason,
    }

    impl FixedDenialPermission {
        fn new(reason: PermissionDenialReason) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                reason,
            }
        }
    }

    #[async_trait::async_trait]
    impl PermissionGateway for FixedDenialPermission {
        async fn authorize(
            &self,
            _tool_call: &ToolCall,
            _args: &serde_json::Value,
            _bash_command: Option<&str>,
        ) -> PermissionOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            PermissionOutcome::Deny(PermissionDenial {
                content: format!("permission outcome: {}", self.reason.terminal_reason()),
                reason: self.reason,
                duration_ms: 60_000,
            })
        }
    }

    #[derive(Default)]
    struct CountingTools {
        executed: Mutex<Vec<String>>,
        fail_next_verification: AtomicBool,
    }

    impl CountingTools {
        fn executed(&self) -> Vec<String> {
            self.executed.lock().expect("executed").clone()
        }

        fn failing_first_verification() -> Self {
            Self {
                executed: Mutex::new(Vec::new()),
                fail_next_verification: AtomicBool::new(true),
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolBackend for CountingTools {
        async fn list_schemas(&self) -> Vec<ToolDefinition> {
            vec![tool_definition()]
        }

        async fn execute(
            &self,
            call: &ToolCall,
            args: &serde_json::Value,
            _ctx: &ToolCtx,
        ) -> Result<ToolInvocationResult, ToolError> {
            let (command, kind) = self.classify(call, args);
            self.executed
                .lock()
                .expect("executed")
                .push(command.clone());
            let verification = matches!(kind, ToolKind::Verification);
            let failed = verification && self.fail_next_verification.swap(false, Ordering::SeqCst);
            Ok(ToolInvocationResult {
                content: if failed {
                    "test result: FAILED. 1 failed".into()
                } else if verification {
                    "test result: ok. 1 passed; 0 failed".into()
                } else {
                    "updated src/lib.rs".into()
                },
                is_error: failed,
                status: if failed {
                    crate::tool::ToolExecutionStatus::Error
                } else {
                    crate::tool::ToolExecutionStatus::Done
                },
                command,
                kind,
                return_code: Some(if failed { 1 } else { 0 }),
                stdout: if failed {
                    String::new()
                } else if verification {
                    "test result: ok".into()
                } else {
                    String::new()
                },
                stderr: if failed {
                    "test result: FAILED".into()
                } else {
                    String::new()
                },
                error: failed.then(|| "test result: FAILED".into()),
                metadata: None,
                next_working_directory: None,
                duration_ms: 1,
            })
        }
    }

    #[derive(Default)]
    struct CountingFactChecker {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl crate::services::FactChecker for CountingFactChecker {
        fn fact_check(&self, _reply: &str, _instruction: &str) -> Option<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Some("run another probe".into())
        }
    }

    struct FixedContext;

    #[async_trait::async_trait]
    impl crate::services::ContextPolicy for FixedContext {
        async fn context_window(&self, _estimated_tokens: u32) -> (u32, u32) {
            (100_000, 100_000)
        }

        async fn supports_vision(&self) -> bool {
            true
        }

        async fn round_reasoning_effort(&self) -> String {
            String::new()
        }
    }

    struct TextOnlyContext;

    #[async_trait::async_trait]
    impl crate::services::ContextPolicy for TextOnlyContext {
        async fn context_window(&self, _estimated_tokens: u32) -> (u32, u32) {
            (100_000, 100_000)
        }

        async fn supports_vision(&self) -> bool {
            false
        }

        async fn round_reasoning_effort(&self) -> String {
            String::new()
        }
    }

    /// Leaves the ordinary preflight untouched, then removes one old generated
    /// message when the overflow arm asks for the stricter emergency budget.
    /// The newest user input is intentionally preserved verbatim.
    struct EmergencyShrinkingCompressor;

    impl ContextCompactor for EmergencyShrinkingCompressor {
        fn compact(
            &self,
            mut messages: Vec<ChatMessage>,
            _system_prompt: &str,
            context_limit: u32,
            _tool_definitions: &[ToolDefinition],
        ) -> CompactionOutcome {
            if context_limit >= 100_000 {
                return CompactionOutcome {
                    messages,
                    ..Default::default()
                };
            }
            let before = crate::context::estimate_prompt_tokens(&messages, "test system");
            if let Some(index) = messages
                .iter()
                .position(|message| message.role == "assistant")
            {
                messages.remove(index);
            }
            let after = crate::context::estimate_prompt_tokens(&messages, "test system");
            CompactionOutcome {
                messages,
                compacted: after < before,
                elided_count: usize::from(after < before),
                tokens_freed: before.saturating_sub(after),
            }
        }
    }

    /// Adversarial seam implementation: claiming `compacted=true` must never
    /// be trusted unless the exact provider prompt estimate is smaller.
    struct LyingNoShrinkCompressor;

    impl ContextCompactor for LyingNoShrinkCompressor {
        fn compact(
            &self,
            messages: Vec<ChatMessage>,
            _system_prompt: &str,
            _context_limit: u32,
            _tool_definitions: &[ToolDefinition],
        ) -> CompactionOutcome {
            CompactionOutcome {
                messages,
                compacted: true,
                elided_count: 1,
                tokens_freed: 999,
            }
        }
    }

    struct CountingCompactor {
        calls: Arc<AtomicUsize>,
    }

    impl ContextCompactor for CountingCompactor {
        fn compact(
            &self,
            messages: Vec<ChatMessage>,
            _system_prompt: &str,
            _context_limit: u32,
            _tool_definitions: &[ToolDefinition],
        ) -> CompactionOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            CompactionOutcome {
                messages,
                ..Default::default()
            }
        }
    }

    struct ScriptedContextCompactionGate {
        decisions: Mutex<VecDeque<Result<(), String>>>,
        calls: AtomicUsize,
    }

    impl ScriptedContextCompactionGate {
        fn new(decisions: Vec<Result<(), String>>) -> Self {
            Self {
                decisions: Mutex::new(decisions.into()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ContextCompactionGate for ScriptedContextCompactionGate {
        async fn authorize_compaction(&self) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.decisions
                .lock()
                .expect("compaction gate decisions")
                .pop_front()
                .unwrap_or_else(|| Err("unexpected compaction authorization".into()))
        }
    }

    fn usage(prompt_tokens: u32, completion_tokens: u32) -> Usage {
        serde_json::from_value(serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens
        }))
        .expect("usage")
    }

    fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            r#type: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn response(text: &str, tool_calls: Vec<ToolCall>, round: u32) -> ModelResponse {
        ModelResponse {
            text: text.into(),
            tool_calls,
            usage: Some(usage(round + 1, round + 2)),
            reasoning: None,
            effective_route: None,
            route_change: None,
        }
    }

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            r#type: "function".into(),
            function: FunctionDefinition {
                name: "scripted".into(),
                description: "test".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }
    }

    fn inputs() -> LoopInputs {
        LoopInputs {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: MessageContent::Text("修复代码并运行测试验证".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            }],
            system_prompt: "test system".into(),
            tool_defs: vec![tool_definition()],
            completion_instruction: "修复代码并运行测试验证".into(),
            fact_check_instruction: String::new(),
            audit_session_id: "audit".into(),
            root_turn_id: Some("root".into()),
            mutation_permit: None,
            knowledge_library_ids: None,
            cancel: None,
        }
    }

    fn config() -> RunConfig {
        RunConfig {
            finalization: FinalizationPolicy::ReleaseWithWarning,
            turn_capability: TurnCapability::Implement,
            gate_benchmark: false,
            progress_window: 8,
            recovery_limit: 0,
            max_iterations: 2,
            wall_budget_applies: false,
            context_compression: true,
            overload_backoff: false,
            overload_retry_delays: [
                std::time::Duration::from_secs(20),
                std::time::Duration::from_secs(40),
            ],
            inspection_budget: false,
            replay_rejected_draft: false,
            tool_heartbeat_interval: None,
            long_tool_wait_threshold: std::time::Duration::from_secs(60),
            tool_amplification_threshold: None,
            session_id: "session".into(),
            endpoint_name: "test".into(),
            model_id: "model".into(),
            base_url: "http://example.invalid".into(),
            usage_run_id: "run".into(),
            surface: "interactive".into(),
            task_id: None,
            anonymous: false,
            is_chatgpt: false,
            cwd: std::path::PathBuf::from("/tmp"),
        }
    }

    fn services(
        transport: Arc<dyn ModelTransport>,
        persistence: Arc<dyn Persistence>,
        events: Arc<dyn EventSink>,
    ) -> LoopServices {
        LoopServices {
            transport,
            tools: Arc::new(ScriptedTools),
            persistence,
            events,
            budget: Arc::new(NullBudget),
            compactor: Arc::new(crate::services::DefaultCompressor),
            context_compaction_gate: Arc::new(crate::services::AllowAllContextCompaction),
            permission: Arc::new(AllowAllPermissions),
            hooks: Arc::new(NoOpHooks),
            context_policy: Arc::new(FixedContext),
            fact_checker: Arc::new(NoOpFactChecker),
            steer: Arc::new(crate::services::NoSteering),
        }
    }

    #[tokio::test]
    async fn stale_context_authorization_fences_before_normal_compaction_or_provider() {
        let transport = Arc::new(ScriptedTransport::new(vec![response(
            "unsafe provider response",
            vec![],
            0,
        )]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let compactor_calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(ScriptedContextCompactionGate::new(vec![Err(
            "objective revision or claim changed".into(),
        )]));
        let mut svc = services(transport.clone(), persistence.clone(), events.clone());
        svc.compactor = Arc::new(CountingCompactor {
            calls: compactor_calls.clone(),
        });
        svc.context_compaction_gate = gate.clone();

        let error = run_agent_loop(inputs(), config(), svc)
            .await
            .expect_err("stale authorization must stop the old runner");

        assert!(error.to_string().contains("CONTEXT_RECOVERY_FENCED"));
        assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
        assert_eq!(compactor_calls.load(Ordering::SeqCst), 0);
        assert!(transport.requests().is_empty());
        assert!(persistence
            .recovery_attempts
            .lock()
            .expect("recovery attempts")
            .is_empty());
        assert!(persistence.notices.lock().expect("notices").is_empty());
        assert!(!events
            .events()
            .iter()
            .any(|event| matches!(event, StreamEvent::ContextCompressed { .. })));
    }

    #[tokio::test]
    async fn claim_takeover_fences_before_emergency_compaction_or_retry_side_effects() {
        let transport = Arc::new(ScriptedTransport::from_results(vec![
            Err(TransportError::Fatal("context window exceeded".into())),
            Ok(response("unsafe retry", vec![], 0)),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let compactor_calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(ScriptedContextCompactionGate::new(vec![
            Ok(()),
            Err("claim epoch changed".into()),
        ]));
        let mut svc = services(transport.clone(), persistence.clone(), events.clone());
        svc.compactor = Arc::new(CountingCompactor {
            calls: compactor_calls.clone(),
        });
        svc.context_compaction_gate = gate.clone();

        let error = run_agent_loop(inputs(), config(), svc)
            .await
            .expect_err("replacement owner must fence emergency compaction");

        assert!(error.to_string().contains("CONTEXT_RECOVERY_FENCED"));
        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
        assert_eq!(compactor_calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.requests().len(), 1);
        assert!(persistence
            .recovery_attempts
            .lock()
            .expect("recovery attempts")
            .is_empty());
        assert!(persistence.notices.lock().expect("notices").is_empty());
        assert!(!events
            .events()
            .iter()
            .any(|event| matches!(event, StreamEvent::ContextCompressed { .. })));
    }

    #[tokio::test]
    async fn second_context_overflow_becomes_durable_context_owned_wait_without_third_request() {
        let transport = Arc::new(ScriptedTransport::from_results(vec![
            Err(TransportError::Fatal(
                "maximum context length exceeded on first request".into(),
            )),
            Err(TransportError::Fatal(
                "maximum context length exceeded after emergency compression".into(),
            )),
            Ok(response("unsafe third request", vec![], 0)),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut overflow_inputs = inputs();
        overflow_inputs.messages = vec![
            ChatMessage {
                role: "user".into(),
                content: MessageContent::Text("older request".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: MessageContent::Text("generated history ".repeat(20_000)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
            ChatMessage {
                role: "user".into(),
                content: MessageContent::Text("current request must stay verbatim".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
        ];
        let mut svc = services(transport.clone(), persistence.clone(), events.clone());
        svc.compactor = Arc::new(EmergencyShrinkingCompressor);

        let outcome = run_agent_loop(overflow_inputs, config(), svc)
            .await
            .expect("context exhaustion must transfer to durable system recovery");

        assert_eq!(outcome.stop_reason, StopReason::PlatformIncident);
        let requests = transport.requests();
        assert_eq!(
            requests.len(),
            2,
            "a second overflow is the hard retry bound"
        );
        let first = crate::context::estimate_prompt_tokens(&requests[0], "test system");
        let second = crate::context::estimate_prompt_tokens(&requests[1], "test system");
        assert!(
            second < first,
            "every emergency level must shrink the exact prompt"
        );
        assert!(requests[1].iter().any(|message| {
            matches!(&message.content, MessageContent::Text(text) if text == "current request must stay verbatim")
        }));
        let attempts = persistence
            .recovery_attempts
            .lock()
            .expect("recovery attempts");
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].domain, "context");
        assert_eq!(attempts[0].terminal_decision, "continue");
        assert_eq!(attempts[1].domain, "context");
        assert_eq!(attempts[1].terminal_decision, "waiting_system");
        assert!(persistence
            .activities
            .lock()
            .expect("activities")
            .iter()
            .any(|activity| activity.status == "waiting"
                && activity.terminal_reason.as_deref()
                    == Some("context_overflow_after_compaction")));
        assert!(!persistence
            .notices
            .lock()
            .expect("notices")
            .iter()
            .any(|(_, notice)| notice.contains("请继续")
                || notice.contains("请重试")
                || notice.contains("回复")));
    }

    #[tokio::test]
    async fn emergency_compaction_that_does_not_shrink_never_retries_provider() {
        let transport = Arc::new(ScriptedTransport::from_results(vec![
            Err(TransportError::Fatal("context window exceeded".into())),
            Ok(response("unsafe retry", vec![], 0)),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut svc = services(transport.clone(), persistence.clone(), events);
        svc.compactor = Arc::new(LyingNoShrinkCompressor);

        let outcome = run_agent_loop(inputs(), config(), svc)
            .await
            .expect("non-shrinking compaction must fail closed into Context recovery");

        assert_eq!(outcome.stop_reason, StopReason::PlatformIncident);
        assert_eq!(
            transport.requests().len(),
            1,
            "unchanged payload must not be resent"
        );
        assert!(persistence
            .activities
            .lock()
            .expect("activities")
            .iter()
            .any(|activity| activity.status == "waiting"
                && activity.terminal_reason.as_deref() == Some("context_compaction_exhausted")));
        let attempts = persistence
            .recovery_attempts
            .lock()
            .expect("recovery attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].failure_code, "CONTEXT_COMPACTION_EXHAUSTED");
    }

    #[tokio::test]
    async fn context_recovery_remembers_output_from_an_earlier_tool_round() {
        let transport = Arc::new(ScriptedTransport::from_results(vec![
            Ok(response(
                "visible progress before a read-only check",
                vec![call(
                    "verify-before-overflow",
                    "bash",
                    serde_json::json!({"command":"cargo test --lib"}),
                )],
                0,
            )),
            Err(TransportError::Fatal(
                "maximum context length exceeded".into(),
            )),
            Err(TransportError::Fatal(
                "maximum context length exceeded after compaction".into(),
            )),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut svc = services(transport, persistence.clone(), events);
        svc.compactor = Arc::new(EmergencyShrinkingCompressor);

        let outcome = run_agent_loop(inputs(), config(), svc)
            .await
            .expect("context failure remains system owned");

        assert_eq!(outcome.stop_reason, StopReason::PlatformIncident);
        let attempts = persistence
            .recovery_attempts
            .lock()
            .expect("recovery attempts");
        let terminal = attempts.last().expect("terminal context attempt");
        assert_eq!(terminal.domain, "context");
        assert!(
            terminal.output_started,
            "visible output from any earlier round must fence blind replay"
        );
        assert!(
            !terminal.side_effect_started,
            "a successful verification is observation, not a mutation"
        );
    }

    #[tokio::test]
    async fn long_tool_emits_sanitized_coalesced_activity_until_its_terminal_result() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "执行长验证",
                vec![call(
                    "slow-1",
                    "bash",
                    serde_json::json!({"command": "SECRET_COMMAND --token hidden"}),
                )],
                0,
            ),
            response("验证完成", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut cfg = config();
        cfg.tool_heartbeat_interval = Some(std::time::Duration::from_millis(10));
        cfg.long_tool_wait_threshold = std::time::Duration::from_millis(20);
        let mut svc = services(transport, persistence.clone(), events.clone());
        svc.tools = Arc::new(SlowTools {
            delay: std::time::Duration::from_millis(45),
        });

        run_agent_loop(inputs(), cfg, svc).await.expect("loop runs");

        let recorded = events.events();
        let heartbeat_positions: Vec<_> = recorded
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                StreamEvent::TurnActivityUpdated {
                    recent_activity_label,
                    waiting_reason,
                    ..
                } if recent_activity_label.contains("仍在运行") => {
                    Some((index, recent_activity_label.clone(), waiting_reason.clone()))
                }
                _ => None,
            })
            .collect();
        assert!(
            heartbeat_positions.len() >= 2,
            "expected periodic heartbeat: {recorded:?}"
        );
        assert!(heartbeat_positions
            .iter()
            .any(|(_, _, reason)| reason.as_deref().is_some_and(|text| text.contains("运行"))));
        assert!(heartbeat_positions.iter().all(|(_, label, reason)| {
            !label.contains("SECRET_COMMAND")
                && !label.contains("hidden")
                && reason
                    .as_deref()
                    .is_none_or(|text| !text.contains("SECRET_COMMAND") && !text.contains("hidden"))
        }));
        let result_position = recorded
            .iter()
            .position(|event| matches!(event, StreamEvent::ToolResult { tool_call_id, .. } if tool_call_id == "slow-1"))
            .expect("terminal tool result");
        assert!(heartbeat_positions
            .iter()
            .all(|(position, _, _)| *position < result_position));
        assert!(recorded
            .iter()
            .skip(result_position + 1)
            .any(|event| matches!(
                event,
                StreamEvent::TurnActivityUpdated {
                    recent_activity_kind,
                    waiting_reason: None,
                    ..
                } if recent_activity_kind == "tool_finished"
            )));
        assert!(
            persistence
                .messages
                .lock()
                .expect("messages")
                .iter()
                .all(|(_, content, _)| !content.contains("仍在运行")),
            "heartbeats must not create transcript messages"
        );
    }

    #[tokio::test]
    async fn provider_overload_budget_stops_after_three_failures_and_returns_system_owned_wait() {
        let transport = Arc::new(ScriptedTransport::from_results(vec![
            Err(TransportError::Retryable("HTTP 503 overloaded one".into())),
            Err(TransportError::Retryable("HTTP 503 overloaded two".into())),
            Err(TransportError::Retryable(
                "HTTP 503 overloaded three".into(),
            )),
            Ok(response("unsafe fourth request", vec![], 0)),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let mut cfg = config();
        cfg.overload_backoff = true;
        cfg.overload_retry_delays = [std::time::Duration::ZERO, std::time::Duration::ZERO];

        let error = run_agent_loop(
            inputs(),
            cfg,
            services(
                transport.clone(),
                persistence.clone(),
                Arc::new(CollectingEventSink::new()),
            ),
        )
        .await
        .expect_err("third overload must return to durable system-owned recovery");

        assert!(error
            .to_string()
            .contains("PROVIDER_OVERLOAD_BUDGET_EXHAUSTED"));
        assert_eq!(transport.advertised_tool_counts().len(), 3);
        let attempts = persistence
            .recovery_attempts
            .lock()
            .expect("recovery attempts");
        assert_eq!(attempts.len(), 3);
        assert_eq!(attempts[0].terminal_decision, "continue");
        assert_eq!(attempts[1].terminal_decision, "continue");
        assert_eq!(attempts[2].terminal_decision, "waiting_system");
    }

    #[tokio::test]
    async fn heartbeat_persistence_failure_does_not_cancel_the_inflight_tool() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "执行长验证",
                vec![call(
                    "slow-1",
                    "bash",
                    serde_json::json!({"command": "slow"}),
                )],
                0,
            ),
            response("验证完成", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut cfg = config();
        cfg.tool_heartbeat_interval = Some(std::time::Duration::from_millis(10));
        *persistence
            .activity_fail_kind
            .lock()
            .expect("activity fail kind") = Some("tool_wait".into());
        let mut svc = services(transport, persistence, events.clone());
        svc.tools = Arc::new(SlowTools {
            delay: std::time::Duration::from_millis(25),
        });

        run_agent_loop(inputs(), cfg, svc)
            .await
            .expect("diagnostic heartbeat failure must not abort the tool");

        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult { tool_call_id, status, .. }
                if tool_call_id == "slow-1" && status == "done"
        )));
    }

    #[tokio::test]
    async fn cancellation_during_a_waiting_tool_emits_one_terminal_done() {
        let transport = Arc::new(ScriptedTransport::new(vec![response(
            "",
            vec![call("slow-delivery", "scripted", serde_json::json!({}))],
            0,
        )]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut loop_inputs = inputs();
        loop_inputs.cancel = Some(cancel.clone());
        let mut cfg = config();
        cfg.tool_heartbeat_interval = Some(std::time::Duration::from_millis(2));
        let mut svc = services(transport, persistence, events.clone());
        svc.tools = Arc::new(SlowTools {
            delay: std::time::Duration::from_millis(100),
        });
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(8)).await;
            cancel.store(true, Ordering::SeqCst);
        });

        let outcome = run_agent_loop(loop_inputs, cfg, svc)
            .await
            .expect("cancellation is a normal terminal result");

        assert_eq!(outcome.stop_reason, StopReason::Cancelled);
        assert_eq!(
            events
                .events()
                .iter()
                .filter(|event| matches!(event, StreamEvent::Done { .. }))
                .count(),
            1,
        );
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult { tool_call_id, status, .. }
                if tool_call_id == "slow-delivery" && status == "cancelled"
        )));
    }

    #[tokio::test]
    async fn core_input_required_delivery_uses_structured_terminal_facts_not_model_prose() {
        let transport = Arc::new(ScriptedTransport::new(vec![response(
            "I shipped the release successfully",
            vec![call("delivery", "deliver_changes", serde_json::json!({}))],
            0,
        )]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut cfg = config();
        cfg.turn_capability = TurnCapability::Deliver;
        let mut svc = services(transport.clone(), persistence, events.clone());
        svc.tools = Arc::new(BlockedDeliveryTools);

        let outcome = run_agent_loop(inputs(), cfg, svc).await.expect("loop runs");

        assert_eq!(outcome.stop_reason, StopReason::Blocked);
        assert!(outcome
            .final_text
            .contains("delivery_signing_identity_required"));
        assert!(outcome.final_text.contains("release_triggered"));
        assert!(!outcome.final_text.contains("shipped successfully"));
        assert_eq!(
            transport.requests().len(),
            1,
            "no model-written blocker summary"
        );
        assert_eq!(
            events
                .events()
                .iter()
                .filter(|event| matches!(event, StreamEvent::Done { .. }))
                .count(),
            1,
        );
    }

    #[tokio::test]
    async fn terminal_activity_persistence_failure_does_not_fail_the_completed_tool_round() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "执行长验证",
                vec![call(
                    "slow-1",
                    "bash",
                    serde_json::json!({"command": "slow"}),
                )],
                0,
            ),
            response("验证完成", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        *persistence
            .activity_fail_kind
            .lock()
            .expect("activity fail kind") = Some("tool_finished".into());
        let events = Arc::new(CollectingEventSink::new());
        let mut cfg = config();
        cfg.tool_heartbeat_interval = Some(std::time::Duration::from_millis(10));
        let mut svc = services(transport, persistence, events.clone());
        svc.tools = Arc::new(SlowTools {
            delay: std::time::Duration::from_millis(25),
        });

        run_agent_loop(inputs(), cfg, svc)
            .await
            .expect("terminal activity failure must not fail a completed tool round");

        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult { tool_call_id, status, .. }
                if tool_call_id == "slow-1" && status == "done"
        )));
        assert!(events
            .events()
            .iter()
            .any(|event| matches!(event, StreamEvent::Done { .. })));
    }

    #[tokio::test]
    async fn ordinary_long_tool_error_stays_recoverable_instead_of_blocking_the_turn() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "执行长验证",
                vec![call(
                    "slow-1",
                    "bash",
                    serde_json::json!({"command": "slow"}),
                )],
                0,
            ),
            response("已根据错误完成修复", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut cfg = config();
        cfg.tool_heartbeat_interval = Some(std::time::Duration::from_millis(10));
        let mut svc = services(transport, persistence, events.clone());
        svc.tools = Arc::new(SlowErrorTools {
            delay: std::time::Duration::from_millis(25),
        });

        run_agent_loop(inputs(), cfg, svc).await.expect("loop runs");

        let recorded = events.events();
        assert!(recorded.iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated {
                status,
                recent_activity_kind,
                terminal_reason: None,
                ..
            } if status == "active" && recent_activity_kind == "tool_failed"
        )));
        assert!(!recorded.iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated {
                status,
                terminal_reason: Some(reason),
                ..
            } if status == "blocked" && reason == "tool_error"
        )));
    }

    #[tokio::test]
    async fn fortieth_tool_call_emits_one_visible_signal_and_one_convergence_prompt() {
        let tool_calls = (0..41)
            .map(|index| {
                call(
                    &format!("tool-{index}"),
                    "read_file",
                    serde_json::json!({"path": format!("doc-{index}.md")}),
                )
            })
            .collect();
        let transport = Arc::new(ScriptedTransport::new(vec![
            response("实施并验证", tool_calls, 0),
            response("已完成", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut cfg = config();
        cfg.turn_capability = TurnCapability::ReviewOnly;
        cfg.tool_amplification_threshold = Some(40);

        run_agent_loop(
            inputs(),
            cfg,
            services(transport.clone(), persistence.clone(), events.clone()),
        )
        .await
        .expect("loop runs");

        let warnings: Vec<_> = events
            .events()
            .into_iter()
            .filter_map(|event| match event {
                StreamEvent::CompletionGateAction { kind, detail } if kind == "warning" => {
                    Some(detail)
                }
                _ => None,
            })
            .filter(|detail| detail.contains("工具调用较多"))
            .collect();
        assert_eq!(warnings.len(), 1, "one visible signal per root turn");
        assert_eq!(
            persistence
                .notices
                .lock()
                .expect("notices")
                .iter()
                .filter(|(_, content)| content.contains("工具调用较多"))
                .count(),
            1,
            "one persisted signal per root turn"
        );
        let requests = transport.requests();
        let convergence_prompts: Vec<_> = requests
            .iter()
            .flat_map(|request| request.iter())
            .filter_map(|message| {
                if message.role != "user" {
                    return None;
                }
                match &message.content {
                    MessageContent::Text(text) if text.contains("本回合已累计 40 次工具调用") => {
                        Some(text)
                    }
                    _ => None,
                }
            })
            .collect();
        assert_eq!(
            convergence_prompts.len(),
            1,
            "one convergence prompt per root turn"
        );
        assert!(convergence_prompts[0].contains("最少只读检查"));
        assert!(!convergence_prompts[0].contains("实施"));
    }

    #[tokio::test]
    async fn resumed_root_turn_counts_prior_tool_calls_before_emitting_one_signal() {
        let persistence = Arc::new(RecordingPersistence::default());
        persistence.tool_call_count.store(30, Ordering::SeqCst);
        let first_transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "继续检查",
                (0..11)
                    .map(|index| {
                        call(
                            &format!("resume-tool-{index}"),
                            "bash",
                            serde_json::json!({"command": format!("verify-{index}")}),
                        )
                    })
                    .collect(),
                0,
            ),
            response("完成", vec![], 1),
        ]));
        let mut cfg = config();
        cfg.tool_amplification_threshold = Some(40);

        run_agent_loop(
            inputs(),
            cfg.clone(),
            services(
                first_transport,
                persistence.clone(),
                Arc::new(CollectingEventSink::new()),
            ),
        )
        .await
        .expect("first resumed run");

        let second_transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "再次续跑",
                vec![call(
                    "resume-tool-final",
                    "bash",
                    serde_json::json!({"command": "verify-final"}),
                )],
                0,
            ),
            response("完成", vec![], 1),
        ]));
        run_agent_loop(
            inputs(),
            cfg,
            services(
                second_transport,
                persistence.clone(),
                Arc::new(CollectingEventSink::new()),
            ),
        )
        .await
        .expect("second resumed run");

        assert_eq!(
            persistence
                .notices
                .lock()
                .expect("notices")
                .iter()
                .filter(|(_, content)| content.contains("工具调用较多"))
                .count(),
            1,
            "restart/resume must preserve one warning per root turn"
        );
    }

    #[test]
    fn approximate_wait_labels_do_not_round_up_an_entire_minute() {
        for (seconds, expected) in [
            (59, "59 秒"),
            (60, "1 分钟"),
            (61, "1 分钟"),
            (119, "1 分钟"),
            (120, "2 分钟"),
        ] {
            assert_eq!(
                approximate_wait_label(std::time::Duration::from_secs(seconds)),
                expected
            );
        }
    }

    /// A steer inbox that starts empty and produces its message only on the
    /// Nth drain — modelling a user who types while a tool is running, not one
    /// who had already typed before the turn began.
    #[derive(Default)]
    struct ScriptedSteer {
        deliver_on_drain: u32,
        message: String,
        drains: Mutex<u32>,
    }

    #[async_trait::async_trait]
    impl crate::services::SteerInbox for ScriptedSteer {
        async fn drain(&self) -> Vec<String> {
            let mut drains = self.drains.lock().expect("drains");
            *drains += 1;
            if *drains == self.deliver_on_drain {
                vec![self.message.clone()]
            } else {
                Vec::new()
            }
        }

        fn capability_override(&self, content: &str) -> Option<TurnCapability> {
            (content == "继续执行").then_some(TurnCapability::Implement)
        }
    }

    #[tokio::test]
    async fn an_explicit_steer_updates_action_intent_before_the_next_tool_batch() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "先检查",
                vec![call(
                    "t1",
                    "bash",
                    serde_json::json!({"command": "git status --short"}),
                )],
                0,
            ),
            response(
                "收到，开始实施",
                vec![call("t2", "scripted", serde_json::json!({}))],
                1,
            ),
            response(
                "验证",
                vec![call(
                    "t3",
                    "bash",
                    serde_json::json!({"command": "pnpm test"}),
                )],
                2,
            ),
            response("完成", vec![], 3),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let steer = Arc::new(ScriptedSteer {
            deliver_on_drain: 2,
            message: "继续执行".into(),
            drains: Mutex::new(0),
        });
        let mut cfg = config();
        cfg.turn_capability = TurnCapability::ReviewOnly;
        cfg.max_iterations = 4;
        let mut svc = services(transport, persistence, events.clone());
        svc.steer = steer;

        run_agent_loop(inputs(), cfg, svc).await.expect("loop runs");

        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult {
                tool_call_id,
                status,
                ..
            } if tool_call_id == "t2" && status == "done"
        )));
        assert!(!events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult {
                tool_call_id,
                status,
                ..
            } if tool_call_id == "t2" && status == "denied"
        )));
    }

    #[tokio::test]
    async fn an_action_intent_denial_cannot_end_as_task_completed() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "尝试修改",
                vec![call("t1", "scripted", serde_json::json!({}))],
                0,
            ),
            response("当前边界下未执行修改。", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut cfg = config();
        cfg.turn_capability = TurnCapability::ReviewOnly;

        run_agent_loop(
            inputs(),
            cfg,
            services(transport, persistence, events.clone()),
        )
        .await
        .expect("loop runs");

        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated { status, .. } if status == "blocked"
        )));
        assert!(!events.events().iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated { status, .. } if status == "completed"
        )));
    }

    #[tokio::test]
    async fn release_with_warning_is_failed_internal_but_still_closes_transport() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "先修改工作区。",
                vec![call("write-1", "scripted", serde_json::json!({}))],
                0,
            ),
            response("当前结果仍缺少修改后的验证证据。", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut cfg = config();
        cfg.recovery_limit = 0;

        let outcome = run_agent_loop(
            inputs(),
            cfg,
            services(transport, persistence.clone(), events.clone()),
        )
        .await
        .expect("an incomplete chat result still terminates normally");

        assert_eq!(outcome.stop_reason, StopReason::FailedInternal);
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated {
                status,
                recent_activity_kind,
                terminal_reason: Some(reason),
                ..
            } if status == "failed_internal"
                && recent_activity_kind == "failed_internal"
                && reason == "completion_recovery_exhausted"
        )));
        assert!(!events.events().iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated { status, .. } if status == "completed"
        )));
        assert_eq!(
            events
                .events()
                .iter()
                .filter(|event| matches!(event, StreamEvent::Done { .. }))
                .count(),
            1,
            "transport Done remains required to clear streaming",
        );
        let notices = persistence.notices.lock().expect("notices");
        let warning = notices
            .iter()
            .find(|(state, _)| state == "gate_warning")
            .map(|(_, content)| content)
            .expect("incomplete result must include a visible system notice");
        assert!(warning.contains("未完成"));
        assert!(!warning.contains("回复「继续"));
    }

    /// End-to-end through the real loop, not just the pure rule.
    ///
    /// `policy::capability_denial` unit tests prove the DECISION; they cannot
    /// prove the WIRING — that the loop passes the tool args to the gate, that
    /// the allowed write actually reaches the tool backend, and that the write
    /// tools are advertised to the model in the first place. The 2026-07-30
    /// field report was exactly a wiring problem: the rule "review turns don't
    /// mutate" was working as written, and that was the bug.
    #[tokio::test]
    async fn a_review_turn_writes_its_planning_document_and_still_refuses_code() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "把这份方案落盘",
                vec![
                    call(
                        "t1",
                        "write_file",
                        serde_json::json!({
                            "path": "docs/plans/embedded-browser-pane.md",
                            "content": "# 内置浏览器 pane\n",
                        }),
                    ),
                    call(
                        "t2",
                        "write_file",
                        serde_json::json!({
                            "path": "src/browser/pane.rs",
                            "content": "pub fn open() {}",
                        }),
                    ),
                ],
                0,
            ),
            response(
                "方案已写入 docs/plans/embedded-browser-pane.md；代码改动等你确认后再做。",
                vec![],
                1,
            ),
        ]));
        let events = Arc::new(CollectingEventSink::new());
        let tools = Arc::new(CountingTools::default());
        let mut cfg = config();
        cfg.turn_capability = TurnCapability::ReviewOnly;
        let mut loop_inputs = inputs();
        loop_inputs.tool_defs = vec![
            write_tool_definition("write_file"),
            write_tool_definition("edit_file"),
            tool_definition(),
        ];
        let mut svc = services(
            transport.clone(),
            Arc::new(RecordingPersistence::default()),
            events.clone(),
        );
        svc.tools = tools.clone();

        run_agent_loop(loop_inputs, cfg, svc)
            .await
            .expect("loop runs");

        // The planning document really executed …
        assert_eq!(
            tools.executed(),
            vec!["write_file docs/plans/embedded-browser-pane.md".to_string()],
            "the planning document must reach the tool backend, and the code write must not"
        );
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult { tool_call_id, status, .. }
                if tool_call_id == "t1" && status == "done"
        )));
        // … and the code write in the SAME batch was still refused.
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult { tool_call_id, status, .. }
                if tool_call_id == "t2" && status == "denied"
        )));

        // The model must SEE the write tools, carrying their document-only
        // bound; otherwise it falls back to a shell heredoc the gate blocks.
        let first_round = transport
            .advertised_tools()
            .into_iter()
            .next()
            .expect("at least one round");
        let write = first_round
            .iter()
            .find(|definition| definition.function.name == "write_file")
            .expect("write_file is advertised on a review turn");
        assert!(
            write.function.description.contains("docs/"),
            "advertised description must state the document-only bound: {}",
            write.function.description
        );
    }

    fn write_tool_definition(name: &str) -> ToolDefinition {
        ToolDefinition {
            r#type: "function".into(),
            function: FunctionDefinition {
                name: name.into(),
                description: "Create or overwrite a file with the given content.".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }
    }

    #[tokio::test]
    async fn permission_transport_interruptions_wait_for_system_recovery() {
        for reason in [
            PermissionDenialReason::TimedOut,
            PermissionDenialReason::ChannelClosed,
        ] {
            let transport = Arc::new(ScriptedTransport::new(vec![
                response(
                    "执行同一目标中的两个等价操作",
                    vec![
                        call("t1", "scripted", serde_json::json!({})),
                        call("t2", "scripted", serde_json::json!({})),
                    ],
                    0,
                ),
                response("授权通道暂不可用。", vec![], 1),
            ]));
            let persistence = Arc::new(RecordingPersistence::default());
            let events = Arc::new(CollectingEventSink::new());
            let tools = Arc::new(CountingTools::default());
            let permission = Arc::new(FixedDenialPermission::new(reason));
            let mut svc = services(transport, persistence, events.clone());
            svc.tools = tools.clone();
            svc.permission = permission.clone();

            let outcome = run_agent_loop(inputs(), config(), svc)
                .await
                .expect("permission transport interruption remains a settled turn");

            assert!(
                tools.executed().is_empty(),
                "an unresolved permission must not execute either side effect"
            );
            assert_eq!(
                permission.calls.load(Ordering::SeqCst),
                1,
                "the current batch stops safely while the objective moves to system-owned waiting"
            );
            assert!(events.events().iter().any(|event| matches!(
                event,
                StreamEvent::ToolResult {
                    tool_call_id,
                    status,
                    ..
                } if tool_call_id == "t2" && status == "cancelled"
            )));
            assert!(
                events.events().iter().any(|event| matches!(
                    event,
                    StreamEvent::TurnActivityUpdated {
                        status,
                        terminal_reason: Some(observed_reason),
                        ..
                    } if status == "waiting" && observed_reason == reason.terminal_reason()
                )),
                "{:?} is a system-owned wait, not a user-owned blocker",
                reason
            );
            assert!(!events.events().iter().any(|event| matches!(
                event,
                StreamEvent::TurnActivityUpdated { status, .. } if status == "blocked"
            )));
            assert_eq!(
                outcome.stop_reason,
                StopReason::PlatformIncident,
                "the transport turn may settle, but the objective remains owned by recovery"
            );
        }
    }

    #[tokio::test]
    async fn system_owned_unknown_tool_result_never_becomes_a_user_blocker_summary() {
        let transport = Arc::new(ScriptedTransport::new(vec![response(
            "执行两个外部变更",
            vec![
                call("t1", "scripted", serde_json::json!({})),
                call("t2", "scripted", serde_json::json!({})),
            ],
            0,
        )]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let tools = Arc::new(SystemOwnedWaitingTools::default());
        let mut svc = services(transport.clone(), persistence.clone(), events.clone());
        svc.tools = tools.clone();

        let outcome = run_agent_loop(inputs(), config(), svc)
            .await
            .expect("unknown external state must settle into durable system recovery");

        assert_eq!(outcome.stop_reason, StopReason::PlatformIncident);
        assert_eq!(tools.calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.requests().len(), 1, "no blocker-summary reprompt");
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult {
                tool_call_id,
                status,
                ..
            } if tool_call_id == "t2" && status == "cancelled"
        )));
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated {
                status,
                terminal_reason: Some(reason),
                ..
            } if status == "waiting" && reason == "external_state_uncertain"
        )));
        assert!(!persistence
            .messages
            .lock()
            .expect("messages")
            .iter()
            .any(|(role, content, _)| role == "user" && content.contains("需要用户")));
    }

    #[tokio::test]
    async fn explicit_permission_denial_stops_equivalent_side_effects() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "执行同一目标中的两个等价操作",
                vec![
                    call("t1", "scripted", serde_json::json!({})),
                    call("t2", "scripted", serde_json::json!({})),
                ],
                0,
            ),
            response("用户明确拒绝了该授权。", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let tools = Arc::new(CountingTools::default());
        let permission = Arc::new(FixedDenialPermission::new(
            PermissionDenialReason::DeniedByUser,
        ));
        let mut svc = services(transport, persistence, events.clone());
        svc.tools = tools.clone();
        svc.permission = permission;

        let outcome = run_agent_loop(inputs(), config(), svc)
            .await
            .expect("explicit denial settles normally");

        assert!(tools.executed().is_empty());
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::ToolResult {
                tool_call_id,
                status,
                ..
            } if tool_call_id == "t2" && status == "cancelled"
        )));
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated {
                status,
                terminal_reason: Some(reason),
                ..
            } if status == "blocked" && reason == "permission_denied_by_user"
        )));
        assert_eq!(outcome.stop_reason, StopReason::Blocked);
    }

    #[tokio::test]
    async fn a_steer_typed_during_a_tool_call_lands_at_the_next_round_boundary() {
        // Round 1 calls a tool; the user types while it runs; round 2 must
        // already carry their correction.
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "先看一下当前实现",
                vec![call("t1", "scripted", serde_json::json!({}))],
                0,
            ),
            response("好的，改用 chrome channel", vec![], 1),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let steer = Arc::new(ScriptedSteer {
            deliver_on_drain: 2,
            message: "改用 chrome channel".into(),
            drains: Mutex::new(0),
        });
        let mut svc = services(transport.clone(), persistence.clone(), events.clone());
        svc.steer = steer.clone();

        run_agent_loop(inputs(), config(), svc)
            .await
            .expect("loop runs");

        // Persisted as a real user turn — the user's own words, not a
        // framework control row.
        let persisted = persistence.messages.lock().expect("messages").clone();
        assert!(
            persisted
                .iter()
                .any(|(role, content, _)| role == "user" && content == "改用 chrome channel"),
            "steer must be persisted as a user message, got {persisted:?}",
        );

        // Announced exactly once, and only after it actually landed.
        let applied: Vec<_> = events
            .events()
            .into_iter()
            .filter_map(|event| match event {
                StreamEvent::SteerApplied { content, .. } => Some(content),
                _ => None,
            })
            .collect();
        assert_eq!(applied, vec!["改用 chrome channel".to_string()]);

        // The model saw it before its second request — that is the whole point.
        let second_request = transport
            .requests()
            .get(1)
            .cloned()
            .expect("a second round ran");
        assert!(
            second_request.iter().any(|m| {
                m.role == "user"
                    && matches!(&m.content, MessageContent::Text(t) if t == "改用 chrome channel")
            }),
            "the correction must be in the next request, got {second_request:?}",
        );
    }

    #[tokio::test]
    async fn an_empty_steer_inbox_changes_nothing() {
        let transport = Arc::new(ScriptedTransport::new(vec![response("完成", vec![], 0)]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut svc = services(transport, persistence.clone(), events.clone());
        svc.steer = Arc::new(ScriptedSteer::default());

        run_agent_loop(inputs(), config(), svc)
            .await
            .expect("loop runs");

        assert!(!events
            .events()
            .iter()
            .any(|event| matches!(event, StreamEvent::SteerApplied { .. })));
        assert!(!persistence
            .messages
            .lock()
            .expect("messages")
            .iter()
            .any(|(role, _, _)| role == "user"));
    }

    #[tokio::test]
    async fn repeated_successful_local_verification_is_reused_until_workspace_mutates() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "",
                vec![call(
                    "write-1",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "one"}),
                )],
                0,
            ),
            response(
                "",
                vec![call(
                    "verify-1",
                    "bash",
                    serde_json::json!({"command": "cargo test -p codefactory-agent-loop"}),
                )],
                1,
            ),
            response(
                "",
                vec![call(
                    "verify-duplicate",
                    "bash",
                    serde_json::json!({"command": "cargo test -p codefactory-agent-loop"}),
                )],
                2,
            ),
            response(
                "",
                vec![call(
                    "write-2",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "two"}),
                )],
                3,
            ),
            response(
                "",
                vec![call(
                    "verify-after-write",
                    "bash",
                    serde_json::json!({"command": "cargo test -p codefactory-agent-loop"}),
                )],
                4,
            ),
            response("修复完成，相关测试已通过。", vec![], 5),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let tools = Arc::new(CountingTools::default());
        let mut cfg = config();
        cfg.max_iterations = 8;
        let mut svc = services(transport, persistence, events);
        svc.tools = tools.clone();

        let outcome = run_agent_loop(inputs(), cfg, svc)
            .await
            .expect("scripted run");

        assert_eq!(
            tools.executed(),
            vec![
                "write_file src/lib.rs",
                "cargo test -p codefactory-agent-loop",
                "write_file src/lib.rs",
                "cargo test -p codefactory-agent-loop",
            ],
            "the unchanged duplicate must reuse the green result, while a later mutation invalidates it",
        );
        assert_eq!(
            outcome
                .completion_evidence
                .last_successful_verification_sequence,
            Some(4),
            "the skipped duplicate must not advance completion progress",
        );
    }

    #[tokio::test]
    async fn failed_local_verification_is_never_reused() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "",
                vec![call(
                    "write-1",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "one"}),
                )],
                0,
            ),
            response(
                "",
                vec![call(
                    "verify-failed",
                    "bash",
                    serde_json::json!({"command": "cargo test -p codefactory-agent-loop"}),
                )],
                1,
            ),
            response(
                "",
                vec![call(
                    "verify-retry",
                    "bash",
                    serde_json::json!({"command": "cargo test -p codefactory-agent-loop"}),
                )],
                2,
            ),
            response("测试已重新执行并通过。", vec![], 3),
        ]));
        let tools = Arc::new(CountingTools::failing_first_verification());
        let mut cfg = config();
        cfg.max_iterations = 8;
        let mut svc = services(
            transport,
            Arc::new(RecordingPersistence::default()),
            Arc::new(CollectingEventSink::new()),
        );
        svc.tools = tools.clone();

        run_agent_loop(inputs(), cfg, svc)
            .await
            .expect("scripted run");

        assert_eq!(
            tools.executed(),
            vec![
                "write_file src/lib.rs",
                "cargo test -p codefactory-agent-loop",
                "cargo test -p codefactory-agent-loop",
            ],
            "a failed check must execute again",
        );
    }

    #[tokio::test]
    async fn autonomous_final_summary_never_executes_returned_tool_calls() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "",
                vec![call(
                    "write-1",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "one"}),
                )],
                0,
            ),
            response(
                "",
                vec![call(
                    "verify-1",
                    "bash",
                    serde_json::json!({"command": "cargo test -p codefactory-agent-loop"}),
                )],
                1,
            ),
            response(
                "",
                vec![call(
                    "verify-from-summary",
                    "bash",
                    serde_json::json!({"command": "cargo test -p codefactory-agent-loop"}),
                )],
                2,
            ),
            response("修复完成，相关测试已通过。", vec![], 3),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let tools = Arc::new(CountingTools::default());
        let mut cfg = config();
        cfg.finalization = FinalizationPolicy::BlockOnIncomplete;
        cfg.max_iterations = 8;
        let mut svc = services(transport.clone(), persistence.clone(), events);
        svc.tools = tools.clone();

        run_agent_loop(inputs(), cfg, svc)
            .await
            .expect("scripted run");

        assert_eq!(
            tools.executed(),
            vec![
                "write_file src/lib.rs",
                "cargo test -p codefactory-agent-loop",
            ],
            "a tools-disabled final summary must not reopen verification",
        );
        assert_eq!(
            persistence
                .notices
                .lock()
                .expect("notices")
                .iter()
                .filter(|(state, _)| state == "gate_ready")
                .count(),
            1,
            "the final-summary instruction must be emitted only once",
        );
        assert_eq!(
            transport.advertised_tool_counts(),
            vec![1, 1, 0, 0],
            "a protocol-violating summary gets at most one tools-disabled summary retry",
        );
    }

    #[tokio::test]
    async fn autonomous_final_summary_skips_fact_check_reentry() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "",
                vec![call(
                    "write-1",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "one"}),
                )],
                0,
            ),
            response(
                "",
                vec![call(
                    "verify-1",
                    "bash",
                    serde_json::json!({"command": "cargo test -p codefactory-agent-loop"}),
                )],
                1,
            ),
            response("修复完成，相关测试已通过。", vec![], 2),
        ]));
        let fact_checker = Arc::new(CountingFactChecker::default());
        let mut cfg = config();
        cfg.finalization = FinalizationPolicy::BlockOnIncomplete;
        cfg.max_iterations = 8;
        let mut svc = services(
            transport,
            Arc::new(RecordingPersistence::default()),
            Arc::new(CollectingEventSink::new()),
        );
        svc.fact_checker = fact_checker.clone();

        run_agent_loop(inputs(), cfg, svc)
            .await
            .expect("scripted run");

        assert_eq!(
            fact_checker.calls.load(Ordering::SeqCst),
            0,
            "completed evidence must not trigger another fact-check/probe from the final summary",
        );
    }

    #[tokio::test]
    async fn text_only_model_blocks_image_turn_before_transport_instead_of_stripping() {
        let transport = Arc::new(ScriptedTransport::new(vec![response(
            "should never be called",
            vec![],
            0,
        )]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());
        let mut image_inputs = inputs();
        image_inputs.messages[0].content = MessageContent::Parts(vec![
            ContentPart {
                r#type: "text".into(),
                text: Some("这张图是什么？".into()),
                image_url: None,
            },
            ContentPart {
                r#type: "image_url".into(),
                text: None,
                image_url: Some(ImageUrl {
                    url: "data:image/png;base64,AA==".into(),
                }),
            },
        ]);
        let mut loop_services = services(transport.clone(), persistence.clone(), events.clone());
        loop_services.context_policy = Arc::new(TextOnlyContext);

        let error = run_agent_loop(image_inputs, config(), loop_services)
            .await
            .expect_err("image must be blocked before any provider request");

        assert!(error.to_string().contains("IMAGE_INPUT_UNSUPPORTED"));
        assert!(transport.advertised_tool_counts().is_empty());
        assert!(persistence.notices.lock().expect("notices").is_empty());
    }

    #[tokio::test]
    async fn chat_segment_checkpoint_persists_summary_and_automatically_continues() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "",
                vec![call(
                    "write-1",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "one"}),
                )],
                0,
            ),
            response(
                "",
                vec![call(
                    "write-2",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "two"}),
                )],
                1,
            ),
            response("已完成第一段修改，我会自动继续验证。", vec![], 2),
            response(
                "",
                vec![call(
                    "verify-1",
                    "bash",
                    serde_json::json!({"command": "cargo test"}),
                )],
                3,
            ),
            response("修复完成，测试已通过。", vec![], 4),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());

        run_agent_loop(
            inputs(),
            config(),
            services(transport.clone(), persistence.clone(), events.clone()),
        )
        .await
        .expect("scripted run");

        assert_eq!(
            transport.advertised_tool_counts(),
            vec![1, 1, 0, 1, 1],
            "the tools-disabled checkpoint must be followed automatically by another tool round"
        );
        assert!(
            persistence
                .messages
                .lock()
                .expect("messages")
                .iter()
                .any(|(role, body, _)| role == "assistant" && body.contains("自动继续验证")),
            "checkpoint body must survive reload"
        );
        assert_eq!(
            *persistence.usage_ids.lock().expect("usage ids"),
            vec!["run:0", "run:1", "run:2", "run:3", "run:4"]
        );
        let terminal_events = events
            .events()
            .into_iter()
            .filter(|event| matches!(event, StreamEvent::Done { .. } | StreamEvent::Error { .. }))
            .collect::<Vec<_>>();
        assert_eq!(terminal_events.len(), 1);
        assert!(matches!(
            terminal_events[0],
            StreamEvent::Done {
                input_tokens,
                output_tokens
            } if input_tokens > 0 && output_tokens > 0
        ));
    }

    #[tokio::test]
    async fn checkpoint_tool_calls_are_never_silently_discarded() {
        let transport = Arc::new(ScriptedTransport::new(vec![
            response(
                "",
                vec![call(
                    "write-1",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "one"}),
                )],
                0,
            ),
            response(
                "",
                vec![call(
                    "write-2",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "two"}),
                )],
                1,
            ),
            response(
                "I ignored the tools-disabled instruction.",
                vec![call(
                    "unexpected",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs", "content": "unsafe"}),
                )],
                2,
            ),
        ]));
        let persistence = Arc::new(RecordingPersistence::default());
        let events = Arc::new(CollectingEventSink::new());

        let outcome = run_agent_loop(
            inputs(),
            config(),
            services(transport.clone(), persistence.clone(), events.clone()),
        )
        .await
        .expect("checkpoint protocol violation is handled as a visible terminal");

        assert_eq!(outcome.stop_reason, StopReason::FailedInternal);
        assert_eq!(transport.advertised_tool_counts(), vec![1, 1, 0]);
        assert!(
            persistence
                .notices
                .lock()
                .expect("notices")
                .iter()
                .any(|(state, body)| state == "turn_notice"
                    && body.contains("未执行的工具请求")
                    && body.contains("系统内部修复")
                    && !body.contains("回复「继续")),
            "the protocol violation must be persisted as a system-owned failure"
        );
        assert!(events.events().iter().any(|event| matches!(
            event,
            StreamEvent::TurnActivityUpdated {
                status,
                terminal_reason: Some(reason),
                ..
            } if status == "failed_internal" && reason == "checkpoint_protocol_violation"
        )));
        assert!(
            events
                .events()
                .iter()
                .any(|event| matches!(event, StreamEvent::Error { message } if message.contains("未执行的工具请求"))),
            "the live turn must end with a visible error, never an empty Done"
        );
        assert!(!events
            .events()
            .iter()
            .any(|event| matches!(event, StreamEvent::Done { .. })));
    }

    #[test]
    fn finalization_policy_variants_are_distinct() {
        assert_ne!(
            FinalizationPolicy::ReleaseWithWarning,
            FinalizationPolicy::Benchmark
        );
        assert_ne!(
            FinalizationPolicy::BlockOnIncomplete,
            FinalizationPolicy::Benchmark
        );
    }

    #[test]
    fn loop_error_display_is_the_underlying_message_verbatim() {
        // Verbatim through every arm so a desktop adapter's
        // `AppError::Other(e.to_string())` and the loop's context-overflow /
        // vision greps (which read the Transport arm's Display) stay byte-correct.
        let t: LoopError = TransportError::Fatal("context length exceeded".into()).into();
        assert!(matches!(t, LoopError::Transport(_)));
        assert_eq!(t.to_string(), "context length exceeded");

        let p: LoopError = PersistError {
            message: "db is locked".into(),
        }
        .into();
        assert_eq!(p.to_string(), "db is locked");

        let tool: LoopError = ToolError {
            message: "unknown tool".into(),
        }
        .into();
        assert_eq!(tool.to_string(), "unknown tool");
    }

    #[test]
    fn run_config_holds_divergent_constants_explicitly() {
        fn identity() -> (
            bool,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            bool,
            bool,
            std::path::PathBuf,
        ) {
            (
                true,
                "s".into(),
                "ep".into(),
                "m".into(),
                "http://x".into(),
                "run".into(),
                "interactive".into(),
                None,
                false,
                false,
                std::path::PathBuf::from("/tmp"),
            )
        }
        let (
            wall_budget_applies,
            session_id,
            endpoint_name,
            model_id,
            base_url,
            usage_run_id,
            surface,
            task_id,
            anonymous,
            is_chatgpt,
            cwd,
        ) = identity();
        let desktop = RunConfig {
            finalization: FinalizationPolicy::ReleaseWithWarning,
            turn_capability: TurnCapability::Implement,
            gate_benchmark: false,
            progress_window: 8,
            recovery_limit: 3,
            max_iterations: 30,
            wall_budget_applies,
            context_compression: true,
            overload_backoff: false,
            overload_retry_delays: [
                std::time::Duration::from_secs(20),
                std::time::Duration::from_secs(40),
            ],
            inspection_budget: false,
            replay_rejected_draft: false,
            tool_heartbeat_interval: Some(std::time::Duration::from_secs(30)),
            long_tool_wait_threshold: std::time::Duration::from_secs(60),
            tool_amplification_threshold: Some(40),
            session_id: session_id.clone(),
            endpoint_name: endpoint_name.clone(),
            model_id: model_id.clone(),
            base_url: base_url.clone(),
            usage_run_id: usage_run_id.clone(),
            surface: surface.clone(),
            task_id: task_id.clone(),
            anonymous,
            is_chatgpt,
            cwd: cwd.clone(),
        };
        let sidecar = RunConfig {
            finalization: FinalizationPolicy::Benchmark,
            turn_capability: TurnCapability::Implement,
            gate_benchmark: true,
            progress_window: 4,
            recovery_limit: 1,
            max_iterations: 80,
            wall_budget_applies,
            context_compression: false,
            overload_backoff: true,
            overload_retry_delays: [
                std::time::Duration::from_secs(20),
                std::time::Duration::from_secs(40),
            ],
            inspection_budget: true,
            replay_rejected_draft: true,
            tool_heartbeat_interval: None,
            long_tool_wait_threshold: std::time::Duration::from_secs(60),
            tool_amplification_threshold: None,
            session_id,
            endpoint_name,
            model_id,
            base_url,
            usage_run_id,
            surface,
            task_id,
            anonymous,
            is_chatgpt,
            cwd,
        };
        // The whole point: these live as data, not as two forked code paths.
        assert!(!desktop.gate_benchmark && desktop.progress_window == 8);
        assert!(sidecar.gate_benchmark && sidecar.progress_window == 4);
    }

    #[test]
    fn effective_route_overrides_usage_attribution_without_changing_run_identity() {
        let base = UsageIdentity {
            session_id: "session".into(),
            endpoint_name: "chatgpt".into(),
            model_id: "gpt-5.5".into(),
            base_url: "https://chatgpt.com/backend-api/codex".into(),
            usage_run_id: "run".into(),
            surface: "interactive".into(),
            task_id: None,
            anonymous: false,
            is_chatgpt: true,
        };
        let actual = crate::transport::EffectiveRoute {
            endpoint_name: "deepseek".into(),
            model_id: "deepseek-v4-pro".into(),
            base_url: "https://api.deepseek.com".into(),
            is_chatgpt: false,
        };

        let resolved = usage_identity_for_route(&base, Some(&actual));

        assert_eq!(resolved.endpoint_name, "deepseek");
        assert_eq!(resolved.model_id, "deepseek-v4-pro");
        assert_eq!(resolved.base_url, "https://api.deepseek.com");
        assert!(!resolved.is_chatgpt);
        assert_eq!(resolved.session_id, "session");
        assert_eq!(resolved.usage_run_id, "run");
    }
}
