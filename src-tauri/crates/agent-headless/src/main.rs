use codefactory_agent_core::{
    build_budget_convergence_prompt, build_completion_ready_prompt,
    build_completion_recovery_prompt, build_product_system_prompt, build_system_prompt,
    build_time_convergence_prompt, classify_command, effective_command_timeout_sec,
    evaluate_budget_command_with_time_in_directory, execution_contract_sha256,
    sanitize_completion_summary, should_prompt_budget_convergence, should_prompt_time_convergence,
    BenchmarkPolicy, CompletionEvidence, CompletionGate, PolicyDecision, ProductPolicy,
    ProgressTracker, ToolOutcome,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::io::{
    self, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter,
};

const MAX_CONTEXT_CHARS: usize = 40_000;
const MAX_TOOL_STREAM_CHARS: usize = 3_000;
const MODEL_REQUEST_INITIAL_ATTEMPTS: usize = 3;
const MODEL_REQUEST_PROGRESS_ATTEMPTS: usize = 5;
const MODEL_ERROR_BODY_CHARS: usize = 1_000;

#[derive(Debug, Clone)]
struct ToolHistoryEntry {
    command: String,
    return_code: Option<i32>,
    stdout: String,
    stderr: String,
    error: Option<String>,
}

impl ToolHistoryEntry {
    fn new(
        command: impl Into<String>,
        return_code: Option<i32>,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
        error: Option<String>,
    ) -> Self {
        Self {
            command: command.into(),
            return_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
            error,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum InputMessage {
    #[serde(rename = "start")]
    Start {
        instruction: String,
        model: String,
        api_key: String,
        base_url: String,
        max_steps: u32,
        model_timeout_sec: u64,
        shell_timeout_sec: u64,
        #[serde(default)]
        wall_time_budget_sec: Option<u64>,
        #[serde(default)]
        working_directory: Option<String>,
        allow_network: bool,
        #[serde(default)]
        policy_profile: RuntimePolicyProfile,
        execution_contract_sha256: String,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        id: String,
        return_code: Option<i32>,
        stdout: String,
        stderr: String,
        error: Option<String>,
        #[serde(default)]
        next_working_directory: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
enum OutputMessage {
    #[serde(rename = "tool_request")]
    ToolRequest {
        id: String,
        command: String,
        timeout_sec: u64,
        usage: Usage,
    },
    #[serde(rename = "event")]
    UsageSnapshot { name: String, usage: Usage },
    #[serde(rename = "finished")]
    Finished {
        final_text: String,
        execution_contract_sha256: String,
        completion_evidence: CompletionEvidence,
        usage: Usage,
    },
}

#[derive(Debug, Clone)]
struct StartConfig {
    instruction: String,
    model: String,
    api_key: String,
    base_url: String,
    max_steps: u32,
    model_timeout_sec: u64,
    shell_timeout_sec: u64,
    wall_time_budget_sec: Option<u64>,
    working_directory: Option<String>,
    allow_network: bool,
    policy_profile: RuntimePolicyProfile,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RuntimePolicyProfile {
    Product,
    #[default]
    Benchmark,
}

enum RuntimePolicy {
    Product(ProductPolicy),
    Benchmark(BenchmarkPolicy),
}

impl RuntimePolicy {
    fn new(profile: RuntimePolicyProfile, allow_network: bool) -> Self {
        match profile {
            RuntimePolicyProfile::Product => Self::Product(ProductPolicy::new(allow_network)),
            RuntimePolicyProfile::Benchmark => Self::Benchmark(BenchmarkPolicy::new(allow_network)),
        }
    }

    fn evaluate_command(&self, command: &str) -> PolicyDecision {
        match self {
            Self::Product(policy) => policy.evaluate_command(command),
            Self::Benchmark(policy) => policy.evaluate_command(command),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Usage {
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

#[derive(Debug)]
struct ToolCall {
    id: String,
    command: String,
    timeout_sec: u64,
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
    let execution_started = Instant::now();
    let mut stopped_for_wall_budget = false;

    let max_steps = config.max_steps.max(1);
    for step_index in 0..max_steps {
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
        let model_request_attempts = model_request_attempts(tool_history.len());
        let response = match request_model(
            &client,
            &endpoint,
            &config,
            &messages,
            !finalization_response,
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
            continue;
        }

        finalization_pending = false;
        messages.push(message);
        let mut progress_prompt = None;
        let mut emitted_tool_request = false;
        let remaining = max_steps.saturating_sub(step_index + 1);
        for tool_call in tool_calls {
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

async fn read_start<R>(input: &mut R) -> Result<StartConfig, HeadlessError>
where
    R: AsyncBufRead + Unpin,
{
    let line = read_protocol_line(input)
        .await?
        .ok_or(HeadlessError::MissingStart)?;
    match serde_json::from_str::<InputMessage>(&line)? {
        InputMessage::Start {
            instruction,
            model,
            api_key,
            base_url,
            max_steps,
            model_timeout_sec,
            shell_timeout_sec,
            wall_time_budget_sec,
            working_directory,
            allow_network,
            policy_profile,
            execution_contract_sha256: bridge_hash,
        } => {
            let sidecar_hash = execution_contract_sha256();
            if bridge_hash != sidecar_hash {
                return Err(HeadlessError::ContractMismatch {
                    bridge: bridge_hash,
                    sidecar: sidecar_hash,
                });
            }
            Ok(StartConfig {
                instruction,
                model,
                api_key,
                base_url,
                max_steps,
                model_timeout_sec,
                shell_timeout_sec,
                wall_time_budget_sec,
                working_directory,
                allow_network,
                policy_profile,
            })
        }
        InputMessage::ToolResult { .. } => Err(HeadlessError::ExpectedStart),
    }
}

async fn read_tool_result<R>(
    input: &mut R,
    expected_id: &str,
) -> Result<(Option<i32>, String, String, Option<String>, Option<String>), HeadlessError>
where
    R: AsyncBufRead + Unpin,
{
    let line = read_protocol_line(input)
        .await?
        .ok_or_else(|| HeadlessError::MissingToolResult(expected_id.to_owned()))?;
    match serde_json::from_str::<InputMessage>(&line)? {
        InputMessage::ToolResult {
            id,
            return_code,
            stdout,
            stderr,
            error,
            next_working_directory,
        } if id == expected_id => Ok((return_code, stdout, stderr, error, next_working_directory)),
        InputMessage::ToolResult { id, .. } => Err(HeadlessError::UnexpectedToolResult {
            expected: expected_id.to_owned(),
            actual: id,
        }),
        InputMessage::Start { .. } => Err(HeadlessError::UnexpectedToolResult {
            expected: expected_id.to_owned(),
            actual: "start".to_owned(),
        }),
    }
}

async fn read_protocol_line<R>(input: &mut R) -> Result<Option<String>, HeadlessError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    let bytes = input.read_line(&mut line).await?;
    if bytes == 0 {
        Ok(None)
    } else {
        Ok(Some(line.trim_end().to_owned()))
    }
}

async fn write_output<W>(output: &mut W, message: &OutputMessage) -> Result<(), HeadlessError>
where
    W: AsyncWrite + Unpin,
{
    let mut serialized = serde_json::to_vec(message)?;
    serialized.push(b'\n');
    output.write_all(&serialized).await?;
    output.flush().await?;
    Ok(())
}

async fn request_model(
    client: &Client,
    endpoint: &str,
    config: &StartConfig,
    messages: &[Value],
    allow_tools: bool,
    attempt_timeout_sec: u64,
    max_attempts: usize,
    wall_deadline: Option<Instant>,
) -> Result<Value, HeadlessError> {
    let mut payload = json!({
        "model": config.model,
        "messages": messages,
        "tool_choice": if allow_tools { "auto" } else { "none" }
    });
    if allow_tools {
        payload["tools"] = json!([{
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
        }]);
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

fn is_retryable_model_error(error: &reqwest::Error) -> bool {
    error.is_timeout()
        || error.is_connect()
        || error.is_request()
        || error.is_body()
        || error.is_decode()
}

async fn wait_before_model_retry(attempt: usize, wall_deadline: Option<Instant>) {
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

fn model_request_attempts(tool_outcome_count: usize) -> usize {
    if tool_outcome_count == 0 {
        MODEL_REQUEST_INITIAL_ATTEMPTS
    } else {
        MODEL_REQUEST_PROGRESS_ATTEMPTS
    }
}

fn response_body_preview(body: &[u8]) -> String {
    truncate_for_model(&String::from_utf8_lossy(body), MODEL_ERROR_BODY_CHARS)
}

fn parse_tool_calls(
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

fn message_content(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(content)) => content.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn tool_result_content(
    return_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    error: Option<&str>,
) -> String {
    json!({
        "return_code": return_code,
        "stdout": truncate_for_model(stdout, MAX_TOOL_STREAM_CHARS),
        "stderr": truncate_for_model(stderr, MAX_TOOL_STREAM_CHARS),
        "error": error,
    })
    .to_string()
}

fn compact_messages(messages: &mut Vec<Value>, history: &[ToolHistoryEntry], max_chars: usize) {
    if messages.len() <= 3
        || serde_json::to_string(messages)
            .map(|value| value.len() <= max_chars)
            .unwrap_or(true)
    {
        return;
    }

    let recent_start = messages
        .iter()
        .enumerate()
        .skip(2)
        .rev()
        .find(|(_, message)| {
            message.get("role").and_then(Value::as_str) == Some("assistant")
                && message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_some()
        })
        .map(|(index, _)| index)
        .unwrap_or_else(|| messages.len().saturating_sub(2));

    let mut summary = String::from("Compacted execution history (oldest details omitted):\n");
    for (index, entry) in history
        .iter()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .enumerate()
    {
        let command = entry.command.lines().next().unwrap_or("");
        let output = if let Some(error) = &entry.error {
            error.as_str()
        } else if !entry.stderr.trim().is_empty() {
            entry.stderr.trim()
        } else {
            entry.stdout.trim()
        };
        summary.push_str(&format!(
            "{}. rc={:?} command={} output={}\n",
            index + 1,
            entry.return_code,
            truncate_for_model(command, 200),
            truncate_for_model(output, 400),
        ));
    }
    summary = truncate_for_model(&summary, max_chars.saturating_div(2).max(800));

    let system = messages[0].clone();
    let task = messages[1].clone();
    let recent = messages[recent_start..].to_vec();
    *messages = vec![system, task, json!({"role": "user", "content": summary})];
    messages.extend(recent);
}

fn truncate_for_model(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let head_limit = limit / 4;
    let tail_limit = limit.saturating_sub(head_limit);
    let head = value.chars().take(head_limit).collect::<String>();
    let tail = value
        .chars()
        .rev()
        .take(tail_limit)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!(
        "{head}\n[truncated middle; kept first {head_limit} and last {tail_limit} characters]\n{tail}"
    )
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn remaining_wall_time(started: Instant, wall_time_budget_sec: Option<u64>) -> Option<(u64, u64)> {
    let total = wall_time_budget_sec?.max(1);
    let remaining = total.saturating_sub(started.elapsed().as_secs());
    Some((remaining, total))
}

fn clamp_timeout_to_wall_reserve(requested: u64, remaining: u64, reserve: u64) -> u64 {
    requested.min(remaining.saturating_sub(reserve).max(1))
}

fn budget_exhaustion_message(stopped_for_wall_budget: bool) -> &'static str {
    if stopped_for_wall_budget {
        "Stopped because the wall-clock budget entered its final reserve before completion."
    } else {
        "Stopped because the model step budget was exhausted before completion."
    }
}

fn should_finish_after_model_error(wall_time: Option<(u64, u64)>, outcome_count: usize) -> bool {
    let Some((remaining, total)) = wall_time else {
        return false;
    };
    outcome_count > 0 && remaining <= (total / 15).max(60)
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
                let body = response.to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
