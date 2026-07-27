// SPDX-License-Identifier: Apache-2.0
//! The sidecar's implementations of the shared loop's capability seams
//! (keystone slice 4.8). These are what let the Terminal-Bench eval harness
//! drive the SAME `agent_loop::run::run_agent_loop` the desktop does, instead of
//! carrying its own copy of the loop.
//!
//! The eval-scoring surface is deliberately preserved rather than adopted from
//! the desktop:
//! - [`CharBudgetCompactor`] keeps the sidecar's destructive char-budget digest
//!   (the desktop's token-based elision would move scores).
//! - [`SidecarPermissions`] keeps the sidecar's `RuntimePolicy` and its
//!   `policy denied command ({rule}): {reason}` denial wording.
//! - [`DelegatingToolBackend::classify`] classifies `run_shell` with the REAL
//!   `classify_command` — the shared default is bash-only and would mark every
//!   call `ReadOnly`, so the completion gate would never see a mutation.
//!
//! NOT YET CONSUMED: `run()` still drives the sidecar's own loop body. Flipping
//! it over additionally needs the two `usage_snapshot` emission points the
//! bridge contract requires but the shared loop has no hook for (b13/b14) — see
//! `docs/design/sidecar-shared-loop-4.8.md`.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Instant;

use codefactory_agent_core::{classify_command, effective_command_timeout_sec, PolicyDecision, ToolKind};
use codefactory_agent_loop::events::EventSink;
use codefactory_agent_loop::journal::Budget;
use codefactory_agent_loop::services::{
    CompactionOutcome, ContextCompactor, PermissionGateway, PermissionOutcome,
};
use codefactory_agent_loop::tool::{ToolBackend, ToolCtx, ToolError, ToolInvocationResult};
use codefactory_agent_loop::types::{ChatMessage, MessageContent, StreamEvent, ToolCall};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::Mutex;

use crate::compaction::{tool_result_content, ToolHistoryEntry};
use crate::policy::RuntimePolicy;
use crate::protocol::{read_tool_result, write_output, OutputMessage};
use crate::Usage;

/// Shared stdin/stdout, so the tool backend's `tool_request`, the event sink's
/// `usage_snapshot`, and `main()`'s `finished` interleave in the pinned order.
/// The loop is single-threaded per turn, so these mutexes are uncontended.
pub(crate) struct Jsonl<R, W> {
    pub(crate) input: Mutex<R>,
    pub(crate) output: Mutex<W>,
    pub(crate) usage: Mutex<Usage>,
}

/// Runs a tool by DELEGATING it over the JSONL protocol: writes a
/// `tool_request` and blocks on the matching `tool_result`. Correlation is
/// strict and synchronous — one request in flight, id must match, and every
/// protocol violation is fatal, exactly as the sidecar's own loop had it.
pub(crate) struct DelegatingToolBackend<R, W> {
    pub(crate) io: Arc<Jsonl<R, W>>,
    pub(crate) schema: codefactory_agent_loop::types::ToolDefinition,
    pub(crate) shell_timeout_sec: u64,
    /// Wall clock, so the per-call timeout can be clamped to the reserve.
    pub(crate) started: Instant,
    pub(crate) wall_time_budget_sec: Option<u64>,
    /// Untruncated streams for the compaction digest — the message copy is
    /// already truncated, so only the backend sees the full text.
    pub(crate) history: Arc<Mutex<Vec<ToolHistoryEntry>>>,
    /// Shared with the event sink so `round_ended` knows whether this round
    /// already put usage on the wire (b14).
    pub(crate) emitted_usage_this_round: Arc<std::sync::atomic::AtomicBool>,
}

impl<R, W> DelegatingToolBackend<R, W> {
    fn command_of(args: &serde_json::Value) -> String {
        args.get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    }
}

#[async_trait::async_trait]
impl<R, W> ToolBackend for DelegatingToolBackend<R, W>
where
    R: AsyncBufRead + Unpin + Send + Sync,
    W: AsyncWrite + Unpin + Send + Sync,
{
    async fn list_schemas(&self) -> Vec<codefactory_agent_loop::types::ToolDefinition> {
        vec![self.schema.clone()]
    }

    /// The sidecar's shell tool is `run_shell`, so the shared bash-only default
    /// would classify every call `ReadOnly` and the gate would never see a
    /// mutation. Classify the real command instead (keystone 4.8c b5).
    fn classify(&self, _call: &ToolCall, args: &serde_json::Value) -> (String, ToolKind) {
        let command = Self::command_of(args);
        let timeout_ms =
            effective_command_timeout_sec(&command, self.shell_timeout_sec, self.shell_timeout_sec)
                .saturating_mul(1_000);
        let kind = classify_command(&command, timeout_ms);
        (command, kind)
    }

    async fn execute(
        &self,
        call: &ToolCall,
        args: &serde_json::Value,
        ctx: &ToolCtx,
    ) -> Result<ToolInvocationResult, ToolError> {
        let command = Self::command_of(args);
        let requested = ctx
            .timeout_sec
            .or_else(|| args.get("timeout_sec").and_then(|v| v.as_u64()))
            .unwrap_or(self.shell_timeout_sec);
        let timeout_sec =
            effective_command_timeout_sec(&command, requested, self.shell_timeout_sec);
        let started = Instant::now();

        {
            // Snapshot usage INTO the request, as the sidecar's protocol pins.
            let usage = { self.io.usage.lock().await.clone() };
            let mut out = self.io.output.lock().await;
            write_output(
                &mut *out,
                &OutputMessage::ToolRequest {
                    id: call.id.clone(),
                    command: command.clone(),
                    timeout_sec,
                    usage,
                },
            )
            .await
            .map_err(|e| ToolError {
                message: e.to_string(),
            })?;
            // The bridge now has usage for this round (b14).
            self.emitted_usage_this_round
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        let (return_code, stdout, stderr, error, next_working_directory) = {
            let mut input = self.io.input.lock().await;
            read_tool_result(&mut *input, &call.id)
                .await
                .map_err(|e| ToolError {
                    message: e.to_string(),
                })?
        };

        // The digest reads UNTRUNCATED streams; the model copy is truncated.
        self.history.lock().await.push(ToolHistoryEntry::new(
            command.clone(),
            return_code,
            stdout.clone(),
            stderr.clone(),
            error.clone(),
        ));

        let content = tool_result_content(return_code, &stdout, &stderr, error.as_deref());
        let (_, kind) = self.classify(call, args);
        Ok(ToolInvocationResult {
            is_error: error.is_some() || return_code.is_some_and(|c| c != 0),
            content,
            command,
            kind,
            return_code,
            stdout,
            stderr,
            error,
            next_working_directory,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// Swallows the desktop's `StreamEvent`s — the sidecar's wire vocabulary is
/// `tool_request` / `usage_snapshot` / `finished`. Its real job is `round_ended`:
/// upholding the bridge invariant that EVERY model round emits at least one
/// line carrying usage (b14). A round whose tool calls were all denied writes
/// no `tool_request`, so this fills the gap — which is why `round_ended` is
/// async: it genuinely writes to the shared stdout at the round boundary.
pub(crate) struct JsonlEventSink<R, W> {
    pub(crate) io: Arc<Jsonl<R, W>>,
    /// Set by the tool backend on each `tool_request`; cleared here per round.
    pub(crate) emitted_usage_this_round: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl<R, W> EventSink for JsonlEventSink<R, W>
where
    R: Send + Sync,
    W: AsyncWrite + Unpin + Send + Sync,
{
    fn emit(&self, _event: StreamEvent) {}

    async fn round_ended(&self) {
        use std::sync::atomic::Ordering;
        // If the backend already wrote a tool_request this round, the bridge
        // has its usage; otherwise emit the snapshot now.
        if self.emitted_usage_this_round.swap(false, Ordering::SeqCst) {
            return;
        }
        let usage = { self.io.usage.lock().await.clone() };
        let mut out = self.io.output.lock().await;
        let _ = write_output(
            &mut *out,
            &OutputMessage::UsageSnapshot {
                name: "usage_snapshot".to_string(),
                usage,
            },
        )
        .await;
    }
}

/// Stops the run when the wall-clock reserve is reached, and hands the loop the
/// live clock so the completion policy can use its time-based windows.
pub(crate) struct WallClockBudget {
    pub(crate) started: Instant,
    pub(crate) wall_time_budget_sec: Option<u64>,
    pub(crate) max_steps: usize,
}

impl WallClockBudget {
    /// `(remaining, total)` seconds — `None` when the run is untimed.
    pub(crate) fn remaining(&self) -> Option<(u64, u64)> {
        let total = self.wall_time_budget_sec?;
        let elapsed = self.started.elapsed().as_secs();
        Some((total.saturating_sub(elapsed), total))
    }
}

impl Budget for WallClockBudget {
    fn may_continue(&self, iteration: usize) -> bool {
        if iteration >= self.max_steps {
            return false;
        }
        // Same 30s reserve the sidecar's own loop used.
        !self
            .remaining()
            .is_some_and(|(remaining, _)| remaining <= 30)
    }

    fn wall_time(&self) -> Option<(u64, u64)> {
        self.remaining()
    }
}

/// The sidecar's `RuntimePolicy` (network + shell profile) as the loop's
/// permission seam, including its own denial wording.
pub(crate) struct SidecarPermissions {
    pub(crate) policy: RuntimePolicy,
}

#[async_trait::async_trait]
impl PermissionGateway for SidecarPermissions {
    async fn authorize(
        &self,
        _tool_call: &ToolCall,
        args: &serde_json::Value,
        _bash_command: Option<&str>,
    ) -> PermissionOutcome {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match self.policy.evaluate_command(command) {
            PolicyDecision::Allow => PermissionOutcome::Allow,
            PolicyDecision::Deny { rule, reason } => {
                PermissionOutcome::Deny(self.format_budget_denial(&rule, &reason))
            }
        }
    }

    /// The bridge's contract, not the desktop sentence.
    fn format_budget_denial(&self, rule: &str, reason: &str) -> String {
        tool_result_content(
            None,
            "",
            "",
            Some(&format!("policy denied command ({rule}): {reason}")),
        )
    }
}

/// The sidecar's char-budget digest, ported to canonical `ChatMessage`.
///
/// Deliberately NOT the desktop's token-based elision: this one is destructive
/// (everything between the preamble and the most recent tool-calling assistant
/// is replaced by a digest and never returns) and it triggers on the SERIALIZED
/// length of the whole array. Swapping in token-based compression would change
/// what the model sees and therefore move eval scores.
pub(crate) struct CharBudgetCompactor {
    pub(crate) max_chars: usize,
    pub(crate) history: Arc<Mutex<Vec<ToolHistoryEntry>>>,
}

impl ContextCompactor for CharBudgetCompactor {
    fn compact(
        &self,
        messages: Vec<ChatMessage>,
        _system_prompt: &str,
        _context_limit: u32,
    ) -> CompactionOutcome {
        let serialized_len = serde_json::to_string(&messages)
            .map(|s| s.len())
            .unwrap_or(0);
        if messages.len() <= 3 || serialized_len <= self.max_chars {
            return CompactionOutcome {
                messages,
                ..Default::default()
            };
        }
        // Anchor on the LAST assistant message carrying tool_calls, skipping the
        // [system, task] preamble; fall back to keeping the final message.
        let recent_start = messages
            .iter()
            .enumerate()
            .skip(2)
            .filter(|(_, m)| m.role == "assistant" && m.tool_calls.is_some())
            .map(|(i, _)| i)
            .next_back()
            .unwrap_or_else(|| messages.len().saturating_sub(2));

        let history = self.history.try_lock();
        let digest = match history {
            Ok(entries) => crate::compaction::history_digest(&entries),
            // Never block the loop on the digest; an empty one still compacts.
            Err(_) => String::new(),
        };

        let elided_count = recent_start.saturating_sub(2);
        let mut out: Vec<ChatMessage> = messages.iter().take(2).cloned().collect();
        out.push(ChatMessage {
            role: "user".into(),
            content: MessageContent::Text(digest),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        });
        out.extend(messages.into_iter().skip(recent_start));
        CompactionOutcome {
            messages: out,
            compacted: elided_count > 0,
            elided_count,
            tokens_freed: 0,
        }
    }
}

/// One model round for the sidecar: canonical `ChatMessage` history serializes
/// straight to the OpenAI chat-completions array it already sent, then
/// `request_model` applies the retry/backoff/fallback policy unchanged.
///
/// Owns the shared stdout, so it satisfies the bridge invariant itself on the
/// error path (b13): a round that fails still emits a `usage_snapshot` before
/// the error propagates, with no loop hook needed.
pub(crate) struct SidecarTransport<R, W> {
    pub(crate) io: Arc<Jsonl<R, W>>,
    pub(crate) client: reqwest::Client,
    pub(crate) endpoint: String,
    pub(crate) config: crate::protocol::StartConfig,
    pub(crate) started: Instant,
    /// Set by the tool backend when it writes a `tool_request`; the event sink
    /// clears it each round. Lets `round_ended` know whether the bridge already
    /// saw usage for this round (b14).
    pub(crate) emitted_usage_this_round: Arc<std::sync::atomic::AtomicBool>,
}

impl<R, W> SidecarTransport<R, W>
where
    W: AsyncWrite + Unpin + Send + Sync,
{
    async fn emit_usage_snapshot(&self) {
        let usage = { self.io.usage.lock().await.clone() };
        let mut out = self.io.output.lock().await;
        let _ = write_output(&mut *out, &OutputMessage::UsageSnapshot {
            name: "usage_snapshot".to_string(),
            usage,
        }).await;
        self.emitted_usage_this_round
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn wall_deadline(&self) -> Option<Instant> {
        self.config
            .wall_time_budget_sec
            .map(|total| self.started + std::time::Duration::from_secs(total))
    }
}

#[async_trait::async_trait]
impl<R, W> codefactory_agent_loop::transport::ModelTransport for SidecarTransport<R, W>
where
    R: Send + Sync,
    W: AsyncWrite + Unpin + Send + Sync,
{
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[codefactory_agent_loop::types::ToolDefinition],
        opts: &codefactory_agent_loop::transport::RoundOptions,
    ) -> Result<
        codefactory_agent_loop::transport::ModelResponse,
        codefactory_agent_loop::transport::TransportError,
    > {
        // ChatMessage IS the OpenAI chat-completions shape, so this is a direct
        // serialization rather than a translation.
        let wire: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null))
            .collect();

        // b15: the sidecar allows more attempts once work has been done —
        // abandoning a run that already produced tool outcomes costs more.
        let max_attempts = crate::transport::model_request_attempts(opts.tool_outcomes_so_far);
        let remaining = self
            .wall_deadline()
            .map(|d| d.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(self.config.model_timeout_sec);
        let attempt_timeout_sec = self.config.model_timeout_sec.min(remaining.max(1));

        let response = crate::transport::request_model(
            &self.client,
            &self.endpoint,
            &self.config,
            &wire,
            !tools.is_empty(),
            opts.require_tool,
            attempt_timeout_sec,
            max_attempts,
            self.wall_deadline(),
        )
        .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                // b13: the bridge needs usage for EVERY round, including one
                // that died. We own stdout, so emit before propagating.
                self.emit_usage_snapshot().await;
                return Err(codefactory_agent_loop::transport::TransportError::Fatal(
                    error.to_string(),
                ));
            }
        };

        {
            let mut usage = self.io.usage.lock().await;
            usage.add_response(&response);
        }

        let message = match response.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("message")) {
            Some(message) => message.clone(),
            None => {
                self.emit_usage_snapshot().await;
                return Err(codefactory_agent_loop::transport::TransportError::Fatal(
                    "model response did not contain choices[0].message".to_string(),
                ));
            }
        };

        let text = crate::compaction::message_content(&message);
        let parsed =
            match crate::transport::parse_tool_calls(&message, self.config.shell_timeout_sec) {
                Ok(calls) => calls,
                Err(error) => {
                    self.emit_usage_snapshot().await;
                    return Err(codefactory_agent_loop::transport::TransportError::Fatal(
                        error.to_string(),
                    ));
                }
            };
        // parse_tool_calls already validated + computed the per-call timeout;
        // carry both through the canonical shape so nothing is re-derived.
        let tool_calls: Vec<ToolCall> = parsed
            .into_iter()
            .map(|c| ToolCall {
                id: c.id,
                r#type: "function".into(),
                function: codefactory_agent_loop::types::FunctionCall {
                    name: "run_shell".into(),
                    arguments: serde_json::json!({
                        "command": c.command,
                        "timeout_sec": c.timeout_sec,
                    })
                    .to_string(),
                },
            })
            .collect();

        Ok(codefactory_agent_loop::transport::ModelResponse {
            text,
            tool_calls,
            // The sidecar accounts usage in its own cumulative `Usage` (which
            // carries `model_requests`); the loop's per-round row is a desktop
            // concern and would double-count here.
            usage: None,
            reasoning: None,
            // Desktop failover concepts; the sidecar has a single fixed route.
            effective_route: None,
            route_change: None,
        })
    }
}
