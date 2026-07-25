// SPDX-License-Identifier: Apache-2.0
//! Desktop model transport (keystone slice 4.5a).
//!
//! The OpenAI-family transport — `call_openai_transport` (dispatch +
//! required→auto fallback), `call_chatgpt_model` (ChatGPT Responses SSE), and
//! `call_openai_model` (OpenAI-compatible chat SSE) — extracted verbatim out of
//! `AgentLoop` into `DesktopModelTransport`. This is a PURE mechanical move
//! (zero behaviour change): inherent methods, NOT the agent-loop `ModelTransport`
//! trait yet (that is slice 4.5b). The reactive retries (context-overflow,
//! vision-strip) stay in the loop wrapping the call; the `max_tokens →
//! max_completion_tokens` adaptation moves with `call_openai_model` (it is
//! internal to it).
//!
//! The struct owns only the transport's slice of `AgentLoop` state — no
//! `AppHandle` (#166), no `db`/`settings` (DB-pure since 4.4d; `reasoning_effort`
//! is a pre-resolved `&str` param). `cancel` is the SAME `Arc<AtomicBool>` the
//! loop holds (cloned, not fresh), so mid-stream and post-stream cancellation
//! observe identical state. Shared helpers live in the parent `agent` module and
//! are reached via `super::` (this is a child module, so it sees them).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use codefactory_agent_core::{provider_rejects_required_tool_choice, sanitize_completion_summary};
use reqwest::Client;

use codefactory_agent_loop::transport::{
    ModelResponse, ModelTransport, RoundOptions, TransportError,
};

use super::events::EventSink;
use super::{next_stream_item, openai_tool_controls, validate_openai_sse_completion, StreamPoll};
use crate::config::settings::ApiStyle;
use crate::errors::Result;
use crate::openrouter::types::{
    ChatMessage, ChatRequest, FunctionCall, StreamChunk, StreamEvent, StreamOptions, ToolCall,
    ToolDefinition, Usage,
};

/// The transport's slice of `AgentLoop` state. Built once per run by
/// `AgentLoop::model_transport` (clones only Arc/Client handles + small
/// Strings). Owns no `AppHandle` and no `db`/`settings`.
pub(super) struct DesktopModelTransport {
    pub(super) http: Client,
    pub(super) events: Arc<dyn EventSink>,
    pub(super) model_id: String,
    pub(super) session_id: String,
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) api_style: ApiStyle,
    pub(super) cancel: Option<Arc<AtomicBool>>,
}

impl DesktopModelTransport {
    /// Mirrors `AgentLoop::is_cancelled` exactly, off the SHARED cancel flag —
    /// so the cancellation-skips-SSE-validation invariant is byte-identical.
    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    pub(super) async fn call_openai_transport(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        require_tool: bool,
        reasoning_effort: &str,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>, Option<String>)> {
        let first = match self.api_style {
            ApiStyle::Chatgpt => {
                self.call_chatgpt_model(messages, tool_defs, require_tool, reasoning_effort)
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
                self.call_chatgpt_model(messages, tool_defs, false, reasoning_effort)
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
        // Pre-resolved by the loop once per round (keystone slice 4.4d), so this
        // transport reads no DB — a step toward the DB-pure ModelTransport (4.5).
        reasoning_effort: &str,
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
                "system" => instructions = super::AgentLoop::content_to_text(&m.content),
                "tool" => input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": super::AgentLoop::content_to_text(&m.content),
                })),
                "assistant" => {
                    let text = super::AgentLoop::content_to_text(&m.content);
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
                    "content": super::AgentLoop::content_to_chatgpt_user_parts(&m.content),
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
            "reasoning": { "effort": reasoning_effort, "summary": "auto" },
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
            |notice| super::AgentLoop::emit_transport_retry(self.events.as_ref(), notice),
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

        validate_openai_sse_completion(saw_terminal_marker, byte_buffer.len(), malformed_data_lines)
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
            |notice| super::AgentLoop::emit_transport_retry(self.events.as_ref(), notice),
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
                    |notice| super::AgentLoop::emit_transport_retry(self.events.as_ref(), notice),
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

        validate_openai_sse_completion(saw_terminal_marker, byte_buffer.len(), malformed_data_lines)
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
}

/// The agent-loop `ModelTransport` seam (keystone slice 4.5b). Wraps the
/// inherent `call_openai_transport` (which the loop still calls directly until
/// slice 4.6 switches it onto `complete`): maps the internal 4-tuple to
/// [`ModelResponse`] and the bin's `AppError` to a fatal [`TransportError`]
/// (message preserved verbatim). `RoundOptions` supplies the per-round
/// `require_tool` + pre-resolved `reasoning_effort`; the sink and cancel handle
/// are the transport's own fields, so `complete` needs neither as a param.
impl DesktopModelTransport {
    /// One Anthropic round (keystone slice 4.7): convert canonical `ChatMessage`
    /// history to the Anthropic wire at the edge, run `stream_anthropic`, and
    /// apply the required→auto tool-choice fallback (moved here from
    /// `AgentLoop::call_anthropic_transport` so the shared loop, which only calls
    /// `complete`, keeps it). `reasoning_effort` is ignored for Anthropic.
    async fn call_anthropic_model(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        require_tool: bool,
    ) -> Result<super::anthropic_client::AnthropicResponse> {
        let (system, wire_messages) = chat_messages_to_anthropic(messages);
        let first = super::anthropic_client::stream_anthropic(
            &self.http,
            &self.base_url,
            &self.api_key,
            &self.model_id,
            &system,
            wire_messages.clone(),
            tools,
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
        super::anthropic_client::stream_anthropic(
            &self.http,
            &self.base_url,
            &self.api_key,
            &self.model_id,
            &system,
            wire_messages,
            tools,
            false,
            self.cancel.as_ref(),
            self.events.as_ref(),
        )
        .await
    }
}

/// Map the Anthropic streamed answer onto the provider-independent
/// `ModelResponse` (keystone slice 4.7). Usage is gated on `(input>0||output>0)`
/// — the same guard `record_usage_event_for_round` uses; `reasoning` is `None`
/// (Anthropic thinking is neither requested nor parsed); `tool_calls` are
/// cleared on cancel for parity with the OpenAI path (unobservable — the loop
/// breaks first); the per-response `cancelled` bool is dropped (cancellation
/// flows through the shared `Arc`).
fn anthropic_response_to_model_response(
    resp: super::anthropic_client::AnthropicResponse,
) -> ModelResponse {
    let usage = (resp.input_tokens > 0 || resp.output_tokens > 0).then(|| {
        let input = resp.input_tokens.max(0) as u32;
        let output = resp.output_tokens.max(0) as u32;
        codefactory_agent_loop::types::Usage {
            prompt_tokens: input,
            completion_tokens: output,
            total_tokens: input + output,
            cost: None,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        }
    });
    let tool_calls = if resp.cancelled {
        Vec::new()
    } else {
        resp.tool_calls
    };
    ModelResponse {
        text: resp.text,
        tool_calls,
        usage,
        reasoning: None,
    }
}

#[async_trait::async_trait]
impl ModelTransport for DesktopModelTransport {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        opts: &RoundOptions,
    ) -> std::result::Result<ModelResponse, TransportError> {
        // Dispatch on the provider dialect. The OpenAI path's `_ =>` would
        // silently mis-route Anthropic to call_openai_model — MUST branch here.
        match self.api_style {
            ApiStyle::Anthropic => {
                let resp = self
                    .call_anthropic_model(messages, tools, opts.require_tool)
                    .await
                    .map_err(|e| TransportError::Fatal(e.to_string()))?;
                Ok(anthropic_response_to_model_response(resp))
            }
            _ => {
                let (text, tool_calls, usage, reasoning) = self
                    .call_openai_transport(messages, tools, opts.require_tool, &opts.reasoning_effort)
                    .await
                    .map_err(|e| TransportError::Fatal(e.to_string()))?;
                Ok(ModelResponse {
                    text,
                    tool_calls,
                    usage,
                    reasoning,
                })
            }
        }
    }
}

/// Convert canonical `ChatMessage` history into the Anthropic wire shape
/// (keystone slice 4.7): returns the extracted top-level `system` string and the
/// `messages` array. The INVERSE of the old `build_anthropic_messages` plus the
/// live tool_result batching — the single representation boundary for Anthropic,
/// EDGE-only (`run_agent_loop` never sees `Value`).
///
/// - a leading `role:"system"` ChatMessage → the `system` string (not emitted);
/// - a maximal run of consecutive `role:"tool"` ChatMessages → ONE
///   `{role:"user", content:[tool_result…]}` (the deliberate non-transparent
///   merge — matches the live loop's already-batched shape);
/// - assistant → text block if non-empty, then `tool_use` blocks
///   (`input = from_str(args).unwrap_or({})`); empty-both → `[{text:""}]`;
/// - user `Text` → a bare JSON string; user `Parts` → `[text | image]` blocks,
///   `image_url` data-URLs split back to `{type:image, source:{base64,…}}`.
fn chat_messages_to_anthropic(
    messages: &[codefactory_agent_loop::types::ChatMessage],
) -> (String, Vec<serde_json::Value>) {
    use codefactory_agent_loop::types::MessageContent;
    let mut system = String::new();
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let m = &messages[i];
        match m.role.as_str() {
            "system" => {
                system = super::AgentLoop::content_to_text(&m.content);
                i += 1;
            }
            "tool" => {
                // Merge the maximal run of consecutive tool messages into ONE
                // user message of N tool_result blocks (never absorb a following
                // non-tool message).
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                while i < messages.len() && messages[i].role == "tool" {
                    let tm = &messages[i];
                    blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tm.tool_call_id.clone().unwrap_or_default(),
                        "content": super::AgentLoop::content_to_text(&tm.content),
                    }));
                    i += 1;
                }
                out.push(serde_json::json!({ "role": "user", "content": blocks }));
            }
            "assistant" => {
                let mut content_blocks: Vec<serde_json::Value> = Vec::new();
                let text = super::AgentLoop::content_to_text(&m.content);
                if !text.is_empty() {
                    content_blocks.push(serde_json::json!({ "type": "text", "text": text }));
                }
                for tc in m.tool_calls.as_deref().unwrap_or_default() {
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
                out.push(serde_json::json!({ "role": "assistant", "content": content_blocks }));
                i += 1;
            }
            _ => {
                let content = match &m.content {
                    MessageContent::Text(s) => serde_json::Value::String(s.clone()),
                    MessageContent::Parts(parts) => {
                        let blocks: Vec<serde_json::Value> = parts
                            .iter()
                            .map(|p| {
                                if p.r#type == "image_url" {
                                    let url =
                                        p.image_url.as_ref().map(|u| u.url.as_str()).unwrap_or("");
                                    match parse_data_url(url) {
                                        Some((media_type, data)) => serde_json::json!({
                                            "type": "image",
                                            "source": {
                                                "type": "base64",
                                                "media_type": media_type,
                                                "data": data,
                                            },
                                        }),
                                        // Defensive: a non-data image_url (raw http)
                                        // is never produced by attachments today, but
                                        // must degrade without corrupting the request.
                                        None => serde_json::json!({
                                            "type": "image",
                                            "source": { "type": "url", "url": url },
                                        }),
                                    }
                                } else {
                                    serde_json::json!({
                                        "type": "text",
                                        "text": p.text.clone().unwrap_or_default(),
                                    })
                                }
                            })
                            .collect();
                        serde_json::Value::Array(blocks)
                    }
                };
                out.push(serde_json::json!({ "role": m.role, "content": content }));
                i += 1;
            }
        }
    }
    (system, out)
}

/// Split a `data:<media_type>;base64,<data>` URL into `(media_type, data)`.
fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(";base64,")?;
    Some((media_type.to_string(), data.to_string()))
}

#[cfg(test)]
mod tests {
    //! `DesktopModelTransport` owns no `AppHandle`, so it constructs from bare
    //! handles in a test (#166-safe). We prove the `ModelTransport` trait is
    //! satisfied and object-safe (`Arc<dyn ModelTransport>`); `complete` itself
    //! is a network call, exercised end-to-end by the desktop app, not here.
    use super::*;
    use codefactory_agent_loop::types::{
        ChatMessage, ContentPart, FunctionCall, ImageUrl, MessageContent, ToolCall,
    };
    use serde_json::json;

    // ── chat_messages_to_anthropic golden pins (keystone slice 4.7 step 4) ──
    // The Anthropic representation switch is fully pinned HERE, before it is
    // wired, so any drift in the edge conversion fails a unit test.

    fn cm(role: &str, text: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Text(text.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }
    fn tool_cm(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: "tool".into(),
            content: MessageContent::Text(content.into()),
            tool_calls: None,
            tool_call_id: Some(id.into()),
            name: None,
            reasoning_content: None,
        }
    }
    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            r#type: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: args.into(),
            },
        }
    }

    #[test]
    fn edge_extracts_leading_system_and_emits_no_system_message() {
        let (system, msgs) = chat_messages_to_anthropic(&[cm("system", "you are helpful"), cm("user", "hi")]);
        assert_eq!(system, "you are helpful");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hi");
    }

    #[test]
    fn edge_assistant_text_only() {
        let (_, msgs) = chat_messages_to_anthropic(&[cm("assistant", "hello")]);
        assert_eq!(msgs[0]["content"], json!([{"type":"text","text":"hello"}]));
    }

    #[test]
    fn edge_assistant_tool_only_has_no_text_block() {
        let mut a = cm("assistant", "");
        a.tool_calls = Some(vec![call("t1", "bash", r#"{"cmd":"ls"}"#)]);
        let (_, msgs) = chat_messages_to_anthropic(&[a]);
        assert_eq!(
            msgs[0]["content"],
            json!([{"type":"tool_use","id":"t1","name":"bash","input":{"cmd":"ls"}}])
        );
    }

    #[test]
    fn edge_assistant_text_then_tool_preserves_order() {
        let mut a = cm("assistant", "running it");
        a.tool_calls = Some(vec![call("t1", "bash", r#"{"cmd":"ls"}"#)]);
        let (_, msgs) = chat_messages_to_anthropic(&[a]);
        assert_eq!(
            msgs[0]["content"],
            json!([
                {"type":"text","text":"running it"},
                {"type":"tool_use","id":"t1","name":"bash","input":{"cmd":"ls"}},
            ])
        );
    }

    #[test]
    fn edge_assistant_empty_both_gets_placeholder_text_block() {
        let (_, msgs) = chat_messages_to_anthropic(&[cm("assistant", "")]);
        assert_eq!(msgs[0]["content"], json!([{"type":"text","text":""}]));
    }

    #[test]
    fn edge_malformed_tool_args_become_empty_object() {
        let mut a = cm("assistant", "");
        a.tool_calls = Some(vec![call("t1", "bash", "not json")]);
        let (_, msgs) = chat_messages_to_anthropic(&[a]);
        assert_eq!(msgs[0]["content"][0]["input"], json!({}));
    }

    #[test]
    fn edge_merges_consecutive_tool_results_into_one_user_message() {
        // THE deliberate non-transparent merge: N tool rows → ONE user message
        // of N tool_result blocks (matches the live loop's batched shape).
        let (_, msgs) = chat_messages_to_anthropic(&[tool_cm("t1", "ok1"), tool_cm("t2", "ok2")]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(
            msgs[0]["content"],
            json!([
                {"type":"tool_result","tool_use_id":"t1","content":"ok1"},
                {"type":"tool_result","tool_use_id":"t2","content":"ok2"},
            ])
        );
    }

    #[test]
    fn edge_tool_run_then_user_progress_stays_two_user_messages() {
        // The merge must NEVER absorb a following non-tool message.
        let (_, msgs) = chat_messages_to_anthropic(&[tool_cm("t1", "ok"), cm("user", "continue")]);
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0]["content"],
            json!([{"type":"tool_result","tool_use_id":"t1","content":"ok"}])
        );
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "continue");
    }

    #[test]
    fn edge_user_image_data_url_becomes_base64_source() {
        let user = ChatMessage {
            role: "user".into(),
            content: MessageContent::Parts(vec![
                ContentPart {
                    r#type: "text".into(),
                    text: Some("look".into()),
                    image_url: None,
                },
                ContentPart {
                    r#type: "image_url".into(),
                    text: None,
                    image_url: Some(ImageUrl {
                        url: "data:image/png;base64,AAAB".into(),
                    }),
                },
            ]),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        };
        let (_, msgs) = chat_messages_to_anthropic(&[user]);
        assert_eq!(
            msgs[0]["content"],
            json!([
                {"type":"text","text":"look"},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAB"}},
            ])
        );
    }

    #[test]
    fn edge_user_text_is_a_bare_json_string() {
        let (_, msgs) = chat_messages_to_anthropic(&[cm("user", "just text")]);
        assert_eq!(msgs[0]["content"], serde_json::Value::String("just text".into()));
    }

    #[test]
    fn anthropic_response_maps_usage_reasoning_and_cancel() {
        use crate::agent::anthropic_client::AnthropicResponse;
        // input/output>0 → Some usage; reasoning always None; tool_calls kept.
        let mr = anthropic_response_to_model_response(AnthropicResponse {
            text: "hi".into(),
            tool_calls: vec![call("t1", "bash", "{}")],
            input_tokens: 5,
            output_tokens: 7,
            cancelled: false,
        });
        assert_eq!(mr.text, "hi");
        assert_eq!(mr.tool_calls.len(), 1);
        assert!(mr.reasoning.is_none());
        let u = mr.usage.expect("usage present when tokens > 0");
        assert_eq!((u.prompt_tokens, u.completion_tokens, u.total_tokens), (5, 7, 12));
        // (0,0) tokens → None (matches the record_usage guard).
        let none = anthropic_response_to_model_response(AnthropicResponse {
            text: String::new(),
            tool_calls: vec![],
            input_tokens: 0,
            output_tokens: 0,
            cancelled: false,
        });
        assert!(none.usage.is_none());
        // cancelled → tool_calls cleared (OpenAI parity, unobservable).
        let cancelled = anthropic_response_to_model_response(AnthropicResponse {
            text: String::new(),
            tool_calls: vec![call("t1", "bash", "{}")],
            input_tokens: 1,
            output_tokens: 1,
            cancelled: true,
        });
        assert!(cancelled.tool_calls.is_empty());
    }
    use std::sync::Arc;

    fn transport() -> DesktopModelTransport {
        DesktopModelTransport {
            http: Client::new(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            model_id: "m".into(),
            session_id: "s".into(),
            base_url: "http://127.0.0.1:0".into(),
            api_key: "k".into(),
            api_style: ApiStyle::Openai,
            cancel: None,
        }
    }

    #[test]
    fn desktop_transport_is_object_safe_as_dyn_model_transport() {
        // The shared loop (4.6) holds Arc<dyn ModelTransport>; prove the desktop
        // impl coerces and constructs with no AppHandle.
        let _t: Arc<dyn ModelTransport> = Arc::new(transport());
    }
}
