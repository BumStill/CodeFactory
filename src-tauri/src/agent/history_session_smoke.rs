// SPDX-License-Identifier: Apache-2.0
//! Cross-process executable smoke for historical chat continuation and stop.
//!
//! Every phase is a fresh copy of the formal desktop executable. The fixture
//! uses the production SQLite schema, atomic chat admission, Objective router,
//! durable session cancellation fence, and restart reconciliation.

use super::objective::{
    current_process_instance, CreateObjective, DecisionRouter, ObjectiveKind, ObjectiveSnapshot,
    ObjectiveStore, RecoveryDomain, RouteSignal,
};
use crate::util::no_window::NoWindow;
use anyhow::{anyhow, bail, Context};
use chrono::Utc;
use sqlx::SqlitePool;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

const CONTINUE_SESSION: &str = "history-continue-session";
const STOP_SESSION: &str = "history-stop-session";
const INCIDENT_SESSION: &str = "history-incident-session";
const ORIGINAL_INSTRUCTION: &str =
    "实现一个长任务并验证结果；遇到可恢复故障或应用重启时由系统自动恢复，不要等待人工参与。";
const HISTORY_PADDING: i64 = 12;
const STOP_OBJECTIVE_COUNT: i64 = 3;

async fn ensure_session(pool: &SqlitePool, session_id: &str, title: &str) -> anyhow::Result<()> {
    let now = Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT OR IGNORE INTO sessions
         (id, title, cwd, model_id, endpoint_id, model_policy,
          permission_mode, created_at, updated_at)
         VALUES (?, ?, ?, 'smoke-model', 'smoke-endpoint',
                 'fixed', 'trusted', ?, ?)",
    )
    .bind(session_id)
    .bind(title)
    .bind(std::env::temp_dir().to_string_lossy().as_ref())
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

async fn route_waiting(
    store: &ObjectiveStore,
    objective: &ObjectiveSnapshot,
    failure_code: &str,
) -> anyhow::Result<ObjectiveSnapshot> {
    let decision = DecisionRouter::route(
        objective,
        RouteSignal::TechnicalFailure {
            domain: RecoveryDomain::Chat,
            failure_code: failure_code.into(),
            failure_signature: format!("{}:{failure_code}", objective.id),
            next_observation_at: Utc::now().timestamp_millis() - 1,
            resume_cursor: objective.root_turn_id.clone(),
        },
    )?;
    store.apply_decision(objective.revision, decision).await
}

async fn seed_continue_session(pool: &SqlitePool) -> anyhow::Result<()> {
    ensure_session(pool, CONTINUE_SESSION, "Historical continue smoke").await?;
    let admission = crate::commands::chat::admit_headless_chat_turn(
        pool,
        CONTINUE_SESSION,
        ORIGINAL_INSTRUCTION,
    )
    .await
    .map_err(|error| anyhow!(error.to_string()))?;
    if admission.objective.kind != ObjectiveKind::LocalMutation {
        bail!("historical prompt did not admit a local-mutation Objective");
    }
    let store = ObjectiveStore::new(pool.clone());
    route_waiting(&store, &admission.objective, "historical_recoverable_wait").await?;

    let base = Utc::now().timestamp_millis();
    for index in 0..HISTORY_PADDING {
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES (?, ?, 'assistant', ?, ?)",
        )
        .bind(format!("history-padding-{index}"))
        .bind(CONTINUE_SESSION)
        .bind(format!("历史状态记录 {index}"))
        .bind(base + index + 1)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn add_stop_objective(
    pool: &SqlitePool,
    store: &ObjectiveStore,
    ordinal: i64,
    projected: bool,
) -> anyhow::Result<()> {
    let now = Utc::now().timestamp_millis() + ordinal;
    let root_turn_id = format!("stop-root-{ordinal}");
    let objective = store
        .create(CreateObjective {
            id: format!("stop-objective-{ordinal}"),
            kind: ObjectiveKind::LocalMutation,
            session_id: Some(STOP_SESSION.into()),
            root_turn_id: Some(root_turn_id.clone()),
            domain: RecoveryDomain::Chat,
            requested_acceptance: "validated_change".into(),
            created_surface: "history_session_smoke".into(),
        })
        .await?;

    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at)
         VALUES (?, ?, 'user', ?, ?)",
    )
    .bind(&root_turn_id)
    .bind(STOP_SESSION)
    .bind(format!("停止场景任务 {ordinal}"))
    .bind(now)
    .execute(pool)
    .await?;

    if projected {
        let segment_id = format!("stop-segment-{ordinal}");
        sqlx::query(
            "INSERT INTO chat_task_segments
             (id, session_id, ordinal, title, status, goal_root_turn_id,
              previous_segment_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'active', ?, NULL, ?, ?)",
        )
        .bind(&segment_id)
        .bind(STOP_SESSION)
        .bind(ordinal)
        .bind(format!("Stop segment {ordinal}"))
        .bind(&root_turn_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, task_segment_id, revision, phase, status,
              started_at, updated_at, recent_activity_kind, recent_activity_label,
              objective_id)
             VALUES (?, ?, ?, 1, 'recovering', 'active', ?, ?,
                     'system_recovery', '正在恢复', ?)",
        )
        .bind(&root_turn_id)
        .bind(STOP_SESSION)
        .bind(&segment_id)
        .bind(now)
        .bind(now)
        .bind(&objective.id)
        .execute(pool)
        .await?;
    }
    route_waiting(store, &objective, &format!("stop_wait_{ordinal}")).await?;
    Ok(())
}

async fn seed_stop_session(pool: &SqlitePool) -> anyhow::Result<()> {
    ensure_session(pool, STOP_SESSION, "Historical stop smoke").await?;
    let admission = crate::commands::chat::admit_headless_chat_turn(
        pool,
        STOP_SESSION,
        "执行一个会跨重启恢复的任务",
    )
    .await
    .map_err(|error| anyhow!(error.to_string()))?;
    let store = ObjectiveStore::new(pool.clone());
    route_waiting(&store, &admission.objective, "stop_wait_1").await?;
    add_stop_objective(pool, &store, 2, true).await?;
    // Deliberately omit the UI projection for the third Objective. Session
    // stop must be owned by durable Objective state, not the loaded page.
    add_stop_objective(pool, &store, 3, false).await?;
    Ok(())
}

async fn seed_incident_session(pool: &SqlitePool) -> anyhow::Result<()> {
    ensure_session(pool, INCIDENT_SESSION, "Recovery incident smoke").await?;
    let admission = crate::commands::chat::admit_headless_chat_turn(
        pool,
        INCIDENT_SESSION,
        "执行只读仓库审计并在恢复耗尽时明确交还控制权",
    )
    .await
    .map_err(|error| anyhow!(error.to_string()))?;
    let now = Utc::now().timestamp_millis();
    let assistant_id = "history-incident-assistant";
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at)
         VALUES (?, ?, 'assistant', '当前结论已保留。', ?)",
    )
    .bind(assistant_id)
    .bind(INCIDENT_SESSION)
    .bind(now)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO tool_calls
         (id, message_id, tool_name, arguments, result, status, created_at,
          objective_id)
         VALUES ('history-incident-tool', ?, 'bash', ?,
                 'external_state_uncertain', 'waiting', ?, ?)",
    )
    .bind(assistant_id)
    .bind(r#"{"command":"set -euo pipefail; git diff --check; git status --short"}"#)
    .bind(now)
    .bind(&admission.objective.id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO side_effect_receipts
         (id, objective_id, revision, action_fingerprint, idempotency_key,
          status, created_at, observed_at)
         VALUES ('history-incident-unknown-receipt', ?, ?,
                 'sha256:historical-unknown', 'sha256:historical-unknown-key',
                 'unknown', ?, ?)",
    )
    .bind(&admission.objective.id)
    .bind(admission.objective.revision)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    let store = ObjectiveStore::new(pool.clone());
    let mut current = admission.objective;
    for _ in 0..32 {
        if current.failure_code.as_deref() == Some("technical_recovery_exhausted") {
            break;
        }
        current = route_waiting(&store, &current, "external_state_uncertain").await?;
        if current.failure_code.as_deref() == Some("technical_recovery_exhausted") {
            break;
        }
        if current.status.as_str() == "waiting_system" {
            let claims = store
                .claim_due_remediations("history-incident-worker", 1, 30_000)
                .await?;
            if claims.len() != 1 {
                bail!("incident recovery round did not claim exactly once");
            }
            current = store
                .get(&current.id)
                .await?
                .context("reload incident objective")?;
        }
    }
    if current.requires_user_action
        || current.status.as_str() != "waiting_system"
        || current.failure_code.as_deref() != Some("technical_recovery_exhausted")
    {
        bail!("production recovery ceiling did not park a system-owned incident");
    }
    Ok(())
}

async fn seed(pool: &SqlitePool) -> anyhow::Result<()> {
    // Exhaust the incident fixture before seeding the independent continue/stop
    // Objectives. The recovery claimant is intentionally global, so relying on
    // row ordering after three fixtures exist makes this oracle nondeterministic.
    seed_incident_session(pool).await?;
    seed_continue_session(pool).await?;
    seed_stop_session(pool).await
}

async fn verify_incident_after_restart(pool: &SqlitePool) -> anyhow::Result<()> {
    let row: (
        String,
        i64,
        String,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<i64>,
        String,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT objective.status, objective.revision, turn.status,
                    turn.turn_settled_at, turn.stream_closed_at,
                    turn.terminal_revision, turn.visible_final_message_id,
                    turn.visible_final_kind, turn.next_action,
                    control.status, control.settled_at, tool.status,
                    receipt.status, incident.status, message.role
             FROM objectives objective
             JOIN chat_turn_state turn ON turn.objective_id=objective.id
             JOIN chat_run_controls control ON control.objective_id=objective.id
             JOIN tool_calls tool ON tool.objective_id=objective.id
             JOIN side_effect_receipts receipt ON receipt.objective_id=objective.id
             JOIN objective_incidents incident ON incident.objective_id=objective.id
             JOIN messages message ON message.id=turn.visible_final_message_id
             WHERE objective.session_id=?",
    )
    .bind(INCIDENT_SESSION)
    .fetch_one(pool)
    .await?;
    if row.0 != "waiting_system"
        || row.2 != "waiting_system"
        || row.3.is_none()
        || row.4.is_none()
        || row.5 != Some(row.1)
        || row.6.as_deref().map_or(true, str::is_empty)
        || row.7.as_deref() != Some("system_incident")
        || row.8.as_deref() != Some("await_system_recovery")
        || row.9 != "completed"
        || row.10.is_none()
        || row.11 != "blocked"
        || row.12 != "unknown"
        || row.13 != "open"
        || row.14 != "assistant"
    {
        bail!(
            "restarted incident oracle rejected objective={} revision={} turn={} terminal_revision={:?} final_kind={:?} next_action={:?} run={} tool={} receipt={} incident={} message_role={}",
            row.0,
            row.1,
            row.2,
            row.5,
            row.7,
            row.8,
            row.9,
            row.11,
            row.12,
            row.13,
            row.14,
        );
    }
    let claimable: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objective_remediations remediation
         JOIN objectives objective ON objective.id=remediation.objective_id
         WHERE objective.session_id=?
           AND remediation.status IN ('queued','waiting','claimed')",
    )
    .bind(INCIDENT_SESSION)
    .fetch_one(pool)
    .await?;
    if claimable != 0 {
        bail!("parked incident became claimable after restart");
    }
    Ok(())
}

async fn continue_after_restart(pool: &SqlitePool) -> anyhow::Result<()> {
    let store = ObjectiveStore::new(pool.clone());
    store
        .reconcile_stale_chat_run_controls(&current_process_instance())
        .await?;
    let existing_id: String = sqlx::query_scalar(
        "SELECT id FROM objectives WHERE session_id=? ORDER BY created_at, id LIMIT 1",
    )
    .bind(CONTINUE_SESSION)
    .fetch_one(pool)
    .await?;
    let admission = crate::commands::chat::admit_headless_chat_turn(pool, CONTINUE_SESSION, "继续")
        .await
        .map_err(|error| anyhow!(error.to_string()))?;
    if admission.objective.id != existing_id {
        bail!("historical continue created a second Objective");
    }
    Ok(())
}

async fn verify_continue_after_second_restart(pool: &SqlitePool) -> anyhow::Result<()> {
    let store = ObjectiveStore::new(pool.clone());
    store
        .reconcile_stale_chat_run_controls(&current_process_instance())
        .await?;
    let objective_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE session_id=?")
            .bind(CONTINUE_SESSION)
            .fetch_one(pool)
            .await?;
    let user_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id=? AND role='user'")
            .bind(CONTINUE_SESSION)
            .fetch_one(pool)
            .await?;
    let bound_turns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_turn_state turn
         JOIN objectives objective ON objective.id=turn.objective_id
         WHERE turn.session_id=? AND objective.session_id=?",
    )
    .bind(CONTINUE_SESSION)
    .bind(CONTINUE_SESSION)
    .fetch_one(pool)
    .await?;
    let driver_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_turn_state
         WHERE session_id=? AND user_reprompt_driver='system_owned_remediation_open'",
    )
    .bind(CONTINUE_SESSION)
    .fetch_one(pool)
    .await?;
    let padding_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE session_id=? AND role='assistant' AND id LIKE 'history-padding-%'",
    )
    .bind(CONTINUE_SESSION)
    .fetch_one(pool)
    .await?;
    if (
        objective_count,
        user_count,
        bound_turns,
        driver_count,
        padding_count,
    ) != (1, 2, 2, 1, HISTORY_PADDING)
    {
        bail!(
            "historical continue oracle rejected {objective_count}/{user_count}/{bound_turns}/{driver_count}/{padding_count}"
        );
    }
    Ok(())
}

async fn request_stop_then_crash(pool: &SqlitePool, state_dir: &Path) -> anyhow::Result<()> {
    let store = ObjectiveStore::new(pool.clone());
    store
        .reconcile_stale_chat_run_controls(&current_process_instance())
        .await?;
    store.request_chat_session_cancel(STOP_SESSION).await?;
    let claims = store
        .claim_due_remediations("history-stop-fenced-worker", 32, 60_000)
        .await?;
    if !claims.is_empty() {
        bail!("durable stop fence allowed remediation claims before consumption");
    }
    std::fs::write(state_dir.join("stop-fence-ready"), b"requested\n")?;
    // The parent hard-kills this worker after observing the durable marker.
    // A normal return here would weaken the restart contract into shutdown.
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn verify_cancelled(pool: &SqlitePool, expect_consumed: usize) -> anyhow::Result<()> {
    let store = ObjectiveStore::new(pool.clone());
    let consumed = store.consume_pending_chat_session_cancellations().await?;
    if consumed != expect_consumed {
        bail!("expected {expect_consumed} pending session stops, consumed {consumed}");
    }
    store
        .reconcile_stale_chat_run_controls(&current_process_instance())
        .await?;
    let claims = store
        .claim_due_remediations("history-stop-restart-worker", 32, 60_000)
        .await?;
    let live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives
         WHERE session_id=? AND status NOT IN ('completed','cancelled','legacy_orphan')",
    )
    .bind(STOP_SESSION)
    .fetch_one(pool)
    .await?;
    let cancelled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives
         WHERE session_id=? AND status='cancelled'
           AND cancellation_provenance='explicit_cancel'",
    )
    .bind(STOP_SESSION)
    .fetch_one(pool)
    .await?;
    let intent: String =
        sqlx::query_scalar("SELECT status FROM chat_session_cancel_intents WHERE session_id=?")
            .bind(STOP_SESSION)
            .fetch_one(pool)
            .await?;
    if live != 0 || cancelled != STOP_OBJECTIVE_COUNT || intent != "settled" || !claims.is_empty() {
        bail!(
            "restart cancellation oracle rejected live={live} cancelled={cancelled} intent={intent} claims={}",
            claims.len()
        );
    }
    Ok(())
}

pub(crate) async fn run_worker(state_dir: &Path, phase: &str) -> anyhow::Result<()> {
    let db_url = format!("sqlite:{}", state_dir.join("history-session.db").display());
    let pool = crate::storage::db::connect(&db_url).await?;
    let result = match phase {
        "seed" => seed(&pool).await,
        "verify-incident" | "verify-incident-again" => verify_incident_after_restart(&pool).await,
        "continue" => continue_after_restart(&pool).await,
        "verify-continue" => verify_continue_after_second_restart(&pool).await,
        "stop-request" => request_stop_then_crash(&pool, state_dir).await,
        "verify-stop" => verify_cancelled(&pool, 1).await,
        "verify-stop-again" => verify_cancelled(&pool, 0).await,
        _ => bail!("unknown history-session worker phase {phase}"),
    };
    crate::storage::db::close_and_release_files(pool).await;
    result
}

fn spawn_worker(state_dir: &Path, phase: &str) -> anyhow::Result<std::process::Child> {
    Command::new(std::env::current_exe()?)
        .no_window()
        .arg("--history-session-worker")
        .arg(state_dir)
        .arg(phase)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn history-session worker {phase}"))
}

async fn run_phase(state_dir: &Path, phase: &str) -> anyhow::Result<u32> {
    let mut child = spawn_worker(state_dir, phase)?;
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(pid);
            }
            bail!("history-session worker {phase} exited {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("history-session worker {phase} did not settle within 30 seconds");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn run_stop_request_fault(state_dir: &Path) -> anyhow::Result<u32> {
    let mut child = spawn_worker(state_dir, "stop-request")?;
    let pid = child.id();
    let marker = state_dir.join("stop-fence-ready");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait()? {
            bail!("stop-request worker exited before hard kill: {status}");
        }
        if marker.exists() {
            child.kill().context("hard-kill stop-request worker")?;
            let status = child.wait().context("reap stop-request worker")?;
            if status.success() {
                bail!("stop-request hard kill unexpectedly returned success");
            }
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("stop-request worker did not persist its fence within 30 seconds");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn run_parent() -> anyhow::Result<serde_json::Value> {
    let root = std::env::temp_dir().join(format!(
        "codefactory-history-session-smoke-{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root)?;
    let result = async {
        let phases = ["seed", "verify-incident", "continue", "verify-continue"];
        let mut pids = Vec::new();
        for phase in phases {
            pids.push(run_phase(&root, phase).await?);
        }
        pids.push(run_stop_request_fault(&root).await?);
        for phase in ["verify-stop", "verify-stop-again", "verify-incident-again"] {
            pids.push(run_phase(&root, phase).await?);
        }
        pids.sort_unstable();
        pids.dedup();
        if pids.len() != 8 {
            bail!("history-session smoke did not observe distinct worker processes");
        }

        let db_url = format!("sqlite:{}", root.join("history-session.db").display());
        let pool = crate::storage::db::connect(&db_url).await?;
        let continuation_objective_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE session_id=?")
                .bind(CONTINUE_SESSION)
                .fetch_one(&pool)
                .await?;
        let continuation_user_message_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id=? AND role='user'")
                .bind(CONTINUE_SESSION)
                .fetch_one(&pool)
                .await?;
        let continuation_bound_turn_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_turn_state turn
             JOIN objectives objective ON objective.id=turn.objective_id
             WHERE turn.session_id=? AND objective.session_id=?",
        )
        .bind(CONTINUE_SESSION)
        .bind(CONTINUE_SESSION)
        .fetch_one(&pool)
        .await?;
        let live_stop_objectives: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objectives
             WHERE session_id=? AND status NOT IN ('completed','cancelled','legacy_orphan')",
        )
        .bind(STOP_SESSION)
        .fetch_one(&pool)
        .await?;
        let explicit_cancel_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objectives
             WHERE session_id=? AND status='cancelled'
               AND cancellation_provenance='explicit_cancel'",
        )
        .bind(STOP_SESSION)
        .fetch_one(&pool)
        .await?;
        let claimable_remediation_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE objective.session_id=?
               AND remediation.status IN ('queued','waiting','claimed')",
        )
        .bind(STOP_SESSION)
        .fetch_one(&pool)
        .await?;
        let cancel_intent_status: String =
            sqlx::query_scalar("SELECT status FROM chat_session_cancel_intents WHERE session_id=?")
                .bind(STOP_SESSION)
                .fetch_one(&pool)
                .await?;
        crate::storage::db::close_and_release_files(pool).await;

        let same_objective = continuation_objective_count == 1
            && continuation_bound_turn_count == 2
            && continuation_user_message_count == 2;
        let all_live_objectives_cancelled =
            live_stop_objectives == 0 && explicit_cancel_count == STOP_OBJECTIVE_COUNT;
        if !same_objective
            || !all_live_objectives_cancelled
            || claimable_remediation_count != 0
            || cancel_intent_status != "settled"
        {
            bail!("final historical session oracle rejected persisted state");
        }
        Ok(serde_json::json!({
            "ok": true,
            "scenario_ids": ["E2E-002", "E2E-003", "E2E-007"],
            "build_git_sha": option_env!("CODEFACTORY_BUILD_GIT_SHA").unwrap_or("unknown"),
            "process_restart_count": 7,
            "stop_request_was_hard_killed": true,
            "same_objective": same_objective,
            "continuation_objective_count": continuation_objective_count,
            "continuation_user_message_count": continuation_user_message_count,
            "continuation_bound_turn_count": continuation_bound_turn_count,
            "history_outside_recent_page": true,
            "all_live_objectives_cancelled": all_live_objectives_cancelled,
            "explicit_cancel_count": explicit_cancel_count,
            "claimable_remediation_count": claimable_remediation_count,
            "cancel_intent_status": cancel_intent_status,
            "second_restart_stayed_cancelled": true,
            "system_incident_survived_two_restarts": true,
            "ui_oracle_status": "remaining_L3_real_desktop_gap",
            "cleanup_ok": false
        }))
    }
    .await;

    crate::util::fs_cleanup::remove_fixture_dir(&root).await;
    let cleanup_ok = !root.exists();
    match result {
        Ok(mut receipt) if cleanup_ok => {
            receipt["cleanup_ok"] = serde_json::Value::Bool(true);
            Ok(receipt)
        }
        Ok(_) => bail!("history-session smoke did not clean its isolated state"),
        Err(error) => Err(error),
    }
}
