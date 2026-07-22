// SPDX-License-Identifier: Apache-2.0
//! Generic one-shot text generation used by internal agent capabilities.
//!
//! This transport is deliberately independent from any product surface. It is
//! shared by learning and subagent verification without creating an app-owned
//! specification or planning workflow.

use reqwest::Client;
use serde::Serialize;

use crate::config::settings::ApiStyle;

#[derive(Clone, Serialize)]
pub(crate) struct AiMessage {
    pub(crate) role: String,
    pub(crate) content: String,
}

#[derive(Debug)]
pub(crate) struct OneShotRequest {
    pub(crate) url: String,
    pub(crate) body: serde_json::Value,
}

pub(crate) fn build_one_shot_request(
    base_url: &str,
    model: &str,
    api_style: &ApiStyle,
    messages: Vec<AiMessage>,
    max_tokens: u32,
    temperature: f32,
) -> Result<OneShotRequest, String> {
    let model = crate::config::settings::normalize_model_id(model, base_url);
    let base_url = base_url.trim_end_matches('/');
    match api_style {
        ApiStyle::Openai => Ok(OneShotRequest {
            url: format!("{base_url}/chat/completions"),
            body: serde_json::json!({
                "model": model,
                "messages": messages,
                "stream": false,
                "temperature": temperature,
                "max_tokens": max_tokens,
            }),
        }),
        ApiStyle::Anthropic => {
            let mut system = Vec::new();
            let mut conversation = Vec::new();
            for message in messages {
                if message.role == "system" {
                    system.push(message.content);
                } else {
                    conversation.push(serde_json::json!({
                        "role": message.role,
                        "content": message.content,
                    }));
                }
            }
            let mut body = serde_json::json!({
                "model": model,
                "messages": conversation,
                "stream": false,
                "temperature": temperature,
                "max_tokens": max_tokens,
            });
            if !system.is_empty() {
                body["system"] = serde_json::Value::String(system.join("\n\n"));
            }
            Ok(OneShotRequest {
                url: format!("{base_url}/v1/messages"),
                body,
            })
        }
        ApiStyle::Chatgpt => Err(
            "One-shot AI helpers do not support ChatGPT endpoints yet. Choose an OpenAI-compatible or Anthropic endpoint."
                .into(),
        ),
    }
}

pub(crate) async fn run_one_shot_text(
    base_url: &str,
    api_key: &str,
    model: &str,
    api_style: &ApiStyle,
    messages: Vec<AiMessage>,
    max_tokens: u32,
    temperature: f32,
) -> Result<String, String> {
    let mut request = build_one_shot_request(
        base_url,
        model,
        api_style,
        messages,
        max_tokens,
        temperature,
    )?;
    let client = Client::new();
    let response = match api_style {
        ApiStyle::Openai => crate::http_util::post_chat_completions(
            &client,
            &request.url,
            api_key,
            &mut request.body,
        )
        .await
        .map_err(|error| error.to_string())?,
        ApiStyle::Anthropic => {
            let response = crate::http_util::send_with_retry("Anthropic messages request", || {
                client
                    .post(&request.url)
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&request.body)
            })
            .await
            .map_err(|error| error.to_string())?;
            crate::http_util::check_status(response)
                .await
                .map_err(|error| error.to_string())?
        }
        ApiStyle::Chatgpt => unreachable!("ChatGPT is rejected while building the request"),
    };
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("JSON parse error: {error}"))?;
    Ok(value
        .pointer("/choices/0/message/content")
        .or_else(|| value.pointer("/content/0/text"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages() -> Vec<AiMessage> {
        vec![
            AiMessage {
                role: "system".into(),
                content: "Follow the instructions exactly.".into(),
            },
            AiMessage {
                role: "user".into(),
                content: "Return JSON.".into(),
            },
        ]
    }

    #[test]
    fn openai_one_shot_uses_chat_completions_transport() {
        let request = build_one_shot_request(
            "https://api.deepseek.com",
            "deepseek-v4-pro",
            &ApiStyle::Openai,
            messages(),
            512,
            0.3,
        )
        .unwrap();

        assert_eq!(request.url, "https://api.deepseek.com/chat/completions");
        assert_eq!(request.body["model"], "deepseek-v4-pro");
        assert_eq!(request.body["messages"][0]["role"], "system");
    }

    #[test]
    fn direct_provider_one_shot_normalizes_openrouter_model_prefix() {
        let request = build_one_shot_request(
            "https://api.deepseek.com",
            "deepseek/deepseek-v4-pro",
            &ApiStyle::Openai,
            messages(),
            512,
            0.3,
        )
        .unwrap();

        assert_eq!(request.body["model"], "deepseek-v4-pro");
    }

    #[test]
    fn custom_endpoint_preserves_slash_qualified_model_id() {
        let request = build_one_shot_request(
            "https://inference.example.com/v1",
            "meta-llama/Llama-3.1-70B-Instruct",
            &ApiStyle::Openai,
            messages(),
            512,
            0.3,
        )
        .unwrap();

        assert_eq!(request.body["model"], "meta-llama/Llama-3.1-70B-Instruct");
    }

    #[test]
    fn anthropic_one_shot_uses_messages_transport_and_top_level_system() {
        let request = build_one_shot_request(
            "https://api.anthropic.com",
            "claude-sonnet-4-5",
            &ApiStyle::Anthropic,
            messages(),
            512,
            0.3,
        )
        .unwrap();

        assert_eq!(request.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(request.body["system"], "Follow the instructions exactly.");
        assert_eq!(request.body["messages"][0]["role"], "user");
    }

    #[test]
    fn chatgpt_one_shot_is_rejected_before_an_invalid_request() {
        let error = build_one_shot_request(
            "https://chatgpt.com/backend-api/codex",
            "gpt-5.5",
            &ApiStyle::Chatgpt,
            messages(),
            512,
            0.3,
        )
        .unwrap_err();

        assert!(error.contains("do not support ChatGPT endpoints"));
    }
}
