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
const ABANDONED_SESSION: &str = "history-abandoned-session";
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
            // A repeating signature is scheduled with a growing backoff, so the
            // real supervisor would sit out minutes between these rounds. This
            // smoke is asserting convergence semantics, not wall-clock
            // scheduling, so stand in for that wait rather than weakening the
            // "claims exactly once per round" invariant below.
            sqlx::query(
                "UPDATE objective_remediations SET next_observation_at=?
                 WHERE objective_id=? AND status IN ('queued','waiting')",
            )
            .bind(chrono::Utc::now().timestamp_millis() - 1)
            .bind(&current.id)
            .execute(pool)
            .await?;
            let claims = store
                .claim_due_remediations("history-incident-worker", 1, 30_000)
                .await?;
            if claims.len() != 1 {
                bail!("incident recovery round did not claim exactly once");
            }
            let claim = &claims[0];
            if !store
                .charge_claimed_remediation_attempt(
                    &claim.objective.id,
                    &claim.remediation_id,
                    "history-incident-worker",
                    claim.claim_epoch,
                )
                .await?
            {
                bail!("incident recovery round lost its execution permit");
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

/// Replay of the two shapes a 2026-09-08 field report left behind, taken as
/// anonymised structure only — no session id, message text, path or tool
/// argument from the real database appears here.
///
/// 1. An Objective that stopped while `active`, owning no lease, no live
///    remediation and no `next_observation_at`, with a chat turn whose
///    heartbeat is ninety minutes old. Nothing claims it, so no poll can see
///    it: the real one sat like this for 83 minutes while the UI said 进行中.
/// 2. An Objective that reached a terminal state still carrying an unsettled
///    side-effect receipt. Production held five of those aged 17 to 20 days,
///    and the only remedy on record was editing the database by hand.
async fn seed_abandoned_session(pool: &SqlitePool) -> anyhow::Result<()> {
    ensure_session(pool, ABANDONED_SESSION, "Abandoned recovery smoke").await?;
    let admission = crate::commands::chat::admit_headless_chat_turn(
        pool,
        ABANDONED_SESSION,
        "继续推进既有长任务，并在系统故障时自行恢复。",
    )
    .await
    .map_err(|error| anyhow!(error.to_string()))?;
    let now = Utc::now().timestamp_millis();
    let stale = now - 90 * 60_000;

    // Nothing owns it and nothing is scheduled to look at it again.
    sqlx::query(
        "UPDATE objectives
         SET lease_owner=NULL, lease_expires_at=NULL, next_observation_at=NULL,
             last_progress_at=?
         WHERE id=?",
    )
    .bind(stale)
    .bind(&admission.objective.id)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE chat_turn_state SET updated_at=? WHERE objective_id=?")
        .bind(stale)
        .bind(&admission.objective.id)
        .execute(pool)
        .await?;

    // A terminal Objective in the same session, still holding an unsettled
    // receipt that no ladder will ever come back for.
    sqlx::query(
        "INSERT INTO objectives
         (id, revision, kind, status, decision_type, domain,
          requested_acceptance, cancellation_provenance, completed_at,
          created_surface, created_at, updated_at)
         VALUES ('history-abandoned-terminal', 1, 'informational', 'cancelled',
                 'cancelled', 'chat', 'informational_answer',
                 'explicit_cancel', ?, 'smoke', ?, ?)",
    )
    .bind(stale)
    .bind(stale)
    .bind(stale)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO side_effect_receipts
         (id, objective_id, revision, action_fingerprint, idempotency_key,
          status, created_at, observed_at)
         VALUES ('history-abandoned-orphan-receipt', 'history-abandoned-terminal',
                 1, 'sha256:abandoned-orphan', 'sha256:abandoned-orphan-key',
                 'unknown', ?, ?)",
    )
    .bind(now - 20 * 24 * 3_600_000)
    .bind(now - 20 * 24 * 3_600_000)
    .execute(pool)
    .await?;
    Ok(())
}

/// A fresh process, a database written by an earlier one: prove the gap is real
/// from here, then prove the sweeps close it.
async fn sweep_abandoned_after_restart(pool: &SqlitePool) -> anyhow::Result<()> {
    let store = ObjectiveStore::new(pool.clone());
    let stalled: String =
        sqlx::query_scalar("SELECT id FROM objectives WHERE session_id=? AND status='active'")
            .bind(ABANDONED_SESSION)
            .fetch_one(pool)
            .await?;

    // The gap itself: the ordinary claim path cannot see this Objective at all.
    let claims = store
        .claim_due_remediation_batch("history-abandoned-probe", 8, 30_000)
        .await?;
    if claims.claims.iter().any(|claim| claim.objective.id == stalled) {
        bail!("fixture is wrong: the stalled Objective was already claimable");
    }

    let reaped = store
        .reap_stalled_active_objectives(super::objective::STALLED_ACTIVE_OBJECTIVE_MS)
        .await?;
    if reaped.len() != 1 || reaped[0].id != stalled {
        bail!(
            "expected exactly the stalled Objective to be reaped, got {:?}",
            reaped.iter().map(|o| o.id.as_str()).collect::<Vec<_>>()
        );
    }
    if reaped[0].failure_code.as_deref() != Some(super::objective::OBJECTIVE_PROGRESS_STALLED) {
        bail!("a reaped Objective must carry the stalled failure code");
    }

    let swept = store.cancel_receipts_on_terminal_objectives().await?;
    if swept != 1 {
        bail!("expected one orphaned receipt to be cancelled, got {swept}");
    }
    // Both sweeps are idempotent: a second pass in the same process changes
    // nothing, which is what makes running them on a timer safe.
    if !store
        .reap_stalled_active_objectives(super::objective::STALLED_ACTIVE_OBJECTIVE_MS)
        .await?
        .is_empty()
        || store.cancel_receipts_on_terminal_objectives().await? != 0
    {
        bail!("sweeps are not idempotent");
    }
    Ok(())
}

/// One more restart: the settlement has to be durable, not a projection that
/// re-opens when the next process hydrates.
async fn verify_abandoned_settled(pool: &SqlitePool) -> anyhow::Result<()> {
    let (status, failure_code, claimable): (String, Option<String>, i64) = sqlx::query_as(
        "SELECT objective.status, objective.failure_code,
                (SELECT COUNT(*) FROM objective_remediations remediation
                 WHERE remediation.objective_id=objective.id
                   AND remediation.status IN ('queued','waiting','claimed'))
         FROM objectives objective
         WHERE objective.session_id=? AND objective.id<>'history-abandoned-terminal'",
    )
    .bind(ABANDONED_SESSION)
    .fetch_one(pool)
    .await?;
    if status == "active" {
        bail!("a swept Objective must not go back to claiming it is running");
    }
    if failure_code.as_deref() != Some(super::objective::OBJECTIVE_PROGRESS_STALLED) {
        bail!("the stall must survive the restart as a named failure");
    }
    if claimable == 0 {
        bail!("a reaped Objective must re-enter the recovery ladder, not merely stop");
    }
    let uncertain: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM side_effect_receipts
         WHERE objective_id='history-abandoned-terminal'
           AND status IN ('started','unknown')",
    )
    .fetch_one(pool)
    .await?;
    if uncertain != 0 {
        bail!("a terminal Objective must stop poisoning its own id across restarts");
    }
    // The E2E-001 baseline, asserted rather than asserted-about: the whole
    // sweep must happen with no one at the keyboard. One seeded user message,
    // and never a second — a recovery that needs someone to type 继续 is the
    // failure this fixture exists to catch.
    let user_messages: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id=? AND role='user'")
            .bind(ABANDONED_SESSION)
            .fetch_one(pool)
            .await?;
    if user_messages != 1 {
        bail!("unattended baseline broken: user message 总数为 {user_messages}, expected 1");
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
        "seed-abandoned" => seed_abandoned_session(&pool).await,
        "sweep-abandoned" => sweep_abandoned_after_restart(&pool).await,
        "verify-abandoned" => verify_abandoned_settled(&pool).await,
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
        for phase in [
            "verify-stop",
            "verify-stop-again",
            "verify-incident-again",
            // Appended last, and scoped to their own session, so replaying the
            // 2026-09-08 shapes cannot perturb the oracles above.
            "seed-abandoned",
            "sweep-abandoned",
            "verify-abandoned",
        ] {
            pids.push(run_phase(&root, phase).await?);
        }
        pids.sort_unstable();
        pids.dedup();
        if pids.len() != 11 {
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
            "scenario_ids": ["E2E-002", "E2E-003", "E2E-007", "E2E-012"],
            "build_git_sha": option_env!("CODEFACTORY_BUILD_GIT_SHA").unwrap_or("unknown"),
            "process_restart_count": 10,
            "abandoned_objective_reaped_across_restart": true,
            "terminal_objective_orphan_receipt_swept": true,
            "sweeps_are_idempotent": true,
            "sweep_timer_oracle_status": "supervisor_cadence_covered_by_unit_wiring_only",
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
