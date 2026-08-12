// SPDX-License-Identifier: Apache-2.0
// Failure-first contract tests for the provider/auth recovery stores.  The
// production modules are included directly until the Objective supervisor
// integration lands; this keeps the persistence core independently testable
// without weakening the adapter registry with a temporary partial wiring.

#[path = "../src/agent/auth_recovery.rs"]
mod auth_recovery;
#[path = "../src/agent/provider_recovery.rs"]
mod provider_recovery;

use auth_recovery::{
    AuthCapabilityProbe, AuthCapabilityStatus, AuthObservationSource, AuthRecoveryDisposition,
    AuthRecoveryStore,
};
use provider_recovery::{
    OverloadBudgetDecision, ProviderAttemptSpec, ProviderEpisodeSpec, ProviderMutation,
    ProviderOwnerPermit, ProviderRecoveryDisposition, ProviderRecoveryStore,
};
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use std::sync::atomic::{AtomicUsize, Ordering};

const NOW: i64 = 1_800_000_000_000;

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/0007_unified_objective_control_plane.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0009_chat_run_controls.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/0011_provider_auth_recovery.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    pool
}

async fn insert_claimed_provider_objective(pool: &SqlitePool) -> ProviderOwnerPermit {
    sqlx::query(
        "INSERT INTO objectives
         (id, revision, kind, session_id, root_turn_id, status, decision_type,
          domain, requested_acceptance, recovery_owner, remediation_id,
          resume_cursor, lease_owner, lease_expires_at, created_at, updated_at)
         VALUES
         ('objective-opaque', 4, 'informational', 'session-1', 'turn-1',
          'waiting_system', 'waiting', 'provider', 'answer',
          'objective-supervisor:provider', 'remediation-1', 'turn-1',
          'provider-owner', ?, ?, ?)",
    )
    .bind(NOW + 60_000)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO objective_bindings
         (id, objective_id, domain, resource_kind, resource_id,
          resource_generation, identity_digest, resume_cursor, created_at, updated_at)
         VALUES
         ('binding-1', 'objective-opaque', 'provider', 'chat_root_turn', 'turn-1',
          3, 'sha256:binding', 'turn-1', ?, ?)",
    )
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO objective_remediations
         (id, objective_id, binding_id, domain, status, failure_code,
          failure_signature, strategy, attempt_index, resume_cursor,
          next_observation_at, lease_owner, lease_expires_at, created_at, updated_at)
         VALUES
         ('remediation-1', 'objective-opaque', 'binding-1', 'provider', 'claimed',
          'provider_unavailable', 'sha256:failure', 'provider_reconcile', 7,
          'turn-1', ?, 'provider-owner', ?, ?, ?)",
    )
    .bind(NOW)
    .bind(NOW + 60_000)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .unwrap();

    ProviderOwnerPermit::remediation(
        "objective-opaque",
        4,
        "binding-1",
        3,
        "remediation-1",
        "provider-owner",
        7,
    )
}

fn episode() -> ProviderEpisodeSpec {
    ProviderEpisodeSpec {
        id: "episode-1".into(),
        session_id: "session-1".into(),
        root_turn_id: "turn-1".into(),
        policy: "prefer".into(),
        candidate_snapshot_digest:
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        candidate_snapshot_json:
            r#"[{"endpoint":"chatgpt","model":"gpt-5.6-sol"},{"endpoint":"deepseek","model":"deepseek-chat"}]"#.into(),
        resume_cursor: "turn-1".into(),
    }
}

fn attempt(id: &str, episode_id: &str, endpoint: &str) -> ProviderAttemptSpec {
    ProviderAttemptSpec {
        id: id.into(),
        episode_id: episode_id.into(),
        endpoint: endpoint.into(),
        model: format!("{endpoint}-model"),
        request_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .into(),
        resume_cursor: "turn-1".into(),
    }
}

#[tokio::test]
async fn attempt_is_write_ahead_and_bound_to_opaque_objective_revision() {
    let pool = pool().await;
    let permit = insert_claimed_provider_objective(&pool).await;
    let store = ProviderRecoveryStore::new(pool.clone());

    let opened = store.open_episode(&permit, &episode(), NOW).await.unwrap();
    assert!(matches!(opened, ProviderMutation::Applied(_)));
    let admitted = store
        .begin_attempt(
            &permit,
            &attempt("attempt-1", "episode-1", "chatgpt"),
            NOW + 1,
        )
        .await
        .unwrap();
    let ProviderMutation::Applied(admitted) = admitted else {
        panic!("live owner must admit the write-ahead attempt")
    };

    let row = sqlx::query(
        "SELECT objective_id, objective_revision, binding_id, resource_generation,
                episode_id, attempt_order, status
         FROM provider_route_attempts WHERE id='attempt-1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("objective_id"), "objective-opaque");
    assert_eq!(row.get::<i64, _>("objective_revision"), 4);
    assert_eq!(row.get::<String, _>("binding_id"), "binding-1");
    assert_eq!(row.get::<i64, _>("resource_generation"), 3);
    assert_eq!(row.get::<String, _>("episode_id"), "episode-1");
    assert_eq!(row.get::<i64, _>("attempt_order"), 1);
    assert_eq!(row.get::<String, _>("status"), "prepared");
    assert_eq!(admitted.id, "attempt-1");
}

#[tokio::test]
async fn latches_are_per_attempt_not_copied_from_the_whole_run() {
    let pool = pool().await;
    let permit = insert_claimed_provider_objective(&pool).await;
    let store = ProviderRecoveryStore::new(pool.clone());
    store.open_episode(&permit, &episode(), NOW).await.unwrap();

    store
        .begin_attempt(&permit, &attempt("attempt-a", "episode-1", "a"), NOW + 1)
        .await
        .unwrap();
    store
        .mark_in_flight(&permit, "attempt-a", NOW + 2)
        .await
        .unwrap();
    store
        .record_failure(
            &permit,
            "attempt-a",
            "endpoint_unavailable",
            "ENDPOINT_UNAVAILABLE",
            true,
            NOW + 3,
        )
        .await
        .unwrap();
    store
        .begin_attempt(&permit, &attempt("attempt-b", "episode-1", "b"), NOW + 4)
        .await
        .unwrap();
    store
        .mark_in_flight(&permit, "attempt-b", NOW + 5)
        .await
        .unwrap();
    store
        .append_partial_output(&permit, "attempt-b", "visible", NOW + 6)
        .await
        .unwrap();
    store
        .begin_side_effect(
            &permit,
            "attempt-b",
            "tool:write-visible-result",
            "provider-attempt-b:tool-1",
            NOW + 7,
        )
        .await
        .unwrap();

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT id, output_started, side_effect_started
         FROM provider_route_attempts ORDER BY attempt_order",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![("attempt-a".into(), 0, 0), ("attempt-b".into(), 1, 1)]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id='objective-opaque' AND status='started'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    assert!(matches!(
        store.observe("objective-opaque").await.unwrap(),
        ProviderRecoveryDisposition::ObserveOnlySideEffect { .. }
    ));
}

#[tokio::test]
async fn partial_and_unknown_attempts_are_observe_only() {
    let pool = pool().await;
    let permit = insert_claimed_provider_objective(&pool).await;
    let store = ProviderRecoveryStore::new(pool.clone());
    store.open_episode(&permit, &episode(), NOW).await.unwrap();
    store
        .begin_attempt(
            &permit,
            &attempt("attempt-partial", "episode-1", "a"),
            NOW + 1,
        )
        .await
        .unwrap();
    store
        .mark_in_flight(&permit, "attempt-partial", NOW + 2)
        .await
        .unwrap();
    store
        .append_partial_output(&permit, "attempt-partial", "first ", NOW + 3)
        .await
        .unwrap();
    store
        .append_partial_output(&permit, "attempt-partial", "chunk", NOW + 4)
        .await
        .unwrap();

    assert_eq!(
        store.observe("objective-opaque").await.unwrap(),
        ProviderRecoveryDisposition::ObserveOnlyPartial {
            episode_id: "episode-1".into(),
            attempt_id: "attempt-partial".into(),
            checkpoint_content: "first chunk".into(),
        }
    );
    assert!(store
        .begin_attempt(
            &permit,
            &attempt("unsafe-replay", "episode-1", "b"),
            NOW + 5
        )
        .await
        .is_err());

    store
        .mark_unknown(&permit, "attempt-partial", "stream_interrupted", NOW + 6)
        .await
        .unwrap();
    assert_eq!(
        store.observe("objective-opaque").await.unwrap(),
        ProviderRecoveryDisposition::ObserveOnlyUnknown {
            episode_id: "episode-1".into(),
            attempt_id: "attempt-partial".into(),
        }
    );
}

#[tokio::test]
async fn stale_epoch_cannot_post_emit_or_commit() {
    let pool = pool().await;
    let permit = insert_claimed_provider_objective(&pool).await;
    let store = ProviderRecoveryStore::new(pool.clone());
    store.open_episode(&permit, &episode(), NOW).await.unwrap();
    store
        .begin_attempt(
            &permit,
            &attempt("attempt-stale", "episode-1", "a"),
            NOW + 1,
        )
        .await
        .unwrap();

    sqlx::query("UPDATE objective_remediations SET attempt_index=8 WHERE id='remediation-1'")
        .execute(&pool)
        .await
        .unwrap();

    let post_count = AtomicUsize::new(0);
    if matches!(
        store
            .mark_in_flight(&permit, "attempt-stale", NOW + 2)
            .await
            .unwrap(),
        ProviderMutation::Applied(_)
    ) {
        post_count.fetch_add(1, Ordering::SeqCst);
    }
    let emit_count = AtomicUsize::new(0);
    if matches!(
        store
            .append_partial_output(&permit, "attempt-stale", "must-not-emit", NOW + 3)
            .await
            .unwrap(),
        ProviderMutation::Applied(_)
    ) {
        emit_count.fetch_add(1, Ordering::SeqCst);
    }
    let commit = store
        .commit_response(
            &permit,
            "attempt-stale",
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "must-not-commit",
            false,
            NOW + 4,
        )
        .await
        .unwrap();

    assert_eq!(post_count.load(Ordering::SeqCst), 0);
    assert_eq!(emit_count.load(Ordering::SeqCst), 0);
    assert!(matches!(commit, ProviderMutation::Fenced));
    let row: (String, i64) = sqlx::query_as(
        "SELECT status, output_started FROM provider_route_attempts WHERE id='attempt-stale'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("prepared".into(), 0));
}

#[tokio::test]
async fn takeover_cannot_adopt_an_unresolved_attempt_from_the_prior_epoch() {
    let pool = pool().await;
    let old_permit = insert_claimed_provider_objective(&pool).await;
    let store = ProviderRecoveryStore::new(pool.clone());
    store
        .open_episode(&old_permit, &episode(), NOW)
        .await
        .unwrap();
    store
        .begin_attempt(
            &old_permit,
            &attempt("attempt-old-owner", "episode-1", "a"),
            NOW + 1,
        )
        .await
        .unwrap();
    store
        .mark_in_flight(&old_permit, "attempt-old-owner", NOW + 2)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE objectives SET lease_owner='provider-owner-2', lease_expires_at=?
         WHERE id='objective-opaque'",
    )
    .bind(NOW + 120_000)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE objective_remediations
         SET lease_owner='provider-owner-2', lease_expires_at=?, attempt_index=8
         WHERE id='remediation-1'",
    )
    .bind(NOW + 120_000)
    .execute(&pool)
    .await
    .unwrap();
    let new_permit = ProviderOwnerPermit::remediation(
        "objective-opaque",
        4,
        "binding-1",
        3,
        "remediation-1",
        "provider-owner-2",
        8,
    );

    assert!(matches!(
        store
            .append_partial_output(&new_permit, "attempt-old-owner", "must-not-adopt", NOW + 3,)
            .await
            .unwrap(),
        ProviderMutation::Fenced
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM provider_output_checkpoints
             WHERE attempt_id='attempt-old-owner'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn takeover_can_adopt_only_a_prepared_zero_latch_attempt() {
    let pool = pool().await;
    let old_permit = insert_claimed_provider_objective(&pool).await;
    let store = ProviderRecoveryStore::new(pool.clone());
    store
        .open_episode(&old_permit, &episode(), NOW)
        .await
        .unwrap();
    store
        .begin_attempt(
            &old_permit,
            &attempt("attempt-prepared", "episode-1", "a"),
            NOW + 1,
        )
        .await
        .unwrap();

    sqlx::query(
        "UPDATE objectives SET lease_owner='provider-owner-2', lease_expires_at=?
         WHERE id='objective-opaque'",
    )
    .bind(NOW + 120_000)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE objective_remediations
         SET lease_owner='provider-owner-2', lease_expires_at=?, attempt_index=8
         WHERE id='remediation-1'",
    )
    .bind(NOW + 120_000)
    .execute(&pool)
    .await
    .unwrap();
    let new_permit = ProviderOwnerPermit::remediation(
        "objective-opaque",
        4,
        "binding-1",
        3,
        "remediation-1",
        "provider-owner-2",
        8,
    );

    assert!(matches!(
        store
            .mark_in_flight(&old_permit, "attempt-prepared", NOW + 2)
            .await
            .unwrap(),
        ProviderMutation::Fenced
    ));
    let ProviderMutation::Applied(adopted) = store
        .adopt_prepared_attempt(&new_permit, "attempt-prepared", NOW + 3)
        .await
        .unwrap()
    else {
        panic!("current owner must adopt a proven zero-latch prepared attempt")
    };
    assert_eq!(adopted.status, "prepared");
    assert_eq!(adopted.owner_epoch, 8);
    assert!(matches!(
        store
            .mark_in_flight(&new_permit, "attempt-prepared", NOW + 4)
            .await
            .unwrap(),
        ProviderMutation::Applied(_)
    ));
}

#[tokio::test]
async fn idempotent_ids_cannot_be_rebound_to_changed_route_payloads() {
    let pool = pool().await;
    let permit = insert_claimed_provider_objective(&pool).await;
    let store = ProviderRecoveryStore::new(pool.clone());
    store.open_episode(&permit, &episode(), NOW).await.unwrap();

    let mut changed_episode = episode();
    changed_episode.candidate_snapshot_digest =
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into();
    assert!(store
        .open_episode(&permit, &changed_episode, NOW + 1)
        .await
        .is_err());

    store
        .begin_attempt(
            &permit,
            &attempt("attempt-stable-id", "episode-1", "a"),
            NOW + 2,
        )
        .await
        .unwrap();
    let changed_attempt = attempt("attempt-stable-id", "episode-1", "b");
    assert!(store
        .begin_attempt(&permit, &changed_attempt, NOW + 3)
        .await
        .is_err());
    assert!(!matches!(
        store.observe("objective-opaque").await.unwrap(),
        ProviderRecoveryDisposition::ReadyToAttempt { .. }
    ));
}

#[test]
fn overload_budget_yields_durable_waiting_instead_of_an_infinite_loop() {
    assert!(matches!(
        provider_recovery::overload_budget_decision(1, NOW),
        OverloadBudgetDecision::RetryAfter { .. }
    ));
    assert!(matches!(
        provider_recovery::overload_budget_decision(2, NOW),
        OverloadBudgetDecision::RetryAfter { .. }
    ));
    assert_eq!(
        provider_recovery::overload_budget_decision(3, NOW),
        OverloadBudgetDecision::DurableWaiting {
            next_observation_at: NOW + 60_000,
        }
    );
}

#[tokio::test]
async fn durable_overload_wait_reopens_only_after_its_observation_deadline() {
    let pool = pool().await;
    let permit = insert_claimed_provider_objective(&pool).await;
    let store = ProviderRecoveryStore::new(pool);
    store.open_episode(&permit, &episode(), NOW).await.unwrap();

    let mut deadline = None;
    for ordinal in 1..=3 {
        let attempt_id = format!("attempt-overload-{ordinal}");
        store
            .begin_attempt(
                &permit,
                &attempt(&attempt_id, "episode-1", "overloaded-provider"),
                NOW + ordinal * 3,
            )
            .await
            .unwrap();
        store
            .mark_in_flight(&permit, &attempt_id, NOW + ordinal * 3 + 1)
            .await
            .unwrap();
        let ProviderMutation::Applied(decision) = store
            .record_failure(
                &permit,
                &attempt_id,
                "provider_overload",
                "provider_overloaded",
                true,
                NOW + ordinal * 3 + 2,
            )
            .await
            .unwrap()
        else {
            panic!("live owner must settle its overload attempt")
        };
        if let OverloadBudgetDecision::DurableWaiting {
            next_observation_at,
        } = decision
        {
            deadline = Some(next_observation_at);
        }
    }
    let deadline = deadline.expect("third overload must persist a deadline");

    assert_eq!(
        store
            .observe_at("objective-opaque", deadline - 1)
            .await
            .unwrap(),
        ProviderRecoveryDisposition::DurableWaiting {
            episode_id: "episode-1".into(),
            next_observation_at: deadline,
        }
    );
    assert_eq!(
        store
            .observe_at("objective-opaque", deadline)
            .await
            .unwrap(),
        ProviderRecoveryDisposition::RetrySafe {
            episode_id: "episode-1".into(),
            attempt_id: "attempt-overload-3".into(),
        },
        "a due, replay-safe overload must re-enter the executable supervisor path"
    );
}

#[tokio::test]
async fn startup_keychain_ready_observation_is_receipted_without_secrets() {
    let pool = pool().await;
    sqlx::query(
        "INSERT INTO objectives
         (id, revision, kind, session_id, root_turn_id, status, decision_type,
          domain, requested_acceptance, requires_user_action, request_key,
          action_signature, resume_cursor, created_at, updated_at)
         VALUES
         ('objective-auth', 2, 'informational', 'session-auth', 'turn-auth',
          'waiting_authorization', 'authorization_required', 'auth', 'answer', 1,
          'chatgpt-auth:objective-auth', 'oauth:chatgpt:resume:objective-auth',
          'turn-auth', ?, ?)",
    )
    .bind(NOW)
    .bind(NOW)
    .execute(&pool)
    .await
    .unwrap();
    let store = AuthRecoveryStore::new(pool.clone());
    let raw_account_identity = b"acct-sensitive-value";
    let receipt = store
        .record_probe(
            "objective-auth",
            2,
            "chatgpt-auth:objective-auth",
            "chatgpt",
            "codefactory.oauth.chatgpt",
            AuthObservationSource::Startup,
            AuthCapabilityProbe::Ready {
                identity_material: raw_account_identity,
            },
            NOW + 1,
        )
        .await
        .unwrap();

    assert_eq!(receipt.status, AuthCapabilityStatus::Ready);
    assert_eq!(
        store.observe("objective-auth").await.unwrap(),
        AuthRecoveryDisposition::QueueProvider {
            request_key: "chatgpt-auth:objective-auth".into(),
            resume_cursor: "turn-auth".into(),
            capability_digest: receipt.capability_digest.clone(),
        }
    );
    let serialized: String = sqlx::query_scalar(
        "SELECT group_concat(
             id || objective_id || request_key || provider || credential_ref ||
             capability_digest || status || source, '|')
         FROM auth_capability_receipts",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!serialized.contains("acct-sensitive-value"));
    assert!(!serialized.contains("access_token"));
    assert!(!serialized.contains("refresh_token"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM auth_capability_receipts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}
