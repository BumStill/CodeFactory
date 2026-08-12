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
    },
    ObserveOnlyWrongDomain,
    ObserveOnlyNotWaiting,
    ObserveOnlyIdentityIncomplete,
    ObserveOnlyBindingChanged,
    ObserveOnlyOutputStarted,
    ObserveOnlySideEffectStarted,
    ObserveOnlyAttemptUnresolved,
    ObserveOnlyRecoveryBudgetExhausted,
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
            Some(root_turn_id),
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
        if resume_cursor != root_turn_id {
            return Ok(ContextRecoveryDisposition::ObserveOnlyIdentityIncomplete);
        }

        let binding_matches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_bindings
             WHERE id=? AND objective_id=? AND resource_kind='chat_root_turn'
               AND resource_id=? AND resource_generation=?",
        )
        .bind(binding_id)
        .bind(&claim.objective.id)
        .bind(root_turn_id)
        .bind(resource_generation)
        .fetch_one(&self.pool)
        .await?;
        let projected_objective: Option<String> = sqlx::query_scalar(
            "SELECT objective_id FROM chat_turn_state
             WHERE root_turn_id=? AND session_id=?",
        )
        .bind(root_turn_id)
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
            "SELECT failure_code, terminal_decision, output_started, side_effect_started
             FROM objective_recovery_attempts
             WHERE objective_id=? AND root_turn_id=? AND domain='context'
             ORDER BY created_at DESC, attempt_index DESC, id DESC
             LIMIT 1",
        )
        .bind(&claim.objective.id)
        .bind(root_turn_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(last_attempt) = last_attempt else {
            return Ok(ContextRecoveryDisposition::ObserveOnlyAttemptUnresolved);
        };
        let failure_code: String = last_attempt.try_get("failure_code")?;
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
        .bind(root_turn_id)
        .fetch_one(&self.pool)
        .await?;
        if terminal_attempts != 1 {
            return Ok(ContextRecoveryDisposition::ObserveOnlyRecoveryBudgetExhausted);
        }

        Ok(ContextRecoveryDisposition::ReadyToRecompact {
            session_id: session_id.to_string(),
            root_turn_id: root_turn_id.to_string(),
            resume_cursor: resume_cursor.to_string(),
        })
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
