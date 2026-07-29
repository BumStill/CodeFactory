#[cfg(test)]
use codefactory_agent_core::classify_command;
use codefactory_agent_core::{
    build_product_system_prompt, build_system_prompt, execution_contract_sha256,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{self, AsyncBufRead, AsyncWrite, BufReader, BufWriter};
// Test-only: the tokio-test harness drives the reader/writer directly and builds
// CompletionEvidence fixtures; production paths reach them via the modules.
#[cfg(test)]
use codefactory_agent_core::CompletionEvidence;
#[cfg(test)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

mod compaction;
mod loop_services;
mod policy;
mod protocol;
mod transport;

// Re-exported so `run()` and the test module (`use super::*`) keep every
// unqualified name they had before the 4.8a split.
use compaction::*;
use policy::*;
use protocol::*;
use transport::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Usage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    model_requests: u64,
}

impl Usage {
    fn add_response(&mut self, response: &Value) {
        let usage = &response["usage"];
        self.prompt_tokens += usage["prompt_tokens"].as_u64().unwrap_or(0);
        self.completion_tokens += usage["completion_tokens"].as_u64().unwrap_or(0);
        self.total_tokens += usage["total_tokens"].as_u64().unwrap_or(0);
        self.model_requests += 1;
    }
}

#[derive(Debug, Error)]
enum HeadlessError {
    #[error("stdin closed before a start message")]
    MissingStart,
    #[error("invalid protocol JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("first protocol message must be type=start")]
    ExpectedStart,
    #[error("execution contract mismatch: bridge={bridge}, sidecar={sidecar}")]
    ContractMismatch { bridge: String, sidecar: String },
    #[error("stdin closed while waiting for tool result {0}")]
    MissingToolResult(String),
    #[error("expected tool_result for {expected}, received {actual}")]
    UnexpectedToolResult { expected: String, actual: String },
    #[error("model request failed: {0}")]
    ModelRequest(#[from] reqwest::Error),
    #[error("model returned HTTP {status}: {body}")]
    ModelHttpStatus { status: u16, body: String },
    #[error("model response failed after {attempts} attempts: {detail}")]
    ModelResponse { attempts: usize, detail: String },
    #[error("model returned no choices")]
    MissingChoice,
    #[error("model tool call is missing function arguments")]
    MissingToolArguments,
    #[error("model tool arguments require a non-empty command")]
    MissingCommand,
    #[error("protocol I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// A failure surfaced by the shared loop. Every `LoopError` arm's `Display`
    /// is its underlying error verbatim, and the sidecar's transport/tool seams
    /// fill those with `HeadlessError` strings — so the stderr line is
    /// unchanged from when the loop lived here.
    #[error("{0}")]
    Loop(String),
}

#[tokio::main]
async fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let input = BufReader::new(stdin);
    let output = BufWriter::new(stdout);

    if let Err(error) = run(input, output).await {
        eprintln!("codefactory-agent-headless: {error}");
        std::process::exit(1);
    }
}

/// Thin adapter onto the SHARED loop (keystone slice 4.8). Everything that used
/// to be a second copy of the agent loop now lives in
/// `agent_loop::run::run_agent_loop`; this reads the `start` handshake, wires
/// the sidecar's own capability impls (see `loop_services`), and writes the
/// terminal `finished` line from the returned `RunOutcome`.
///
/// The eval-scoring surface is preserved by CHOOSING the sidecar's impls, not
/// by inheriting the desktop's: char-budget compaction, RuntimePolicy denials,
/// `run_shell` classification, and the wall clock all come from `loop_services`.
async fn run<R, W>(input: R, output: W) -> Result<(), HeadlessError>
where
    R: AsyncBufRead + Unpin + Send + Sync + 'static,
    W: AsyncWrite + Unpin + Send + Sync + 'static,
{
    use loop_services::{
        CharBudgetCompactor, DelegatingToolBackend, Jsonl, JsonlEventSink, SidecarPermissions,
        SidecarTransport, WallClockBudget,
    };
    use std::sync::atomic::AtomicBool;

    let mut input = input;
    let config = read_start(&mut input).await?;
    let client = Client::builder()
        .timeout(Duration::from_secs(config.model_timeout_sec.max(1)))
        .build()?;
    let endpoint = chat_completions_endpoint(&config.base_url);
    let policy = RuntimePolicy::new(config.policy_profile, config.allow_network);
    let system_prompt = match config.policy_profile {
        RuntimePolicyProfile::Product => build_product_system_prompt(config.allow_network),
        RuntimePolicyProfile::Benchmark => build_system_prompt(config.allow_network),
    };
    let started = Instant::now();

    // One shared stdin/stdout so `tool_request`, `usage_snapshot` and
    // `finished` keep their pinned interleaving across the backend, the sink
    // and this adapter.
    let io = std::sync::Arc::new(Jsonl {
        input: tokio::sync::Mutex::new(input),
        output: tokio::sync::Mutex::new(output),
        usage: tokio::sync::Mutex::new(Usage::default()),
    });
    let history = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let emitted_usage_this_round = std::sync::Arc::new(AtomicBool::new(false));

    let tool_schema: codefactory_agent_loop::types::ToolDefinition =
        serde_json::from_value(transport::run_shell_schema())
            .map_err(HeadlessError::InvalidJson)?;

    let budget = WallClockBudget {
        started,
        wall_time_budget_sec: config.wall_time_budget_sec,
        max_steps: config.max_steps.max(1) as usize,
    };

    let inputs = codefactory_agent_loop::run::LoopInputs {
        messages: vec![
            codefactory_agent_loop::types::ChatMessage {
                role: "system".into(),
                content: codefactory_agent_loop::types::MessageContent::Text(system_prompt),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
            codefactory_agent_loop::types::ChatMessage {
                role: "user".into(),
                content: codefactory_agent_loop::types::MessageContent::Text(
                    config.instruction.clone(),
                ),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            },
        ],
        system_prompt: String::new(),
        tool_defs: vec![tool_schema.clone()],
        completion_instruction: config.instruction.clone(),
        fact_check_instruction: config.instruction.clone(),
        audit_session_id: String::new(),
        root_turn_id: None,
        knowledge_library_ids: None,
        cancel: None,
    };

    let run_config = codefactory_agent_loop::run::RunConfig {
        finalization: codefactory_agent_loop::run::FinalizationPolicy::Benchmark,
        turn_capability: codefactory_agent_loop::run::TurnCapability::Implement,
        gate_benchmark: true,
        progress_window: 4,
        recovery_limit: 1,
        max_iterations: config.max_steps.max(1) as usize,
        wall_budget_applies: true,
        // The compactor runs; it is the sidecar's CHAR-budget digest, not the
        // desktop's token-based elision — that is what keeps scores comparable.
        context_compression: true,
        overload_backoff: false,
        inspection_budget: true,
        replay_rejected_draft: true,
        session_id: String::new(),
        endpoint_name: String::new(),
        model_id: config.model.clone(),
        base_url: config.base_url.clone(),
        usage_run_id: String::new(),
        surface: "benchmark".into(),
        task_id: None,
        // No DB: NullPersistence swallows every write anyway.
        anonymous: true,
        is_chatgpt: false,
        cwd: config
            .working_directory
            .clone()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(".")),
    };

    let services = codefactory_agent_loop::run::LoopServices {
        transport: std::sync::Arc::new(SidecarTransport {
            io: io.clone(),
            client,
            endpoint,
            config: config.clone(),
            started,
            emitted_usage_this_round: emitted_usage_this_round.clone(),
        }),
        tools: std::sync::Arc::new(DelegatingToolBackend {
            io: io.clone(),
            schema: tool_schema,
            shell_timeout_sec: config.shell_timeout_sec,
            started,
            wall_time_budget_sec: config.wall_time_budget_sec,
            history: history.clone(),
            emitted_usage_this_round: emitted_usage_this_round.clone(),
        }),
        persistence: std::sync::Arc::new(codefactory_agent_loop::journal::NullPersistence),
        events: std::sync::Arc::new(JsonlEventSink {
            io: io.clone(),
            emitted_usage_this_round,
        }),
        budget: std::sync::Arc::new(budget),
        permission: std::sync::Arc::new(SidecarPermissions { policy }),
        hooks: std::sync::Arc::new(codefactory_agent_loop::services::NoOpHooks),
        context_policy: std::sync::Arc::new(loop_services::FixedContext),
        fact_checker: std::sync::Arc::new(codefactory_agent_loop::services::NoOpFactChecker),
        // Unattended eval runs have no user at the keyboard to steer them.
        steer: std::sync::Arc::new(codefactory_agent_loop::services::NoSteering),
        compactor: std::sync::Arc::new(CharBudgetCompactor {
            max_chars: MAX_CONTEXT_CHARS,
            history: history.clone(),
        }),
    };

    let outcome = codefactory_agent_loop::run::run_agent_loop(inputs, run_config, services).await;

    // A transport failure inside the final wall-clock reserve, on a run that
    // already produced tool outcomes, is finished gracefully rather than
    // propagated: the task keeps whatever partial credit it earned instead of
    // the process exiting 1 and scoring zero. Tool/persist failures stay fatal,
    // as they were protocol violations before the flip too.
    let outcome = match outcome {
        Err(codefactory_agent_loop::run::LoopError::Transport(error))
            if should_finish_after_model_error(
                remaining_wall_time(started, config.wall_time_budget_sec),
                history.lock().await.len(),
            ) =>
        {
            Ok(codefactory_agent_loop::run::RunOutcome {
                final_text: format!(
                    "Stopped after a model transport failure in the final wall-clock reserve: {error}"
                ),
                completion_evidence: Default::default(),
                input_tokens: 0,
                output_tokens: 0,
                stop_reason: codefactory_agent_loop::run::StopReason::BudgetExhausted,
            })
        }
        other => other,
    };

    let usage = { io.usage.lock().await.clone() };
    let mut out = io.output.lock().await;
    match outcome {
        Ok(outcome) => {
            let stopped_for_wall_budget = matches!(
                outcome.stop_reason,
                codefactory_agent_loop::run::StopReason::BudgetExhausted
            );
            write_output(
                &mut *out,
                &OutputMessage::Finished {
                    final_text: if outcome.final_text.trim().is_empty() {
                        budget_exhaustion_message(stopped_for_wall_budget).to_owned()
                    } else {
                        outcome.final_text
                    },
                    execution_contract_sha256: execution_contract_sha256(),
                    completion_evidence: outcome.completion_evidence,
                    usage,
                },
            )
            .await?;
            Ok(())
        }
        // The loop's error text is the underlying provider/tool message
        // verbatim, so the exit path reads exactly as it did before.
        Err(error) => Err(HeadlessError::Loop(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use tokio::io::BufReader;

    #[test]
    fn successful_tool_batch_does_not_reopen_text_recovery() {
        // The shared loop owns this now; the sidecar's identical copy is gone.
        // Still pinned here: material progress must NOT refund recovery rounds.
        use codefactory_agent_loop::policy::completion_recovery_attempts_after_tool_batch as recovery_attempts;
        assert_eq!(recovery_attempts(1, true), 1);
        assert_eq!(recovery_attempts(1, false), 1);
    }

    #[test]
    fn protocol_output_uses_exact_bridge_schema() {
        let request = OutputMessage::ToolRequest {
            id: "call-1".to_owned(),
            command: "cargo test".to_owned(),
            timeout_sec: 30,
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 2,
                total_tokens: 12,
                model_requests: 1,
            },
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "type": "tool_request",
                "id": "call-1",
                "command": "cargo test",
                "timeout_sec": 30,
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 2,
                    "total_tokens": 12,
                    "model_requests": 1
                }
            })
        );

        let event = OutputMessage::UsageSnapshot {
            name: "usage_snapshot".to_owned(),
            usage: Usage {
                prompt_tokens: 20,
                completion_tokens: 4,
                total_tokens: 24,
                model_requests: 2,
            },
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "type": "event",
                "name": "usage_snapshot",
                "usage": {
                    "prompt_tokens": 20,
                    "completion_tokens": 4,
                    "total_tokens": 24,
                    "model_requests": 2
                }
            })
        );
    }

    #[tokio::test]
    async fn tool_result_protocol_accepts_timeout_without_exit_code() {
        let line = concat!(
            "{\"type\":\"tool_result\",\"id\":\"call-1\",",
            "\"return_code\":null,\"stdout\":\"\",\"stderr\":\"\",",
            "\"error\":\"command timed out\",",
            "\"next_working_directory\":\"/workspace/project\"}\n"
        );
        let mut input = BufReader::new(line.as_bytes());

        let result = read_tool_result(&mut input, "call-1").await.unwrap();

        assert_eq!(result.0, None);
        assert_eq!(result.3.as_deref(), Some("command timed out"));
        assert_eq!(result.4.as_deref(), Some("/workspace/project"));
    }

    #[test]
    fn finished_schema_includes_contract_evidence_and_usage() {
        let message = OutputMessage::Finished {
            final_text: "done".to_owned(),
            execution_contract_sha256: "abc".to_owned(),
            completion_evidence: CompletionEvidence::default(),
            usage: Usage::default(),
        };
        let value = serde_json::to_value(message).unwrap();
        assert_eq!(value["type"], "finished");
        assert_eq!(value["final_text"], "done");
        assert!(value.get("execution_contract_sha256").is_some());
        assert!(value.get("completion_evidence").is_some());
        assert!(value.get("usage").is_some());
    }

    #[tokio::test]
    async fn start_requires_the_shared_contract_hash() {
        let line = format!(
            "{}\n",
            json!({
                "type": "start",
                "instruction": "inspect and fix the project",
                "model": "test-model",
                "api_key": "secret",
                "base_url": "http://localhost:1234/v1",
                "max_steps": 4,
                "model_timeout_sec": 10,
                "shell_timeout_sec": 30,
                "wall_time_budget_sec": 900,
                "allow_network": false,
                "execution_contract_sha256": execution_contract_sha256()
            })
        );
        let mut input = BufReader::new(line.as_bytes());
        let config = read_start(&mut input).await.unwrap();
        assert_eq!(config.model, "test-model");
        assert!(!config.allow_network);
        assert_eq!(config.wall_time_budget_sec, Some(900));
        assert_eq!(config.policy_profile, RuntimePolicyProfile::Benchmark);
    }

    #[tokio::test]
    async fn product_start_selects_product_policy() {
        let line = format!(
            "{}\n",
            json!({
                "type": "start",
                "instruction": "run the project tests",
                "model": "test-model",
                "api_key": "secret",
                "base_url": "http://localhost:1234/v1",
                "max_steps": 4,
                "model_timeout_sec": 10,
                "shell_timeout_sec": 30,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            })
        );
        let mut input = BufReader::new(line.as_bytes());
        let config = read_start(&mut input).await.unwrap();
        assert_eq!(config.policy_profile, RuntimePolicyProfile::Product);
        let policy = RuntimePolicy::new(config.policy_profile, false);
        assert!(policy
            .evaluate_command("pytest /Users/leo/project/tests/test_api.py")
            .is_allowed());
    }

    #[tokio::test]
    async fn mismatched_contract_is_rejected_before_model_execution() {
        let line = format!(
            "{}\n",
            json!({
                "type": "start",
                "instruction": "task",
                "model": "test-model",
                "api_key": "",
                "base_url": "http://localhost:1234/v1",
                "max_steps": 1,
                "model_timeout_sec": 10,
                "shell_timeout_sec": 30,
                "allow_network": false,
                "execution_contract_sha256": "wrong"
            })
        );
        let mut input = BufReader::new(line.as_bytes());
        assert!(matches!(
            read_start(&mut input).await,
            Err(HeadlessError::ContractMismatch { .. })
        ));
    }

    #[test]
    fn model_tool_timeout_is_clamped_to_bridge_limit() {
        let message = json!({
            "tool_calls": [{
                "id": "call-1",
                "function": {
                    "arguments": "{\"command\":\"cargo test\",\"timeout_sec\":999}"
                }
            }]
        });
        let calls = parse_tool_calls(&message, 45).unwrap();
        assert_eq!(calls[0].timeout_sec, 45);

        let install = json!({
            "tool_calls": [{
                "id": "call-2",
                "function": {
                    "arguments": "{\"command\":\"pip install -e .\",\"timeout_sec\":30}"
                }
            }]
        });
        let calls = parse_tool_calls(&install, 300).unwrap();
        assert_eq!(calls[0].timeout_sec, 300);
        assert_eq!(clamp_timeout_to_wall_reserve(300, 240, 30), 210);
        assert_eq!(clamp_timeout_to_wall_reserve(90, 20, 30), 1);
        assert!(budget_exhaustion_message(true).contains("wall-clock budget"));
        assert!(budget_exhaustion_message(false).contains("model step budget"));
        assert!(should_finish_after_model_error(Some((45, 900)), 3));
        assert!(!should_finish_after_model_error(Some((450, 900)), 3));
        assert!(!should_finish_after_model_error(Some((45, 900)), 0));
        assert_eq!(model_request_attempts(0), MODEL_REQUEST_INITIAL_ATTEMPTS);
        assert_eq!(model_request_attempts(1), MODEL_REQUEST_PROGRESS_ATTEMPTS);
    }

    #[test]
    fn endpoint_accepts_base_or_full_chat_completions_url() {
        assert_eq!(
            chat_completions_endpoint("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("http://localhost:8000/v1/chat/completions"),
            "http://localhost:8000/v1/chat/completions"
        );
    }

    /// Pins the eval-scoring compaction contract on the LIVE path: since the
    /// flip this is `CharBudgetCompactor`, not the sidecar's old
    /// `compact_messages`. Keep [contract, task], replace the middle with a
    /// digest, keep the most recent tool round, land under budget.
    #[test]
    fn context_compaction_preserves_contract_task_and_recent_tool_round() {
        use codefactory_agent_loop::services::ContextCompactor;
        use codefactory_agent_loop::types::{ChatMessage, FunctionCall, MessageContent, ToolCall};

        fn body_of(m: &ChatMessage) -> &str {
            match &m.content {
                MessageContent::Text(t) => t.as_str(),
                MessageContent::Parts(_) => "",
            }
        }
        fn text(role: &str, body: &str) -> ChatMessage {
            ChatMessage {
                role: role.into(),
                content: MessageContent::Text(body.into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            }
        }
        fn calls(id: &str, command: &str) -> ChatMessage {
            ChatMessage {
                role: "assistant".into(),
                content: MessageContent::Text(String::new()),
                tool_calls: Some(vec![ToolCall {
                    id: id.into(),
                    r#type: "function".into(),
                    function: FunctionCall {
                        name: "run_shell".into(),
                        arguments: format!("{{\"command\":\"{command}\"}}"),
                    },
                }]),
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            }
        }
        fn result(id: &str, body: &str) -> ChatMessage {
            ChatMessage {
                role: "tool".into(),
                content: MessageContent::Text(body.into()),
                tool_calls: None,
                tool_call_id: Some(id.into()),
                name: None,
                reasoning_content: None,
            }
        }

        let messages = vec![
            text("system", "shared contract"),
            text("user", "original task"),
            calls("old", "cat huge"),
            result("old", &"x".repeat(2000)),
            calls("recent", "cargo test"),
            result("recent", "all tests passed"),
        ];
        let history = std::sync::Arc::new(tokio::sync::Mutex::new(vec![
            ToolHistoryEntry::new("cat huge", Some(0), "x".repeat(2000), "", None),
            ToolHistoryEntry::new("cargo test", Some(0), "all tests passed", "", None),
        ]));

        let compactor = loop_services::CharBudgetCompactor {
            max_chars: 500,
            history,
        };
        let out = compactor.compact(messages, "", 0);

        assert!(out.compacted);
        assert_eq!(body_of(&out.messages[0]), "shared contract");
        assert_eq!(body_of(&out.messages[1]), "original task");
        assert!(body_of(&out.messages[2]).contains("Compacted execution history"));
        assert!(out
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("recent")));
        assert!(serde_json::to_string(&out.messages).unwrap().len() < 1600);
    }

    #[test]
    fn tool_result_content_is_bounded_before_model_replay() {
        let content = tool_result_content(Some(0), &"x".repeat(50_000), "", None);
        assert!(content.len() < 15_000);
        assert!(content.contains("truncated"));
    }

    #[test]
    fn truncated_tool_output_preserves_head_and_tail() {
        let output = format!("BEGIN:{}:END", "x".repeat(10_000));
        let truncated = truncate_for_model(&output, 1_000);

        assert!(truncated.contains("BEGIN"));
        assert!(truncated.contains("END"));
        assert!(truncated.contains("truncated"));
    }

    #[tokio::test]
    async fn model_loop_finalizes_without_tools_after_completion_evidence() {
        let (base_url, server) = fake_openai_server(vec![
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "mutation-1",
                            "type": "function",
                            "function": {
                                "name": "run_shell",
                                "arguments": "{\"command\":\"printf fixed > result.txt\",\"timeout_sec\":5}"
                            }
                        }]
                    }
                }],
                "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
            }),
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "verify-1",
                            "type": "function",
                            "function": {
                                "name": "run_shell",
                                "arguments": "{\"command\":\"cargo test\",\"timeout_sec\":30}"
                            }
                        }]
                    }
                }],
                "usage": {"prompt_tokens": 20, "completion_tokens": 3, "total_tokens": 23}
            }),
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "Verified and complete."
                    }
                }],
                "usage": {"prompt_tokens": 30, "completion_tokens": 4, "total_tokens": 34}
            }),
        ]);

        let (test_input, run_input) = tokio::io::duplex(16 * 1024);
        let (run_output, test_output) = tokio::io::duplex(16 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Fix the project and verify it.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 4,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        let first = read_test_output(&mut output).await;
        assert_eq!(first["type"], "tool_request");
        assert_eq!(first["id"], "mutation-1");
        assert_eq!(first["usage"]["model_requests"], 1);
        assert_eq!(first["usage"]["total_tokens"], 12);
        write_test_line(
            &mut input,
            &json!({
                "type": "tool_result",
                "id": "mutation-1",
                "return_code": 0,
                "stdout": "",
                "stderr": "",
                "error": null
            }),
        )
        .await;

        let second = read_test_output(&mut output).await;
        assert_eq!(second["type"], "tool_request");
        assert_eq!(second["id"], "verify-1");
        assert_eq!(second["usage"]["model_requests"], 2);
        assert_eq!(second["usage"]["total_tokens"], 35);
        write_test_line(
            &mut input,
            &json!({
                "type": "tool_result",
                "id": "verify-1",
                "return_code": 0,
                "stdout": "all tests passed",
                "stderr": "",
                "error": null
            }),
        )
        .await;

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["final_text"], "Verified and complete.");
        assert_eq!(finished["completion_evidence"]["completed"], true);
        assert_eq!(
            finished["execution_contract_sha256"],
            execution_contract_sha256()
        );
        assert_eq!(finished["usage"]["model_requests"], 3);

        runner.await.unwrap().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2]["tool_choice"], "none");
        assert!(requests[2].get("tools").is_none());
    }

    #[tokio::test]
    async fn explicit_output_rejects_print_only_probe_until_machine_assertion() {
        let (base_url, server) = fake_openai_server(vec![
            fake_tool_response("mutation-1", "printf fixed > result.txt"),
            fake_tool_response("runtime-1", "./tool 6"),
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "The command ran successfully."
                    }
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
            fake_tool_response("assert-1", "actual=$(./tool 6); test \"$actual\" = 42"),
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "Repaired and machine-verified the expected output."
                    }
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        ]);

        let (test_input, run_input) = tokio::io::duplex(16 * 1024);
        let (run_output, test_output) = tokio::io::duplex(16 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Repair the CLI. Running ./tool 6 should output 42.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 8,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        let mutation = read_test_output(&mut output).await;
        assert_eq!(mutation["type"], "tool_request");
        assert_eq!(mutation["id"], "mutation-1");
        write_test_line(
            &mut input,
            &json!({
                "type": "tool_result",
                "id": "mutation-1",
                "return_code": 0,
                "stdout": "",
                "stderr": "",
                "error": null
            }),
        )
        .await;

        let usage_snapshot = read_test_output(&mut output).await;
        assert_eq!(usage_snapshot["type"], "event");
        assert_eq!(usage_snapshot["name"], "usage_snapshot");
        assert_eq!(usage_snapshot["usage"]["model_requests"], 2);

        let recovery_snapshot = read_test_output(&mut output).await;
        assert_eq!(recovery_snapshot["type"], "event");
        assert_eq!(recovery_snapshot["name"], "usage_snapshot");
        assert_eq!(recovery_snapshot["usage"]["model_requests"], 3);

        let assertion = read_test_output(&mut output).await;
        assert_eq!(assertion["type"], "tool_request");
        assert_eq!(assertion["id"], "assert-1");
        write_test_line(
            &mut input,
            &json!({
                "type": "tool_result",
                "id": "assert-1",
                "return_code": 0,
                "stdout": "",
                "stderr": "",
                "error": null
            }),
        )
        .await;

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["completed"], true);
        assert_eq!(
            finished["completion_evidence"]["machine_checked_behavior_required"],
            true
        );
        assert_eq!(
            finished["completion_evidence"]["last_machine_checked_verification_sequence"],
            2
        );
        assert_eq!(finished["completion_evidence"]["outcome_count"], 2);

        runner.await.unwrap().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 5);
        let denied_runtime = requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["tool_call_id"] == "runtime-1")
            .unwrap();
        assert!(denied_runtime["content"]
            .as_str()
            .unwrap()
            .contains("machine-checked verification"));
        let recovery_messages = requests[3]["messages"].as_array().unwrap();
        assert!(recovery_messages.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains("machine-checked assertion"))
        }));
    }

    #[tokio::test]
    async fn rejected_text_only_completion_requires_tool_or_stops_after_one_retry() {
        let (base_url, server) = fake_openai_server(vec![
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Done without evidence."}}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
            }),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Still no tool call."}}],
                "usage": {"prompt_tokens": 6, "completion_tokens": 2, "total_tokens": 8}
            }),
        ]);

        let (test_input, run_input) = tokio::io::duplex(16 * 1024);
        let (run_output, test_output) = tokio::io::duplex(16 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Repair the implementation and verify it.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 6,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        let first_snapshot = read_test_output(&mut output).await;
        assert_eq!(first_snapshot["type"], "event");
        assert_eq!(first_snapshot["usage"]["model_requests"], 1);

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["completed"], false);
        assert_eq!(finished["usage"]["model_requests"], 2);

        runner.await.unwrap().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["tool_choice"], "auto");
        assert_eq!(requests[1]["tool_choice"], "required");
    }

    #[tokio::test]
    async fn required_tool_choice_rejection_falls_back_to_auto_once() {
        let (base_url, server) = fake_openai_server(vec![
            json!({
                "__status": 400,
                "__body": {
                    "error": {"message": "Thinking mode does not support this tool_choice"}
                }
            }),
            fake_tool_response("repair-1", "printf repaired > result.txt"),
        ]);
        let config = StartConfig {
            instruction: "repair".to_owned(),
            model: "fake-model".to_owned(),
            api_key: "test-key".to_owned(),
            base_url,
            max_steps: 2,
            model_timeout_sec: 5,
            shell_timeout_sec: 30,
            wall_time_budget_sec: None,
            working_directory: Some("/workspace".to_owned()),
            allow_network: false,
            policy_profile: RuntimePolicyProfile::Product,
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let response = request_model(
            &client,
            &chat_completions_endpoint(&config.base_url),
            &config,
            &[json!({"role": "user", "content": "repair"})],
            true,
            true,
            5,
            1,
            None,
        )
        .await
        .unwrap();
        assert!(response["choices"][0]["message"]["tool_calls"].is_array());

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["tool_choice"], "required");
        assert_eq!(requests[1]["tool_choice"], "auto");
    }

    #[tokio::test]
    async fn policy_denied_recovery_tool_does_not_reopen_text_recovery() {
        let (base_url, server) = fake_openai_server(vec![
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Done."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
            fake_tool_response("denied-network", "curl https://example.com"),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Still done."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        ]);
        let (test_input, run_input) = tokio::io::duplex(16 * 1024);
        let (run_output, test_output) = tokio::io::duplex(16 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);
        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Repair and verify the implementation.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 10,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 30,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        let first_snapshot = read_test_output(&mut output).await;
        assert_eq!(first_snapshot["type"], "event");
        let denial_snapshot = read_test_output(&mut output).await;
        assert_eq!(denial_snapshot["type"], "event");
        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["completed"], false);

        runner.await.unwrap().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["tool_choice"], "auto");
        assert_eq!(requests[1]["tool_choice"], "required");
        assert_eq!(requests[2]["tool_choice"], "auto");
    }

    #[tokio::test]
    async fn failed_recovery_tool_does_not_reopen_text_recovery() {
        let (base_url, server) = fake_openai_server(vec![
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Done."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
            fake_tool_response("failed-check", "cargo test worker::tests::behavior"),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Still done."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        ]);
        let (test_input, run_input) = tokio::io::duplex(16 * 1024);
        let (run_output, test_output) = tokio::io::duplex(16 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);
        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Repair and verify the implementation.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 10,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 30,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        assert_eq!(read_test_output(&mut output).await["type"], "event");
        let tool_request = read_test_output(&mut output).await;
        assert_eq!(tool_request["type"], "tool_request");
        assert_eq!(tool_request["id"], "failed-check");
        write_test_line(
            &mut input,
            &json!({
                "type": "tool_result",
                "id": "failed-check",
                "return_code": 1,
                "stdout": "",
                "stderr": "assertion failed",
                "error": null
            }),
        )
        .await;

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["completed"], false);
        assert_eq!(finished["usage"]["model_requests"], 3);

        runner.await.unwrap().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["tool_choice"], "auto");
        assert_eq!(requests[1]["tool_choice"], "required");
        assert_eq!(requests[2]["tool_choice"], "auto");
    }

    #[tokio::test]
    async fn host_wall_reserve_stops_remaining_tool_calls_in_same_model_response() {
        let (base_url, server) = fake_openai_server(vec![json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "mutation-1",
                            "type": "function",
                            "function": {
                                "name": "run_shell",
                                "arguments": "{\"command\":\"printf fixed > result.txt\",\"timeout_sec\":5}"
                            }
                        },
                        {
                            "id": "must-not-run",
                            "type": "function",
                            "function": {
                                "name": "run_shell",
                                "arguments": "{\"command\":\"cargo test\",\"timeout_sec\":30}"
                            }
                        }
                    ]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
        })]);

        let (test_input, run_input) = tokio::io::duplex(16 * 1024);
        let (run_output, test_output) = tokio::io::duplex(16 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Fix the project and verify the result.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 2,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "wall_time_budget_sec": 32,
                "allow_network": false,
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        let first = read_test_output(&mut output).await;
        assert_eq!(first["type"], "tool_request");
        assert_eq!(first["id"], "mutation-1");
        tokio::time::sleep(Duration::from_secs(3)).await;
        write_test_line(
            &mut input,
            &json!({
                "type": "tool_result",
                "id": "mutation-1",
                "return_code": 0,
                "stdout": "",
                "stderr": "",
                "error": null
            }),
        )
        .await;

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["completed"], false);
        assert_eq!(finished["usage"]["model_requests"], 1);

        runner.await.unwrap().unwrap();
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn example_only_assertions_require_an_independent_behavior_check() {
        let (base_url, server) = fake_openai_server(vec![
            fake_tool_response("mutation-1", "printf fixed > result.txt"),
            fake_tool_response(
                "examples-1",
                "test \"$(./tool 3)\" = 9 && test \"$(./tool 5)\" = 25",
            ),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Examples pass, complete."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
            fake_tool_response("independent-1", "test \"$(./tool 7)\" = 49"),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Verified beyond the examples."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        ]);

        let (test_input, run_input) = tokio::io::duplex(32 * 1024);
        let (run_output, test_output) = tokio::io::duplex(32 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Implement behavior for arbitrary inputs. For example, ./tool 3 should output 9, and ./tool 5 should output 25.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 8,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        let mutation = read_test_output(&mut output).await;
        assert_eq!(mutation["id"], "mutation-1");
        write_test_line(
            &mut input,
            &json!({"type": "tool_result", "id": "mutation-1", "return_code": 0, "stdout": "", "stderr": "", "error": null}),
        )
        .await;

        let examples = read_test_output(&mut output).await;
        assert_eq!(examples["id"], "examples-1");
        write_test_line(
            &mut input,
            &json!({"type": "tool_result", "id": "examples-1", "return_code": 0, "stdout": "", "stderr": "", "error": null}),
        )
        .await;

        let recovery_snapshot = read_test_output(&mut output).await;
        assert_eq!(recovery_snapshot["type"], "event");
        assert_eq!(recovery_snapshot["name"], "usage_snapshot");

        let independent = read_test_output(&mut output).await;
        assert_eq!(independent["id"], "independent-1");
        write_test_line(
            &mut input,
            &json!({"type": "tool_result", "id": "independent-1", "return_code": 0, "stdout": "", "stderr": "", "error": null}),
        )
        .await;

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["completed"], true);
        assert_eq!(
            finished["completion_evidence"]["last_example_only_verification_sequence"],
            2
        );
        assert_eq!(
            finished["completion_evidence"]["last_independent_verification_sequence"],
            3
        );
        assert_eq!(finished["completion_evidence"]["outcome_count"], 3);

        runner.await.unwrap().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 5);
        assert!(requests[3]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("examples are smoke tests"))
            }));
    }

    #[tokio::test]
    async fn denied_attempt_does_not_desynchronize_the_fix_verification_loop() {
        let (base_url, server) = fake_openai_server(vec![
            fake_tool_response("mutation-1", "printf fixed > result.txt"),
            fake_tool_response("mutation-denied-1", "printf idea-2 > result.txt"),
            fake_tool_response("assert-failed", "actual=$(./tool 6); test \"$actual\" = 42"),
            fake_tool_response("repair-1", "printf repaired > result.txt"),
            fake_tool_response("mutation-denied-2", "printf idea-3 > result.txt"),
            fake_tool_response("assert-passed", "actual=$(./tool 6); test \"$actual\" = 42"),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Repaired and verified."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        ]);

        let (test_input, run_input) = tokio::io::duplex(32 * 1024);
        let (run_output, test_output) = tokio::io::duplex(32 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Repair the CLI. Running ./tool 6 should output 42.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 10,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        let mutation = read_test_output(&mut output).await;
        assert_eq!(mutation["id"], "mutation-1");
        write_test_line(
            &mut input,
            &json!({"type": "tool_result", "id": "mutation-1", "return_code": 0, "stdout": "", "stderr": "", "error": null}),
        )
        .await;

        let first_denial_snapshot = read_test_output(&mut output).await;
        assert_eq!(first_denial_snapshot["type"], "event");
        assert_eq!(first_denial_snapshot["usage"]["model_requests"], 2);

        let failed_assertion = read_test_output(&mut output).await;
        assert_eq!(failed_assertion["id"], "assert-failed");
        write_test_line(
            &mut input,
            &json!({"type": "tool_result", "id": "assert-failed", "return_code": 1, "stdout": "", "stderr": "mismatch", "error": null}),
        )
        .await;

        let repair = read_test_output(&mut output).await;
        assert_eq!(repair["id"], "repair-1");
        write_test_line(
            &mut input,
            &json!({"type": "tool_result", "id": "repair-1", "return_code": 0, "stdout": "", "stderr": "", "error": null}),
        )
        .await;

        let second_denial_snapshot = read_test_output(&mut output).await;
        assert_eq!(second_denial_snapshot["type"], "event");
        assert_eq!(second_denial_snapshot["usage"]["model_requests"], 5);

        let passed_assertion = read_test_output(&mut output).await;
        assert_eq!(passed_assertion["id"], "assert-passed");
        write_test_line(
            &mut input,
            &json!({"type": "tool_result", "id": "assert-passed", "return_code": 0, "stdout": "", "stderr": "", "error": null}),
        )
        .await;

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["outcome_count"], 4);
        assert_eq!(finished["completion_evidence"]["last_mutation_sequence"], 3);
        assert_eq!(
            finished["completion_evidence"]["last_machine_checked_verification_sequence"],
            4
        );
        assert_eq!(finished["completion_evidence"]["completed"], true);

        runner.await.unwrap().unwrap();
        assert_eq!(server.join().unwrap().len(), 7);
    }

    #[tokio::test]
    async fn final_stage_denies_second_read_after_failure_until_repair() {
        let (base_url, server) = fake_openai_server(vec![
            fake_tool_response("mutation-1", "printf candidate > result.txt"),
            fake_tool_response("verify-failed", "cargo test focused_behavior"),
            fake_tool_response("diagnostic-1", "sed -n '1,120p' src/worker.rs"),
            fake_tool_response("diagnostic-denied", "cat src/another_module.rs"),
            fake_tool_response("repair-1", "printf repaired > result.txt"),
            fake_tool_response("verify-passed", "cargo test focused_behavior"),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Repaired and verified."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        ]);

        let (test_input, run_input) = tokio::io::duplex(32 * 1024);
        let (run_output, test_output) = tokio::io::duplex(32 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Repair the implementation and verify the behavior.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 10,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        for (id, return_code, stdout, stderr) in [
            ("mutation-1", 0, "", ""),
            ("verify-failed", 1, "", "assertion failed"),
            ("diagnostic-1", 0, "relevant source", ""),
        ] {
            let request = read_test_output(&mut output).await;
            assert_eq!(request["type"], "tool_request");
            assert_eq!(request["id"], id);
            write_test_line(
                &mut input,
                &json!({
                    "type": "tool_result",
                    "id": id,
                    "return_code": return_code,
                    "stdout": stdout,
                    "stderr": stderr,
                    "error": null
                }),
            )
            .await;
        }

        let denial_snapshot = read_test_output(&mut output).await;
        assert_eq!(denial_snapshot["type"], "event");
        assert_eq!(denial_snapshot["name"], "usage_snapshot");
        assert_eq!(denial_snapshot["usage"]["model_requests"], 4);

        for id in ["repair-1", "verify-passed"] {
            let request = read_test_output(&mut output).await;
            assert_eq!(request["type"], "tool_request");
            assert_eq!(request["id"], id);
            write_test_line(
                &mut input,
                &json!({
                    "type": "tool_result",
                    "id": id,
                    "return_code": 0,
                    "stdout": if id == "verify-passed" { "test passed" } else { "" },
                    "stderr": "",
                    "error": null
                }),
            )
            .await;
        }

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["completed"], true);
        assert_eq!(
            finished["completion_evidence"]["last_failure_diagnostic_sequence"],
            3
        );

        runner.await.unwrap().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 7);
        let denied_result = requests[4]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["tool_call_id"] == "diagnostic-denied")
            .unwrap();
        assert!(denied_result["content"]
            .as_str()
            .unwrap()
            .contains("failure_repair_loop"));
    }

    #[tokio::test]
    async fn malformed_tool_response_persists_usage_before_fatal_exit() {
        let (base_url, server) = fake_openai_server(vec![json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "malformed-1",
                        "type": "function",
                        "function": {"name": "run_shell", "arguments": "{}"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
        })]);

        let (test_input, run_input) = tokio::io::duplex(8 * 1024);
        let (run_output, test_output) = tokio::io::duplex(8 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Repair the CLI.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 2,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        let snapshot = read_test_output(&mut output).await;
        assert_eq!(snapshot["type"], "event");
        assert_eq!(snapshot["name"], "usage_snapshot");
        assert_eq!(snapshot["usage"]["model_requests"], 1);
        assert_eq!(snapshot["usage"]["total_tokens"], 10);

        // Transport failures now reach `main` through `LoopError`, so the typed
        // variant collapses into `Loop`. What the contract actually pins is the
        // operator-visible stderr line and the non-zero exit — assert the text
        // itself rather than the discriminant.
        let error = runner.await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), HeadlessError::MissingCommand.to_string());
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn post_mutation_inspection_budget_forces_action_before_more_reads() {
        let (base_url, server) = fake_openai_server(vec![
            fake_tool_response("mutation-1", "printf fixed > result.txt"),
            fake_tool_response("read-1", "cat source-1.txt"),
            fake_tool_response("read-2", "cat source-2.txt"),
            fake_tool_response("read-3", "cat source-3.txt"),
            fake_tool_response("read-4", "cat source-4.txt"),
            fake_tool_response("read-denied", "cat source-5.txt"),
            fake_tool_response("verify-1", "timeout 5 curl http://localhost:8000/health"),
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "Changed the implementation and verified the runtime behavior."
                    }
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        ]);

        let (test_input, run_input) = tokio::io::duplex(32 * 1024);
        let (run_output, test_output) = tokio::io::duplex(32 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Fix the implementation and verify the running service.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 20,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "policy_profile": "product",
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        assert_eq!(
            classify_command("printf fixed > result.txt", 30_000),
            codefactory_agent_core::ToolKind::Mutation
        );
        for expected_id in ["mutation-1", "read-1", "read-2", "read-3"] {
            let request = read_test_output(&mut output).await;
            assert_eq!(request["type"], "tool_request");
            assert_eq!(request["id"], expected_id);
            write_test_line(
                &mut input,
                &json!({
                    "type": "tool_result",
                    "id": expected_id,
                    "return_code": 0,
                    "stdout": "observed",
                    "stderr": "",
                    "error": null
                }),
            )
            .await;
        }

        let mut verification = read_test_output(&mut output).await;
        if verification["id"] == "read-4" {
            write_test_line(
                &mut input,
                &json!({
                    "type": "tool_result",
                    "id": "read-4",
                    "return_code": 0,
                    "stdout": "observed",
                    "stderr": "",
                    "error": null
                }),
            )
            .await;
            verification = read_test_output(&mut output).await;
        }
        assert_eq!(verification["type"], "event");
        assert_eq!(verification["name"], "usage_snapshot");
        assert_eq!(verification["usage"]["model_requests"], 6);
        verification = read_test_output(&mut output).await;
        assert_eq!(verification["type"], "tool_request");
        assert_eq!(verification["id"], "verify-1");
        write_test_line(
            &mut input,
            &json!({
                "type": "tool_result",
                "id": "verify-1",
                "return_code": 0,
                "stdout": "service healthy",
                "stderr": "",
                "error": null
            }),
        )
        .await;

        let finished = read_test_output(&mut output).await;
        assert_eq!(finished["type"], "finished");
        assert_eq!(finished["completion_evidence"]["completed"], true);
        assert_eq!(finished["completion_evidence"]["outcome_count"], 6);
        assert_eq!(finished["usage"]["model_requests"], 8);

        runner.await.unwrap().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 8);
        let denied_tool_result = requests[6]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["tool_call_id"] == "read-denied")
            .unwrap();
        assert!(
            denied_tool_result["content"]
                .as_str()
                .unwrap()
                .contains("post-change inspection is exhausted"),
            "{denied_tool_result}"
        );
    }

    fn fake_tool_response(id: &str, command: &str) -> Value {
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": "run_shell",
                            "arguments": json!({"command": command, "timeout_sec": 30}).to_string()
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
    }

    async fn write_test_line<W>(writer: &mut W, value: &Value)
    where
        W: AsyncWrite + Unpin,
    {
        writer
            .write_all(value.to_string().as_bytes())
            .await
            .unwrap();
        writer.write_all(b"\n").await.unwrap();
        writer.flush().await.unwrap();
    }

    /// The bridge contract the Python harness depends on: EVERY model round must
    /// put at least one usage-carrying line on the wire (`tool_request` carries
    /// usage inline, otherwise a `usage_snapshot` stands in), because
    /// `codefactory_bench/agent.py` reads per-round cost from that stream. Two
    /// separate seams uphold it now that the sidecar shares the desktop's loop —
    /// the tool backend on the tool path, `round_ended` on every other path — so
    /// this states the rule directly instead of leaving it implied by the
    /// line-by-line expectations in the scenario tests.
    #[tokio::test]
    async fn every_model_round_puts_usage_on_the_wire() {
        fn tool_round(id: &str, command: &str, total: u64) -> Value {
            json!({
                "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [{
                    "id": id,
                    "type": "function",
                    "function": {"name": "run_shell", "arguments": format!("{{\"command\":\"{command}\",\"timeout_sec\":5}}")}
                }]}}],
                "usage": {"prompt_tokens": total, "completion_tokens": 1, "total_tokens": total + 1}
            })
        }

        // Round 1 mutates, round 2 is a text-only reply the gate REJECTS (the
        // round that emits no tool_request — historically the easy one to lose),
        // round 3 verifies, round 4 closes the run out.
        let (base_url, server) = fake_openai_server(vec![
            tool_round("mutate-1", "printf fixed > result.txt", 10),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "All done."}}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 1, "total_tokens": 21}
            }),
            tool_round("verify-1", "cargo test", 30),
            json!({
                "choices": [{"message": {"role": "assistant", "content": "Verified and complete."}}],
                "usage": {"prompt_tokens": 40, "completion_tokens": 1, "total_tokens": 41}
            }),
        ]);

        let (test_input, run_input) = tokio::io::duplex(16 * 1024);
        let (run_output, test_output) = tokio::io::duplex(16 * 1024);
        let runner = tokio::spawn(async move {
            let input = BufReader::new(run_input);
            let output = run_output;
            run(input, output).await
        });
        let mut input = test_input;
        let mut output = BufReader::new(test_output);

        write_test_line(
            &mut input,
            &json!({
                "type": "start",
                "instruction": "Fix the project and verify it.",
                "model": "fake-model",
                "api_key": "test-key",
                "base_url": base_url,
                "max_steps": 6,
                "model_timeout_sec": 5,
                "shell_timeout_sec": 60,
                "allow_network": false,
                "execution_contract_sha256": execution_contract_sha256()
            }),
        )
        .await;

        // Drain to `finished`, answering tool requests and recording which
        // lines carried usage.
        let mut lines = Vec::new();
        loop {
            let line = read_test_output(&mut output).await;
            let kind = line["type"].as_str().unwrap_or_default().to_string();
            lines.push(line.clone());
            match kind.as_str() {
                "tool_request" => {
                    write_test_line(
                        &mut input,
                        &json!({
                            "type": "tool_result",
                            "id": line["id"],
                            "return_code": 0,
                            "stdout": "",
                            "stderr": "",
                            "error": null
                        }),
                    )
                    .await;
                }
                "finished" => break,
                _ => {}
            }
        }
        runner.await.unwrap().unwrap();

        // Every line the sidecar writes carries usage, and the model_requests
        // counter never goes backwards.
        let mut seen_requests = 0_u64;
        for line in &lines {
            let usage = line
                .get("usage")
                .unwrap_or_else(|| panic!("line without usage: {line}"));
            let requests = usage["model_requests"].as_u64().unwrap();
            assert!(
                requests >= seen_requests,
                "model_requests went backwards: {seen_requests} -> {requests}"
            );
            seen_requests = requests;
        }

        // Four model rounds happened, and the wire accounts for all four.
        assert_eq!(server.join().unwrap().len(), 4);
        assert_eq!(seen_requests, 4);
        // The rejected text-only round contributed a standalone snapshot: it
        // wrote no tool_request, so without `round_ended` its cost would be
        // invisible to the bridge until the next line.
        assert!(
            lines
                .iter()
                .any(|l| l["type"] == "event" && l["name"] == "usage_snapshot"),
            "the gate-rejected round emitted no usage_snapshot"
        );
    }

    async fn read_test_output<R>(reader: &mut R) -> Value
    where
        R: AsyncBufRead + Unpin,
    {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn fake_openai_server(responses: Vec<Value>) -> (String, thread::JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut captured = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if complete_http_request(&request) {
                        break;
                    }
                }
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                captured.push(serde_json::from_slice(&request[header_end + 4..]).unwrap());
                let status = response
                    .get("__status")
                    .and_then(Value::as_u64)
                    .unwrap_or(200);
                let body = response.get("__body").unwrap_or(&response).to_string();
                let reason = if status == 200 { "OK" } else { "Bad Request" };
                let reply = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(reply.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
            captured
        });
        (format!("http://{address}/v1"), handle)
    }

    fn complete_http_request(request: &[u8]) -> bool {
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        request.len() >= header_end + 4 + content_length
    }

    #[tokio::test]
    async fn model_request_retries_when_response_body_is_truncated() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            read_http_request(&mut first);
            first
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 200\r\nConnection: close\r\n\r\n{\"choices\":",
                )
                .unwrap();
            first.flush().unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            read_http_request(&mut second);
            let body = json!({
                "choices": [{"message": {"role": "assistant", "content": "done"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })
            .to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            second.write_all(reply.as_bytes()).unwrap();
            second.flush().unwrap();
        });

        let config = StartConfig {
            instruction: "task".to_owned(),
            model: "fake-model".to_owned(),
            api_key: "test-key".to_owned(),
            base_url: format!("http://{address}/v1"),
            max_steps: 1,
            model_timeout_sec: 5,
            shell_timeout_sec: 30,
            wall_time_budget_sec: None,
            working_directory: Some("/workspace".to_owned()),
            allow_network: false,
            policy_profile: RuntimePolicyProfile::Benchmark,
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let response = request_model(
            &client,
            &chat_completions_endpoint(&config.base_url),
            &config,
            &[json!({"role": "user", "content": "task"})],
            true,
            false,
            5,
            MODEL_REQUEST_INITIAL_ATTEMPTS,
            None,
        )
        .await
        .unwrap();

        assert_eq!(response["choices"][0]["message"]["content"], "done");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn model_request_recovers_after_four_truncated_response_bodies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                request_tx
                    .send(read_http_request_text(&mut stream))
                    .unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 200\r\nConnection: close\r\n\r\n{\"choices\":",
                    )
                    .unwrap();
                stream.flush().unwrap();
            }

            let (mut stream, _) = listener.accept().unwrap();
            request_tx
                .send(read_http_request_text(&mut stream))
                .unwrap();
            let body = json!({
                "choices": [{"message": {"role": "assistant", "content": "recovered"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })
            .to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(reply.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let config = StartConfig {
            instruction: "task".to_owned(),
            model: "fake-model".to_owned(),
            api_key: "test-key".to_owned(),
            base_url: format!("http://{address}/v1"),
            max_steps: 1,
            model_timeout_sec: 5,
            shell_timeout_sec: 30,
            wall_time_budget_sec: Some(30),
            working_directory: Some("/workspace".to_owned()),
            allow_network: false,
            policy_profile: RuntimePolicyProfile::Product,
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let response = request_model(
            &client,
            &chat_completions_endpoint(&config.base_url),
            &config,
            &[json!({"role": "user", "content": "task"})],
            true,
            false,
            5,
            MODEL_REQUEST_PROGRESS_ATTEMPTS,
            None,
        )
        .await
        .unwrap();

        assert_eq!(response["choices"][0]["message"]["content"], "recovered");
        server.join().unwrap();
        let requests = request_rx.into_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 5);
        assert!(requests.iter().all(|request| request
            .to_ascii_lowercase()
            .contains("accept-encoding: identity\r\n")));
    }

    #[tokio::test]
    async fn model_request_timeout_preserves_a_real_retry_window() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            read_http_request(&mut first);
            thread::sleep(Duration::from_millis(1_500));
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            read_http_request(&mut second);
            let body = json!({
                "choices": [{"message": {"role": "assistant", "content": "recovered"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })
            .to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = second.write_all(reply.as_bytes());
            let _ = second.flush();
        });

        let config = StartConfig {
            instruction: "task".to_owned(),
            model: "fake-model".to_owned(),
            api_key: "test-key".to_owned(),
            base_url: format!("http://{address}/v1"),
            max_steps: 1,
            model_timeout_sec: 1,
            shell_timeout_sec: 30,
            wall_time_budget_sec: None,
            working_directory: Some("/workspace".to_owned()),
            allow_network: false,
            policy_profile: RuntimePolicyProfile::Product,
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let response = request_model(
            &client,
            &chat_completions_endpoint(&config.base_url),
            &config,
            &[json!({"role": "user", "content": "task"})],
            true,
            false,
            1,
            MODEL_REQUEST_INITIAL_ATTEMPTS,
            None,
        )
        .await
        .unwrap();

        assert_eq!(response["choices"][0]["message"]["content"], "recovered");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn model_request_does_not_start_after_the_wall_deadline() {
        let config = StartConfig {
            instruction: "task".to_owned(),
            model: "fake-model".to_owned(),
            api_key: "test-key".to_owned(),
            base_url: "http://127.0.0.1:9/v1".to_owned(),
            max_steps: 1,
            model_timeout_sec: 5,
            shell_timeout_sec: 30,
            wall_time_budget_sec: Some(60),
            working_directory: Some("/workspace".to_owned()),
            allow_network: false,
            policy_profile: RuntimePolicyProfile::Product,
        };
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let error = request_model(
            &client,
            &chat_completions_endpoint(&config.base_url),
            &config,
            &[json!({"role": "user", "content": "task"})],
            true,
            false,
            5,
            MODEL_REQUEST_INITIAL_ATTEMPTS,
            Some(Instant::now() - Duration::from_millis(1)),
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            HeadlessError::ModelResponse {
                attempts: 0,
                ref detail
            } if detail.contains("wall-clock deadline")
        ));
    }

    fn read_http_request(stream: &mut std::net::TcpStream) {
        let _ = read_http_request_text(stream);
    }

    fn read_http_request_text(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..count]);
            if complete_http_request(&request) {
                break;
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }
}
