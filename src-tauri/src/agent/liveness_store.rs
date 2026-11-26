// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use super::liveness::{LivenessOwner, LivenessOwnerKind, LivenessViolation};

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
    .bind(owner_kind)
    .bind(violation_kind)
    .bind(event.idle_ms.max(0))
    .bind(event.deadline_at)
    .bind(event.detected_at)
    .bind(&event.action)
    .bind(&event.outcome)
    .bind((event.outcome != "detected").then_some(event.detected_at))
    .bind(&event.detail_code)
    .execute(pool)
    .await?;
    Ok(id)
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
        let owner_kind = match status.as_str() {
            "waiting_authorization" => LivenessOwnerKind::Permission,
            "waiting_core_input" | "waiting_business_decision" => LivenessOwnerKind::UserInput,
            "waiting_system" => LivenessOwnerKind::Wake,
            _ if domain == "tool" => LivenessOwnerKind::Tool,
            _ if domain == "task" => LivenessOwnerKind::BackgroundTask,
            _ if domain == "delivery" => LivenessOwnerKind::Delivery,
            _ => LivenessOwnerKind::Model,
        };
        let lease_expires: Option<i64> = row.get("lease_expires_at");
        let next_observation: Option<i64> = row.get("next_observation_at");
        let deadline_at = match status.as_str() {
            "waiting_core_input" | "waiting_business_decision" => now_ms.saturating_add(60_000),
            "waiting_system" => next_observation.unwrap_or_else(|| now_ms.saturating_add(60_000)),
            _ => lease_expires
                .or(next_observation)
                .unwrap_or_else(|| updated_at.saturating_add(60_000)),
        };
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
                owner_id: owner_id.or(recovery_owner).unwrap_or_else(|| format!("objective:{objective_id}")),
                heartbeat_at: if matches!(status.as_str(), "waiting_core_input" | "waiting_business_decision") { now_ms } else { updated_at },
                deadline_at,
                not_before_at: next_observation,
                channel_alive: None,
                display_summary: status.clone(),
            },
            &status,
            None,
            now_ms,
        ).await?;
        projected += 1;
    }
    Ok(projected)
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
    Ok(LivenessMetrics {
        hours: bounded_hours,
        total_violations,
        longest_idle_ms,
        by_kind,
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
}
