// SPDX-License-Identifier: Apache-2.0
use futures_util::StreamExt;
use reqwest::Client;
use tauri::{AppHandle, Emitter};

use super::types::*;
use crate::errors::Result;

pub struct OpenRouterClient {
    http: Client,
    base_url: String,
    api_key: String,
}

/// Minimal OpenAI-compatible models response (OpenAI API shape), used as a
/// fallback when the OpenRouter-shaped `ModelsResponse` fails to deserialize.
#[derive(Debug, serde::Deserialize)]
struct OpenAiModelListFallback {
    data: Vec<OpenAiModelEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

impl OpenRouterClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
        }
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/models", self.base_url);
        let response = self
            .http
            .get(&url)
            .bearer_auth(&self.api_key)
            .header("X-Title", "CodeFactory")
            .send()
            .await?;
        let response = crate::http_util::check_status(response).await?;
        let body = response.text().await?;
        let mut models = parse_models_from_body(&body)?;
        // R1: Only chat-capable models — filter known non-chat kinds by id hints.
        models.retain(|m| is_chat_capable(&m.id));
        Ok(models)
    }

    /// Stream a chat completion, emitting `StreamEvent`s on the Tauri event bus.
    /// `session_id` is used as the event channel name so the frontend can subscribe.
    // Scaffolding: full streaming path, not yet wired into the live agent loop
    // (which uses non-streaming completions). This `#[allow]` cascades to keep the
    // trait/events it alone uses — `ArgsChunk` and the `ToolCallArgsDelta` /
    // `ToolCallEnd` stream variants.
    #[allow(dead_code)]
    pub async fn stream_chat(
        &self,
        app: AppHandle,
        session_id: &str,
        request: ChatRequest,
    ) -> Result<()> {
        let url = format!("{}/chat/completions", self.base_url);
        let event_name = format!("stream:{session_id}");

        let body = serde_json::to_value(&request)?;
        let response = crate::http_util::send_with_retry_and_notify(
            "OpenRouter stream request",
            || {
                self.http
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .header("X-Title", "CodeFactory")
                    .header("Content-Type", "application/json")
                    .json(&body)
            },
            |notice| {
                app.emit(
                    &event_name,
                    StreamEvent::TransportRetry {
                        label: notice.label,
                        attempt: notice.attempt as u32,
                        max_attempts: notice.max_attempts as u32,
                        delay_ms: notice.delay.as_millis() as u64,
                        reason: notice.reason,
                    },
                )
                .ok();
            },
        )
        .await?;
        let response = crate::http_util::check_status(response).await?;

        let mut stream = response.bytes_stream();

        // In-flight tool call accumulator: index → (id, name, args_buf)
        let mut tool_calls: std::collections::HashMap<u32, (String, String, String)> =
            std::collections::HashMap::new();

        let mut input_tokens = 0u32;
        let mut output_tokens = 0u32;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            let text = String::from_utf8_lossy(&bytes);

            for line in text.lines() {
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    app.emit(
                        &event_name,
                        StreamEvent::Done {
                            input_tokens,
                            output_tokens,
                        },
                    )
                    .ok();
                    return Ok(());
                }

                let Ok(sc) = serde_json::from_str::<StreamChunk>(data) else {
                    continue;
                };

                if let Some(usage) = sc.usage {
                    input_tokens = usage.prompt_tokens;
                    output_tokens = usage.completion_tokens;
                }

                for choice in sc.choices {
                    let delta = choice.delta;

                    if let Some(text) = delta.content {
                        if !text.is_empty() {
                            app.emit(&event_name, StreamEvent::TextDelta { content: text })
                                .ok();
                        }
                    }

                    if let Some(tc_deltas) = delta.tool_calls {
                        for tc in tc_deltas {
                            let entry = tool_calls.entry(tc.index).or_default();
                            if let Some(id) = tc.id {
                                entry.0 = id.clone();
                                // first fragment that has both id and name = start event
                                if let Some(f) = &tc.function {
                                    if let Some(name) = &f.name {
                                        entry.1 = name.clone();
                                        app.emit(
                                            &event_name,
                                            StreamEvent::ToolCallStart {
                                                id,
                                                name: name.clone(),
                                                args: serde_json::Value::Null,
                                            },
                                        )
                                        .ok();
                                    }
                                }
                            }
                            if let Some(f) = tc.function {
                                if let Some(args) = f.args_chunk() {
                                    entry.2.push_str(&args);
                                    app.emit(
                                        &event_name,
                                        StreamEvent::ToolCallArgsDelta {
                                            index: tc.index,
                                            chunk: args,
                                        },
                                    )
                                    .ok();
                                }
                            }

                            if choice.finish_reason.as_deref() == Some("tool_calls") {
                                app.emit(&event_name, StreamEvent::ToolCallEnd { index: tc.index })
                                    .ok();
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Try parsing the model list using the OpenRouter/compatible `ModelsResponse`,
/// then fall back to the OpenAI API `/v1/models` shape. Returns a minimal
/// `ModelInfo` vector on fallback (id-only; name=id; zero context; no pricing).
pub(crate) fn parse_models_from_body(body: &str) -> Result<Vec<ModelInfo>> {
    // First: try the OpenRouter/compatible shape.
    if let Ok(parsed) = serde_json::from_str::<ModelsResponse>(body) {
        return Ok(parsed.data);
    }
    // Fallback: OpenAI API shape: {"object":"list","data":[{"id":"gpt-..."}, ...]}
    let parsed: OpenAiModelListFallback = serde_json::from_str(body)?;
    let models = parsed
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id.clone(),
            name: m.id,
            context_length: 0,
            pricing: None,
            supported_parameters: None,
            is_custom: false,
        })
        .collect();
    Ok(models)
}

fn is_chat_capable(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    // Heuristic: exclude embeddings, audio-only, and fine-tune job ids.
    if lower.contains("embedding") || lower.contains("embeddings") {
        return false;
    }
    if lower.contains("whisper") || lower.contains("tts") || lower.contains("audio-") {
        return false;
    }
    if lower.contains("fine-tune") || lower.contains("ft:") {
        return false;
    }
    true
}

trait ArgsChunk {
    fn args_chunk(&self) -> Option<String>;
}

impl ArgsChunk for FunctionDelta {
    fn args_chunk(&self) -> Option<String> {
        self.arguments.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_openrouter_models_response() {
        let body = r#"{
            "data": [
                {
                    "id": "deepseek-v4-pro",
                    "name": "DeepSeek V4 Pro",
                    "context_length": 131072,
                    "pricing": {"prompt": "$1.0/million", "completion": "$2.0/million"},
                    "supported_parameters": ["temperature"],
                    "is_custom": false
                }
            ]
        }"#;
        let models = parse_models_from_body(body).expect("should parse openrouter shape");
        assert_eq!(models.len(), 1);
        let m = &models[0];
        assert_eq!(m.id, "deepseek-v4-pro");
        assert_eq!(m.name, "DeepSeek V4 Pro");
        assert_eq!(m.context_length, 131072);
        assert!(!m.is_custom);
    }

    #[test]
    fn parse_openai_models_response_minimal_mapping() {
        let body = r#"{
            "object": "list",
            "data": [
                {"id": "gpt-4o-mini", "object": "model"},
                {"id": "o3-mini-high", "object": "model"}
            ]
        }"#;
        let models = parse_models_from_body(body).expect("should parse openai shape");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o-mini");
        assert_eq!(models[0].name, "gpt-4o-mini");
        assert_eq!(models[0].context_length, 0);
        assert!(models[0].pricing.is_none());
        assert_eq!(models[1].id, "o3-mini-high");
    }
}
