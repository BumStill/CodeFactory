// SPDX-License-Identifier: Apache-2.0
//! Formal executable system smoke for a once-authorized long task.
//!
//! The provider is the only fake boundary. Chat admission, AgentLoop, native
//! tools, permissions, SQLite migrations, Objective recovery, mutation
//! receipts and settlement are the production implementations.

#[path = "scenario_case_observation.rs"]
mod scenario_case_observation;

use super::events::{CollectingEventSink, EventSink};
use super::objective::{
    current_process_instance, ClaimedRemediation, DecisionRouter, ObjectiveStatus, ObjectiveStore,
    RecoveryDomain, RouteSignal, MAX_SIGNATURE_RECOVERY_ATTEMPTS, RECOVERY_CAPABILITY_REVISION,
    TECHNICAL_RECOVERY_EXHAUSTED,
};
use super::{AgentExecutionContext, AgentLoop, AgentMode, TurnCapability, UsageSurface};
use crate::config::settings::{ApiStyle, Settings};
use crate::mcp::McpManager;
use crate::util::no_window::NoWindow;
use anyhow::{anyhow, bail, Context};
use scenario_case_observation::{
    attach_e2e001_case_observation, e2e001_failure_legacy_receipt, CleanupObservation,
    ProcessObservation,
};
use sqlx::Row;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    active_connections: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProviderFixture {
    fn start(active_connections: Arc<AtomicUsize>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let requests = Arc::new(AtomicUsize::new(0));
        let blocked_round_seen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = requests.clone();
        let thread_blocked = blocked_round_seen.clone();
        let thread_stop = stop.clone();
        let thread_connections = active_connections.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let request_index = thread_requests.fetch_add(1, Ordering::SeqCst) + 1;
                        let connection_stop = thread_stop.clone();
                        let connection_blocked = thread_blocked.clone();
                        let connection_count = thread_connections.clone();
                        connection_count.fetch_add(1, Ordering::SeqCst);
                        std::thread::spawn(move || {
                            let _connection_lease = ConnectionLease(connection_count);
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
            active_connections,
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
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.active_connections.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

struct ConnectionLease(Arc<AtomicUsize>);

impl Drop for ConnectionLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn wait_for_child_exit_sync(child: &mut Child, timeout: Duration) -> std::io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct ManagedWorker {
    child: Child,
    process_tree: crate::util::process_tree::StdProcessTree,
    live_workers: Arc<AtomicUsize>,
    sweep_state: Arc<ProcessSweepState>,
    reaped: bool,
    swept: bool,
}

#[derive(Default)]
struct ProcessSweepState {
    spawned_workers: AtomicUsize,
    swept_workers: AtomicUsize,
    sweep_failures: AtomicUsize,
    active_descendants: AtomicUsize,
}

impl ManagedWorker {
    fn spawn(
        state_dir: &Path,
        base_url: &str,
        phase: u8,
        live_workers: Arc<AtomicUsize>,
        sweep_state: Arc<ProcessSweepState>,
    ) -> anyhow::Result<Self> {
        let mut child = spawn_worker(state_dir, base_url, phase)?;
        let process_tree = match crate::util::process_tree::StdProcessTree::attach(&child) {
            Ok(process_tree) => process_tree,
            Err(error) => {
                let _ = child.kill();
                let _ = wait_for_child_exit_sync(&mut child, Duration::from_secs(2));
                sweep_state.sweep_failures.fetch_add(1, Ordering::SeqCst);
                return Err(error).context("attach unattended worker process tree");
            }
        };
        live_workers.fetch_add(1, Ordering::SeqCst);
        sweep_state.spawned_workers.fetch_add(1, Ordering::SeqCst);
        let mut worker = Self {
            child,
            process_tree,
            live_workers,
            sweep_state,
            reaped: false,
            swept: false,
        };
        let gate_result = worker
            .child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("unattended worker start gate is unavailable"))
            .and_then(|mut gate| {
                gate.write_all(b"start\n")
                    .context("release unattended worker start gate")
            });
        if let Err(error) = gate_result {
            drop(worker);
            return Err(error);
        }
        Ok(worker)
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    fn terminate_tree(&mut self) -> std::io::Result<()> {
        self.process_tree.terminate(&mut self.child)
    }

    fn mark_reaped(&mut self) {
        if !self.reaped {
            self.reaped = true;
            self.live_workers.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn sweep(&mut self) {
        if self.swept {
            return;
        }
        self.swept = true;
        self.sweep_state
            .swept_workers
            .fetch_add(1, Ordering::SeqCst);

        let mut failed = self.process_tree.terminate(&mut self.child).is_err();
        if !self.reaped {
            match wait_for_child_exit_sync(&mut self.child, Duration::from_secs(2)) {
                Ok(true) => self.mark_reaped(),
                Ok(false) | Err(_) => failed = true,
            }
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        let active_descendants = loop {
            match self.process_tree.active_process_count(&mut self.child) {
                Ok(0) => break 0,
                Ok(count) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                    let _ = count;
                }
                Ok(count) => break count as usize,
                Err(_) => {
                    failed = true;
                    break 1;
                }
            }
        };
        if active_descendants != 0 {
            self.sweep_state
                .active_descendants
                .fetch_add(active_descendants, Ordering::SeqCst);
        }
        if failed {
            self.sweep_state
                .sweep_failures
                .fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Drop for ManagedWorker {
    fn drop(&mut self) {
        self.sweep();
    }
}

pub(crate) struct UnattendedSmokeRunOutcome {
    pub receipt: serde_json::Value,
    pub error: Option<anyhow::Error>,
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
    let mut command = Command::new(executable).no_window();
    command
        .arg("--unattended-long-task-worker")
        .arg(state_dir)
        .arg(base_url)
        .arg(phase.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    crate::util::process_tree::isolate_std_process_tree(&mut command);
    command.spawn().context("spawn unattended long-task worker")
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

async fn wait_for_worker(
    child: &mut std::process::Child,
    phase: &str,
    timeout: Duration,
) -> anyhow::Result<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            bail!("{phase} worker did not settle within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn exhaust_claim_into_legacy_incident(
    pool: &sqlx::SqlitePool,
    store: &ObjectiveStore,
    owner: &str,
    mut claim: ClaimedRemediation,
) -> anyhow::Result<i64> {
    let signature: String = sqlx::query_scalar(
        "SELECT failure_signature FROM objective_remediations WHERE id=?",
    )
    .bind(&claim.remediation_id)
    .fetch_one(pool)
    .await?;
    let mut executed_attempts = 0_i64;
    loop {
        if !store
            .charge_claimed_remediation_attempt(
                &claim.objective.id,
                &claim.remediation_id,
                owner,
                claim.claim_epoch,
            )
            .await?
        {
            bail!("legacy recovery attempt lost its exact claim before execution");
        }
        executed_attempts += 1;
        let permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id.clone(),
            owner: owner.into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id.clone(),
            resource_generation: claim.resource_generation,
        };
        let decision = DecisionRouter::route(
            &claim.objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_transport_failed_before_output".into(),
                failure_signature: signature.clone(),
                next_observation_at: chrono::Utc::now().timestamp_millis(),
                resume_cursor: claim.objective.root_turn_id.clone(),
            },
        )?;
        let current = store
            .apply_claimed_decision(claim.objective.revision, decision, &permit)
            .await?;
        if current.failure_code.as_deref() == Some(TECHNICAL_RECOVERY_EXHAUSTED) {
            if executed_attempts != MAX_SIGNATURE_RECOVERY_ATTEMPTS {
                bail!(
                    "legacy recovery parked after {executed_attempts} executed attempts, expected {MAX_SIGNATURE_RECOVERY_ATTEMPTS}"
                );
            }
            return Ok(executed_attempts);
        }
        let accelerated_now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE objective_remediations SET next_observation_at=?, updated_at=?
             WHERE objective_id=? AND id=? AND status IN ('queued','waiting')",
        )
        .bind(accelerated_now)
        .bind(accelerated_now)
        .bind(&current.id)
        .bind(current.remediation_id.as_deref())
        .execute(pool)
        .await?;
        sqlx::query(
            "UPDATE objectives SET next_observation_at=?, updated_at=?
             WHERE id=? AND remediation_id=?",
        )
        .bind(accelerated_now)
        .bind(accelerated_now)
        .bind(&current.id)
        .bind(current.remediation_id.as_deref())
        .execute(pool)
        .await?;
        claim = store
            .claim_due_remediations(owner, 1, 60_000)
            .await?
            .pop()
            .ok_or_else(|| anyhow!("legacy recovery did not publish its next due remediation"))?;
    }
}

pub(crate) async fn run_parent() -> UnattendedSmokeRunOutcome {
    let smoke_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("codefactory-unattended-smoke-{smoke_id}"));
    let project = root.join("project");
    let live_workers = Arc::new(AtomicUsize::new(0));
    let process_sweep = Arc::new(ProcessSweepState::default());
    let active_provider_connections = Arc::new(AtomicUsize::new(0));
    let mut process_observation = ProcessObservation::default();
    let mut root_created = false;
    let mut process_tracking_started = false;

    let result: anyhow::Result<serde_json::Value> = async {
        std::fs::create_dir_all(&root)?;
        root_created = true;
        process_tracking_started = true;
        seed_project(&project)?;
        let fixture = ProviderFixture::start(active_provider_connections.clone())?;

        let mut phase_one = ManagedWorker::spawn(
            &root,
            &fixture.base_url,
            1,
            live_workers.clone(),
            process_sweep.clone(),
        )?;
        let phase_one_pid = phase_one.id();
        wait_for_fault_point(
            phase_one.child_mut(),
            &fixture,
            &project.join("artifact.txt"),
        )
        .await?;
        phase_one
            .terminate_tree()
            .context("hard-kill phase-one worker process tree")?;
        process_observation.supervisor_hard_kill_issued = true;
        let killed_status = wait_for_worker(
            phase_one.child_mut(),
            "phase-one hard-kill",
            Duration::from_secs(5),
        )
        .await?;
        phase_one.mark_reaped();
        process_observation.worker_reaped = true;
        process_observation.phase_one_exit_was_failure = !killed_status.success();

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

        let mut phase_two = ManagedWorker::spawn(
            &root,
            &fixture.base_url,
            2,
            live_workers.clone(),
            process_sweep.clone(),
        )?;
        process_observation.replacement_process_distinct = phase_two.id() != phase_one_pid;
        let phase_two_status =
            wait_for_worker(phase_two.child_mut(), "phase-two", Duration::from_secs(45)).await?;
        phase_two.mark_reaped();
        if !phase_two_status.success() {
            bail!("phase-two worker exited {phase_two_status}");
        }

        let parked_pool = crate::storage::db::connect(&db_url).await?;
        let parked: (String, Option<String>, i64, i64) = sqlx::query_as(
            "SELECT objective.status, objective.failure_code,
                    incident.blocked_capability_revision, incident.reactivation_count
             FROM objectives objective
             JOIN objective_incidents incident ON incident.objective_id=objective.id
             WHERE objective.id=? AND incident.status='open'",
        )
        .bind(&before.0)
        .fetch_one(&parked_pool)
        .await?;
        let parked_claimable_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status IN ('queued','waiting','claimed')",
        )
        .bind(&before.0)
        .fetch_one(&parked_pool)
        .await?;
        crate::storage::db::close_and_release_files(parked_pool).await;
        if parked.0 != "waiting_system"
            || parked.1.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED)
            || parked.2 != 0
            || parked.3 != 0
            || parked_claimable_count != 0
        {
            bail!("phase-two did not leave one bounded legacy incident");
        }

        let mut phase_three = ManagedWorker::spawn(
            &root,
            &fixture.base_url,
            3,
            live_workers.clone(),
            process_sweep.clone(),
        )?;
        let phase_three_status = wait_for_worker(
            phase_three.child_mut(),
            "phase-three",
            Duration::from_secs(45),
        )
        .await?;
        phase_three.mark_reaped();
        if !phase_three_status.success() {
            bail!("phase-three worker exited {phase_three_status}");
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
        let incident_reactivation: (String, String, i64, Option<i64>) = sqlx::query_as(
            "SELECT status, reactivation_status, reactivation_count,
                    last_reactivated_revision
             FROM objective_incidents WHERE objective_id=?",
        )
        .bind(&objective_id)
        .fetch_one(&pool)
        .await?;
        let capability_revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM recovery_capabilities WHERE domain='chat' AND executable=1",
        )
        .fetch_one(&pool)
        .await?;
        let executed_recovery_attempts: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(execution_attempt_index), 0) FROM objective_remediations
             WHERE objective_id=?
               AND failure_code<>'recovery_capability_reactivated'",
        )
        .bind(&objective_id)
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
            && incident_reactivation.0 == "resolved"
            && incident_reactivation.1 == "admitted"
            && incident_reactivation.2 == 1
            && incident_reactivation.3 == Some(RECOVERY_CAPABILITY_REVISION)
            && capability_revision == RECOVERY_CAPABILITY_REVISION
            && executed_recovery_attempts == MAX_SIGNATURE_RECOVERY_ATTEMPTS
            && fixture.requests.load(Ordering::SeqCst) >= 5;
        if !ok {
            bail!("unattended smoke oracle rejected the recovered trajectory");
        }
        Ok(serde_json::json!({
            "ok": true,
            "scenario_id": "HLT-001",
            "build_git_sha": option_env!("CODEFACTORY_BUILD_GIT_SHA").unwrap_or("unknown"),
            "process_restart_observed": process_observation.worker_reaped
                && process_observation.replacement_process_distinct,
            "phase_one_was_hard_killed": process_observation.supervisor_hard_kill_issued
                && process_observation.worker_reaped
                && process_observation.phase_one_exit_was_failure,
            "legacy_incident_parked": true,
            "incident_reactivated": incident_reactivation.1 == "admitted",
            "incident_reactivation_count": incident_reactivation.2,
            "capability_revision": capability_revision,
            "executed_recovery_attempts": executed_recovery_attempts,
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

    let cleanup_attempted = root_created;
    if cleanup_attempted {
        crate::util::fs_cleanup::remove_fixture_dir(&root).await;
    }
    let spawned_workers = process_sweep.spawned_workers.load(Ordering::SeqCst);
    let swept_workers = process_sweep.swept_workers.load(Ordering::SeqCst);
    let sweep_failures = process_sweep.sweep_failures.load(Ordering::SeqCst);
    let descendant_process_count = process_sweep.active_descendants.load(Ordering::SeqCst);
    let orphan_sweep_performed = process_tracking_started
        && spawned_workers == swept_workers
        && sweep_failures == 0
        && descendant_process_count == 0;
    let fixture_root_leak = match root.try_exists() {
        Ok(false) if root_created => 0,
        Ok(false) => 0,
        Ok(true) | Err(_) => 1,
    };
    let leaked_resource_count = descendant_process_count
        + sweep_failures
        + live_workers.load(Ordering::SeqCst)
        + active_provider_connections.load(Ordering::SeqCst)
        + fixture_root_leak;
    process_observation.descendant_process_count =
        descendant_process_count.min(u32::MAX as usize) as u32;
    let cleanup_observation = CleanupObservation {
        cleanup_attempted,
        orphan_sweep_performed,
        leaked_resource_count: leaked_resource_count.min(u32::MAX as usize) as u32,
    };

    let (mut legacy_receipt, mut error) = match result {
        Ok(receipt) => (receipt, None),
        Err(error) => (e2e001_failure_legacy_receipt(), Some(error)),
    };
    if leaked_resource_count != 0 {
        legacy_receipt["ok"] = serde_json::Value::Bool(false);
        if error.is_none() {
            error = Some(anyhow!(
                "unattended smoke cleanup left {leaked_resource_count} resource(s)"
            ));
        }
    }
    let receipt =
        attach_e2e001_case_observation(legacy_receipt, process_observation, cleanup_observation);
    UnattendedSmokeRunOutcome { receipt, error }
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
    if phase != 2 && phase != 3 {
        bail!("unknown unattended worker phase {phase}");
    }

    let store = ObjectiveStore::new(pool.clone());
    let process_instance = current_process_instance();
    let owner = format!("unattended-smoke:{process_instance}");
    let mut claims = if phase == 2 {
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
        store.claim_due_remediations(&owner, 1, 60_000).await?
    } else {
        store.sync_recovery_capabilities().await?;
        let reactivated = store.reactivate_eligible_incidents(1).await?;
        if reactivated.len() != 1 {
            bail!(
                "new recovery capability reactivated {} incidents instead of one",
                reactivated.len()
            );
        }
        store.claim_due_remediations(&owner, 1, 60_000).await?
    };
    let claim = claims
        .pop()
        .ok_or_else(|| anyhow!("restart reconciliation produced no claimable remediation"))?;
    if !claims.is_empty() || claim.objective.status != ObjectiveStatus::WaitingSystem {
        bail!("restart reconciliation produced an ambiguous claim set");
    }
    if phase == 2 {
        let executed_attempts =
            exhaust_claim_into_legacy_incident(&pool, &store, &owner, claim).await?;
        if executed_attempts != MAX_SIGNATURE_RECOVERY_ATTEMPTS {
            bail!("phase-two did not consume the exact executable recovery budget");
        }
        crate::storage::db::close_and_release_files(pool).await;
        return Ok(());
    }
    super::objective_supervisor::require_provider_resume_evidence(
        &pool,
        &claim.objective.id,
        false,
    )
    .await
    .map_err(|error| anyhow!(error.to_string()))?;
    if !store
        .charge_claimed_remediation_attempt(
            &claim.objective.id,
            &claim.remediation_id,
            &owner,
            claim.claim_epoch,
        )
        .await?
    {
        bail!("reactivated recovery lost its exact claim before execution");
    }
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
