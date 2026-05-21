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
use reqwest::Response;

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
