// SPDX-License-Identifier: Apache-2.0

#[path = "../src/agent/liveness.rs"]
mod liveness;
#[path = "../src/agent/liveness_store.rs"]
mod liveness_store;

use liveness::{
    evaluate_session_liveness, normalize_wake_at, LivenessEvaluation, LivenessOwner,
    LivenessOwnerKind, LivenessViolation, SessionLivenessSnapshot,
};
use liveness_store::{claim_owner, metrics, record_event, LivenessEventInput};
use sqlx::SqlitePool;

fn owner(kind: LivenessOwnerKind, deadline_at: i64) -> LivenessOwner {
    LivenessOwner {
        kind,
        owner_id: "integration-owner".into(),
        heartbeat_at: 9_000,
        deadline_at,
        not_before_at: None,
        channel_alive: None,
        display_summary: "正在推进".into(),
    }
}

fn snapshot(owners: Vec<LivenessOwner>) -> SessionLivenessSnapshot {
    SessionLivenessSnapshot {
        session_id: "integration-session".into(),
        objective_id: Some("integration-objective".into()),
        owners,
    }
}

async fn pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0027_session_liveness.sql"))
        .execute(&pool)
        .await
        .unwrap();
    pool
}

#[test]
fn m287_public_invariant_accepts_one_owner_and_rejects_zero_or_two() {
    let active = owner(LivenessOwnerKind::Model, 20_000);
    assert_eq!(
        evaluate_session_liveness(&snapshot(vec![active.clone()]), 10_000),
        LivenessEvaluation::Healthy(active)
    );
    assert_eq!(
        evaluate_session_liveness(&snapshot(vec![]), 10_000),
        LivenessEvaluation::Violation(LivenessViolation::MissingOwner)
    );
    assert_eq!(
        evaluate_session_liveness(
            &snapshot(vec![
                owner(LivenessOwnerKind::Tool, 20_000),
                owner(LivenessOwnerKind::Wake, 20_000),
            ]),
            10_000,
        ),
        LivenessEvaluation::Violation(LivenessViolation::MultipleOwners)
    );
}

#[test]
fn m287_public_wake_boundary_never_fires_before_cooldown() {
    assert_eq!(normalize_wake_at(10_000, 12_000, 9_998), 12_000);
}

#[tokio::test]
async fn m287_owner_transfer_is_atomic_and_metrics_are_read_only() {
    let pool = pool().await;
    claim_owner(
        &pool,
        "session-1",
        Some("objective-1"),
        Some("turn-1"),
        &owner(LivenessOwnerKind::Model, 20_000),
        "model_connecting",
        Some("process-1"),
        10_000,
    )
    .await
    .unwrap();
    claim_owner(
        &pool,
        "session-1",
        Some("objective-1"),
        Some("turn-1"),
        &owner(LivenessOwnerKind::Tool, 30_000),
        "tool_running",
        Some("process-1"),
        11_000,
    )
    .await
    .unwrap();
    record_event(
        &pool,
        &LivenessEventInput {
            session_id: "session-1".into(),
            objective_id: Some("objective-1".into()),
            owner_kind: Some(LivenessOwnerKind::Tool),
            violation: LivenessViolation::DeadlineExceeded,
            idle_ms: 12_000,
            deadline_at: Some(30_000),
            detected_at: 42_000,
            action: "cancel_and_resume".into(),
            outcome: "recovered".into(),
            detail_code: Some("foreground_command_timeout".into()),
        },
    )
    .await
    .unwrap();

    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_liveness_leases WHERE session_id='session-1' AND status='active'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 1);
    let report = metrics(&pool, 24, 50_000).await.unwrap();
    assert_eq!(report.total_violations, 1);
    assert_eq!(report.longest_idle_ms, 12_000);
}

#[test]
fn m287_production_wiring_declares_modules_command_and_invoke_entry() {
    let agent_mod = include_str!("../src/agent/mod.rs");
    assert!(agent_mod.contains("pub(crate) mod liveness;"));
    assert!(agent_mod.contains("pub(crate) mod liveness_store;"));

    let commands_mod = include_str!("../src/commands/mod.rs");
    assert!(commands_mod.contains("pub mod session_liveness;"));

    let lib = include_str!("../src/lib.rs");
    assert!(lib.contains("commands::session_liveness::session_liveness_metrics"));
}
