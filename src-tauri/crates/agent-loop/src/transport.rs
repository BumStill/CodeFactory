// SPDX-License-Identifier: Apache-2.0
//! The model-transport seam (keystone slice 4.2).
//!
//! [`ModelTransport`] hides the three provider dialects (OpenAI, ChatGPT,
//! Anthropic — desktop) and the non-streaming buffered POST (sidecar) behind
//! one call. `system_prompt` is a SEPARATE parameter because Anthropic passes
//! it as a top-level field while OpenAI/sidecar fold it into a system message.
//! The required-tool-choice→auto fallback and the provider-specific reactive
//! retries (vision-strip / context-overflow / overload) live INSIDE the
//! concrete `complete()`; context compression stays in the loop (it mutates
//! history). Streaming transports emit `TextDelta`/`ToolCallStart` through the
//! injected `EventSink` mid-call; the sidecar transport emits nothing.
//!
//! Nothing consumes this yet; the desktop impl lands in slice 4.5.

use crate::events::EventSink;
use crate::types::{ChatMessage, ToolCall, ToolDefinition, Usage};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Per-round knobs resolved by the caller BEFORE the transport runs, so the
/// transport stays DB-pure (e.g. the ChatGPT `reasoning_effort` DB read is
/// hoisted out of the transport into this struct — hardest-problem #5).
#[derive(Debug, Clone, Default)]
pub struct RoundOptions {
    /// Force a tool call this round (the gate's "you must act" contract). The
    /// wrapper downgrades to `auto` for providers that reject required choice.
    pub require_tool: bool,
    /// Pre-resolved reasoning effort (never read from a DB inside the transport).
    pub reasoning_effort: Option<String>,
    /// Cooperative between-round cancel; `None` for non-chat runs.
    pub cancel: Option<Arc<AtomicBool>>,
}

/// The canonical model answer, provider-independent. (Not `Clone`: `Usage` is
/// a deserialize-only wire type; the loop consumes a `ModelResponse` once.)
#[derive(Debug, Default)]
pub struct ModelResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    /// Reasoning trace (thinking-mode models) to persist/replay verbatim.
    pub reasoning: Option<String>,
}

/// Transport failure taxonomy. The loop distinguishes a retry-worthy transport
/// hiccup from a fatal error; reactive strip-and-retry is already handled
/// inside `complete()`, so what surfaces here is the post-retry verdict.
#[derive(Debug, Clone)]
pub enum TransportError {
    /// Transient transport/gateway failure that exhausted in-transport retries.
    Retryable(String),
    /// Non-retryable failure (bad request, auth, etc.).
    Fatal(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Retryable(m) => write!(f, "retryable transport error: {m}"),
            TransportError::Fatal(m) => write!(f, "fatal transport error: {m}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// One model round. Object-safe so the loop can hold `Arc<dyn ModelTransport>`.
#[async_trait::async_trait]
pub trait ModelTransport: Send + Sync {
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        opts: &RoundOptions,
        events: &dyn EventSink,
    ) -> Result<ModelResponse, TransportError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::CollectingEventSink;

    /// Non-streaming stub: returns a canned response, emits nothing — mirrors
    /// the sidecar transport's shape.
    struct StubTransport;

    #[async_trait::async_trait]
    impl ModelTransport for StubTransport {
        async fn complete(
            &self,
            _system_prompt: &str,
            _messages: &[ChatMessage],
            _tools: &[ToolDefinition],
            opts: &RoundOptions,
            _events: &dyn EventSink,
        ) -> Result<ModelResponse, TransportError> {
            Ok(ModelResponse {
                text: format!("require_tool={}", opts.require_tool),
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn model_transport_is_object_safe_and_dispatches() {
        let transport: Arc<dyn ModelTransport> = Arc::new(StubTransport);
        let sink = CollectingEventSink::new();
        let resp = transport
            .complete(
                "sys",
                &[],
                &[],
                &RoundOptions {
                    require_tool: true,
                    ..Default::default()
                },
                &sink,
            )
            .await
            .expect("stub never errors");
        assert_eq!(resp.text, "require_tool=true");
        assert!(sink.events().is_empty());
    }

    #[test]
    fn transport_error_is_a_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(TransportError::Fatal("boom".into()));
        assert!(e.to_string().contains("boom"));
    }
}
