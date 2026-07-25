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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use codefactory_agent_core::CompletionEvidence;

use crate::events::EventSink;
use crate::journal::{Persistence, PersistError, UsageRow};
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
    } else if let Some(cost) = usage.cost.filter(|value| value.is_finite() && *value >= 0.0) {
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
    /// Chat surface: release with an amber warning instead of blocking (#135/#136).
    ReleaseWithWarning,
    /// Autonomous/subagent: block + Error on unmet evidence, scheduler respawns.
    BlockOnIncomplete,
    /// Terminal-Bench sidecar: 2-way completed/recovery (no release-with-warning).
    Benchmark,
}

/// Per-run configuration that the surface supplies; keeps divergent constants
/// (gate benchmark flag, tracker window, recovery limit, wall budget) explicit
/// instead of forked per copy. The usage-attribution identity + flags live here
/// too so the loop names no bin enum (`ApiStyle`/`UsageSurface` are pre-derived
/// to `is_chatgpt`/`surface`).
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub finalization: FinalizationPolicy,
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
    pub permission: Arc<dyn crate::services::PermissionGateway>,
    pub hooks: Arc<dyn crate::services::LifecycleHooks>,
    pub context_policy: Arc<dyn crate::services::ContextPolicy>,
    pub fact_checker: Arc<dyn crate::services::FactChecker>,
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
}

/// The single shared agent loop (keystone slice 4.6b). Drives one chat turn to a
/// terminal entirely through the [`LoopServices`] trait objects — no `Settings`,
/// no DB, no `AppHandle` — so the desktop app and the sidecar run the SAME body.
/// The desktop adapter builds the inputs/config/services and discards the
/// returned [`RunOutcome`] (it already emitted `Done` via the sink).
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
        knowledge_library_ids,
        cancel,
    } = inputs;
    let RunConfig {
        finalization,
        recovery_limit,
        max_iterations,
        wall_budget_applies: wall_budget,
        context_compression,
        overload_backoff,
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
        gate_benchmark,
        progress_window,
    } = config;
    let LoopServices {
        transport,
        tools: tool_backend,
        persistence,
        events,
        budget: _budget,
        permission,
        hooks,
        context_policy,
        fact_checker,
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
        // Proactive capability match: strip images BEFORE the first request
        // when the model is KNOWN text-only, instead of burning a 400 round
        // trip every turn. The reactive strip-and-retry stays as the net for
        // unknown models and wrong guesses.
        if !context_policy.supports_vision().await {
            let stripped = crate::protocol::strip_image_parts(&mut messages);
            if stripped > 0 {
                let notice = format!(
                    "当前模型不支持图片输入,已在发送前将历史中的 {stripped} 张图片替换为\
占位文本;切换到支持图片的模型可恢复图片理解。"
                );
                persistence.persist_gate_message_once("已在发送前", &notice, "turn_notice")
                    .await?;
            }
        }

        // Did we emit a terminal Done/Error this run? Used to guarantee the
        // stream always closes even if the loop runs to its iteration ceiling.
        let mut emitted_terminal = false;
        let mut completion_gate =
            codefactory_agent_core::CompletionGate::new_for_instruction(gate_benchmark, &completion_instruction);
        let mut completion_sequence = 0_u64;
        let mut last_completion_nudge_sequence = None;
        let mut progress_tracker =
            codefactory_agent_core::ProgressTracker::new(progress_window as u32);
        let mut finalization_pending = false;
        let mut completion_recovery_attempts = 0_u32;
        let mut fact_check_used = false;
        let mut require_tool_next = false;
        for iteration in 0..max_iterations {
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
                break;
            }
            // ── Context-window management ────────────────────────────────────
            // Estimate prompt tokens before sending. If we're over 75% of the
            // model's window, elide oversized tool results from the older
            // half. Notify the UI so the user knows what happened.
            let estimated = crate::context::estimate_prompt_tokens(&messages, system_prompt);
            let (context_limit, max_context_limit) =
                context_policy.context_window(estimated).await;
            // Compression is OpenAI/ChatGPT-only (slice 4.7): the Anthropic path
            // never elides history, so with `context_compression=false` the
            // history passes through untouched (no mem::take/repair/event) — we
            // still resolve the window above for the ContextUsage denominator.
            if context_compression {
                let compression = crate::context::compress_if_needed(
                    std::mem::take(&mut messages),
                    system_prompt,
                    context_limit,
                );
                // Storage repair is not enough: context compression can change the
                // final provider payload. Enforce the OpenAI tool-call protocol at
                // the last possible boundary before every model request.
                messages = crate::protocol::repair_openai_tool_protocol(compression.messages);
                if compression.compressed {
                    events.emit(crate::types::StreamEvent::ContextCompressed {
                        elided_count: compression.elided_count,
                        tokens_freed: compression.tokens_freed,
                    });
                }
            }

            let active_tool_defs = crate::policy::active_tool_definitions(tool_defs, finalization_pending);
            let required_tool_response = require_tool_next && !finalization_pending;
            // Resolve reasoning effort ONCE per round via ContextPolicy (slice
            // 4.6): it re-reads db+settings each round (freshness) and returns ""
            // for non-ChatGPT styles, so the transport reads no DB. Held in
            // `round_options` so the two reactive retries below reuse this round's
            // value.
            let round_options = crate::transport::RoundOptions {
                require_tool: required_tool_response,
                reasoning_effort: context_policy.round_reasoning_effort().await,
            };
            let call_result = transport
                .complete(&messages, active_tool_defs, &round_options)
                .await;
            let crate::transport::ModelResponse {
                text,
                tool_calls,
                usage,
                reasoning,
            } = match call_result {
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
                Err(e) if context_compression && crate::context::is_context_overflow(&e.to_string()) => {
                    let emergency_limit = (context_limit / 5).max(1) * 4;
                    let compression = crate::context::compress_if_needed(
                        std::mem::take(&mut messages),
                        system_prompt,
                        emergency_limit,
                    );
                    if !compression.compressed {
                        return Err(e.into());
                    }
                    messages = crate::protocol::repair_openai_tool_protocol(compression.messages);
                    let notice = format!(
                        "上下文超出模型窗口,已压缩 {} 条历史(约释放 {} tokens)后重试。",
                        compression.elided_count, compression.tokens_freed
                    );
                    persistence.persist_gate_message(&notice, "turn_notice").await?;
                    events.emit(crate::types::StreamEvent::CompletionGateAction {
                                kind: "turn_notice".into(),
                                detail: notice.clone(),
                            });
                    transport
                        .complete(&messages, active_tool_defs, &round_options)
                        .await?
                }
                // Transient provider saturation (Anthropic 529/overloaded, 503,
                // rate limit): back off + retry, up to twice, cancel-aware. No
                // StreamEvent — a persisted turn_notice is the only trace. Only
                // installed for surfaces that set `overload_backoff` (Anthropic);
                // OpenAI leaves it off, so this arm never matches there (slice 4.7).
                Err(e) if overload_backoff && crate::context::is_provider_overloaded(&e.to_string()) => {
                    let notice = "模型服务过载,正在自动退避重试(最多 2 次)。".to_string();
                    persistence
                        .persist_gate_message_once("自动退避重试", &notice, "turn_notice")
                        .await?;
                    let mut last_err = e;
                    let mut recovered = None;
                    for delay in [20u64, 40] {
                        if is_cancelled(cancel.as_ref()) {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                        match transport
                            .complete(&messages, active_tool_defs, &round_options)
                            .await
                        {
                            Ok(ok) => {
                                recovered = Some(ok);
                                break;
                            }
                            Err(next)
                                if crate::context::is_provider_overloaded(&next.to_string()) =>
                            {
                                last_err = next;
                            }
                            Err(next) => return Err(next.into()),
                        }
                    }
                    match recovered {
                        Some(resp) => resp,
                        None => return Err(last_err.into()),
                    }
                }
                Err(e) if crate::protocol::is_vision_rejection(&e.to_string()) => {
                    let stripped = crate::protocol::strip_image_parts(&mut messages);
                    if stripped == 0 {
                        return Err(e.into());
                    }
                    let notice = format!(
                        "已自动移除历史中的 {stripped} 张图片后重试:当前模型不支持图片输入。\
如需图片理解,请切换回支持图片的模型。"
                    );
                    persistence.persist_gate_message(&notice, "turn_notice").await?;
                    events.emit(crate::types::StreamEvent::CompletionGateAction {
                                kind: "turn_notice".into(),
                                detail: notice.clone(),
                            });
                    transport
                        .complete(&messages, active_tool_defs, &round_options)
                        .await?
                }
                Err(e) => return Err(e.into()),
            };
            finalization_pending = false;
            require_tool_next = false;

            // The provider request has completed. Persist already-consumed
            // Usage before honoring a cancellation that arrived in flight.
            let usage_request_id = usage_request_id(&usage_run_id, iteration);
            if let Some(round_usage) = usage.as_ref() {
                record_usage_event_for_round(persistence.as_ref(), events.as_ref(), &usage_identity, round_usage, iteration)
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
                    persistence.persist_message(
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
                        Some(&usage_request_id),
                    )
                    .await?
                } else {
                    None
                };
            if let Some(message_id) = assistant_message_id.as_deref() {
                for tc in &tool_calls {
                    persistence
                        .record_tool_call_started(message_id, tc)
                        .await?;
                }
            }

            if tool_calls.is_empty() {
                // Systemic fact-check: a tool-call-free reply asserting a
                // machine-verifiable obstacle (delivery blocked / command
                // missing / waiting on a checkable condition) gets ONE live
                // probe-backed correction — facts over stale memory.
                if !fact_check_used {
                    if let Some(correction) =
                        fact_checker.fact_check(&text, &fact_check_instruction)
                    {
                        fact_check_used = true;
                        persistence.persist_gate_message(&correction, "turn_notice").await?;
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
                match crate::policy::completion_finalization(&evidence, completion_recovery_attempts, finalization, recovery_limit) {
                    crate::policy::CompletionFinalization::Recover(prompt) => {
                        completion_recovery_attempts += 1;
                        require_tool_next = true;
                        // Make the rejection visible instead of silently looping:
                        // collapse the rejected candidate in the UI, persist the
                        // injected instruction so rebuilt history stays faithful.
                        persistence.mark_rejected_candidate(assistant_message_id.as_deref())
                            .await?;
                        persistence.persist_gate_message(&prompt, "gate_recovery").await?;
                        events.emit(crate::types::StreamEvent::CompletionGateAction {
                                    kind: "recovery".into(),
                                    detail: evidence.blockers.join("; "),
                                });
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
                        // The reply stands — no folding, no Error. Persist a
                        // visible warning and fall through to the normal Done.
                        persistence.persist_gate_message(&warning, "gate_warning").await?;
                        events.emit(crate::types::StreamEvent::CompletionGateAction {
                                    kind: "warning".into(),
                                    detail: warning.clone(),
                                });
                    }
                    crate::policy::CompletionFinalization::Blocked(message) => {
                        persistence.mark_rejected_candidate(assistant_message_id.as_deref())
                            .await?;
                        persistence.persist_gate_message(&message, "gate_blocked").await?;
                        events.emit(crate::types::StreamEvent::Error { message });
                        emitted_terminal = true;
                        break;
                    }
                    crate::policy::CompletionFinalization::Complete => {}
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
            let completion_evidence_before_tool_batch = completion_gate.evidence();

            for (tool_index, tc) in tool_calls.iter().enumerate() {
                if let Some(remaining) =
                    cancelled_tool_suffix(cancel.as_ref(), &tool_calls, tool_index)
                {
                    finish_cancelled_tool_batch(persistence.as_ref(), events.as_ref(), remaining)
                        .await?;
                    return Ok(run_outcome_for_terminal(&completion_gate));
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

                events.emit(crate::types::StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args: args.clone(),
                        });

                let remaining = max_iterations.saturating_sub(iteration + 1) as u32;
                let completion_evidence = completion_gate.evidence();
                let denial_content = if let Some(content) = crate::policy::autonomous_budget_denial(
                    wall_budget,
                    remaining,
                    &completion_evidence,
                    &tc.function.name,
                    &args,
                    &cwd,
                ) {
                    Some(content)
                } else {
                    match permission
                        .authorize(tc, &args, bash_cmd.as_deref())
                        .await
                    {
                        PermissionOutcome::Allow => None,
                        PermissionOutcome::Deny(content) => Some(content),
                        PermissionOutcome::Cancelled => {
                            finish_cancelled_tool_batch(
                                persistence.as_ref(),
                                events.as_ref(),
                                &tool_calls[tool_index..],
                            )
                            .await?;
                            return Ok(run_outcome_for_terminal(&completion_gate));
                        }
                    }
                };

                if let Some(content) = denial_content {
                    persistence.record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    events.emit(crate::types::StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
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

                // Pre-tool hook: may cancel. Headless (no hooks) always allows.
                let pre_allowed = hooks.pre_tool(&tc.function.name, &args).await;
                if !pre_allowed {
                    let content = "Tool call cancelled by hook.".to_string();
                    persistence.record_tool_call_outcome(tc, "denied", None, Some(&content), 0)
                        .await?;
                    events.emit(crate::types::StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                                status: "denied".into(),
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

                // Tool execution flows through the shared ToolBackend seam
                // (keystone slice 4.3): the desktop backend builds the ExecCtx
                // and runs MCP-first / native-dispatch. Timing stays in the loop
                // so it covers the fatal-error path exactly as before. The backend
                // is the run-scoped `tool_backend` hoisted above (slice 4.6b).
                let tool_ctx = crate::tool::ToolCtx {
                    working_directory: cwd.clone(),
                    session_id: Some(audit_session_id.clone()),
                    task_id: task_id.clone(),
                    knowledge_library_ids: knowledge_library_ids.clone(),
                    timeout_sec: None,
                };

                let tool_start = std::time::Instant::now();
                let exec_result = tool_backend.execute(tc, &args, &tool_ctx).await;
                let duration_ms = tool_start.elapsed().as_millis() as u64;
                let output = match exec_result {
                    Ok(result) => result,
                    Err(error) => {
                        let error_text = error.to_string();
                        persistence.record_tool_call_outcome(
                            tc,
                            "error",
                            None,
                            Some(&error_text),
                            duration_ms,
                        )
                        .await?;
                        return Err(LoopError::Tool(crate::tool::ToolError { message: error_text }));
                    }
                };
                persistence.record_tool_call_outcome(
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

                if let Some(prompt) = crate::policy::record_completion_outcome(
                    &mut completion_gate,
                    &mut progress_tracker,
                    &mut completion_sequence,
                    &cwd,
                    &tc.function.name,
                    &completion_args,
                    &output.content,
                    output.is_error,
                ) {
                    progress_prompt = Some(prompt);
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
                            status: if output.is_error {
                                "error".into()
                            } else {
                                "done".into()
                            },
                        });

                result_messages.push(crate::types::ChatMessage {
                    role: "tool".into(),
                    content: crate::types::MessageContent::Text(output.content),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(tc.function.name.clone()),
                    reasoning_content: None,
                });
            }

            completion_recovery_attempts = crate::policy::completion_recovery_attempts_after_tool_batch(
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
            if crate::policy::completion_ready_applies(finalization)
                && evidence.completed
                && evidence.last_successful_verification_sequence != last_completion_nudge_sequence
            {
                last_completion_nudge_sequence = evidence.last_successful_verification_sequence;
                finalization_pending = true;
                persistence.persist_gate_message(codefactory_agent_core::build_completion_ready_prompt(), "gate_ready")
                    .await?;
                events.emit(crate::types::StreamEvent::CompletionGateAction {
                            kind: "ready".into(),
                            detail: String::new(),
                        });
                messages.push(crate::types::ChatMessage {
                    role: "user".into(),
                    content: crate::types::MessageContent::Text(codefactory_agent_core::build_completion_ready_prompt().to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                });
            } else if wall_budget {
                let remaining = max_iterations.saturating_sub(iteration + 1);
                if codefactory_agent_core::should_prompt_budget_convergence(remaining as u32) {
                    messages.push(crate::types::ChatMessage {
                        role: "user".into(),
                        content: crate::types::MessageContent::Text(codefactory_agent_core::build_budget_convergence_prompt(
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
                max_iterations,
                evidence.completed,
            );
            events.emit(crate::policy::iteration_ceiling_terminal_event(&evidence, finalization));
        }


        Ok(run_outcome_for_terminal(&completion_gate))
    }

/// The `RunOutcome` for a terminal. The desktop adapter discards it (it already
/// emitted the terminal `StreamEvent`), so only the completion evidence is
/// filled; the sidecar slice will populate `final_text`/tokens.
fn run_outcome_for_terminal(gate: &codefactory_agent_core::CompletionGate) -> RunOutcome {
    RunOutcome {
        final_text: String::new(),
        completion_evidence: gate.evidence(),
        input_tokens: 0,
        output_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        fn identity() -> (bool, String, String, String, String, String, String, Option<String>, bool, bool, std::path::PathBuf) {
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
        let (wall_budget_applies, session_id, endpoint_name, model_id, base_url, usage_run_id, surface, task_id, anonymous, is_chatgpt, cwd) =
            identity();
        let desktop = RunConfig {
            finalization: FinalizationPolicy::ReleaseWithWarning,
            gate_benchmark: false,
            progress_window: 8,
            recovery_limit: 3,
            max_iterations: 30,
            wall_budget_applies,
            context_compression: true,
            overload_backoff: false,
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
            gate_benchmark: true,
            progress_window: 4,
            recovery_limit: 1,
            max_iterations: 80,
            wall_budget_applies,
            context_compression: false,
            overload_backoff: true,
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
}
