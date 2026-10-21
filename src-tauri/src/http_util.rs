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

/// M145 R1 — bounded budgets for the model request path.
///
/// Live evidence (2026-10-21, Windows v1.91.13): a self-hosted DeepSeek
/// endpoint accepted the TCP connection and then never answered. `reqwest`'s
/// default client has no read timeout, so `send()` future never resolved: the
/// turn showed "正在做 · 等待中" forever with no tool call and no error. Every
/// model request must therefore have a *finite* budget for (a) connecting,
/// (b) receiving the first response byte, and (c) gaps between stream chunks.
pub const MODEL_CONNECT_TIMEOUT_MS: u64 = 10_000;
pub const MODEL_FIRST_BYTE_TIMEOUT_MS: u64 = 60_000;
pub const MODEL_STREAM_IDLE_TIMEOUT_MS: u64 = 120_000;

fn bounded_ms_env(key: &str, fallback_ms: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(fallback_ms)
}

/// Budget for establishing the TCP/TLS connection to a model endpoint.
pub fn model_connect_timeout() -> Duration {
    Duration::from_millis(bounded_ms_env(
        "CODEFACTORY_MODEL_CONNECT_TIMEOUT_MS",
        MODEL_CONNECT_TIMEOUT_MS,
    ))
}

/// Budget for "request sent → first response byte". Never infinite: this is the
/// budget the user sees as `等待模型首个响应`.
pub fn model_first_byte_timeout() -> Duration {
    Duration::from_millis(bounded_ms_env(
        "CODEFACTORY_MODEL_FIRST_BYTE_TIMEOUT_MS",
        MODEL_FIRST_BYTE_TIMEOUT_MS,
    ))
}

/// Budget for a gap between two stream chunks. A stream that goes silent for
/// longer than this is treated as a transport failure, not as a running turn.
pub fn model_stream_idle_timeout() -> Duration {
    Duration::from_millis(bounded_ms_env(
        "CODEFACTORY_MODEL_STREAM_IDLE_TIMEOUT_MS",
        MODEL_STREAM_IDLE_TIMEOUT_MS,
    ))
}

/// The client used for every model request (chat, ChatGPT/Codex, internal text).
///
/// `read_timeout` is reqwest's *per-read inactivity* deadline, so it bounds both
/// "headers never arrived" and "the stream stalled mid-response" without
/// capping the total length of a long generation.
pub fn model_http_client() -> Client {
    Client::builder()
        .connect_timeout(model_connect_timeout())
        .read_timeout(model_stream_idle_timeout())
        .build()
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "model HTTP client fell back to defaults");
            Client::new()
        })
}

/// Bound one model request by [`model_first_byte_timeout`].
///
/// The wrapped future covers connect + request write + response headers (i.e.
/// the first byte). A budget overrun is reported with `timeout` in the message
/// so `classify_provider_failure` routes it to `TransportUnreachable`, which is
/// what lets M125 failover/retry take over instead of the turn hanging.
pub async fn send_model_request_bounded_with_policy<F, N>(
    label: &str,
    model: &str,
    build_request: F,
    notify_retry: N,
    response_body: RetryResponseBody,
    max_attempts: usize,
) -> Result<Response>
where
    F: FnMut() -> RequestBuilder,
    N: FnMut(RetryNotice),
{
    let budget = model_first_byte_timeout();
    let attempt = send_with_attempt_budget_and_notify_policy(
        label,
        build_request,
        notify_retry,
        response_body,
        max_attempts,
    );
    match tokio::time::timeout(budget, attempt).await {
        Ok(result) => result,
        Err(_) => Err(AppError::Other(format!(
            "MODEL_FIRST_BYTE_TIMEOUT: {label} 等待 {model} 首个响应超过 {}s（timeout），已中断并按回落/重试策略处理",
            budget.as_secs()
        ))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryResponseBody {
    Include,
    Redact,
}

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
    send_with_retry_and_notify_policy(
        label,
        || build_request(),
        |notice| notify_retry(notice),
        RetryResponseBody::Include,
    )
    .await
}

pub async fn send_with_retry_and_notify_policy<F, N>(
    label: &str,
    mut build_request: F,
    mut notify_retry: N,
    response_body: RetryResponseBody,
) -> Result<Response>
where
    F: FnMut() -> RequestBuilder,
    N: FnMut(RetryNotice),
{
    send_with_attempt_budget_and_notify_policy(
        label,
        || build_request(),
        |notice| notify_retry(notice),
        response_body,
        MODEL_HTTP_MAX_ATTEMPTS,
    )
    .await
}

/// Execute a model request with an explicit POST budget. Durable provider
/// attempts pass `1`: retrying a timeout inside an already in-flight attempt
/// would otherwise create an unreceipted duplicate request. Callers without a
/// write-ahead provider attempt retain the legacy bounded retry budget.
pub async fn send_with_attempt_budget_and_notify_policy<F, N>(
    label: &str,
    mut build_request: F,
    mut notify_retry: N,
    response_body: RetryResponseBody,
    max_attempts: usize,
) -> Result<Response>
where
    F: FnMut() -> RequestBuilder,
    N: FnMut(RetryNotice),
{
    let max_attempts = max_attempts.clamp(1, MODEL_HTTP_MAX_ATTEMPTS);
    for attempt in 1..=max_attempts {
        match build_request()
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                if attempt < max_attempts && is_retryable_status(status) {
                    let detail = response.text().await.unwrap_or_default();
                    let delay = retry_delay(attempt);
                    let visible_detail = match response_body {
                        RetryResponseBody::Include => detail.chars().take(240).collect::<String>(),
                        RetryResponseBody::Redact => String::new(),
                    };
                    let reason = format!(
                        "HTTP {}{}",
                        status,
                        if visible_detail.trim().is_empty() {
                            String::new()
                        } else {
                            format!(": {visible_detail}")
                        }
                    );
                    if response_body == RetryResponseBody::Redact {
                        tracing::warn!(
                            "{} returned transient HTTP {} on attempt {}/{}; retrying in {:?}; response body redacted",
                            label,
                            status,
                            attempt,
                            max_attempts,
                            delay,
                        );
                    } else {
                        tracing::warn!(
                            "{} returned transient HTTP {} on attempt {}/{}; retrying in {:?}: {}",
                            label,
                            status,
                            attempt,
                            max_attempts,
                            delay,
                            visible_detail
                        );
                    }
                    notify_retry(RetryNotice {
                        label: label.to_string(),
                        attempt,
                        max_attempts,
                        delay,
                        reason,
                    });
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Ok(response);
            }
            Err(err) if attempt < max_attempts && is_retryable_reqwest_error(&err) => {
                let delay = retry_delay(attempt);
                let reason = format!("transport error: {err}");
                tracing::warn!(
                    "{} transport failure on attempt {}/{}; retrying in {:?}: {}",
                    label,
                    attempt,
                    max_attempts,
                    delay,
                    err
                );
                notify_retry(RetryNotice {
                    label: label.to_string(),
                    attempt,
                    max_attempts,
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
    async fn redacted_retry_policy_never_exposes_provider_response_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for (status, body) in [
                ("503 Service Unavailable", "echoed-private-title-input"),
                ("200 OK", "{}"),
            ] {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut buf = [0_u8; 4096];
                let _ = stream.read(&mut buf);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .expect("write response");
            }
        });
        // Keep the loopback fixture independent from the developer machine's
        // HTTP(S)_PROXY settings so the first response is always our 503 body.
        let client = Client::builder()
            .no_proxy()
            .build()
            .expect("build direct test client");
        let mut notices = Vec::new();

        let response = send_with_retry_and_notify_policy(
            "private metadata request",
            || client.post(&url).json(&json!({"ok": true})),
            |notice| notices.push(notice),
            RetryResponseBody::Redact,
        )
        .await
        .expect("retry should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].reason.starts_with("HTTP "));
        assert!(!notices[0].reason.contains("private-title-input"));
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

    /// M145 R1 — a model endpoint that accepts the TCP connection but never
    /// sends a response must fail with a visible timeout instead of leaving the
    /// turn in "等待中" forever. Before this change the `send()` future never
    /// resolved, because the default reqwest client has no read timeout.
    #[tokio::test]
    async fn model_request_without_first_byte_times_out_instead_of_hanging() {
        std::env::set_var("CODEFACTORY_MODEL_FIRST_BYTE_TIMEOUT_MS", "250");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            // Accept, read the request, then stay silent: the pathological
            // endpoint from the reported incident.
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer);
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
        });

        let client = model_http_client();
        let started = std::time::Instant::now();
        let result = send_model_request_bounded_with_policy(
            "test model request",
            "dfa_self_deepseek_v4_flash",
            || client.post(format!("http://{addr}/chat/completions")),
            |_notice| {},
            RetryResponseBody::Redact,
            1,
        )
        .await;
        let elapsed = started.elapsed();
        std::env::remove_var("CODEFACTORY_MODEL_FIRST_BYTE_TIMEOUT_MS");

        let error = result.expect_err("silent endpoint must time out").to_string();
        assert!(
            error.contains("MODEL_FIRST_BYTE_TIMEOUT"),
            "error must name the timeout: {error}"
        );
        assert!(
            error.contains("dfa_self_deepseek_v4_flash"),
            "error must name the model: {error}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "timeout budget must be bounded, elapsed {elapsed:?}"
        );
    }

    /// M145 R1 — the documented default budgets stay finite.
    #[test]
    fn model_request_budgets_are_finite() {
        assert_eq!(model_first_byte_timeout(), Duration::from_secs(60));
        assert_eq!(model_stream_idle_timeout(), Duration::from_secs(120));
        assert_eq!(model_connect_timeout(), Duration::from_secs(10));
    }

    /// M145 R1 — the timeout message routes through the existing failover
    /// classifier as a transport failure, so M125 retry/failover handles it.
    #[test]
    fn first_byte_timeout_classifies_as_transport_failure() {
        let class = crate::agent::failover::classify_provider_failure(
            "MODEL_FIRST_BYTE_TIMEOUT: OpenAI-compatible chat stream request 等待 x 首个响应超过 60s（timeout）",
        );
        assert_eq!(
            class,
            crate::agent::failover::ProviderFailureClass::TransportUnreachable
        );
    }
}
