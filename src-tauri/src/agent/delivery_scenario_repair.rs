// SPDX-License-Identifier: Apache-2.0
//! E2E-004 synthetic provider/tool adapters around the production agent loop.
use super::*;
use codefactory_agent_loop::{run::*, services::*, tool::*, transport::*, types::*};
use std::sync::{Arc, Mutex};

pub(super) struct RepairTools {
    pub checkout: PathBuf,
    pub calls: Mutex<Vec<String>>,
    pub exits: Mutex<Vec<i32>>,
}

#[async_trait::async_trait]
impl ToolBackend for RepairTools {
    async fn list_schemas(&self) -> Vec<ToolDefinition> {
        vec!["write_file", "read_file", "bash"].into_iter().map(|name| ToolDefinition {
            r#type: "function".into(), function: FunctionDefinition { name: name.into(), description: "Synthetic fixture operation".into(), parameters: json!({"type":"object"}) },
        }).collect()
    }
    async fn execute(&self, call: &ToolCall, args: &serde_json::Value, ctx: &ToolCtx) -> Result<ToolInvocationResult, ToolError> {
        assert_eq!(ctx.session_id.as_deref(), Some("synthetic-session"));
        assert_eq!(ctx.root_turn_id.as_deref(), Some("synthetic-root"));
        self.calls.lock().unwrap().push(call.function.name.clone());
        let source = self.checkout.join("app.rs");
        let (kind, code, stdout, stderr, command) = match call.function.name.as_str() {
            "write_file" => {
                let content = args["content"].as_str().unwrap();
                std::fs::write(source, content).unwrap();
                (codefactory_agent_core::ToolKind::Mutation, 0, "app.rs updated".into(), String::new(), "write_file app.rs".into())
            }
            "read_file" => (codefactory_agent_core::ToolKind::ReadOnly, 0, std::fs::read_to_string(source).unwrap(), String::new(), "read_file app.rs".into()),
            "bash" => {
                assert_eq!(args["command"], "cargo test");
                let name = if cfg!(windows) { "synthetic-check.exe" } else { "synthetic-check" };
                let compile = Command::new(std::env::var_os("CF_FIXTURE_RUSTC").unwrap()).no_window()
                    .args(["--test", "app.rs", "-o", name]).current_dir(&self.checkout).output().unwrap();
                assert!(compile.status.success(), "{}", String::from_utf8_lossy(&compile.stderr));
                let output = Command::new(self.checkout.join(name)).no_window().current_dir(&self.checkout).output().unwrap();
                std::fs::remove_file(self.checkout.join(name)).unwrap();
                let code = output.status.code().unwrap();
                self.exits.lock().unwrap().push(code);
                (codefactory_agent_core::ToolKind::Verification, code, String::from_utf8(output.stdout).unwrap(), String::from_utf8(output.stderr).unwrap(), "cargo test".into())
            }
            other => panic!("unexpected synthetic tool {other}"),
        };
        Ok(ToolInvocationResult { content: stdout.clone(), is_error: code != 0, status: if code == 0 { ToolExecutionStatus::Done } else { ToolExecutionStatus::Error }, command, kind, return_code: Some(code), stdout, stderr, error: (code != 0).then(|| "acceptance assertion failed".into()), metadata: None, next_working_directory: None, duration_ms: 0 })
    }
}

struct RepairProvider {
    round: Mutex<usize>,
    tools: Arc<RepairTools>,
    queue: crate::commands::interjections::InterjectionQueue,
    requests: Mutex<Vec<Vec<ChatMessage>>>,
    corrected: String,
}

fn call(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall { id: id.into(), r#type: "function".into(), function: FunctionCall { name: name.into(), arguments: args.to_string() } }
}

const BROKEN: &str = "pub fn double(value: i32) -> i32 { value + 2 }\n#[test] fn acceptance() { for value in [-3, 0, 2, 7, 19] { assert_eq!(double(value), value * 2); } }\n";

#[async_trait::async_trait]
impl ModelTransport for RepairProvider {
    async fn complete(&self, messages: &[ChatMessage], _: &[ToolDefinition], _: &RoundOptions) -> Result<ModelResponse, TransportError> {
        let round = { let mut round = self.round.lock().unwrap(); let old = *round; *round += 1; old };
        self.requests.lock().unwrap().push(messages.to_vec());
        let (text, calls) = match round {
            0 => {
                // Interject after the loop is already running, not before entry.
                use crate::commands::interjections::enqueue_interjection;
                assert!(enqueue_interjection(&self.queue, "synthetic-session", "accept: double(7) = 14", Some("accept")).await.unwrap());
                assert!(enqueue_interjection(&self.queue, "synthetic-session", "correct: multiply, do not add", Some("correct")).await.unwrap());
                ("Implement the initial attempt", vec![call("mutate", "write_file", json!({"content": BROKEN}))])
            }
            1 => {
                let raw = serde_json::to_string(messages).unwrap();
                let accept = raw.find("accept: double(7) = 14").unwrap();
                let correction = raw.find("correct: multiply, do not add").unwrap();
                assert!(accept < correction);
                ("Verify the mutation against the new acceptance", vec![call("verify-failure", "bash", json!({"command":"cargo test"}))])
            }
            2 => {
                assert_eq!(*self.tools.exits.lock().unwrap(), vec![101]);
                let raw = serde_json::to_string(messages).unwrap();
                assert!(raw.contains("assertion `left == right` failed"));
                ("The assertion failed; read the implementation before repairing", vec![call("diagnose", "read_file", json!({"path":"app.rs"}))])
            }
            3 => {
                let raw = serde_json::to_string(messages).unwrap();
                assert!(raw.contains("value + 2"));
                ("Addition violates the correction; change only the expression", vec![call("repair", "write_file", json!({"content": self.corrected}))])
            }
            4 => ("Re-run the identical acceptance command", vec![call("verify-success", "bash", json!({"command":"cargo test"}))]),
            5 => {
                assert_eq!(*self.tools.exits.lock().unwrap(), vec![101, 0]);
                ("Implemented double using multiplication. All five acceptance inputs pass.", vec![])
            }
            _ => return Err(TransportError::Fatal("synthetic repair script exhausted".into())),
        };
        Ok(ModelResponse { text: text.into(), tool_calls: calls, ..Default::default() })
    }
}

struct FixedContext;
#[async_trait::async_trait]
impl ContextPolicy for FixedContext {
    async fn context_window(&self, _: u32) -> (u32, u32) { (100_000, 100_000) }
    async fn supports_vision(&self) -> bool { false }
    async fn round_reasoning_effort(&self) -> String { String::new() }
}

pub(super) async fn repair_in_loop(world: &Path, expression: &str) -> (Vec<String>, Vec<i32>) {
    let tools = Arc::new(RepairTools { checkout: world.join("delivery-worktree"), calls: Mutex::new(Vec::new()), exits: Mutex::new(Vec::new()) });
    let queue: crate::commands::interjections::InterjectionQueue = Default::default();
    let provider = Arc::new(RepairProvider { round: Mutex::new(0), tools: tools.clone(), queue: queue.clone(), requests: Mutex::new(Vec::new()), corrected: BROKEN.replacen("value + 2", expression, 1) });
    let inputs = LoopInputs { messages: vec![ChatMessage { role: "user".into(), content: MessageContent::Text("Implement double and run tests".into()), tool_calls: None, tool_call_id: None, name: None, reasoning_content: None }], system_prompt: "Synthetic coding fixture".into(), tool_defs: tools.list_schemas().await, completion_instruction: "Implement double and run tests".into(), fact_check_instruction: String::new(), audit_session_id: "synthetic-session".into(), root_turn_id: Some("synthetic-root".into()), mutation_permit: None, knowledge_library_ids: None, cancel: None };
    let config = RunConfig { finalization: FinalizationPolicy::ReleaseWithWarning, turn_capability: TurnCapability::Implement, gate_benchmark: false, progress_window: 8, recovery_limit: 0, max_iterations: 8, wall_budget_applies: false, context_compression: false, overload_backoff: false, overload_retry_delays: [Duration::ZERO; 2], inspection_budget: false, replay_rejected_draft: false, tool_heartbeat_interval: None, long_tool_wait_threshold: Duration::from_secs(60), tool_amplification_threshold: None, session_id: "synthetic-session".into(), endpoint_name: "synthetic".into(), model_id: "synthetic".into(), base_url: "http://synthetic.invalid".into(), usage_run_id: "synthetic-root".into(), surface: "interactive".into(), task_id: None, anonymous: true, is_chatgpt: false, cwd: world.join("delivery-worktree") };
    let services = LoopServices { transport: provider, tools: tools.clone(), persistence: Arc::new(codefactory_agent_loop::journal::NullPersistence), events: Arc::new(codefactory_agent_loop::events::CollectingEventSink::new()), budget: Arc::new(codefactory_agent_loop::journal::NullBudget), compactor: Arc::new(NoOpCompactor), context_compaction_gate: Arc::new(AllowAllContextCompaction), permission: Arc::new(AllowAllPermissions), hooks: Arc::new(NoOpHooks), context_policy: Arc::new(FixedContext), fact_checker: Arc::new(NoOpFactChecker), steer: Arc::new(crate::commands::interjections::SessionSteerInbox { queue, session_id: "synthetic-session".into(), delivery_authorized: true }) };
    let outcome = run_agent_loop(inputs, config, services).await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Finished, "{outcome:?}");
    assert_eq!(*tools.calls.lock().unwrap(), ["write_file", "bash", "read_file", "write_file", "bash"]);
    let exits = tools.exits.lock().unwrap().clone();
    (vec!["accept: double(7) = 14".into(), "correct: multiply, do not add".into()], exits)
}
