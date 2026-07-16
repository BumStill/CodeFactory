// SPDX-License-Identifier: Apache-2.0
//! Streaming client for the Anthropic Messages API.

use codefactory_agent_core::sanitize_completion_summary;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use super::{next_stream_item, StreamPoll};
use crate::errors::Result;
use crate::openrouter::types::{FunctionCall, StreamEvent, ToolCall, ToolDefinition};

/// Convert OpenAI-style tool definitions to Anthropic format.
pub fn openai_tools_to_anthropic(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.function.name,
                "description": t.function.description,
                "input_schema": t.function.parameters,
            })
        })
        .collect()
}

pub struct AnthropicResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cancelled: bool,
}

pub async fn stream_anthropic(
    http: &Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    messages: Vec<serde_json::Value>,
    tools: &[ToolDefinition],
    cancel: Option<&Arc<AtomicBool>>,
    app_handle: &AppHandle,
    event_name: &str,
) -> Result<AnthropicResponse> {
    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));

    // Same prefix-stripping safeguard as the OpenAI path — see normalize_model_id
    // doc-comment for why this is here.
    let model = crate::config::settings::normalize_model_id(model, base_url);

    let anthropic_tools = openai_tools_to_anthropic(tools);
    let finalization_response = anthropic_tools.is_empty();

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": 8096,
        "system": system_prompt,
        "messages": messages,
        "stream": true,
    });
    if !anthropic_tools.is_empty() {
        body["tools"] = serde_json::Value::Array(anthropic_tools);
    }

    let response = http
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;
    let response = crate::http_util::check_status(response).await?;

    let mut byte_stream = response.bytes_stream();

    let mut text_buf = String::new();
    // tool call accumulation: index -> (id, name, input_json_buf)
    let mut tool_map: HashMap<usize, (String, String, String)> = HashMap::new();
    // current block index and type
    let mut current_block_idx: usize = 0;
    let mut current_block_type: Option<String> = None;
    // token usage (populated from message_start / message_delta events)
    let mut input_tokens: i64 = 0;
    let mut output_tokens: i64 = 0;
    let mut cancelled = false;

    // SSE line buffering — see agent/mod.rs for full rationale.
    // TL;DR: SSE events split across TCP chunks must be reassembled, or
    // every cross-chunk event gets silently dropped, corrupting tool args.
    let mut byte_buffer: Vec<u8> = Vec::with_capacity(4096);

    loop {
        let chunk = match next_stream_item(&mut byte_stream, cancel).await {
            StreamPoll::Item(Some(chunk)) => chunk,
            StreamPoll::Item(None) => break,
            StreamPoll::Cancelled => {
                cancelled = true;
                break;
            }
        };
        let bytes = chunk?;
        byte_buffer.extend_from_slice(&bytes);

        while let Some(nl_pos) = byte_buffer.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = byte_buffer.drain(..=nl_pos).collect();
            let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
            let line = line.trim_end_matches('\r');

            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }

            let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                tracing::warn!("dropped malformed Anthropic SSE event (len={})", data.len());
                continue;
            };

            let event_type = event
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            match event_type.as_str() {
                "content_block_start" => {
                    let idx = event.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    current_block_idx = idx;

                    let block_type = event
                        .pointer("/content_block/type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    current_block_type = Some(block_type.clone());

                    if block_type == "tool_use" {
                        let id = event
                            .pointer("/content_block/id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = event
                            .pointer("/content_block/name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        tool_map.insert(idx, (id, name, String::new()));
                    }
                }
                "content_block_delta" => {
                    let delta_type = event
                        .pointer("/delta/type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    match delta_type.as_str() {
                        "text_delta" => {
                            let text = event
                                .pointer("/delta/text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !text.is_empty() {
                                if !finalization_response {
                                    app_handle
                                        .emit(
                                            event_name,
                                            StreamEvent::TextDelta {
                                                content: text.clone(),
                                            },
                                        )
                                        .ok();
                                }
                                text_buf.push_str(&text);
                            }
                        }
                        "input_json_delta" => {
                            let partial = event
                                .pointer("/delta/partial_json")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if let Some(entry) = tool_map.get_mut(&current_block_idx) {
                                entry.2.push_str(&partial);
                            }
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    // When a tool_use block stops, emit ToolCallStart with accumulated data.
                    if current_block_type.as_deref() == Some("tool_use") {
                        if let Some((id, name, args_json)) = tool_map.get(&current_block_idx) {
                            let args: serde_json::Value =
                                serde_json::from_str(args_json).unwrap_or(serde_json::json!({}));
                            app_handle
                                .emit(
                                    event_name,
                                    StreamEvent::ToolCallStart {
                                        id: id.clone(),
                                        name: name.clone(),
                                        args,
                                    },
                                )
                                .ok();
                        }
                    }
                }
                "message_start" => {
                    // {"type":"message_start","message":{"usage":{"input_tokens":N,...}}}
                    if let Some(u) = event
                        .pointer("/message/usage/input_tokens")
                        .and_then(|v| v.as_i64())
                    {
                        input_tokens = u;
                    }
                }
                "message_delta" => {
                    // {"type":"message_delta","usage":{"output_tokens":N}}
                    if let Some(u) = event
                        .pointer("/usage/output_tokens")
                        .and_then(|v| v.as_i64())
                    {
                        output_tokens = u;
                    }
                }
                "message_stop" => {
                    // Nothing extra needed.
                }
                _ => {}
            }
        }
    }

    // Build final tool_calls vec
    let mut tool_calls: Vec<ToolCall> = tool_map
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
        app_handle
            .emit(
                event_name,
                StreamEvent::TextDelta {
                    content: text_buf.clone(),
                },
            )
            .ok();
    }

    Ok(AnthropicResponse {
        text: text_buf,
        tool_calls,
        input_tokens,
        output_tokens,
        cancelled,
    })
}
