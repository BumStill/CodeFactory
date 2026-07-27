// SPDX-License-Identifier: Apache-2.0
//! The model-transport seam (keystone slice 4.2, refined + consumed in 4.5b).
//!
//! [`ModelTransport`] hides the three provider dialects (OpenAI, ChatGPT,
//! Anthropic — desktop) and the non-streaming buffered POST (sidecar) behind
//! one `complete()`. The system prompt travels as a system-role entry in
//! `messages` (each transport folds/extracts it as its API needs — OpenAI/ChatGPT
//! keep it inline, Anthropic pulls it into the top-level `system` field), so the
//! trait needs no separate `system_prompt` param. Each concrete transport OWNS
//! its own event sink and cancel handle (the desktop `DesktopModelTransport`
//! holds an `Arc<dyn EventSink>` + the shared cancel `Arc`), so streaming happens
//! inside `complete()` with no injected sink. The required-tool-choice→auto
//! fallback and the provider reactive retries live INSIDE `complete()`; context
//! compression stays in the loop (it mutates history).
//!
//! The desktop impl lands in slice 4.5b (`agent::model_transport`); the loop
//! switches its call sites onto `complete()` in slice 4.6.

use crate::types::{ChatMessage, ToolCall, ToolDefinition, Usage};

/// Per-round knobs resolved by the caller BEFORE the transport runs, so the
/// transport stays DB-pure (the ChatGPT `reasoning_effort` DB read was hoisted
/// into this in slice 4.4d). The cancel handle is NOT here — each transport owns
/// its own shared cancel `Arc`.
#[derive(Debug, Clone, Default)]
pub struct RoundOptions {
    /// Force a tool call this round (the gate's "you must act" contract). The
    /// transport downgrades to `auto` for providers that reject required choice.
    pub require_tool: bool,
    /// Pre-resolved reasoning effort (empty for api styles that ignore it, e.g.
    /// non-ChatGPT). Never read from a DB inside the transport.
    pub reasoning_effort: String,
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
    /// Actual endpoint/model that produced this round. Desktop failover can
    /// differ from the user-selected primary; sidecars may leave it `None`.
    pub effective_route: Option<EffectiveRoute>,
    /// Present only on the first successful response after an automatic route
    /// change. The loop persists this as a natural conversational notice.
    pub route_change: Option<RouteChange>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveRoute {
    pub endpoint_name: String,
    pub model_id: String,
    pub base_url: String,
    pub is_chatgpt: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteChange {
    pub from_endpoint: String,
    pub from_model: String,
    pub to_endpoint: String,
    pub to_model: String,
    pub reason: String,
    pub notice: String,
}

/// Transport failure taxonomy. Reactive strip-and-retry is already handled
/// inside `complete()`, so what surfaces here is the post-retry verdict.
#[derive(Debug, Clone)]
pub enum TransportError {
    /// Transient transport/gateway failure that exhausted in-transport retries.
    Retryable(String),
    /// Non-retryable failure (bad request, auth, etc.).
    Fatal(String),
}

impl TransportError {
    /// The raw provider/error message, without any decoration.
    pub fn message(&self) -> &str {
        match self {
            TransportError::Retryable(m) | TransportError::Fatal(m) => m,
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Emit the raw message verbatim: the loop greps it for context-overflow
        // / vision-rejection markers and surfaces it to the user, so a decorative
        // prefix must not pollute it.
        f.write_str(self.message())
    }
}

impl std::error::Error for TransportError {}

/// One model round. Object-safe so the loop can hold `Arc<dyn ModelTransport>`.
#[async_trait::async_trait]
pub trait ModelTransport: Send + Sync {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        opts: &RoundOptions,
    ) -> Result<ModelResponse, TransportError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Non-streaming stub: returns a canned response, emits nothing.
    struct StubTransport;

    #[async_trait::async_trait]
    impl ModelTransport for StubTransport {
        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ToolDefinition],
            opts: &RoundOptions,
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
        let resp = transport
            .complete(
                &[],
                &[],
                &RoundOptions {
                    require_tool: true,
                    reasoning_effort: String::new(),
                },
            )
            .await
            .expect("stub never errors");
        assert_eq!(resp.text, "require_tool=true");
    }

    #[test]
    fn transport_error_displays_the_raw_message_without_decoration() {
        let e: Box<dyn std::error::Error> = Box::new(TransportError::Fatal("boom".into()));
        assert_eq!(e.to_string(), "boom");
        assert_eq!(TransportError::Retryable("x".into()).message(), "x");
    }
}
