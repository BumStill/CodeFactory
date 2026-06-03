// SPDX-License-Identifier: Apache-2.0
//! Helpers for HTTP error handling.
//!
//! `reqwest::Response::error_for_status` drops the response body, leaving only
//! the status code in the error chain. That's catastrophic for debugging
//! third-party LLM APIs — DeepSeek, OpenAI, Anthropic all return a
//! human-readable JSON body explaining exactly what's wrong (unsupported
//! field, bad model id, invalid tool schema, etc.). Without it the user sees
//! a generic "HTTP 400" and has nothing to act on.
//!
//! `check_status` reads the body on failure, tries to extract the
//! provider-standard `error.message` (works for OpenAI/DeepSeek/OpenRouter)
//! or `error` (Anthropic), and surfaces the full text as `AppError::Other`.

use crate::errors::{AppError, Result};
use reqwest::{Client, Response};
use serde_json::Value;

pub async fn check_status(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    // Try to read the body — but if THAT fails (e.g. dropped connection),
    // fall back to just the status code.
    let body = match response.text().await {
        Ok(b) => b,
        Err(e) => {
            return Err(AppError::Other(format!(
                "HTTP {status}: failed to read response body: {e}"
            )));
        }
    };

    // Most providers return JSON like
    //   { "error": { "message": "...", "type": "...", "code": "..." } }
    // Anthropic uses
    //   { "type": "error", "error": { "type": "...", "message": "..." } }
    // OpenRouter sometimes returns a plain string. Try each shape; on miss,
    // fall through to the raw body capped to 500 chars.
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| {
                    // {error: {message: "..."}}
                    e.get("message")
                        .and_then(|m| m.as_str())
                        .map(String::from)
                        // or {error: "..."} (plain string)
                        .or_else(|| e.as_str().map(String::from))
                })
                .or_else(|| {
                    // {message: "..."} at top level (some compat shims)
                    v.get("message").and_then(|m| m.as_str()).map(String::from)
                })
        })
        .unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.len() > 500 {
                format!("{}…", &trimmed[..500])
            } else {
                trimmed.to_string()
            }
        });

    Err(AppError::Other(format!("HTTP {status}: {detail}")))
}

/// POST a one-shot Chat Completions request, reactively switching `max_tokens`
/// → `max_completion_tokens` only when the server actually rejects it.
///
/// Sends `body` exactly as built first — preserving `max_tokens` + `temperature`,
/// which the overwhelming majority of endpoints (including proxies and
/// aggregators that serve GPT-5-named models) accept. Only when the server
/// itself answers 400 "use 'max_completion_tokens' instead" does it rewrite the
/// body and resend once. This is the non-streaming twin of the interactive-chat
/// path, so task decomposition, spec assist, post-mortems and acceptance checks
/// get the same name-independent handling — without the name-based guessing that
/// broke endpoints perfectly happy with the legacy fields.
pub async fn post_chat_completions(
    client: &Client,
    url: &str,
    api_key: &str,
    body: &mut Value,
) -> Result<Response> {
    let mut resp = client
        .post(url)
        .bearer_auth(api_key)
        .header("X-Title", "CodeFactory")
        .header("Content-Type", "application/json")
        .json(&*body)
        .send()
        .await?;

    if resp.status().as_u16() == 400 && body.get("max_tokens").is_some() {
        let err_text = resp.text().await.unwrap_or_default();
        if err_text.contains("max_completion_tokens") {
            crate::config::settings::force_max_completion_tokens(body);
            resp = client
                .post(url)
                .bearer_auth(api_key)
                .header("X-Title", "CodeFactory")
                .header("Content-Type", "application/json")
                .json(&*body)
                .send()
                .await?;
        } else {
            return Err(AppError::Other(format!("HTTP 400 Bad Request: {err_text}")));
        }
    }

    check_status(resp).await
}
