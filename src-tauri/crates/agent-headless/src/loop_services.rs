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
        let requested = ctx.timeout_sec.unwrap_or(self.shell_timeout_sec);
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

/// Emits nothing: the sidecar's wire vocabulary is `tool_request` /
/// `usage_snapshot` / `finished`, not the desktop's `StreamEvent`s. Holding the
/// shared stdout keeps ordering correct once usage snapshots are wired in.
pub(crate) struct JsonlEventSink;

impl EventSink for JsonlEventSink {
    fn emit(&self, _event: StreamEvent) {}
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
