use codefactory_agent_core::{
    build_budget_convergence_prompt, build_completion_ready_prompt,
    build_completion_recovery_prompt, build_product_system_prompt, build_system_prompt,
    build_time_convergence_prompt, classify_command, completion_evidence_made_progress, evaluate_budget_command_with_time_in_directory,
    execution_contract_sha256, sanitize_completion_summary,
    should_prompt_budget_convergence, should_prompt_time_convergence, CompletionGate, PolicyDecision, ProgressTracker,
    ToolOutcome,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
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
}


#[tokio::main]
async fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = BufReader::new(stdin);
    let mut output = BufWriter::new(stdout);

    if let Err(error) = run(&mut input, &mut output).await {
        eprintln!("codefactory-agent-headless: {error}");
        std::process::exit(1);
    }
}

async fn run<R, W>(input: &mut R, output: &mut W) -> Result<(), HeadlessError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut config = read_start(input).await?;
    let client = Client::builder()
        .timeout(Duration::from_secs(config.model_timeout_sec.max(1)))
        .build()?;
    let endpoint = chat_completions_endpoint(&config.base_url);
    let policy = RuntimePolicy::new(config.policy_profile, config.allow_network);
    let system_prompt = match config.policy_profile {
        RuntimePolicyProfile::Product => build_product_system_prompt(config.allow_network),
        RuntimePolicyProfile::Benchmark => build_system_prompt(config.allow_network),
    };
    let mut messages = vec![
        json!({"role": "system", "content": system_prompt}),
        json!({"role": "user", "content": config.instruction}),
    ];
    let mut gate = CompletionGate::new_for_instruction(true, &config.instruction);
    let mut usage = Usage::default();
    let mut sequence = 0_u64;
    let mut last_final_text = String::new();
    let mut tool_history = Vec::new();
    let mut last_completion_nudge_sequence = None;
    let mut progress_tracker = ProgressTracker::new(4);
    let mut finalization_pending = false;
    let mut require_tool_next = false;
    let mut completion_recovery_attempts = 0_u32;
    let execution_started = Instant::now();
    let mut stopped_for_wall_budget = false;

    let max_steps = config.max_steps.max(1);
    'execution: for step_index in 0..max_steps {
        let wall_time = remaining_wall_time(execution_started, config.wall_time_budget_sec);
        if wall_time.is_some_and(|(remaining, _)| remaining <= 30) {
            stopped_for_wall_budget = true;
            break;
        }
        compact_messages(&mut messages, &tool_history, MAX_CONTEXT_CHARS);
        let request_timeout_sec = wall_time
            .map(|(remaining, _)| {
                clamp_timeout_to_wall_reserve(config.model_timeout_sec, remaining, 30)
            })
            .unwrap_or(config.model_timeout_sec);
        let model_wall_deadline = config.wall_time_budget_sec.map(|total| {
            execution_started + Duration::from_secs(total.max(1).saturating_sub(30).max(1))
        });
        let finalization_response = finalization_pending;
        let required_tool_response = require_tool_next && !finalization_response;
        let model_request_attempts = model_request_attempts(tool_history.len());
        let response = match request_model(
            &client,
            &endpoint,
            &config,
            &messages,
            !finalization_response,
            required_tool_response,
            request_timeout_sec,
            model_request_attempts,
            model_wall_deadline,
        )
        .await
        {
            Ok(response) => response,
            Err(error)
                if should_finish_after_model_error(
                    remaining_wall_time(execution_started, config.wall_time_budget_sec),
                    tool_history.len(),
                ) =>
            {
                last_final_text = format!(
                    "Stopped after a model transport failure in the final wall-clock reserve: {error}"
                );
                stopped_for_wall_budget = true;
                break;
            }
            Err(error) => return Err(error),
        };
        usage.add_response(&response);
        let message = match response["choices"]
            .get(0)
            .and_then(|choice| choice.get("message"))
            .cloned()
        {
            Some(message) => message,
            None => {
                write_output(
                    output,
                    &OutputMessage::UsageSnapshot {
                        name: "usage_snapshot".to_owned(),
                        usage: usage.clone(),
                    },
                )
                .await?;
                return Err(HeadlessError::MissingChoice);
            }
        };
        let final_text = message_content(&message);
        let mut tool_calls = match parse_tool_calls(&message, config.shell_timeout_sec) {
            Ok(tool_calls) => tool_calls,
            Err(error) => {
                write_output(
                    output,
                    &OutputMessage::UsageSnapshot {
                        name: "usage_snapshot".to_owned(),
                        usage: usage.clone(),
                    },
                )
                .await?;
                return Err(error);
            }
        };
        if finalization_response {
            finalization_pending = false;
            tool_calls.clear();
        }

        if tool_calls.is_empty() {
            last_final_text = if finalization_response {
                sanitize_completion_summary(&final_text)
            } else {
                final_text
            };
            let evidence = gate.evidence();
            if evidence.completed {
                write_output(
                    output,
                    &OutputMessage::Finished {
                        final_text: last_final_text,
                        execution_contract_sha256: execution_contract_sha256(),
                        completion_evidence: evidence,
                        usage,
                    },
                )
                .await?;
                return Ok(());
            }
            if required_tool_response || completion_recovery_attempts >= 1 {
                write_output(
                    output,
                    &OutputMessage::Finished {
                        final_text: last_final_text,
                        execution_contract_sha256: execution_contract_sha256(),
                        completion_evidence: evidence,
                        usage,
                    },
                )
                .await?;
                return Ok(());
            }
            write_output(
                output,
                &OutputMessage::UsageSnapshot {
                    name: "usage_snapshot".to_owned(),
                    usage: usage.clone(),
                },
            )
            .await?;
            messages.push(message);
            messages.push(json!({
                "role": "user",
                "content": build_completion_recovery_prompt(&evidence),
            }));
            completion_recovery_attempts += 1;
            require_tool_next = true;
            continue;
        }

        finalization_pending = false;
        require_tool_next = false;
        messages.push(message);
        let mut progress_prompt = None;
        let mut emitted_tool_request = false;
        let completion_evidence_before_tool_batch = gate.evidence();
        let remaining = max_steps.saturating_sub(step_index + 1);
        for tool_call in tool_calls {
            if remaining_wall_time(execution_started, config.wall_time_budget_sec)
                .is_some_and(|(remaining, _)| remaining <= 30)
            {
                stopped_for_wall_budget = true;
                break 'execution;
            }
            let started_at_ms = unix_time_ms();
            let wall_time = remaining_wall_time(execution_started, config.wall_time_budget_sec);
            let effective_tool_timeout_sec = wall_time
                .map(|(remaining, _)| {
                    clamp_timeout_to_wall_reserve(tool_call.timeout_sec, remaining, 30)
                })
                .unwrap_or(tool_call.timeout_sec);
            let kind = classify_command(&tool_call.command, effective_tool_timeout_sec * 1_000);
            let policy_decision = match policy.evaluate_command(&tool_call.command) {
                PolicyDecision::Allow
                    if progress_tracker.read_only_exhausted()
                        && matches!(kind, codefactory_agent_core::ToolKind::ReadOnly) =>
                {
                    PolicyDecision::Deny {
                        rule: "inspection_budget".to_owned(),
                        reason: if progress_tracker.mutation_seen() {
                            "post-change inspection is exhausted; make the smallest corrective edit, run a bounded functional verification, or batch a specifically justified read with that action"
                                .to_owned()
                        } else {
                            "initial inspection is exhausted; batch any remaining reads with the first implementation or begin the smallest candidate implementation now"
                                .to_owned()
                        },
                    }
                }
                PolicyDecision::Allow => evaluate_budget_command_with_time_in_directory(
                    remaining,
                    wall_time,
                    &gate.evidence(),
                    &tool_call.command,
                    &kind,
                    config.working_directory.as_deref(),
                ),
                denied => denied,
            };
            let (return_code, stdout, stderr, error, next_working_directory) = match policy_decision
            {
                PolicyDecision::Allow => {
                    sequence += 1;
                    write_output(
                        output,
                        &OutputMessage::ToolRequest {
                            id: tool_call.id.clone(),
                            command: tool_call.command.clone(),
                            timeout_sec: effective_tool_timeout_sec,
                            usage: usage.clone(),
                        },
                    )
                    .await?;
                    emitted_tool_request = true;
                    read_tool_result(input, &tool_call.id).await?
                }
                PolicyDecision::Deny { rule, reason } => (
                    None,
                    String::new(),
                    String::new(),
                    Some(format!("policy denied command ({rule}): {reason}")),
                    None,
                ),
            };

            let outcome = ToolOutcome {
                request_id: tool_call.id.clone(),
                command: tool_call.command.clone(),
                working_directory: config.working_directory.clone(),
                kind,
                sequence,
                started_at_ms,
                finished_at_ms: unix_time_ms(),
                return_code,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                error: error.clone(),
                semantic_failure: false,
            }
            .with_detected_semantic_failure();
            if let Some(next_working_directory) = next_working_directory
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty() && Path::new(path).is_absolute())
            {
                config.working_directory = Some(next_working_directory.to_owned());
            }
            let policy_denied = error
                .as_deref()
                .is_some_and(|message| message.starts_with("policy denied command"));
            if !policy_denied {
                gate.record(&outcome);
                if let Some(prompt) = progress_tracker.record(&outcome) {
                    progress_prompt = Some(prompt);
                }
            }
            tool_history.push(ToolHistoryEntry::new(
                &tool_call.command,
                return_code,
                &stdout,
                &stderr,
                error.clone(),
            ));

            messages.push(json!({
                "role": "tool",
                "tool_call_id": tool_call.id,
                "content": tool_result_content(return_code, &stdout, &stderr, error.as_deref()),
            }));
        }
        if !emitted_tool_request {
            write_output(
                output,
                &OutputMessage::UsageSnapshot {
                    name: "usage_snapshot".to_owned(),
                    usage: usage.clone(),
                },
            )
            .await?;
        } else {
            completion_recovery_attempts = completion_recovery_attempts_after_tool_batch(
                completion_recovery_attempts,
                completion_evidence_made_progress(
                    &completion_evidence_before_tool_batch,
                    &gate.evidence(),
                ),
            );
        }
        if let Some(prompt) = progress_prompt {
            messages.push(json!({"role": "user", "content": prompt}));
        }
        let evidence = gate.evidence();
        if evidence.completed
            && evidence.last_successful_verification_sequence != last_completion_nudge_sequence
        {
            last_completion_nudge_sequence = evidence.last_successful_verification_sequence;
            finalization_pending = true;
            messages.push(json!({
                "role": "user",
                "content": build_completion_ready_prompt(),
            }));
        } else {
            let wall_time = remaining_wall_time(execution_started, config.wall_time_budget_sec);
            if wall_time
                .is_some_and(|(seconds, total)| should_prompt_time_convergence(seconds, total))
            {
                let seconds = wall_time.map(|(seconds, _)| seconds).unwrap_or_default();
                messages.push(json!({
                    "role": "user",
                    "content": build_time_convergence_prompt(seconds, &evidence),
                }));
            } else if should_prompt_budget_convergence(remaining) {
                messages.push(json!({
                    "role": "user",
                    "content": build_budget_convergence_prompt(remaining, &evidence),
                }));
            }
        }
    }

    write_output(
        output,
        &OutputMessage::Finished {
            final_text: if last_final_text.is_empty() {
                budget_exhaustion_message(stopped_for_wall_budget).to_owned()
            } else {
                last_final_text
            },
            execution_contract_sha256: execution_contract_sha256(),
            completion_evidence: gate.evidence(),
            usage,
        },
    )
    .await?;
    Ok(())
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
        assert_eq!(completion_recovery_attempts_after_tool_batch(1, true), 1);
        assert_eq!(completion_recovery_attempts_after_tool_batch(1, false), 1);
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

    #[test]
    fn context_compaction_preserves_contract_task_and_recent_tool_round() {
        let mut messages = vec![
            json!({"role":"system","content":"shared contract"}),
            json!({"role":"user","content":"original task"}),
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"old","function":{"name":"run_shell","arguments":"{\"command\":\"cat huge\"}"}}]}),
            json!({"role":"tool","tool_call_id":"old","content":"x".repeat(2000)}),
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"recent","function":{"name":"run_shell","arguments":"{\"command\":\"cargo test\"}"}}]}),
            json!({"role":"tool","tool_call_id":"recent","content":"all tests passed"}),
        ];
        let history = vec![
            ToolHistoryEntry::new("cat huge", Some(0), "x".repeat(2000), "", None),
            ToolHistoryEntry::new("cargo test", Some(0), "all tests passed", "", None),
        ];

        compact_messages(&mut messages, &history, 500);

        assert_eq!(messages[0]["content"], "shared contract");
        assert_eq!(messages[1]["content"], "original task");
        assert!(messages[2]["content"]
            .as_str()
            .unwrap()
            .contains("Compacted execution history"));
        assert!(messages
            .iter()
            .any(|message| message["tool_call_id"] == "recent"));
        assert!(serde_json::to_string(&messages).unwrap().len() < 1600);
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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

        let error = runner.await.unwrap().unwrap_err();
        assert!(matches!(error, HeadlessError::MissingCommand));
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
            let mut input = BufReader::new(run_input);
            let mut output = run_output;
            run(&mut input, &mut output).await
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
