// SPDX-License-Identifier: Apache-2.0
//! Model requests: payload build, retries, backoff, and tool-call parsing.
//!
//! Extracted verbatim from `main.rs` (keystone slice 4.8a) — a pure module
//! split with ZERO behaviour change, so the later seam adoption (4.8b) shows up
//! as a small readable diff instead of being buried in a 2775-line file.


use codefactory_agent_core::*;
use crate::HeadlessError;
use crate::protocol::*;
use crate::compaction::*;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(crate) const MODEL_REQUEST_INITIAL_ATTEMPTS: usize = 3;

pub(crate) const MODEL_REQUEST_PROGRESS_ATTEMPTS: usize = 5;

pub(crate) const MODEL_ERROR_BODY_CHARS: usize = 1_000;

#[derive(Debug)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) timeout_sec: u64,
}

pub(crate) async fn request_model(
    client: &Client,
    endpoint: &str,
    config: &StartConfig,
    messages: &[Value],
    allow_tools: bool,
    require_tool: bool,
    attempt_timeout_sec: u64,
    max_attempts: usize,
    wall_deadline: Option<Instant>,
) -> Result<Value, HeadlessError> {
    let first = request_model_with_tool_choice(
        client,
        endpoint,
        config,
        messages,
        allow_tools,
        require_tool,
        attempt_timeout_sec,
        max_attempts,
        wall_deadline,
    )
    .await;
    let required_choice_unsupported = matches!(
        &first,
        Err(HeadlessError::ModelHttpStatus { status, body })
            if require_tool
                && matches!(*status, 400 | 422)
                && provider_rejects_required_tool_choice(body)
    );
    if required_choice_unsupported {
        return request_model_with_tool_choice(
            client,
            endpoint,
            config,
            messages,
            allow_tools,
            false,
            attempt_timeout_sec,
            max_attempts,
            wall_deadline,
        )
        .await;
    }
    first
}

/// The one and only `run_shell` tool schema.
///
/// Both the outbound wire payload and the shared loop's `tool_defs` read this,
/// so the definition the model sees and the definition the loop advertises
/// cannot drift apart.
pub(crate) fn run_shell_schema() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "run_shell",
            "description": "Run one bounded shell script in the task environment. Batch related reads, compatible edits, and focused checks into this call when their order and failure handling are clear, so end-to-end build, install, runtime, and test stages finish within the execution budget.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_sec": {"type": "integer", "minimum": 1}
                },
                "required": ["command"]
            }
        }
    })
}

pub(crate) async fn request_model_with_tool_choice(
    client: &Client,
    endpoint: &str,
    config: &StartConfig,
    messages: &[Value],
    allow_tools: bool,
    require_tool: bool,
    attempt_timeout_sec: u64,
    max_attempts: usize,
    wall_deadline: Option<Instant>,
) -> Result<Value, HeadlessError> {
    let mut payload = json!({
        "model": config.model,
        "messages": messages,
        "tool_choice": if !allow_tools {
            "none"
        } else if require_tool {
            "required"
        } else {
            "auto"
        }
    });
    if allow_tools {
        payload["tools"] = json!([run_shell_schema()]);
    }
    let attempt_timeout = Duration::from_secs(attempt_timeout_sec.max(1));
    let max_attempts = max_attempts.max(1);
    for attempt in 1..=max_attempts {
        if wall_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(HeadlessError::ModelResponse {
                attempts: attempt - 1,
                detail: "wall-clock deadline exhausted before the next model request".to_owned(),
            });
        }
        let effective_timeout = wall_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .map(|remaining| remaining.min(attempt_timeout))
            .unwrap_or(attempt_timeout)
            .max(Duration::from_millis(1));
        let mut request = client
            .post(endpoint)
            .timeout(effective_timeout)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .json(&payload);
        if !config.api_key.is_empty() {
            request = request.bearer_auth(&config.api_key);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) if attempt < max_attempts && is_retryable_model_error(&error) => {
                wait_before_model_retry(attempt, wall_deadline).await;
                continue;
            }
            Err(error) => return Err(HeadlessError::ModelRequest(error)),
        };
        let status = response.status();
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(error) if attempt < max_attempts && is_retryable_model_error(&error) => {
                wait_before_model_retry(attempt, wall_deadline).await;
                continue;
            }
            Err(error) => {
                return Err(HeadlessError::ModelResponse {
                    attempts: attempt,
                    detail: error.to_string(),
                });
            }
        };

        if !status.is_success() {
            let body = response_body_preview(&body);
            if attempt < max_attempts && (status.as_u16() == 429 || status.is_server_error()) {
                wait_before_model_retry(attempt, wall_deadline).await;
                continue;
            }
            return Err(HeadlessError::ModelHttpStatus {
                status: status.as_u16(),
                body,
            });
        }

        match serde_json::from_slice(&body) {
            Ok(value) => return Ok(value),
            Err(_error) if attempt < max_attempts => {
                wait_before_model_retry(attempt, wall_deadline).await;
                continue;
            }
            Err(error) => {
                return Err(HeadlessError::ModelResponse {
                    attempts: attempt,
                    detail: format!(
                        "invalid JSON: {error}; body={}",
                        response_body_preview(&body)
                    ),
                });
            }
        }
    }

    unreachable!("model request loop always returns")
}

pub(crate) fn is_retryable_model_error(error: &reqwest::Error) -> bool {
    error.is_timeout()
        || error.is_connect()
        || error.is_request()
        || error.is_body()
        || error.is_decode()
}

pub(crate) async fn wait_before_model_retry(attempt: usize, wall_deadline: Option<Instant>) {
    let delay = Duration::from_millis(250 * attempt as u64);
    let bounded_delay = wall_deadline
        .map(|deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .min(delay)
        })
        .unwrap_or(delay);
    if !bounded_delay.is_zero() {
        tokio::time::sleep(bounded_delay).await;
    }
}

pub(crate) fn model_request_attempts(tool_outcome_count: usize) -> usize {
    if tool_outcome_count == 0 {
        MODEL_REQUEST_INITIAL_ATTEMPTS
    } else {
        MODEL_REQUEST_PROGRESS_ATTEMPTS
    }
}

pub(crate) fn response_body_preview(body: &[u8]) -> String {
    truncate_for_model(&String::from_utf8_lossy(body), MODEL_ERROR_BODY_CHARS)
}

pub(crate) fn parse_tool_calls(
    message: &Value,
    shell_timeout_sec: u64,
) -> Result<Vec<ToolCall>, HeadlessError> {
    let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    calls
        .iter()
        .map(|call| {
            let id = call["id"].as_str().unwrap_or("tool-call").to_owned();
            let arguments = call["function"]["arguments"]
                .as_str()
                .ok_or(HeadlessError::MissingToolArguments)?;
            let arguments: Value = serde_json::from_str(arguments)?;
            let command = arguments["command"]
                .as_str()
                .filter(|command| !command.trim().is_empty())
                .ok_or(HeadlessError::MissingCommand)?
                .to_owned();
            let requested_timeout = arguments["timeout_sec"]
                .as_u64()
                .unwrap_or(shell_timeout_sec);
            Ok(ToolCall {
                id,
                timeout_sec: effective_command_timeout_sec(
                    &command,
                    requested_timeout,
                    shell_timeout_sec,
                ),
                command,
            })
        })
        .collect()
}

pub(crate) fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/chat/completions")
    }
}
