// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use std::sync::{Mutex, OnceLock};

const DEFAULT_STALE_OBJECTIVE_DAYS: i64 = 7;
const EVENT_RETENTION_LIMIT: i64 = 10_000;
const EVENT_RETENTION_AGE_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
pub const STALE_OBJECTIVE_AGE_MS: i64 = DEFAULT_STALE_OBJECTIVE_DAYS * 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LivenessRecovery {
    pub event_id: String,
    pub session_id: String,
    pub objective_id: Option<String>,
}

use super::liveness::{
    evaluate_session_liveness, LivenessEvaluation, LivenessOwner, LivenessOwnerKind,
    LivenessViolation, SessionLivenessSnapshot,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LivenessEventInput {
    pub session_id: String,
    pub objective_id: Option<String>,
    pub owner_kind: Option<LivenessOwnerKind>,
    pub violation: LivenessViolation,
    pub idle_ms: i64,
    pub deadline_at: Option<i64>,
    pub detected_at: i64,
    pub action: String,
    pub outcome: String,
    pub detail_code: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LivenessViolationCount {
    pub violation_kind: String,
    pub count: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LivenessMetrics {
    pub hours: i64,
    pub total_violations: i64,
    pub longest_idle_ms: i64,
    pub by_kind: Vec<LivenessViolationCount>,
    pub supervision_consecutive_failures: u64,
    pub supervision_last_error: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct SupervisionHealth {
    consecutive_failures: u64,
    last_error: Option<String>,
}

fn supervision_health() -> &'static Mutex<SupervisionHealth> {
    static HEALTH: OnceLock<Mutex<SupervisionHealth>> = OnceLock::new();
    HEALTH.get_or_init(|| Mutex::new(SupervisionHealth::default()))
}

pub fn record_supervision_success() {
    *supervision_health().lock().expect("liveness health lock") = SupervisionHealth::default();
}

pub fn record_supervision_failure(error: &anyhow::Error) {
    let mut health = supervision_health().lock().expect("liveness health lock");
    health.consecutive_failures = health.consecutive_failures.saturating_add(1);
    health.last_error = Some(error.to_string());
}

pub async fn claim_owner(
    pool: &SqlitePool,
    session_id: &str,
    objective_id: Option<&str>,
    root_turn_id: Option<&str>,
    owner: &LivenessOwner,
    display_kind: &str,
    process_instance: Option<&str>,
    now_ms: i64,
) -> anyhow::Result<()> {
    let owner_kind = serde_json::to_value(&owner.kind)?
        .as_str()
        .unwrap_or("supervisor")
        .to_owned();
    sqlx::query(
        "INSERT INTO session_liveness_leases(
             session_id, objective_id, root_turn_id, owner_kind, owner_id,
             process_instance, lease_generation, started_at, heartbeat_at,
             deadline_at, not_before_at, channel_alive, display_kind,
             display_summary, status, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, 'active', ?)
         ON CONFLICT(session_id) DO UPDATE SET
             objective_id=excluded.objective_id,
             root_turn_id=excluded.root_turn_id,
             owner_kind=excluded.owner_kind,
             owner_id=excluded.owner_id,
             process_instance=excluded.process_instance,
             lease_generation=session_liveness_leases.lease_generation+1,
             started_at=excluded.started_at,
             heartbeat_at=excluded.heartbeat_at,
             deadline_at=excluded.deadline_at,
             not_before_at=excluded.not_before_at,
             channel_alive=excluded.channel_alive,
             display_kind=excluded.display_kind,
             display_summary=excluded.display_summary,
             status='active',
             updated_at=excluded.updated_at",
    )
    .bind(session_id)
    .bind(objective_id)
    .bind(root_turn_id)
    .bind(owner_kind)
    .bind(&owner.owner_id)
    .bind(process_instance)
    .bind(now_ms)
    .bind(owner.heartbeat_at)
    .bind(owner.deadline_at)
    .bind(owner.not_before_at)
    .bind(owner.channel_alive.map(i64::from))
    .bind(display_kind)
    .bind(&owner.display_summary)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_event(pool: &SqlitePool, event: &LivenessEventInput) -> anyhow::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let owner_kind = event
        .owner_kind
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .and_then(|value| value.as_str().map(ToOwned::to_owned));
    let violation_kind = serde_json::to_value(&event.violation)?
        .as_str()
        .unwrap_or("unknown")
        .to_owned();
    sqlx::query(
        "INSERT INTO session_liveness_events(
             id, session_id, objective_id, owner_kind, violation_kind, idle_ms,
             deadline_at, detected_at, action, outcome, resolved_at, detail_code
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&event.session_id)
    .bind(&event.objective_id)
    .bind(owner_kind.as_deref())
    .bind(&violation_kind)
    .bind(event.idle_ms.max(0))
    .bind(event.deadline_at)
    .bind(event.detected_at)
    .bind(&event.action)
    .bind(&event.outcome)
    .bind((event.outcome != "detected").then_some(event.detected_at))
    .bind(&event.detail_code)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM session_liveness_events WHERE detected_at < ?")
        .bind(event.detected_at.saturating_sub(EVENT_RETENTION_AGE_MS))
        .execute(pool).await?;
    sqlx::query(
        "DELETE FROM session_liveness_events WHERE id IN (
             SELECT id FROM session_liveness_events ORDER BY detected_at DESC, id DESC
             LIMIT -1 OFFSET ?
         )",
    ).bind(EVENT_RETENTION_LIMIT).execute(pool).await?;
    Ok(id)
}

pub async fn resolve_event(
    pool: &SqlitePool,
    event_id: &str,
    outcome: &str,
    detail_code: &str,
    resolved_at: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE session_liveness_events
         SET outcome=?, detail_code=?, resolved_at=? WHERE id=? AND outcome='detected'",
    ).bind(outcome).bind(detail_code).bind(resolved_at).bind(event_id)
     .execute(pool).await?;
    Ok(())
}

async fn table_exists(pool: &SqlitePool, table: &str) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
    ).bind(table).fetch_one(pool).await? > 0)
}

fn stale_objective_age_ms() -> i64 {
    std::env::var("CODEFACTORY_LIVENESS_STALE_DAYS")
        .ok().and_then(|value| value.parse::<i64>().ok())
        .filter(|days| *days > 0)
        .unwrap_or(DEFAULT_STALE_OBJECTIVE_DAYS)
        .saturating_mul(24 * 60 * 60 * 1_000)
}

pub async fn project_unfinished_objectives(
    pool: &SqlitePool,
    now_ms: i64,
) -> anyhow::Result<u64> {
    let rows = sqlx::query(
        "SELECT id, session_id, root_turn_id, status, domain, recovery_owner,
                lease_owner, lease_expires_at, next_observation_at, updated_at
         FROM objectives
         WHERE session_id IS NOT NULL
           AND status NOT IN ('completed','cancelled','failed','legacy_orphan')
         ORDER BY session_id, updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    let has_background_tasks = table_exists(pool, "background_tasks").await?;
    let has_wakeups = table_exists(pool, "wakeup_schedules").await?;
    let has_delivery_observations = table_exists(pool, "thin_external_observations").await?;
    let stale_age_ms = stale_objective_age_ms();
    let mut projected = 0_u64;
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let session_id: String = row.get("session_id");
        let objective_id: String = row.get("id");
        let status: String = row.get("status");
        let domain: String = row.get("domain");
        let updated_at: i64 = row.get("updated_at");
        if !seen.insert(session_id.clone()) {
            record_event(pool, &LivenessEventInput {
                session_id,
                objective_id: Some(objective_id),
                owner_kind: None,
                violation: LivenessViolation::MultipleOwners,
                idle_ms: now_ms.saturating_sub(updated_at),
                deadline_at: None,
                detected_at: now_ms,
                action: "fence_duplicate_objective".into(),
                outcome: "detected".into(),
                detail_code: Some("multiple_unfinished_objectives".into()),
            }).await?;
            continue;
        }
        if now_ms.saturating_sub(updated_at) > stale_age_ms {
            let settled = sqlx::query(
                "UPDATE objectives SET status='legacy_orphan', requires_user_action=0,
                        completed_at=COALESCE(completed_at,?), updated_at=?
                 WHERE id=? AND status NOT IN ('completed','cancelled','failed','legacy_orphan')
                   AND updated_at=?",
            ).bind(now_ms).bind(now_ms).bind(&objective_id).bind(updated_at)
             .execute(pool).await?.rows_affected();
            if settled > 0 {
                record_event(pool, &LivenessEventInput {
                    session_id, objective_id: Some(objective_id), owner_kind: None,
                    violation: LivenessViolation::DeadlineExceeded,
                    idle_ms: now_ms.saturating_sub(updated_at), deadline_at: None,
                    detected_at: now_ms, action: "settle_stale_objective".into(),
                    outcome: "settled_stale".into(), detail_code: Some("stale_objective".into()),
                }).await?;
            }
            continue;
        }
        let background_owner: Option<(String, i64, i64)> = if has_background_tasks {
            sqlx::query_as(
                "SELECT id,created_at,timeout_sec FROM background_tasks
                 WHERE session_id=? AND status='running' ORDER BY created_at DESC LIMIT 1",
            ).bind(&session_id).fetch_optional(pool).await?
        } else { None };
        let wake_owner: Option<(String, i64)> = if background_owner.is_none() && has_wakeups {
            sqlx::query_as(
                "SELECT id,next_at FROM wakeup_schedules
                 WHERE session_id=? AND status='pending' ORDER BY next_at LIMIT 1",
            ).bind(&session_id).fetch_optional(pool).await?
        } else { None };
        let delivery_owner: Option<(i64, i64)> = if background_owner.is_none()
            && wake_owner.is_none() && has_delivery_observations {
            sqlx::query_as(
                "SELECT pr_number,next_check_at FROM thin_external_observations
                 WHERE session_id=? AND completed_reason IS NULL LIMIT 1",
            ).bind(&session_id).fetch_optional(pool).await?
        } else { None };
        let owner_kind = if background_owner.is_some() {
            LivenessOwnerKind::BackgroundTask
        } else if wake_owner.is_some() {
            LivenessOwnerKind::Wake
        } else if delivery_owner.is_some() {
            LivenessOwnerKind::Delivery
        } else { match status.as_str() {
            "waiting_authorization" => LivenessOwnerKind::Permission,
            "waiting_core_input" | "waiting_business_decision" => LivenessOwnerKind::UserInput,
            "waiting_system" => LivenessOwnerKind::Wake,
            _ if domain == "tool" => LivenessOwnerKind::Tool,
            _ if domain == "task" => LivenessOwnerKind::BackgroundTask,
            _ if domain == "delivery" => LivenessOwnerKind::Delivery,
            _ => LivenessOwnerKind::Model,
        }};
        let lease_expires: Option<i64> = row.get("lease_expires_at");
        let next_observation: Option<i64> = row.get("next_observation_at");
        let deadline_at = if let Some((_, created_at, timeout_sec)) = background_owner.as_ref() {
            created_at.saturating_add(*timeout_sec).saturating_mul(1_000)
        } else if let Some((_, next_at)) = wake_owner.as_ref() {
            next_at.saturating_mul(1_000).saturating_add(super::liveness::LIVENESS_SUPERVISOR_INTERVAL_MS)
        } else if let Some((_, next_check_at)) = delivery_owner.as_ref() {
            *next_check_at
        } else { match status.as_str() {
            "waiting_core_input" | "waiting_business_decision" => now_ms.saturating_add(60_000),
            "waiting_system" => next_observation.unwrap_or_else(|| now_ms.saturating_add(60_000)),
            _ => lease_expires
                .or(next_observation)
                .unwrap_or_else(|| updated_at.saturating_add(60_000)),
        }};
        let owner_id: Option<String> = row.get("lease_owner");
        let recovery_owner: Option<String> = row.get("recovery_owner");
        let root_turn_id: Option<String> = row.get("root_turn_id");
        claim_owner(
            pool,
            &session_id,
            Some(&objective_id),
            root_turn_id.as_deref(),
            &LivenessOwner {
                kind: owner_kind,
                owner_id: background_owner.as_ref().map(|owner| owner.0.clone())
                    .or_else(|| wake_owner.as_ref().map(|owner| owner.0.clone()))
                    .or_else(|| delivery_owner.as_ref().map(|owner| format!("pr:{}", owner.0)))
                    .or(owner_id).or(recovery_owner)
                    .unwrap_or_else(|| format!("objective:{objective_id}")),
                heartbeat_at: if background_owner.is_some() || wake_owner.is_some() || delivery_owner.is_some()
                    || matches!(status.as_str(), "waiting_core_input" | "waiting_business_decision") {
                    now_ms
                } else { updated_at },
                deadline_at,
                not_before_at: next_observation,
                channel_alive: None,
                display_summary: match status.as_str() {
                    "waiting_authorization" => "等待你批准".into(),
                    "waiting_core_input" => "等待你回答".into(),
                    "waiting_business_decision" => "等待你决定".into(),
                    "waiting_system" => match next_observation {
                        Some(at) => format!("系统自动重试/自愈，{at} 自动继续"),
                        None => "系统自动重试/自愈，稍后自动继续".into(),
                    },
                    _ if background_owner.is_some() => "后台任务正在运行".into(),
                    _ if wake_owner.is_some() => "系统自动重试/自愈，稍后自动继续".into(),
                    _ if delivery_owner.is_some() => "正在核对交付状态".into(),
                    _ if domain == "tool" => "正在运行工具".into(),
                    _ if domain == "task" => "后台任务正在运行".into(),
                    _ if domain == "delivery" => "正在核对交付状态".into(),
                    _ => "正在连接模型".into(),
                },
            },
            &status,
            None,
            now_ms,
        ).await?;
        projected += 1;
    }
    Ok(projected)
}

pub async fn scan_liveness_violations(
    pool: &SqlitePool,
    now_ms: i64,
) -> anyhow::Result<Vec<LivenessRecovery>> {
    let rows = sqlx::query(
        "SELECT session_id, objective_id, owner_kind, owner_id, heartbeat_at,
                deadline_at, not_before_at, display_summary
         FROM session_liveness_leases WHERE status='active' ORDER BY session_id",
    ).fetch_all(pool).await?;
    let permission_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='permission_intents'",
    ).fetch_one(pool).await?;
    let mut objectives = Vec::new();
    for row in rows {
        let session_id: String = row.get("session_id");
        let objective_id: Option<String> = row.get("objective_id");
        let kind_text: String = row.get("owner_kind");
        let kind: LivenessOwnerKind = serde_json::from_value(serde_json::Value::String(kind_text.clone()))?;
        let channel_alive = if kind == LivenessOwnerKind::Permission && permission_table > 0 {
            let objective = objective_id.as_deref().unwrap_or("");
            Some(sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM permission_intents WHERE objective_id=? AND status='pending'",
            ).bind(objective).fetch_one(pool).await? > 0)
        } else { None };
        let owner = LivenessOwner {
            kind: kind.clone(), owner_id: row.get("owner_id"),
            heartbeat_at: row.get("heartbeat_at"), deadline_at: row.get("deadline_at"),
            not_before_at: row.get("not_before_at"), channel_alive,
            display_summary: row.get("display_summary"),
        };
        let snapshot = SessionLivenessSnapshot {
            session_id: session_id.clone(), objective_id: objective_id.clone(), owners: vec![owner.clone()],
        };
        let LivenessEvaluation::Violation(violation) = evaluate_session_liveness(&snapshot, now_ms) else { continue; };
        let violation_kind = serde_json::to_value(&violation)?
            .as_str().unwrap_or("unknown").to_owned();
        let already_recorded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM session_liveness_events
             WHERE session_id=? AND owner_kind=? AND deadline_at=? AND violation_kind=?",
        ).bind(&session_id).bind(&kind_text).bind(owner.deadline_at).bind(&violation_kind)
         .fetch_one(pool).await?;
        if already_recorded > 0 {
            sqlx::query(
                "UPDATE session_liveness_leases SET status='settled', updated_at=?
                 WHERE session_id=? AND status='active' AND owner_id=? AND deadline_at=?",
            ).bind(now_ms).bind(&session_id).bind(&owner.owner_id).bind(owner.deadline_at)
             .execute(pool).await?;
            continue;
        }
        let event_id = record_event(pool, &LivenessEventInput {
            session_id: session_id.clone(), objective_id: objective_id.clone(),
            owner_kind: Some(kind), violation: violation.clone(),
            idle_ms: now_ms.saturating_sub(owner.heartbeat_at), deadline_at: Some(owner.deadline_at),
            detected_at: now_ms, action: "resume_original_objective".into(), outcome: "detected".into(),
            detail_code: Some(match violation {
                LivenessViolation::DeadChannel => "permission_channel_dead",
                LivenessViolation::WakeBeforeNotBefore => "wake_before_not_before",
                LivenessViolation::WakeTooLate => "wake_too_late",
                LivenessViolation::DeadlineExceeded => "liveness_deadline_exceeded",
                LivenessViolation::MissingOwner => "missing_owner",
                LivenessViolation::MultipleOwners => "multiple_owners",
            }.into()),
        }).await?;
        sqlx::query(
            "UPDATE session_liveness_leases SET status='settled', updated_at=? WHERE session_id=? AND status='active'",
        ).bind(now_ms).bind(&session_id).execute(pool).await?;
        if objective_id.is_some() {
            objectives.push(LivenessRecovery { event_id, session_id, objective_id });
        }
    }
    Ok(objectives)
}

pub async fn supervise_expired_leases(
    pool: &SqlitePool,
    now_ms: i64,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT session_id, objective_id, owner_kind, heartbeat_at, deadline_at
         FROM session_liveness_leases
         WHERE status='active' AND deadline_at < ?
         ORDER BY deadline_at",
    )
    .bind(now_ms)
    .fetch_all(pool)
    .await?;
    let mut objectives = Vec::new();
    for row in rows {
        let session_id: String = row.get("session_id");
        let objective_id: Option<String> = row.get("objective_id");
        let owner_kind: String = row.get("owner_kind");
        let heartbeat_at: i64 = row.get("heartbeat_at");
        let deadline_at: i64 = row.get("deadline_at");
        let event = LivenessEventInput {
            session_id: session_id.clone(),
            objective_id: objective_id.clone(),
            owner_kind: serde_json::from_value(serde_json::Value::String(owner_kind.clone())).ok(),
            violation: LivenessViolation::DeadlineExceeded,
            idle_ms: now_ms.saturating_sub(heartbeat_at),
            deadline_at: Some(deadline_at),
            detected_at: now_ms,
            action: "resume_original_objective".into(),
            outcome: "recovered".into(),
            detail_code: Some("liveness_deadline_exceeded".into()),
        };
        record_event(pool, &event).await?;
        sqlx::query(
            "UPDATE session_liveness_leases SET status='settled', updated_at=?
             WHERE session_id=? AND status='active' AND deadline_at=?",
        )
        .bind(now_ms)
        .bind(&session_id)
        .bind(deadline_at)
        .execute(pool)
        .await?;
        if let Some(objective_id) = objective_id {
            objectives.push(objective_id);
        }
    }
    Ok(objectives)
}

pub async fn run_supervision_cycle(
    pool: &SqlitePool,
    now_ms: i64,
) -> anyhow::Result<Vec<LivenessRecovery>> {
    project_unfinished_objectives(pool, now_ms).await?;
    scan_liveness_violations(pool, now_ms).await
}

pub async fn metrics(pool: &SqlitePool, hours: i64, now_ms: i64) -> anyhow::Result<LivenessMetrics> {
    let bounded_hours = hours.clamp(1, 24 * 30);
    let since = now_ms.saturating_sub(bounded_hours.saturating_mul(3_600_000));
    let rows = sqlx::query(
        "SELECT violation_kind, COUNT(*) AS count
         FROM session_liveness_events
         WHERE detected_at >= ?
         GROUP BY violation_kind
         ORDER BY violation_kind",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;
    let by_kind = rows
        .into_iter()
        .map(|row| LivenessViolationCount {
            violation_kind: row.get("violation_kind"),
            count: row.get("count"),
        })
        .collect::<Vec<_>>();
    let (total_violations, longest_idle_ms): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(MAX(idle_ms), 0)
         FROM session_liveness_events
         WHERE detected_at >= ?",
    )
    .bind(since)
    .fetch_one(pool)
    .await?;
    let health = supervision_health().lock().expect("liveness health lock").clone();
    Ok(LivenessMetrics {
        hours: bounded_hours,
        total_violations,
        longest_idle_ms,
        by_kind,
        supervision_consecutive_failures: health.consecutive_failures,
        supervision_last_error: health.last_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/0027_session_liveness.sql"))
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn r1_claim_replaces_owner_atomically_and_keeps_one_active_row() {
        let pool = pool().await;
        for (id, kind) in [
            ("model-1", LivenessOwnerKind::Model),
            ("tool-1", LivenessOwnerKind::Tool),
        ] {
            claim_owner(
                &pool,
                "session-1",
                Some("objective-1"),
                Some("turn-1"),
                &LivenessOwner {
                    kind,
                    owner_id: id.into(),
                    heartbeat_at: 10,
                    deadline_at: 20,
                    not_before_at: None,
                    channel_alive: None,
                    display_summary: "推进中".into(),
                },
                "working",
                Some("process-1"),
                10,
            )
            .await
            .unwrap();
        }
        let (count, owner, generation): (i64, String, i64) = sqlx::query_as(
            "SELECT COUNT(*), MAX(owner_id), MAX(lease_generation)
             FROM session_liveness_leases WHERE session_id='session-1' AND status='active'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((count, owner.as_str(), generation), (1, "tool-1", 2));
    }

    #[tokio::test]
    async fn r2_r8_events_are_structured_and_metrics_group_recent_violations() {
        let pool = pool().await;
        for (violation, idle_ms, detected_at) in [
            (LivenessViolation::DeadlineExceeded, 12_000, 90_000),
            (LivenessViolation::DeadlineExceeded, 45_000, 95_000),
            (LivenessViolation::DeadChannel, 8_000, 98_000),
            (LivenessViolation::MissingOwner, 99_000, -4_000_000),
        ] {
            record_event(
                &pool,
                &LivenessEventInput {
                    session_id: "session-1".into(),
                    objective_id: Some("objective-1".into()),
                    owner_kind: None,
                    violation,
                    idle_ms,
                    deadline_at: None,
                    detected_at,
                    action: "resume_original_objective".into(),
                    outcome: "recovered".into(),
                    detail_code: None,
                },
            )
            .await
            .unwrap();
        }
        let metrics = metrics(&pool, 1, 100_000).await.unwrap();
        assert_eq!(metrics.total_violations, 3);
        assert_eq!(metrics.longest_idle_ms, 45_000);
        assert_eq!(metrics.supervision_consecutive_failures, 0);
        assert_eq!(metrics.supervision_last_error, None);
        assert_eq!(
            metrics.by_kind,
            vec![
                LivenessViolationCount {
                    violation_kind: "dead_channel".into(),
                    count: 1,
                },
                LivenessViolationCount {
                    violation_kind: "deadline_exceeded".into(),
                    count: 2,
                },
            ]
        );
    }

    #[tokio::test]
    async fn m287b_r2_repeated_supervision_failure_is_visible_and_recovery_clears_it() {
        record_supervision_success();
        let error = anyhow::anyhow!("synthetic liveness schema failure");
        record_supervision_failure(&error);
        record_supervision_failure(&error);
        let pool = pool().await;
        let degraded = metrics(&pool, 24, 100_000).await.unwrap();
        assert_eq!(degraded.supervision_consecutive_failures, 2);
        assert_eq!(
            degraded.supervision_last_error.as_deref(),
            Some("synthetic liveness schema failure")
        );

        record_supervision_success();
        let recovered = metrics(&pool, 24, 100_000).await.unwrap();
        assert_eq!(recovered.supervision_consecutive_failures, 0);
        assert_eq!(recovered.supervision_last_error, None);
    }

    #[tokio::test]
    async fn r2_supervisor_settles_only_expired_leases_and_records_violation() {
        let pool = pool().await;
        for (session, deadline) in [("expired", 90), ("healthy", 200)] {
            claim_owner(
                &pool,
                session,
                Some(&format!("objective-{session}")),
                Some("turn-1"),
                &LivenessOwner {
                    kind: LivenessOwnerKind::Model,
                    owner_id: format!("owner-{session}"),
                    heartbeat_at: 80,
                    deadline_at: deadline,
                    not_before_at: None,
                    channel_alive: None,
                    display_summary: "正在连接模型".into(),
                },
                "model_connecting",
                Some("process-1"),
                80,
            )
            .await
            .unwrap();
        }

        let recovered = supervise_expired_leases(&pool, 100).await.unwrap();
        assert_eq!(recovered, vec!["objective-expired"]);
        let statuses: Vec<(String, String)> = sqlx::query_as(
            "SELECT session_id, status FROM session_liveness_leases ORDER BY session_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            statuses,
            vec![
                ("expired".into(), "settled".into()),
                ("healthy".into(), "active".into()),
            ]
        );
        let event: (String, String, String) = sqlx::query_as(
            "SELECT violation_kind, action, detail_code FROM session_liveness_events",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            event,
            (
                "deadline_exceeded".into(),
                "resume_original_objective".into(),
                "liveness_deadline_exceeded".into(),
            )
        );
        assert!(supervise_expired_leases(&pool, 100).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn r1_unfinished_objective_projects_one_owner_and_user_wait_renews_visibility() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE objectives(
                id TEXT PRIMARY KEY, session_id TEXT, root_turn_id TEXT, status TEXT,
                domain TEXT, recovery_owner TEXT, lease_owner TEXT,
                lease_expires_at INTEGER, next_observation_at INTEGER, updated_at INTEGER)",
        ).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO objectives VALUES(
                'objective-1','session-1','turn-1','waiting_core_input','chat',
                NULL,NULL,NULL,NULL,10)",
        ).execute(&pool).await.unwrap();

        assert_eq!(project_unfinished_objectives(&pool, 100).await.unwrap(), 1);
        let first: (String, String, i64, i64) = sqlx::query_as(
            "SELECT owner_kind, status, heartbeat_at, deadline_at
             FROM session_liveness_leases WHERE session_id='session-1'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(first, ("user_input".into(), "active".into(), 100, 60_100));

        assert_eq!(project_unfinished_objectives(&pool, 200).await.unwrap(), 1);
        let renewed: (i64, i64) = sqlx::query_as(
            "SELECT heartbeat_at, deadline_at FROM session_liveness_leases
             WHERE session_id='session-1'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(renewed, (200, 60_200));
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM session_liveness_leases
             WHERE session_id='session-1' AND status='active'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn r2_production_scanner_detects_dead_permission_and_bad_wake_boundaries() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE permission_intents(objective_id TEXT, status TEXT)",
        ).execute(&pool).await.unwrap();
        for (session, objective, owner) in [
            ("permission", "objective-permission", LivenessOwner {
                kind: LivenessOwnerKind::Permission,
                owner_id: "prompt-1".into(), heartbeat_at: 90, deadline_at: 200,
                not_before_at: None, channel_alive: None,
                display_summary: "等待你批准".into(),
            }),
            ("wake", "objective-wake", LivenessOwner {
                kind: LivenessOwnerKind::Wake,
                owner_id: "wake-1".into(), heartbeat_at: 90, deadline_at: 110,
                not_before_at: Some(120), channel_alive: None,
                display_summary: "系统自动重试/自愈，120 自动继续".into(),
            }),
        ] {
            claim_owner(
                &pool, session, Some(objective), Some("turn-1"), &owner,
                "waiting", Some("process-1"), 90,
            ).await.unwrap();
        }

        let mut recovered = scan_liveness_violations(&pool, 100).await.unwrap();
        recovered.sort_by(|left, right| left.objective_id.cmp(&right.objective_id));
        assert_eq!(
            recovered.into_iter().filter_map(|item| item.objective_id).collect::<Vec<_>>(),
            vec!["objective-permission".to_string(), "objective-wake".to_string()]
        );
        let events: Vec<(String, String)> = sqlx::query_as(
            "SELECT violation_kind, detail_code FROM session_liveness_events ORDER BY violation_kind",
        ).fetch_all(&pool).await.unwrap();
        assert_eq!(
            events,
            vec![
                ("dead_channel".into(), "permission_channel_dead".into()),
                ("wake_before_not_before".into(), "wake_before_not_before".into()),
            ]
        );
    }

    #[tokio::test]
    async fn r3_objective_projection_emits_standard_user_wording() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE objectives(
                id TEXT PRIMARY KEY, session_id TEXT, root_turn_id TEXT, status TEXT,
                domain TEXT, recovery_owner TEXT, lease_owner TEXT,
                lease_expires_at INTEGER, next_observation_at INTEGER, updated_at INTEGER)",
        ).execute(&pool).await.unwrap();
        for (id, session, status, domain, next_at) in [
            ("objective-model", "model", "active", "chat", None),
            ("objective-tool", "tool", "active", "tool", None),
            ("objective-permit", "permit", "waiting_authorization", "permission", None),
            ("objective-wake", "wake", "waiting_system", "chat", Some(12_000)),
        ] {
            sqlx::query(
                "INSERT INTO objectives VALUES(?,?,NULL,?,?,NULL,NULL,NULL,?,100)",
            ).bind(id).bind(session).bind(status).bind(domain).bind(next_at)
             .execute(&pool).await.unwrap();
        }
        project_unfinished_objectives(&pool, 1_000).await.unwrap();
        let summaries: Vec<String> = sqlx::query_scalar(
            "SELECT display_summary FROM session_liveness_leases ORDER BY session_id",
        ).fetch_all(&pool).await.unwrap();
        assert!(summaries.iter().any(|text| text == "正在连接模型"));
        assert!(summaries.iter().any(|text| text == "正在运行工具"));
        assert!(summaries.iter().any(|text| text == "等待你批准"));
        assert!(summaries.iter().any(|text| text.contains("自动继续")));
    }

    #[tokio::test]
    async fn m287c_r1_running_background_task_owns_its_real_timeout_deadline() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE objectives(
                id TEXT PRIMARY KEY, session_id TEXT, root_turn_id TEXT, status TEXT,
                domain TEXT, recovery_owner TEXT, lease_owner TEXT,
                lease_expires_at INTEGER, next_observation_at INTEGER, updated_at INTEGER)",
        ).execute(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE background_tasks(
                id TEXT PRIMARY KEY, session_id TEXT, status TEXT,
                created_at INTEGER, timeout_sec INTEGER)",
        ).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO objectives VALUES('objective-bg','session-bg','turn-bg','active','chat',NULL,NULL,NULL,NULL,1000)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO background_tasks VALUES('task-bg','session-bg','running',10,120)")
            .execute(&pool).await.unwrap();

        project_unfinished_objectives(&pool, 61_000).await.unwrap();
        let owner: (String, String, i64) = sqlx::query_as(
            "SELECT owner_kind,owner_id,deadline_at FROM session_liveness_leases WHERE session_id='session-bg'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(owner, ("background_task".into(), "task-bg".into(), 130_000));
        assert!(scan_liveness_violations(&pool, 61_000).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn m287c_r2_same_owner_deadline_violation_is_recorded_only_once() {
        let pool = pool().await;
        claim_owner(
            &pool, "session-expired", Some("objective-expired"), Some("turn-1"),
            &LivenessOwner {
                kind: LivenessOwnerKind::Model, owner_id: "model-attempt-1".into(),
                heartbeat_at: 10, deadline_at: 20, not_before_at: None,
                channel_alive: None, display_summary: "正在连接模型".into(),
            }, "model_connecting", Some("process-1"), 10,
        ).await.unwrap();

        assert_eq!(scan_liveness_violations(&pool, 30).await.unwrap().len(), 1);
        assert!(scan_liveness_violations(&pool, 40).await.unwrap().is_empty());
        let event: (i64, String) = sqlx::query_as(
            "SELECT COUNT(*),MAX(outcome) FROM session_liveness_events WHERE session_id='session-expired'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(event, (1, "detected".into()));
    }

    #[tokio::test]
    async fn m287c_r4_stale_objective_is_settled_once_without_recovery() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE objectives(
                id TEXT PRIMARY KEY, session_id TEXT, root_turn_id TEXT, status TEXT,
                domain TEXT, recovery_owner TEXT, lease_owner TEXT,
                lease_expires_at INTEGER, next_observation_at INTEGER, updated_at INTEGER,
                requires_user_action INTEGER DEFAULT 0, completed_at INTEGER)",
        ).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO objectives(id,session_id,root_turn_id,status,domain,updated_at) VALUES('objective-stale','session-stale','turn-stale','active','chat',1)")
            .execute(&pool).await.unwrap();

        assert_eq!(project_unfinished_objectives(&pool, STALE_OBJECTIVE_AGE_MS + 2).await.unwrap(), 0);
        assert_eq!(project_unfinished_objectives(&pool, STALE_OBJECTIVE_AGE_MS + 3).await.unwrap(), 0);
        let status: String = sqlx::query_scalar("SELECT status FROM objectives WHERE id='objective-stale'")
            .fetch_one(&pool).await.unwrap();
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM session_liveness_events WHERE session_id='session-stale' AND action='settle_stale_objective'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "legacy_orphan");
        assert_eq!(events, 1);
    }

    #[tokio::test]
    async fn m287c_r3_detection_does_not_claim_recovery_before_apply_succeeds() {
        let pool = pool().await;
        claim_owner(
            &pool, "session-failed-apply", Some("objective-failed-apply"), Some("turn-1"),
            &LivenessOwner {
                kind: LivenessOwnerKind::Model, owner_id: "attempt".into(),
                heartbeat_at: 10, deadline_at: 20, not_before_at: None,
                channel_alive: None, display_summary: "正在连接模型".into(),
            }, "model_connecting", Some("process-1"), 10,
        ).await.unwrap();

        let recoveries = scan_liveness_violations(&pool, 30).await.unwrap();
        assert_eq!(recoveries.len(), 1);
        let recovery = recoveries.into_iter().next().unwrap();
        let outcome: String = sqlx::query_scalar(
            "SELECT outcome FROM session_liveness_events WHERE session_id='session-failed-apply'",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(outcome, "detected");

        resolve_event(
            &pool, &recovery.event_id, "decision_rejected", "apply_decision_failed", 31,
        ).await.unwrap();
        let resolved: (String, String, i64) = sqlx::query_as(
            "SELECT outcome,detail_code,resolved_at FROM session_liveness_events WHERE id=?",
        ).bind(recovery.event_id).fetch_one(&pool).await.unwrap();
        assert_eq!(resolved, ("decision_rejected".into(), "apply_decision_failed".into(), 31));
    }
}
