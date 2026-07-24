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
/// instead of forked per copy.
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
        let desktop = RunConfig {
            finalization: FinalizationPolicy::ReleaseWithWarning,
            gate_benchmark: false,
            progress_window: 8,
            recovery_limit: 3,
            max_iterations: 30,
        };
        let sidecar = RunConfig {
            finalization: FinalizationPolicy::Benchmark,
            gate_benchmark: true,
            progress_window: 4,
            recovery_limit: 1,
            max_iterations: 80,
        };
        // The whole point: these live as data, not as two forked code paths.
        assert!(!desktop.gate_benchmark && desktop.progress_window == 8);
        assert!(sidecar.gate_benchmark && sidecar.progress_window == 4);
    }
}
