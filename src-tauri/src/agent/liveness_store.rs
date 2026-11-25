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
}
