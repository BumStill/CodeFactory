// SPDX-License-Identifier: Apache-2.0
//! Formal executable system smoke for a once-authorized long task.
//!
//! The provider is the only fake boundary. Chat admission, AgentLoop, native
//! tools, permissions, SQLite migrations, Objective recovery, mutation
//! receipts and settlement are the production implementations.

use super::events::{CollectingEventSink, EventSink};
use super::objective::{current_process_instance, ObjectiveStatus, ObjectiveStore};
use super::{AgentExecutionContext, AgentLoop, AgentMode, TurnCapability, UsageSurface};
use crate::config::settings::{ApiStyle, Settings};
use crate::mcp::McpManager;
use crate::util::no_window::NoWindow;
use anyhow::{anyhow, bail, Context};
use sqlx::Row;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

const SESSION_ID: &str = "unattended-long-task-session";
const ARTIFACT_CONTENT: &str = "durable-once\n";
const USER_INSTRUCTION: &str = "实现这个长任务：创建 artifact.txt，内容必须精确为 durable-once 加换行；运行项目测试验证，然后完成。执行期间遇到可恢复故障或应用重启时不要询问我，也不要等待我回复继续。";

struct ProviderFixture {
    base_url: String,
    requests: Arc<AtomicUsize>,
    blocked_round_seen: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProviderFixture {
    fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let requests = Arc::new(AtomicUsize::new(0));
        let blocked_round_seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = requests.clone();
        let thread_blocked = blocked_round_seen.clone();
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let request_index = thread_requests.fetch_add(1, Ordering::SeqCst) + 1;
                        let connection_stop = thread_stop.clone();
                        let connection_blocked = thread_blocked.clone();
                        std::thread::spawn(move || {
                            if let Err(error) = handle_provider_request(
                                stream,
                                request_index,
                                &connection_blocked,
                                &connection_stop,
                            ) {
                                eprintln!(
                                    "unattended provider request {request_index} failed: {error}"
                                );
                            }
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => {
                        eprintln!("unattended provider fixture stopped: {error}");
                        break;
                    }
                }
            }
        });
        Ok(Self {
            base_url,
            requests,
            blocked_round_seen,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for ProviderFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_http_request(stream: &mut TcpStream) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut bytes = Vec::new();
    let mut header_end = None;
    let mut content_length = 0_usize;
    loop {
        let mut chunk = [0_u8; 16 * 1024];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > 4 * 1024 * 1024 {
            bail!("provider fixture request exceeded 4 MiB");
        }
        if header_end.is_none() {
            header_end = bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4);
            if let Some(end) = header_end {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
            }
        }
        if header_end.is_some_and(|end| bytes.len() >= end + content_length) {
            break;
        }
    }
    Ok(())
}

fn tool_sse(call_id: &str, name: &str, arguments: serde_json::Value) -> String {
    let arguments = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".into());
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "tool_calls": [{
                        "index": 0,
                        "id": call_id,
                        "type": "function",
                        "function": {"name": name, "arguments": arguments}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
    )
}

fn text_sse(content: &str) -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }]
        })
    )
}

fn handle_provider_request(
    mut stream: TcpStream,
    request_index: usize,
    blocked_round_seen: &AtomicBool,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    // Windows hands back an accepted socket that INHERITS the listener's
    // non-blocking mode; POSIX does not. The listener is non-blocking so the
    // accept loop can poll `stop`, which left this connection non-blocking on
    // Windows only -- `set_read_timeout` is then ignored and the first read
    // fails with WSAEWOULDBLOCK (os error 10035) whenever this handler thread
    // wins the race against the client's bytes arriving on loopback. Restore
    // blocking mode for the whole connection instead of retrying the symptom.
    stream.set_nonblocking(false)?;
    read_http_request(&mut stream)?;
    if request_index == 2 {
        blocked_round_seen.store(true, Ordering::SeqCst);
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
        return Ok(());
    }
    let body = match request_index {
        1 => tool_sse(
            "write-before-restart",
            "write_file",
            serde_json::json!({"path": "artifact.txt", "content": ARTIFACT_CONTENT}),
        ),
        3 => tool_sse(
            "write-after-restart",
            "write_file",
            serde_json::json!({"path": "artifact.txt", "content": ARTIFACT_CONTENT}),
        ),
        4 => tool_sse(
            "verify-after-restart",
            "bash",
            serde_json::json!({
                "command": "npm test",
                "description": "Verify the persisted artifact"
            }),
        ),
        _ => text_sse("任务已完成：持久文件通过项目测试，恢复期间没有要求用户介入。"),
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    Ok(())
}

fn seed_project(project: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(project)?;
    std::fs::write(
        project.join("package.json"),
        r#"{"private":true,"scripts":{"test":"node --test artifact.test.mjs"}}"#,
    )?;
    std::fs::write(
        project.join("artifact.test.mjs"),
        format!(
            "import assert from 'node:assert/strict';\nimport {{ readFileSync }} from 'node:fs';\nassert.equal(readFileSync(new URL('./artifact.txt', import.meta.url), 'utf8'), {});\n",
            serde_json::to_string(ARTIFACT_CONTENT)?
        ),
    )?;
    Ok(())
}

fn spawn_worker(
    state_dir: &Path,
    base_url: &str,
    phase: u8,
) -> anyhow::Result<std::process::Child> {
    let executable = std::env::current_exe()?;
    Command::new(executable)
        .no_window()
        .arg("--unattended-long-task-worker")
        .arg(state_dir)
        .arg(base_url)
        .arg(phase.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawn unattended long-task worker")
}

async fn wait_for_fault_point(
    child: &mut std::process::Child,
    fixture: &ProviderFixture,
    artifact: &Path,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            bail!("phase-one worker exited before injected crash: {status}");
        }
        if fixture.blocked_round_seen.load(Ordering::SeqCst)
            && artifact.exists()
            && std::fs::read_to_string(artifact).ok().as_deref() == Some(ARTIFACT_CONTENT)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    bail!("phase-one worker did not reach post-mutation provider wait within 30 seconds")
}

pub(crate) async fn run_parent() -> anyhow::Result<serde_json::Value> {
    let smoke_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("codefactory-unattended-smoke-{smoke_id}"));
    let project = root.join("project");
    std::fs::create_dir_all(&root)?;
    seed_project(&project)?;
    let fixture = ProviderFixture::start()?;

    let result = async {
        let mut phase_one = spawn_worker(&root, &fixture.base_url, 1)?;
        wait_for_fault_point(&mut phase_one, &fixture, &project.join("artifact.txt")).await?;
        phase_one.kill().context("hard-kill phase-one worker")?;
        let killed_status = phase_one.wait().context("reap phase-one worker")?;

        let db_url = format!("sqlite:{}", root.join("smoke.db").display());
        let before_pool = crate::storage::db::connect(&db_url).await?;
        let before: (String, String, i64) = sqlx::query_as(
            "SELECT objective.id, turn.root_turn_id,
                    (SELECT COUNT(*) FROM messages message
                     WHERE message.session_id=? AND message.role='user'
                       AND message.completion_state IS NULL)
             FROM objectives objective
             JOIN chat_turn_state turn ON turn.objective_id=objective.id
             WHERE objective.session_id=? ORDER BY objective.created_at LIMIT 1",
        )
        .bind(SESSION_ID)
        .bind(SESSION_ID)
        .fetch_one(&before_pool)
        .await?;
        let receipt_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND status IN ('committed','reconciled')",
        )
        .bind(&before.0)
        .fetch_one(&before_pool)
        .await?;
        crate::storage::db::close_and_release_files(before_pool).await;
        if receipt_before != 1 {
            bail!("expected one durable receipt before restart, found {receipt_before}");
        }

        let mut phase_two = spawn_worker(&root, &fixture.base_url, 2)?;
        let deadline = Instant::now() + Duration::from_secs(45);
        let phase_two_status = loop {
            if let Some(status) = phase_two.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = phase_two.kill();
                let _ = phase_two.wait();
                bail!("phase-two worker did not settle within 45 seconds");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        if !phase_two_status.success() {
            bail!("phase-two worker exited {phase_two_status}");
        }

        let pool = crate::storage::db::connect(&db_url).await?;
        let objective = sqlx::query(
            "SELECT id, root_turn_id, status, lease_owner FROM objectives
             WHERE session_id=? ORDER BY created_at LIMIT 1",
        )
        .bind(SESSION_ID)
        .fetch_one(&pool)
        .await?;
        let objective_id: String = objective.get("id");
        let root_turn_id: String = objective.get("root_turn_id");
        let objective_status: String = objective.get("status");
        let lease_owner: Option<String> = objective.get("lease_owner");
        let user_message_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages
             WHERE session_id=? AND role='user' AND completion_state IS NULL",
        )
        .bind(SESSION_ID)
        .fetch_one(&pool)
        .await?;
        let human_prompt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_decisions
             WHERE objective_id=? AND requires_user_action=1",
        )
        .bind(&objective_id)
        .fetch_one(&pool)
        .await?;
        let side_effect_receipt_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts WHERE objective_id=?")
                .bind(&objective_id)
                .fetch_one(&pool)
                .await?;
        let replay_call_link_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tool_recovery_call_links link
             JOIN side_effect_receipts receipt ON receipt.id=link.receipt_id
             WHERE receipt.objective_id=?",
        )
        .bind(&objective_id)
        .fetch_one(&pool)
        .await?;
        let claimable_remediation_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations remediation
             WHERE remediation.objective_id=?
               AND remediation.status IN ('queued','waiting','claimed')",
        )
        .bind(&objective_id)
        .fetch_one(&pool)
        .await?;
        let active_run_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_run_controls
             WHERE session_id=? AND status IN ('active','cancel_requested')",
        )
        .bind(SESSION_ID)
        .fetch_one(&pool)
        .await?;
        crate::storage::db::close_and_release_files(pool).await;

        let artifact_ok =
            std::fs::read_to_string(project.join("artifact.txt"))? == ARTIFACT_CONTENT;
        let same_objective = before.0 == objective_id && before.1 == root_turn_id;
        let live_owner_count = i64::from(lease_owner.is_some()) + active_run_count;
        let ok = artifact_ok
            && same_objective
            && before.2 == 1
            && user_message_count == 1
            && human_prompt_count == 0
            && side_effect_receipt_count == 1
            && replay_call_link_count == 2
            && objective_status == "completed"
            && live_owner_count == 0
            && claimable_remediation_count == 0
            && fixture.requests.load(Ordering::SeqCst) >= 5;
        if !ok {
            bail!("unattended smoke oracle rejected the recovered trajectory");
        }
        Ok(serde_json::json!({
            "ok": true,
            "scenario_id": "HLT-001",
            "build_git_sha": option_env!("CODEFACTORY_BUILD_GIT_SHA").unwrap_or("unknown"),
            "process_restart_observed": true,
            "phase_one_was_hard_killed": !killed_status.success(),
            "same_objective": same_objective,
            "user_message_count": user_message_count,
            "human_prompt_count": human_prompt_count,
            "side_effect_receipt_count": side_effect_receipt_count,
            "replay_call_link_count": replay_call_link_count,
            "objective_status": objective_status,
            "live_owner_count": live_owner_count,
            "claimable_remediation_count": claimable_remediation_count,
            "provider_request_count": fixture.requests.load(Ordering::SeqCst),
            "artifact_verified": artifact_ok,
            "cleanup_ok": false
        }))
    }
    .await;

    drop(fixture);
    crate::util::fs_cleanup::remove_fixture_dir(&root).await;
    let cleanup_ok = !root.exists();
    match result {
        Ok(mut receipt) if cleanup_ok => {
            receipt["cleanup_ok"] = serde_json::Value::Bool(true);
            Ok(receipt)
        }
        Ok(_) => bail!("unattended smoke did not clean its isolated state"),
        Err(error) => Err(error),
    }
}

async fn ensure_session(pool: &sqlx::SqlitePool, project: &Path) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT OR IGNORE INTO sessions
         (id, title, cwd, model_id, endpoint_id, model_policy,
          permission_mode, created_at, updated_at)
         VALUES (?, 'Unattended smoke', ?, 'smoke-model', 'smoke-endpoint',
                 'fixed', 'trusted', ?, ?)",
    )
    .bind(SESSION_ID)
    .bind(project.to_string_lossy().as_ref())
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

fn build_agent(
    pool: sqlx::SqlitePool,
    project: PathBuf,
    base_url: &str,
    mutation_permit: Option<codefactory_agent_loop::tool::MutationPermit>,
) -> AgentLoop {
    let sink: Arc<dyn EventSink> = Arc::new(CollectingEventSink::new());
    AgentLoop::new_headless(
        sink,
        pool,
        SESSION_ID.to_string(),
        "smoke-endpoint".into(),
        "smoke-model".into(),
        base_url.to_string(),
        "smoke-key".into(),
        ApiStyle::Openai,
        project,
        Arc::new(RwLock::new(Settings::default())),
        Arc::new(Mutex::new(std::collections::HashMap::new())),
        Arc::new(McpManager::new()),
        Some(AgentExecutionContext {
            parent_session_id: None,
            task_id: None,
            knowledge_library_ids: Vec::new(),
            usage_surface: UsageSurface::Autonomous,
            mutation_permit,
            force_context_compression: None,
        }),
        AgentMode::Autonomous,
    )
    .with_turn_capability(TurnCapability::Implement)
}

pub(crate) async fn run_worker(state_dir: &Path, base_url: &str, phase: u8) -> anyhow::Result<()> {
    let project = state_dir.join("project");
    let db_url = format!("sqlite:{}", state_dir.join("smoke.db").display());
    let pool = crate::storage::db::connect(&db_url).await?;
    ensure_session(&pool, &project).await?;

    if phase == 1 {
        let admission =
            crate::commands::chat::admit_headless_chat_turn(&pool, SESSION_ID, USER_INSTRUCTION)
                .await
                .map_err(|error| anyhow!(error.to_string()))?;
        if admission.objective.kind != super::objective::ObjectiveKind::LocalMutation {
            bail!("smoke prompt did not admit a local-mutation Objective");
        }
        if admission.objective.root_turn_id.as_deref() != Some(admission.root_turn_id.as_str()) {
            bail!("headless admission split its Objective and root-turn identity");
        }
        let history = crate::storage::load_agent_history(&pool, SESSION_ID).await?;
        let mut agent = build_agent(pool.clone(), project, base_url, None);
        let _never_returns_before_parent_kill = agent.run(history).await?;
        bail!("phase-one worker reached a terminal outcome before injected crash")
    }
    if phase != 2 {
        bail!("unknown unattended worker phase {phase}");
    }

    let store = ObjectiveStore::new(pool.clone());
    let process_instance = current_process_instance();
    let stale_runs = store
        .reconcile_stale_chat_run_controls(&process_instance)
        .await?;
    let provider_recoveries =
        super::objective_supervisor::reconcile_provider_recovery_on_startup(&pool).await?;
    let stale_objectives = store
        .reconcile_stale_active_objectives(&process_instance)
        .await?;
    if stale_runs != 1 || provider_recoveries != 1 || stale_objectives != 0 {
        bail!(
            "restart reconciliation expected run/provider/generic 1/1/0, observed {stale_runs}/{provider_recoveries}/{stale_objectives}"
        );
    }
    let owner = format!("unattended-smoke:{process_instance}");
    let mut claims = store.claim_due_remediations(&owner, 1, 60_000).await?;
    let claim = claims
        .pop()
        .ok_or_else(|| anyhow!("restart reconciliation produced no claimable remediation"))?;
    if !claims.is_empty() || claim.objective.status != ObjectiveStatus::WaitingSystem {
        bail!("restart reconciliation produced an ambiguous claim set");
    }
    super::objective_supervisor::require_provider_resume_evidence(
        &pool,
        &claim.objective.id,
        false,
    )
    .await
    .map_err(|error| anyhow!(error.to_string()))?;
    let permit = codefactory_agent_loop::tool::MutationPermit {
        objective_id: claim.objective.id.clone(),
        remediation_id: claim.remediation_id.clone(),
        owner: owner.clone(),
        claim_epoch: claim.claim_epoch,
        binding_id: claim.binding_id.clone(),
        resource_generation: claim.resource_generation,
    };
    let root_turn_id = claim
        .objective
        .root_turn_id
        .clone()
        .ok_or_else(|| anyhow!("claimed chat Objective has no root turn"))?;
    let history = crate::storage::load_agent_history(&pool, SESSION_ID).await?;
    let mut agent = build_agent(pool.clone(), project, base_url, Some(permit.clone()));
    let outcome = agent.run(history).await?;
    let settled = crate::commands::chat::settle_headless_chat_objective_from_outcome(
        &pool,
        &claim.objective.id,
        claim.objective.revision,
        &root_turn_id,
        &outcome,
        Some(&permit),
    )
    .await
    .map_err(|error| anyhow!(error.to_string()))?;
    if settled.status != ObjectiveStatus::Completed {
        let durable_trace: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT 'receipt', binding_id, action_fingerprint, revision
             FROM side_effect_receipts WHERE objective_id=?
             UNION ALL
             SELECT 'tool_call', COALESCE(binding_id, ''),
                    COALESCE(action_signature, ''), COALESCE(resource_generation, 0)
             FROM tool_calls WHERE objective_id=? ORDER BY 1, 2, 3",
        )
        .bind(&claim.objective.id)
        .bind(&claim.objective.id)
        .fetch_all(&pool)
        .await?;
        bail!(
            "recovered Objective settled as {} (failure_code={:?}, stop_reason={:?}, final_text={:?}, evidence={:?}, durable_trace={durable_trace:?})",
            settled.status.as_str(),
            settled.failure_code,
            outcome.stop_reason,
            outcome.final_text,
            outcome.completion_evidence
        );
    }
    crate::storage::db::close_and_release_files(pool).await;
    Ok(())
}
