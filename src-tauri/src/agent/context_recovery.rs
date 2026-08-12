// SPDX-License-Identifier: Apache-2.0
//! Durable observation boundary for Context-domain recovery.
//!
//! A context overflow is replay-safe only when the exact opaque Objective/root
//! binding is current and the failed run produced no output or side effect.
//! The actual executor must create a fresh chat agent so `DesktopContextPolicy`
//! re-reads the current provider route and context window before compressing;
//! this module never snapshots or guesses a model limit.

use anyhow::Result;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::objective::{ClaimedRemediation, ObjectiveStatus, RecoveryDomain};

const TERMINAL_CONTEXT_CODES: [&str; 3] = [
    "CONTEXT_COMPACTION_EXHAUSTED",
    "CONTEXT_OVERFLOW_AFTER_COMPACTION",
    "CONTEXT_COMPRESSION_UNAVAILABLE",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContextRecoveryDisposition {
    /// Safe to construct a fresh chat agent. That construction is what
    /// resolves the *current* provider route/window and reruns compaction.
    ReadyToRecompact {
        session_id: String,
        root_turn_id: String,
        resume_cursor: String,
        source_attempt_id: String,
    },
    ObserveOnlyWrongDomain,
    ObserveOnlyNotWaiting,
    ObserveOnlyIdentityIncomplete,
    ObserveOnlyBindingChanged,
    ObserveOnlyOutputStarted,
    ObserveOnlySideEffectStarted,
    ObserveOnlyAttemptUnresolved,
    ObserveOnlyRecoveryBudgetExhausted,
    ObserveOnlyAuthorizationConsumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextRecoveryAuthorization {
    pub(crate) intent_id: String,
    pub(crate) objective_id: String,
    pub(crate) objective_revision: i64,
    pub(crate) source_attempt_id: String,
    pub(crate) remediation_id: String,
    pub(crate) binding_id: String,
    pub(crate) resource_generation: i64,
    pub(crate) resume_cursor: String,
    pub(crate) lease_owner: String,
    pub(crate) claim_epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContextRecoveryReservation {
    Authorized(ContextRecoveryAuthorization),
    ObserveOnly(ContextRecoveryDisposition),
}

#[derive(Clone)]
pub(crate) struct ContextRecoveryStore {
    pool: SqlitePool,
}

impl ContextRecoveryStore {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Read-only observation. No row is updated and no provider request is
    /// emitted here; the supervisor still has to verify its claim permit
    /// immediately before constructing the fresh agent.
    pub(crate) async fn observe_claimed_recovery(
        &self,
        claim: &ClaimedRemediation,
    ) -> Result<ContextRecoveryDisposition> {
        if claim.domain != RecoveryDomain::Context
            || claim.objective.domain != RecoveryDomain::Context
        {
            return Ok(ContextRecoveryDisposition::ObserveOnlyWrongDomain);
        }
        if claim.objective.status != ObjectiveStatus::WaitingSystem {
            return Ok(ContextRecoveryDisposition::ObserveOnlyNotWaiting);
        }
        if claim.objective.output_started {
            return Ok(ContextRecoveryDisposition::ObserveOnlyOutputStarted);
        }
        if claim.objective.side_effect_started {
            return Ok(ContextRecoveryDisposition::ObserveOnlySideEffectStarted);
        }

        let (
            Some(session_id),
            Some(_objective_anchor_root_turn_id),
            Some(resume_cursor),
            Some(binding_id),
            Some(resource_generation),
        ) = (
            claim.objective.session_id.as_deref(),
            claim.objective.root_turn_id.as_deref(),
            claim.objective.resume_cursor.as_deref(),
            claim.binding_id.as_deref(),
            claim.resource_generation,
        )
        else {
            return Ok(ContextRecoveryDisposition::ObserveOnlyIdentityIncomplete);
        };
        if resume_cursor.trim().is_empty() {
            return Ok(ContextRecoveryDisposition::ObserveOnlyIdentityIncomplete);
        }
        let active_root_turn_id = resume_cursor;

        let binding_matches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_bindings
             WHERE id=? AND objective_id=? AND resource_kind='chat_root_turn'
               AND resource_id=? AND resource_generation=?",
        )
        .bind(binding_id)
        .bind(&claim.objective.id)
        .bind(active_root_turn_id)
        .bind(resource_generation)
        .fetch_one(&self.pool)
        .await?;
        let projected_objective: Option<String> = sqlx::query_scalar(
            "SELECT objective_id FROM chat_turn_state
             WHERE root_turn_id=? AND session_id=?",
        )
        .bind(active_root_turn_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        if binding_matches != 1
            || projected_objective.as_deref() != Some(claim.objective.id.as_str())
        {
            return Ok(ContextRecoveryDisposition::ObserveOnlyBindingChanged);
        }

        let last_attempt = sqlx::query(
            "SELECT id, failure_code, terminal_decision, output_started, side_effect_started
             FROM objective_recovery_attempts
             WHERE objective_id=? AND root_turn_id=? AND domain='context'
             ORDER BY created_at DESC, attempt_index DESC, id DESC
             LIMIT 1",
        )
        .bind(&claim.objective.id)
        .bind(active_root_turn_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(last_attempt) = last_attempt else {
            return Ok(ContextRecoveryDisposition::ObserveOnlyAttemptUnresolved);
        };
        let failure_code: String = last_attempt.try_get("failure_code")?;
        let source_attempt_id: String = last_attempt.try_get("id")?;
        let terminal_decision: String = last_attempt.try_get("terminal_decision")?;
        let output_started = last_attempt.try_get::<i64, _>("output_started")? != 0;
        let side_effect_started = last_attempt.try_get::<i64, _>("side_effect_started")? != 0;
        if output_started {
            return Ok(ContextRecoveryDisposition::ObserveOnlyOutputStarted);
        }
        if side_effect_started {
            return Ok(ContextRecoveryDisposition::ObserveOnlySideEffectStarted);
        }
        if terminal_decision != "waiting_system"
            || !TERMINAL_CONTEXT_CODES.contains(&failure_code.as_str())
        {
            return Ok(ContextRecoveryDisposition::ObserveOnlyAttemptUnresolved);
        }
        let terminal_attempts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_recovery_attempts
             WHERE objective_id=? AND root_turn_id=? AND domain='context'
               AND terminal_decision='waiting_system'",
        )
        .bind(&claim.objective.id)
        .bind(active_root_turn_id)
        .fetch_one(&self.pool)
        .await?;
        if terminal_attempts != 1 {
            return Ok(ContextRecoveryDisposition::ObserveOnlyRecoveryBudgetExhausted);
        }

        Ok(ContextRecoveryDisposition::ReadyToRecompact {
            session_id: session_id.to_string(),
            root_turn_id: active_root_turn_id.to_string(),
            resume_cursor: resume_cursor.to_string(),
            source_attempt_id,
        })
    }

    /// Atomically consume the one recovery opportunity represented by the
    /// exact terminal Context attempt. A committed row is permanent fencing:
    /// after a crash, a replacement owner may observe it but cannot recompact
    /// or issue another provider request from the same attempt.
    pub(crate) async fn reserve_claimed_recovery(
        &self,
        claim: &ClaimedRemediation,
        permit: &codefactory_agent_loop::tool::MutationPermit,
    ) -> Result<ContextRecoveryReservation> {
        let observation = self.observe_claimed_recovery(claim).await?;
        let ContextRecoveryDisposition::ReadyToRecompact {
            resume_cursor,
            source_attempt_id,
            ..
        } = observation
        else {
            return Ok(ContextRecoveryReservation::ObserveOnly(observation));
        };
        let (Some(binding_id), Some(resource_generation)) =
            (permit.binding_id.as_deref(), permit.resource_generation)
        else {
            return Ok(ContextRecoveryReservation::ObserveOnly(
                ContextRecoveryDisposition::ObserveOnlyIdentityIncomplete,
            ));
        };
        if permit.objective_id != claim.objective.id
            || permit.remediation_id != claim.remediation_id
            || permit.claim_epoch != claim.claim_epoch
            || permit.binding_id.as_deref() != claim.binding_id.as_deref()
            || permit.resource_generation != claim.resource_generation
        {
            return Ok(ContextRecoveryReservation::ObserveOnly(
                ContextRecoveryDisposition::ObserveOnlyBindingChanged,
            ));
        }

        let now = chrono::Utc::now().timestamp_millis();
        let intent_id = Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;
        let current: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM objectives objective
             JOIN objective_remediations remediation
               ON remediation.id=objective.remediation_id
              AND remediation.objective_id=objective.id
             JOIN objective_bindings binding
               ON binding.id=remediation.binding_id
              AND binding.objective_id=objective.id
             JOIN chat_turn_state turn
               ON turn.root_turn_id=objective.resume_cursor
              AND turn.session_id=objective.session_id
              AND turn.objective_id=objective.id
             JOIN objective_recovery_attempts attempt
               ON attempt.id=? AND attempt.objective_id=objective.id
              AND attempt.root_turn_id=objective.resume_cursor
              AND attempt.domain='context'
             WHERE objective.id=? AND objective.revision=?
               AND objective.status='waiting_system' AND objective.domain='context'
               AND objective.output_started=0 AND objective.side_effect_started=0
               AND objective.resume_cursor=?
               AND objective.lease_owner=? AND objective.lease_expires_at>?
               AND remediation.id=? AND remediation.status='claimed'
               AND remediation.domain='context' AND remediation.lease_owner=?
               AND remediation.attempt_index=? AND remediation.lease_expires_at>?
               AND binding.id=? AND binding.resource_generation=?
               AND binding.resource_kind='chat_root_turn' AND binding.resource_id=?
               AND binding.output_started=0 AND binding.side_effect_started=0
               AND attempt.terminal_decision='waiting_system'
               AND attempt.output_started=0 AND attempt.side_effect_started=0
               AND attempt.failure_code IN (
                   'CONTEXT_COMPACTION_EXHAUSTED',
                   'CONTEXT_OVERFLOW_AFTER_COMPACTION',
                   'CONTEXT_COMPRESSION_UNAVAILABLE'
               )
               AND 1=(SELECT COUNT(*) FROM objective_recovery_attempts terminal
                      WHERE terminal.objective_id=objective.id
                        AND terminal.root_turn_id=objective.resume_cursor
                        AND terminal.domain='context'
                        AND terminal.terminal_decision='waiting_system')",
        )
        .bind(&source_attempt_id)
        .bind(&claim.objective.id)
        .bind(claim.objective.revision)
        .bind(&resume_cursor)
        .bind(&permit.owner)
        .bind(now)
        .bind(&permit.remediation_id)
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .bind(binding_id)
        .bind(resource_generation)
        .bind(&resume_cursor)
        .fetch_one(&mut *tx)
        .await?;
        if current != 1 {
            tx.rollback().await?;
            return Ok(ContextRecoveryReservation::ObserveOnly(
                ContextRecoveryDisposition::ObserveOnlyBindingChanged,
            ));
        }

        let inserted = sqlx::query(
            "INSERT INTO context_recovery_intents
             (id, objective_id, objective_revision, source_attempt_id,
              remediation_id, binding_id, resource_generation, resume_cursor,
              lease_owner, claim_epoch, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'started', ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(&intent_id)
        .bind(&claim.objective.id)
        .bind(claim.objective.revision)
        .bind(&source_attempt_id)
        .bind(&permit.remediation_id)
        .bind(binding_id)
        .bind(resource_generation)
        .bind(&resume_cursor)
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(ContextRecoveryReservation::ObserveOnly(
                ContextRecoveryDisposition::ObserveOnlyAuthorizationConsumed,
            ));
        }
        tx.commit().await?;
        Ok(ContextRecoveryReservation::Authorized(
            ContextRecoveryAuthorization {
                intent_id,
                objective_id: claim.objective.id.clone(),
                objective_revision: claim.objective.revision,
                source_attempt_id,
                remediation_id: permit.remediation_id.clone(),
                binding_id: binding_id.to_string(),
                resource_generation,
                resume_cursor,
                lease_owner: permit.owner.clone(),
                claim_epoch: permit.claim_epoch,
            },
        ))
    }

    /// Final pre-provider fence. Provider attempt admission performs its own
    /// owner/epoch CAS immediately afterward; this check additionally binds
    /// that request to the single durable Context authorization and cursor.
    pub(crate) async fn authorization_is_current(
        &self,
        authorization: &ContextRecoveryAuthorization,
        permit: &codefactory_agent_loop::tool::MutationPermit,
    ) -> Result<bool> {
        if permit.objective_id != authorization.objective_id
            || permit.remediation_id != authorization.remediation_id
            || permit.owner != authorization.lease_owner
            || permit.claim_epoch != authorization.claim_epoch
            || permit.binding_id.as_deref() != Some(authorization.binding_id.as_str())
            || permit.resource_generation != Some(authorization.resource_generation)
        {
            return Ok(false);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let current: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM context_recovery_intents intent
             JOIN objectives objective ON objective.id=intent.objective_id
             JOIN objective_remediations remediation
               ON remediation.id=intent.remediation_id
              AND remediation.objective_id=objective.id
             JOIN objective_bindings binding
               ON binding.id=intent.binding_id AND binding.objective_id=objective.id
             JOIN chat_turn_state turn
               ON turn.root_turn_id=intent.resume_cursor
              AND turn.session_id=objective.session_id
              AND turn.objective_id=objective.id
             WHERE intent.id=? AND intent.status='started'
               AND intent.objective_id=? AND intent.objective_revision=?
               AND intent.source_attempt_id=? AND intent.remediation_id=?
               AND intent.binding_id=? AND intent.resource_generation=?
               AND intent.resume_cursor=? AND intent.lease_owner=?
               AND intent.claim_epoch=?
               AND objective.revision=intent.objective_revision
               AND objective.status='waiting_system' AND objective.domain='context'
               AND objective.remediation_id=intent.remediation_id
               AND objective.resume_cursor=intent.resume_cursor
               AND objective.output_started=0 AND objective.side_effect_started=0
               AND objective.lease_owner=intent.lease_owner
               AND objective.lease_expires_at>?
               AND remediation.status='claimed' AND remediation.domain='context'
               AND remediation.binding_id=intent.binding_id
               AND remediation.lease_owner=intent.lease_owner
               AND remediation.attempt_index=intent.claim_epoch
               AND remediation.lease_expires_at>?
               AND binding.resource_generation=intent.resource_generation
               AND binding.resource_kind='chat_root_turn'
               AND binding.resource_id=intent.resume_cursor
               AND binding.output_started=0 AND binding.side_effect_started=0",
        )
        .bind(&authorization.intent_id)
        .bind(&authorization.objective_id)
        .bind(authorization.objective_revision)
        .bind(&authorization.source_attempt_id)
        .bind(&authorization.remediation_id)
        .bind(&authorization.binding_id)
        .bind(authorization.resource_generation)
        .bind(&authorization.resume_cursor)
        .bind(&authorization.lease_owner)
        .bind(authorization.claim_epoch)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(current == 1)
    }
}

/// Production implementation of the shared loop's final compaction gate. It
/// owns the exact durable authorization snapshot but never caches the answer:
/// every compactor rung re-reads SQLite so steer, cancellation, lease takeover,
/// binding replacement, or cursor advancement fences the stale runner.
pub(crate) struct DurableContextCompactionGate {
    store: ContextRecoveryStore,
    authorization: ContextRecoveryAuthorization,
    permit: codefactory_agent_loop::tool::MutationPermit,
}

impl DurableContextCompactionGate {
    pub(crate) fn new(
        pool: SqlitePool,
        authorization: ContextRecoveryAuthorization,
        permit: codefactory_agent_loop::tool::MutationPermit,
    ) -> Self {
        Self {
            store: ContextRecoveryStore::new(pool),
            authorization,
            permit,
        }
    }
}

#[async_trait::async_trait]
impl codefactory_agent_loop::services::ContextCompactionGate for DurableContextCompactionGate {
    async fn authorize_compaction(&self) -> std::result::Result<(), String> {
        match self
            .store
            .authorization_is_current(&self.authorization, &self.permit)
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(
                "durable objective revision, claim, binding, lease, or resume cursor changed"
                    .into(),
            ),
            Err(error) => Err(format!("durable authorization check failed: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::objective::{ObjectiveKind, ObjectiveSnapshot};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fixture(
        attempt_output_started: bool,
        attempt_side_effect_started: bool,
    ) -> (SqlitePool, ClaimedRemediation) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE objective_bindings (
               id TEXT PRIMARY KEY, objective_id TEXT NOT NULL,
               resource_kind TEXT NOT NULL, resource_id TEXT NOT NULL,
               resource_generation INTEGER NOT NULL
             )",
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               objective_id TEXT
             )",
            "CREATE TABLE objective_recovery_attempts (
               id TEXT PRIMARY KEY, objective_id TEXT, root_turn_id TEXT,
               domain TEXT NOT NULL, attempt_index INTEGER NOT NULL,
               failure_code TEXT NOT NULL, output_started INTEGER NOT NULL,
               side_effect_started INTEGER NOT NULL, terminal_decision TEXT NOT NULL,
               created_at INTEGER NOT NULL
             )",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO objective_bindings
             VALUES ('binding-context', 'objective-context', 'chat_root_turn', 'turn-context', 3)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             VALUES ('turn-context', 'session-context', 'objective-context')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_recovery_attempts
             VALUES ('attempt-context', 'objective-context', 'turn-context', 'context', 2,
                     'CONTEXT_OVERFLOW_AFTER_COMPACTION', ?, ?, 'waiting_system', 20)",
        )
        .bind(i64::from(attempt_output_started))
        .bind(i64::from(attempt_side_effect_started))
        .execute(&pool)
        .await
        .unwrap();

        let mut objective = ObjectiveSnapshot::new(
            "objective-context",
            ObjectiveKind::Informational,
            RecoveryDomain::Context,
            "answer",
        );
        objective.session_id = Some("session-context".into());
        objective.root_turn_id = Some("turn-context".into());
        objective.resume_cursor = Some("turn-context".into());
        objective.status = ObjectiveStatus::WaitingSystem;
        objective.failure_code = Some("context_overflow_after_compaction".into());
        let claim = ClaimedRemediation {
            objective,
            remediation_id: "remediation-context".into(),
            domain: RecoveryDomain::Context,
            failure_code: "context_overflow_after_compaction".into(),
            claim_epoch: 1,
            binding_id: Some("binding-context".into()),
            resource_generation: Some(3),
        };
        (pool, claim)
    }

    #[tokio::test]
    async fn exact_effect_free_context_claim_recompacts_with_current_route_identity() {
        let (pool, claim) = fixture(false, false).await;
        assert_eq!(
            ContextRecoveryStore::new(pool)
                .observe_claimed_recovery(&claim)
                .await
                .unwrap(),
            ContextRecoveryDisposition::ReadyToRecompact {
                session_id: "session-context".into(),
                root_turn_id: "turn-context".into(),
                resume_cursor: "turn-context".into(),
                source_attempt_id: "attempt-context".into(),
            }
        );
    }

    #[tokio::test]
    async fn continued_chat_recompacts_the_active_resume_cursor_not_the_objective_anchor() {
        let (pool, mut claim) = fixture(false, false).await;
        claim.objective.root_turn_id = Some("turn-anchor".into());

        assert_eq!(
            ContextRecoveryStore::new(pool)
                .observe_claimed_recovery(&claim)
                .await
                .unwrap(),
            ContextRecoveryDisposition::ReadyToRecompact {
                session_id: "session-context".into(),
                root_turn_id: "turn-context".into(),
                resume_cursor: "turn-context".into(),
                source_attempt_id: "attempt-context".into(),
            }
        );
    }

    #[tokio::test]
    async fn prior_output_or_side_effect_never_blindly_replays_context_claim() {
        let (output_pool, output_claim) = fixture(true, false).await;
        assert_eq!(
            ContextRecoveryStore::new(output_pool)
                .observe_claimed_recovery(&output_claim)
                .await
                .unwrap(),
            ContextRecoveryDisposition::ObserveOnlyOutputStarted
        );

        let (effect_pool, effect_claim) = fixture(false, true).await;
        assert_eq!(
            ContextRecoveryStore::new(effect_pool)
                .observe_claimed_recovery(&effect_claim)
                .await
                .unwrap(),
            ContextRecoveryDisposition::ObserveOnlySideEffectStarted
        );
    }

    #[tokio::test]
    async fn stale_root_binding_fails_closed_before_recompaction() {
        let (pool, mut claim) = fixture(false, false).await;
        claim.resource_generation = Some(2);
        assert_eq!(
            ContextRecoveryStore::new(pool)
                .observe_claimed_recovery(&claim)
                .await
                .unwrap(),
            ContextRecoveryDisposition::ObserveOnlyBindingChanged
        );
    }

    #[tokio::test]
    async fn a_second_terminal_context_episode_never_auto_loops() {
        let (pool, claim) = fixture(false, false).await;
        sqlx::query(
            "INSERT INTO objective_recovery_attempts
             VALUES ('attempt-context-2', 'objective-context', 'turn-context', 'context', 4,
                     'CONTEXT_COMPACTION_EXHAUSTED', 0, 0, 'waiting_system', 40)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            ContextRecoveryStore::new(pool)
                .observe_claimed_recovery(&claim)
                .await
                .unwrap(),
            ContextRecoveryDisposition::ObserveOnlyRecoveryBudgetExhausted
        );
    }
}
