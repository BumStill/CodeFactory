// SPDX-License-Identifier: Apache-2.0
//! Silent, bounded model generation for app-owned metadata.
//!
//! This deliberately reuses the full desktop provider transport so OpenAI-
//! compatible, Anthropic, and ChatGPT subscription sessions keep the same wire
//! behavior. It accepts exactly one resolved route: metadata generation must
//! never fail over to a Provider the user did not select for that session.

use std::sync::Arc;
use std::time::Duration;

use codefactory_agent_loop::transport::{ModelTransport, RoundOptions};
use reqwest::Client;

use super::events::CollectingEventSink;
use super::failover::RouteCandidate;
use super::model_transport::DesktopModelTransport;
use crate::config::settings::ApiStyle;
use crate::openrouter::types::{ChatMessage, Usage};

pub(crate) struct InternalTextOutput {
    pub(crate) text: String,
    pub(crate) usage: Option<Usage>,
    pub(crate) endpoint_name: String,
    pub(crate) model_id: String,
    pub(crate) base_url: String,
    pub(crate) is_chatgpt: bool,
}

async fn credential_for_route(route: &RouteCandidate) -> Result<String, String> {
    if matches!(route.api_style, ApiStyle::Chatgpt) {
        return Ok(String::new());
    }
    if let Some(secret) = route.legacy_inline_api_key.as_ref() {
        return Ok(secret.clone());
    }
    let Some(key_ref) = route.credential_ref.as_deref() else {
        return Err(format!(
            "AUTH_MISSING: {} has no configured credential reference",
            route.endpoint_name
        ));
    };
    match crate::credential_broker::CredentialBroker::global()
        .get(key_ref)
        .await
    {
        Ok(Some(secret)) if !secret.trim().is_empty() => Ok(secret),
        Ok(_) => Err(format!(
            "AUTH_MISSING: {} has no configured credential",
            route.endpoint_name
        )),
        Err(error) => Err(format!(
            "CREDENTIAL_ACCESS_REQUIRED: {} ({:?})",
            route.endpoint_name, error.kind
        )),
    }
}

pub(crate) async fn generate_bounded_text(
    route: RouteCandidate,
    session_id: &str,
    messages: Vec<ChatMessage>,
    max_output_tokens: u32,
    deadline: Duration,
) -> Result<InternalTextOutput, String> {
    let route_for_request = route.clone();
    let session_id = session_id.to_string();
    let response = tokio::time::timeout(deadline, async move {
        let api_key = credential_for_route(&route_for_request).await?;
        let transport = DesktopModelTransport {
            http: Client::new(),
            events: Arc::new(CollectingEventSink::new()),
            model_id: route_for_request.model_id.clone(),
            session_id,
            base_url: route_for_request.base_url.clone(),
            api_key,
            api_style: route_for_request.api_style.clone(),
            cancel: None,
            max_output_tokens: Some(max_output_tokens),
            retry_response_body: crate::http_util::RetryResponseBody::Redact,
            provider_attempt: None,
        };
        let options = RoundOptions {
            require_tool: false,
            reasoning_effort: "low".into(),
            tool_outcomes_so_far: 0,
        };
        transport
            .complete(&messages, &[], &options)
            .await
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "SESSION_TITLE_TIMEOUT".to_string())??;
    Ok(InternalTextOutput {
        text: response.text,
        usage: response.usage,
        endpoint_name: route.endpoint_name,
        model_id: route.model_id,
        base_url: route.base_url,
        is_chatgpt: matches!(route.api_style, ApiStyle::Chatgpt),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openrouter::types::MessageContent;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    fn route(base_url: String) -> RouteCandidate {
        RouteCandidate {
            endpoint_name: "fixture".into(),
            model_id: "fixture-model".into(),
            base_url,
            credential_ref: None,
            legacy_inline_api_key: Some("fixture-key".into()),
            supports_vision: false,
            api_style: ApiStyle::Openai,
        }
    }

    fn message(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Text(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    fn read_http_request(socket: &mut TcpStream) -> String {
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        loop {
            let mut chunk = [0_u8; 4096];
            let read = socket.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    #[tokio::test]
    async fn chatgpt_metadata_generation_uses_subscription_auth_path() {
        let route = RouteCandidate {
            endpoint_name: "chatgpt".into(),
            model_id: "gpt-test".into(),
            base_url: crate::codex_auth::CHATGPT_BASE_URL.into(),
            credential_ref: None,
            legacy_inline_api_key: None,
            supports_vision: false,
            api_style: ApiStyle::Chatgpt,
        };
        assert_eq!(credential_for_route(&route).await.unwrap(), "");
    }

    #[tokio::test]
    async fn bounded_generation_keeps_the_route_and_output_ceiling() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (request_tx, request_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request_tx.send(read_http_request(&mut socket)).unwrap();
            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"会话自动命名优化\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":8,\"total_tokens\":20}}\n\n",
                "data: [DONE]\n\n"
            );
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let output = generate_bounded_text(
            route(base_url),
            "session-title-test",
            vec![message("user", "总结这个任务")],
            64,
            Duration::from_secs(2),
        )
        .await
        .unwrap();

        assert_eq!(output.text, "会话自动命名优化");
        assert_eq!(output.endpoint_name, "fixture");
        assert_eq!(output.model_id, "fixture-model");
        let usage = output.usage.unwrap();
        assert_eq!((usage.prompt_tokens, usage.completion_tokens), (12, 8));
        let request = request_rx.recv().unwrap();
        assert!(request.contains("\"max_tokens\":64"), "{request}");
        assert!(request.contains("\"model\":\"fixture-model\""), "{request}");
    }

    #[tokio::test]
    async fn bounded_generation_does_not_invent_a_completion_summary() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _request = read_http_request(&mut socket);
            let body = concat!(
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":0,\"total_tokens\":4}}\n\n",
                "data: [DONE]\n\n"
            );
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let output = generate_bounded_text(
            route(base_url),
            "empty-title-test",
            vec![message("user", "总结")],
            64,
            Duration::from_secs(2),
        )
        .await
        .unwrap();

        assert!(output.text.is_empty(), "unexpected text: {}", output.text);
    }

    #[tokio::test]
    async fn max_tokens_adaptation_preserves_the_metadata_ceiling() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (request_tx, request_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                request_tx.send(read_http_request(&mut socket)).unwrap();
                if attempt == 0 {
                    let body = r#"{"error":{"message":"use max_completion_tokens instead"}}"#;
                    write!(
                        socket,
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
                } else {
                    let body = concat!(
                        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"会话命名\"},\"finish_reason\":\"stop\"}]}\n\n",
                        "data: [DONE]\n\n"
                    );
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
                }
            }
        });

        let output = generate_bounded_text(
            route(base_url),
            "adapt-title-test",
            vec![message("user", "总结")],
            64,
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert_eq!(output.text, "会话命名");
        let first = request_rx.recv().unwrap();
        let second = request_rx.recv().unwrap();
        assert!(first.contains("\"max_tokens\":64"), "{first}");
        assert!(second.contains("\"max_completion_tokens\":64"), "{second}");
        assert!(!second.contains("\"max_tokens\":"), "{second}");
    }

    #[tokio::test]
    async fn anthropic_metadata_request_keeps_its_output_ceiling() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (request_tx, request_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request_tx.send(read_http_request(&mut socket)).unwrap();
            let body = concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"会话命名\"}}\n\n",
                "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":4}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            );
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let anthropic_route = RouteCandidate {
            endpoint_name: "anthropic-fixture".into(),
            model_id: "claude-test".into(),
            base_url,
            credential_ref: None,
            legacy_inline_api_key: Some("fixture-key".into()),
            supports_vision: false,
            api_style: ApiStyle::Anthropic,
        };

        let output = generate_bounded_text(
            anthropic_route,
            "anthropic-title-test",
            vec![message("user", "总结")],
            64,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(output.text, "会话命名");
        let request = request_rx.recv().unwrap();
        assert!(request.contains("\"max_tokens\":64"), "{request}");
        assert!(request.contains("\"model\":\"claude-test\""), "{request}");
        assert!(request.contains("\"temperature\":0.2"), "{request}");
    }

    #[tokio::test]
    async fn anthropic_metadata_rejects_a_partial_stream_without_message_stop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _request = read_http_request(&mut socket);
            let body = concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"会话\"}}\n\n"
            );
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let anthropic_route = RouteCandidate {
            endpoint_name: "anthropic-fixture".into(),
            model_id: "claude-test".into(),
            base_url,
            credential_ref: None,
            legacy_inline_api_key: Some("fixture-key".into()),
            supports_vision: false,
            api_style: ApiStyle::Anthropic,
        };

        let error = match generate_bounded_text(
            anthropic_route,
            "anthropic-partial-title-test",
            vec![message("user", "总结")],
            64,
            Duration::from_secs(2),
        )
        .await
        {
            Ok(_) => panic!("partial Anthropic output must not become a persisted title"),
            Err(error) => error,
        };

        assert!(error.contains("without message_stop"), "{error}");
    }
}
