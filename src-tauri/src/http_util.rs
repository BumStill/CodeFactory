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
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde_json::Value;
use std::time::Duration;

const MODEL_HTTP_MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone)]
pub struct RetryNotice {
    pub label: String,
    pub attempt: usize,
    pub max_attempts: usize,
    pub delay: Duration,
    pub reason: String,
}

fn retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(match attempt {
        1 => 300,
        2 => 900,
        _ => 1500,
    })
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status.as_u16(),
        408 | 409 | 425 | 429 | 500 | 502 | 503 | 504
    )
}

fn is_retryable_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request() || err.is_body()
}

/// Send a provider request with a short retry budget for transient transport
/// and gateway failures. The builder closure is invoked per attempt because
/// reqwest request bodies are one-shot.
pub async fn send_with_retry<F>(label: &str, mut build_request: F) -> Result<Response>
where
    F: FnMut() -> RequestBuilder,
{
    send_with_retry_and_notify(label, || build_request(), |_| {}).await
}

pub async fn send_with_retry_and_notify<F, N>(
    label: &str,
    mut build_request: F,
    mut notify_retry: N,
) -> Result<Response>
where
    F: FnMut() -> RequestBuilder,
    N: FnMut(RetryNotice),
{
    for attempt in 1..=MODEL_HTTP_MAX_ATTEMPTS {
        match build_request()
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if attempt < MODEL_HTTP_MAX_ATTEMPTS && is_retryable_status(status) {
                    let detail = response.text().await.unwrap_or_default();
                    let delay = retry_delay(attempt);
                    let reason = format!(
                        "HTTP {}{}",
                        status,
                        if detail.trim().is_empty() {
                            String::new()
                        } else {
                            format!(": {}", detail.chars().take(240).collect::<String>())
                        }
                    );
                    tracing::warn!(
                        "{} returned transient HTTP {} on attempt {}/{}; retrying in {:?}: {}",
                        label,
                        status,
                        attempt,
                        MODEL_HTTP_MAX_ATTEMPTS,
                        delay,
                        detail.chars().take(240).collect::<String>()
                    );
                    notify_retry(RetryNotice {
                        label: label.to_string(),
                        attempt,
                        max_attempts: MODEL_HTTP_MAX_ATTEMPTS,
                        delay,
                        reason,
                    });
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Ok(response);
            }
            Err(err) if attempt < MODEL_HTTP_MAX_ATTEMPTS && is_retryable_reqwest_error(&err) => {
                let delay = retry_delay(attempt);
                let reason = format!("transport error: {err}");
                tracing::warn!(
                    "{} transport failure on attempt {}/{}; retrying in {:?}: {}",
                    label,
                    attempt,
                    MODEL_HTTP_MAX_ATTEMPTS,
                    delay,
                    err
                );
                notify_retry(RetryNotice {
                    label: label.to_string(),
                    attempt,
                    max_attempts: MODEL_HTTP_MAX_ATTEMPTS,
                    delay,
                    reason,
                });
                tokio::time::sleep(delay).await;
            }
            Err(err) => return Err(err.into()),
        }
    }

    Err(AppError::Other(format!("{label} retry budget exhausted")))
}

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
    let mut resp = send_with_retry("chat completions request", || {
        client
            .post(url)
            .bearer_auth(api_key)
            .header("X-Title", "CodeFactory")
            .header("Content-Type", "application/json")
            .json(&*body)
    })
    .await?;

    if resp.status().as_u16() == 400 && body.get("max_tokens").is_some() {
        let err_text = resp.text().await.unwrap_or_default();
        if err_text.contains("max_completion_tokens") {
            crate::config::settings::force_max_completion_tokens(body);
            resp = send_with_retry(
                "chat completions request after max_tokens adaptation",
                || {
                    client
                        .post(url)
                        .bearer_auth(api_key)
                        .header("X-Title", "CodeFactory")
                        .header("Content-Type", "application/json")
                        .json(&*body)
                },
            )
            .await?;
        } else {
            return Err(AppError::Other(format!("HTTP 400 Bad Request: {err_text}")));
        }
    }

    check_status(resp).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    };

    fn serve_statuses(statuses: Vec<&'static str>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_thread = Arc::clone(&hits);

        std::thread::spawn(move || {
            for status in statuses {
                let (mut stream, _) = listener.accept().expect("accept request");
                hits_for_thread.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf);
                let body = "{}";
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        (url, hits)
    }

    #[tokio::test]
    async fn send_with_retry_retries_transient_status() {
        let (url, hits) = serve_statuses(vec!["503 Service Unavailable", "200 OK"]);
        let client = Client::new();

        let response = send_with_retry("test model request", || {
            client.post(&url).json(&json!({"ok": true}))
        })
        .await
        .expect("retry should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn send_with_retry_notifies_transient_status() {
        let (url, _hits) = serve_statuses(vec!["503 Service Unavailable", "200 OK"]);
        let client = Client::new();
        let mut notices = Vec::new();

        let response = send_with_retry_and_notify(
            "test model request",
            || client.post(&url).json(&json!({"ok": true})),
            |notice| notices.push(notice),
        )
        .await
        .expect("retry should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].label, "test model request");
        assert_eq!(notices[0].attempt, 1);
        assert_eq!(notices[0].max_attempts, 3);
        assert!(notices[0].reason.contains("HTTP 503"));
    }

    #[tokio::test]
    async fn send_with_retry_requests_identity_encoding() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (request_tx, request_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).expect("read request");
            request_tx
                .send(String::from_utf8_lossy(&request[..count]).into_owned())
                .expect("send captured request");
            let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        let client = Client::new();

        send_with_retry("test model request", || client.post(&url))
            .await
            .expect("request should succeed");

        let request = request_rx.recv().expect("captured request").to_lowercase();
        assert!(
            request.contains("accept-encoding: identity\r\n"),
            "{request}"
        );
    }
}
