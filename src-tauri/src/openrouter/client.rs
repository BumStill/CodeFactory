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
        let resp: ModelsResponse = crate::http_util::check_status(response).await?.json().await?;
        Ok(resp.data)
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

trait ArgsChunk {
    fn args_chunk(&self) -> Option<String>;
}

impl ArgsChunk for FunctionDelta {
    fn args_chunk(&self) -> Option<String> {
        self.arguments.clone()
    }
}
