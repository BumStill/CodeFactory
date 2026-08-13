// SPDX-License-Identifier: Apache-2.0
//! Provider routing recovery persistence and fencing primitives.
//!
//! The runtime adapter must call `begin_attempt` before any provider request,
//! `mark_in_flight` immediately before POST, and re-check the returned mutation
//! permit before emitting or committing output.  A partial or unknown attempt
//! is intentionally observation-only; this module never claims that arbitrary
//! streaming output can be resumed safely.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderMutation<T> {
    Applied(T),
    Fenced,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOwnerPermit {
    objective_id: String,
    objective_revision: i64,
    binding_id: String,
    resource_generation: i64,
    owner_kind: String,
    owner_id: String,
    remediation_id: Option<String>,
    owner_epoch: i64,
}

impl ProviderOwnerPermit {
    #[allow(clippy::too_many_arguments)]
    pub fn remediation(
        objective_id: impl Into<String>,
        objective_revision: i64,
        binding_id: impl Into<String>,
        resource_generation: i64,
        remediation_id: impl Into<String>,
        lease_owner: impl Into<String>,
        owner_epoch: i64,
    ) -> Self {
        Self {
            objective_id: objective_id.into(),
            objective_revision,
            binding_id: binding_id.into(),
            resource_generation,
            owner_kind: "remediation".into(),
            owner_id: lease_owner.into(),
            remediation_id: Some(remediation_id.into()),
            owner_epoch,
        }
    }

    /// Foreground model work is fenced by the exact durable chat-run receipt.
    /// The Objective revision is its epoch: a settlement/recovery decision
    /// invalidates this permit before a later process can issue another POST.
    #[allow(clippy::too_many_arguments)]
    pub fn chat_run(
        objective_id: impl Into<String>,
        objective_revision: i64,
        binding_id: impl Into<String>,
        resource_generation: i64,
        run_instance_id: impl Into<String>,
        owner_epoch: i64,
    ) -> Self {
        Self {
            objective_id: objective_id.into(),
            objective_revision,
            binding_id: binding_id.into(),
            resource_generation,
            owner_kind: "chat_run".into(),
            owner_id: run_instance_id.into(),
            remediation_id: None,
            owner_epoch,
        }
    }

    fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("objective_id", self.objective_id.as_str()),
            ("binding_id", self.binding_id.as_str()),
            ("owner_id", self.owner_id.as_str()),
        ] {
            validate_identifier(label, value)?;
        }
        match (self.owner_kind.as_str(), self.remediation_id.as_deref()) {
            ("remediation", Some(remediation_id)) => {
                validate_identifier("remediation_id", remediation_id)?;
            }
            ("chat_run", None) => {}
            _ => bail!("provider owner permit kind/identity mismatch"),
        }
        if self.objective_revision < 1 || self.resource_generation < 1 || self.owner_epoch < 1 {
            bail!("provider owner permit has a non-positive revision, generation, or epoch");
        }
        Ok(())
    }

    pub fn objective_id(&self) -> &str {
        &self.objective_id
    }

    pub fn objective_revision(&self) -> i64 {
        self.objective_revision
    }

    pub fn binding_id(&self) -> &str {
        &self.binding_id
    }

    pub fn resource_generation(&self) -> i64 {
        self.resource_generation
    }

    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    pub fn owner_epoch(&self) -> i64 {
        self.owner_epoch
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderEpisodeSpec {
    pub id: String,
    pub session_id: String,
    pub root_turn_id: String,
    pub policy: String,
    pub candidate_snapshot_digest: String,
    pub candidate_snapshot_json: String,
    pub resume_cursor: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAttemptSpec {
    pub id: String,
    pub episode_id: String,
    pub endpoint: String,
    pub model: String,
    pub request_digest: String,
    pub resume_cursor: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderEpisodeSnapshot {
    pub id: String,
    pub objective_id: String,
    pub objective_revision: i64,
    pub binding_id: String,
    pub resource_generation: i64,
    pub session_id: String,
    pub root_turn_id: String,
    pub policy: String,
    pub candidate_snapshot_digest: String,
    pub candidate_snapshot_json: String,
    pub resume_cursor: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAttemptSnapshot {
    pub id: String,
    pub episode_id: String,
    pub objective_id: String,
    pub objective_revision: i64,
    pub binding_id: String,
    pub resource_generation: i64,
    pub attempt_order: i64,
    pub endpoint: String,
    pub model: String,
    pub request_digest: String,
    pub resume_cursor: String,
    pub status: String,
    pub output_started: bool,
    pub side_effect_started: bool,
    pub side_effect_receipt_id: Option<String>,
    pub owner_kind: String,
    pub owner_id: String,
    pub owner_epoch: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOutputCheckpoint {
    pub attempt_id: String,
    pub state: String,
    pub content: String,
    pub content_digest: String,
    pub chunk_count: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OverloadBudgetDecision {
    RetryAfter { retry_at: i64 },
    DurableWaiting { next_observation_at: i64 },
}

/// A strict three-attempt budget.  The first two failures retain bounded
/// in-process backoff; the third is persisted as waiting and returned to the
/// supervisor instead of looping forever.
pub fn overload_budget_decision(
    completed_overload_attempts: i64,
    now: i64,
) -> OverloadBudgetDecision {
    match completed_overload_attempts {
        i64::MIN..=1 => OverloadBudgetDecision::RetryAfter {
            retry_at: now.saturating_add(20_000),
        },
        2 => OverloadBudgetDecision::RetryAfter {
            retry_at: now.saturating_add(40_000),
        },
        _ => OverloadBudgetDecision::DurableWaiting {
            next_observation_at: now.saturating_add(60_000),
        },
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderRecoveryDisposition {
    NoEpisode,
    ReadyToAttempt {
        episode_id: String,
    },
    ObserveOnlyPrepared {
        episode_id: String,
        attempt_id: String,
    },
    RetrySafe {
        episode_id: String,
        attempt_id: String,
    },
    ObserveOnlyInFlight {
        episode_id: String,
        attempt_id: String,
    },
    ObserveOnlyPartial {
        episode_id: String,
        attempt_id: String,
        checkpoint_content: String,
    },
    ObserveOnlyUnknown {
        episode_id: String,
        attempt_id: String,
    },
    ObserveOnlySideEffect {
        episode_id: String,
        attempt_id: String,
    },
    ResponseCheckpoint {
        episode_id: String,
        attempt_id: String,
        checkpoint_content: String,
    },
    DurableWaiting {
        episode_id: String,
        next_observation_at: i64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderStartupRecovery {
    pub objective_id: String,
    pub objective_revision: i64,
    pub root_turn_id: String,
    pub failure_code: String,
    pub next_observation_at: i64,
}

#[derive(Clone)]
pub struct ProviderRecoveryStore {
    pool: SqlitePool,
}

impl ProviderRecoveryStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn open_episode(
        &self,
        permit: &ProviderOwnerPermit,
        spec: &ProviderEpisodeSpec,
        now: i64,
    ) -> Result<ProviderMutation<ProviderEpisodeSnapshot>> {
        permit.validate()?;
        validate_episode_spec(spec)?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }

        let objective = sqlx::query(
            "SELECT session_id,
                    COALESCE(NULLIF(resume_cursor, ''), root_turn_id) AS active_root_turn_id
             FROM objectives
             WHERE id=? AND revision=?",
        )
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .fetch_one(&mut *tx)
        .await?;
        let session_id: Option<String> = objective.get("session_id");
        let root_turn_id: Option<String> = objective.get("active_root_turn_id");
        if session_id.as_deref() != Some(spec.session_id.as_str())
            || root_turn_id.as_deref() != Some(spec.root_turn_id.as_str())
        {
            bail!("provider episode session/root identity does not match its objective");
        }

        if let Some(existing) = load_episode(&mut tx, &spec.id).await? {
            ensure_existing_episode_matches(&existing, permit, spec)?;
            tx.commit().await?;
            return Ok(ProviderMutation::Applied(existing));
        }

        // A new Objective revision may supersede an older live episode only
        // when durable evidence proves that its last request is replay-safe.
        // Partial/in-flight/unknown/side-effect state stays observation-only and
        // keeps the unique live-binding fence closed.
        if let Some(prior) = sqlx::query(
            "SELECT e.id, e.output_started, e.side_effect_started,
                    b.output_started AS binding_output_started,
                    b.side_effect_started AS binding_side_effect_started,
                    o.output_started AS objective_output_started,
                    o.side_effect_started AS objective_side_effect_started,
                    (SELECT a.status FROM provider_route_attempts a
                     WHERE a.episode_id=e.id ORDER BY a.attempt_order DESC LIMIT 1)
                       AS attempt_status,
                    COALESCE((SELECT a.output_started FROM provider_route_attempts a
                     WHERE a.episode_id=e.id ORDER BY a.attempt_order DESC LIMIT 1), 0)
                       AS attempt_output_started,
                    COALESCE((SELECT a.side_effect_started FROM provider_route_attempts a
                     WHERE a.episode_id=e.id ORDER BY a.attempt_order DESC LIMIT 1), 0)
                       AS attempt_side_effect_started,
                    (SELECT COUNT(*) FROM side_effect_receipts receipt
                     WHERE receipt.objective_id=e.objective_id
                       AND receipt.status IN ('started', 'unknown'))
                       AS unresolved_receipt_count,
                    (SELECT COUNT(*) FROM side_effect_receipts receipt
                     WHERE receipt.objective_id=e.objective_id
                       AND receipt.status IN ('committed', 'reconciled', 'cancelled'))
                       AS settled_receipt_count
             FROM provider_route_episodes e
             JOIN objective_bindings b ON b.id=e.binding_id AND b.objective_id=e.objective_id
             JOIN objectives o ON o.id=e.objective_id
             WHERE e.objective_id=? AND e.binding_id=? AND e.resource_generation=?
               AND e.status IN ('active', 'waiting', 'unknown')
             ORDER BY e.updated_at DESC LIMIT 1",
        )
        .bind(&permit.objective_id)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .fetch_optional(&mut *tx)
        .await?
        {
            let prior_id: String = prior.get("id");
            let attempt_status: Option<String> = prior.try_get("attempt_status")?;
            let historical_side_effect = prior.get::<i64, _>("binding_side_effect_started") != 0
                || prior.get::<i64, _>("objective_side_effect_started") != 0;
            let settled_receipts_prove_history = !historical_side_effect
                || prior.get::<i64, _>("settled_receipt_count") > 0;
            // Episode/binding/objective output latches are intentionally
            // monotonic across model rounds. Recovery safety is therefore
            // decided by the latest request plus exact side-effect receipts,
            // not by historical output that is already in chat history.
            let replay_safe = prior.get::<i64, _>("side_effect_started") == 0
                && prior.get::<i64, _>("unresolved_receipt_count") == 0
                && settled_receipts_prove_history
                && prior.get::<i64, _>("attempt_output_started") == 0
                && prior.get::<i64, _>("attempt_side_effect_started") == 0
                && matches!(
                    attempt_status.as_deref(),
                    None | Some("prepared" | "failed_replayable")
                );
            if !replay_safe {
                bail!(
                    "prior provider episode is not proven replay-safe; observe/reconcile before a new Objective revision"
                );
            }
            sqlx::query(
                "UPDATE provider_route_attempts SET status='cancelled', completed_at=COALESCE(completed_at, ?)
                 WHERE episode_id=? AND status='prepared'",
            )
            .bind(now)
            .bind(&prior_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE provider_route_episodes SET status='completed', completed_at=?, updated_at=?
                 WHERE id=? AND status IN ('active', 'waiting')",
            )
            .bind(now)
            .bind(now)
            .bind(&prior_id)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT INTO provider_route_episodes
             (id, objective_id, admission_revision, last_objective_revision,
              binding_id, resource_generation, session_id, root_turn_id, policy,
              candidate_snapshot_digest, candidate_snapshot_json, status,
              resume_cursor, owner_kind, owner_id, owner_epoch, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?,
                     ?, ?, ?, ?, ?)",
        )
        .bind(&spec.id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(permit.objective_revision)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .bind(&spec.session_id)
        .bind(&spec.root_turn_id)
        .bind(&spec.policy)
        .bind(&spec.candidate_snapshot_digest)
        .bind(&spec.candidate_snapshot_json)
        .bind(&spec.resume_cursor)
        .bind(&permit.owner_kind)
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let snapshot = load_episode(&mut tx, &spec.id)
            .await?
            .context("inserted provider episode disappeared")?;
        tx.commit().await?;
        Ok(ProviderMutation::Applied(snapshot))
    }

    /// Persist a prepared attempt before a caller can issue any network
    /// mutation. Re-entry with the same id is idempotent; a different attempt
    /// is rejected while earlier output or external state is unresolved.
    pub async fn begin_attempt(
        &self,
        permit: &ProviderOwnerPermit,
        spec: &ProviderAttemptSpec,
        now: i64,
    ) -> Result<ProviderMutation<ProviderAttemptSnapshot>> {
        permit.validate()?;
        validate_attempt_spec(spec)?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        ensure_episode_current(&mut tx, permit, &spec.episode_id).await?;

        if let Some(existing) = load_attempt(&mut tx, &spec.id).await? {
            ensure_existing_attempt_matches(&existing, permit, spec)?;
            if !attempt_owner_is_current(&existing, permit) {
                return Ok(ProviderMutation::Fenced);
            }
            tx.commit().await?;
            return Ok(ProviderMutation::Applied(existing));
        }

        if let Some(row) = sqlx::query(
            "SELECT status, output_started, side_effect_started,
                    owner_kind, owner_id, owner_epoch
             FROM provider_route_attempts WHERE episode_id=?
             ORDER BY attempt_order DESC LIMIT 1",
        )
        .bind(&spec.episode_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let status: String = row.get("status");
            let output_started: i64 = row.get("output_started");
            let side_effect_started: i64 = row.get("side_effect_started");
            let may_retry =
                status == "failed_replayable" && output_started == 0 && side_effect_started == 0;
            let may_continue_same_owner = status == "response_committed"
                && output_started != 0
                && side_effect_started == 0
                && row.get::<String, _>("owner_kind") == permit.owner_kind
                && row.get::<String, _>("owner_id") == permit.owner_id
                && row.get::<i64, _>("owner_epoch") == permit.owner_epoch;
            if !may_retry && !may_continue_same_owner {
                bail!(
                    "previous provider attempt {status:?} cannot continue under this live owner; observe/reconcile first"
                );
            }
        }

        let attempt_order: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(attempt_order), 0) + 1
             FROM provider_route_attempts WHERE episode_id=?",
        )
        .bind(&spec.episode_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO provider_route_attempts
             (id, episode_id, objective_id, objective_revision, binding_id,
              resource_generation, attempt_order, endpoint, model, request_digest,
              resume_cursor, status, owner_kind, owner_id, owner_epoch, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'prepared',
                     ?, ?, ?, ?)",
        )
        .bind(&spec.id)
        .bind(&spec.episode_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .bind(attempt_order)
        .bind(&spec.endpoint)
        .bind(&spec.model)
        .bind(&spec.request_digest)
        .bind(&spec.resume_cursor)
        .bind(&permit.owner_kind)
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let snapshot = load_attempt(&mut tx, &spec.id)
            .await?
            .context("inserted provider attempt disappeared")?;
        tx.commit().await?;
        Ok(ProviderMutation::Applied(snapshot))
    }

    /// Transfer a write-ahead attempt to the current owner only while the
    /// durable record proves that POST was never admitted.  Once an attempt is
    /// in-flight, streaming, unknown, or side-effect-latched, takeover must use
    /// observation/reconciliation instead.
    pub async fn adopt_prepared_attempt(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        now: i64,
    ) -> Result<ProviderMutation<ProviderAttemptSnapshot>> {
        permit.validate()?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        let attempt = load_attempt(&mut tx, attempt_id)
            .await?
            .with_context(|| format!("provider attempt {attempt_id:?} does not exist"))?;
        ensure_attempt_identity(&attempt, permit)?;
        if attempt.status != "prepared"
            || attempt.output_started
            || attempt.side_effect_started
            || attempt.side_effect_receipt_id.is_some()
        {
            bail!(
                "only a prepared zero-latch provider attempt can transfer ownership; got {:?}",
                attempt.status
            );
        }
        if attempt_owner_is_current(&attempt, permit) {
            tx.commit().await?;
            return Ok(ProviderMutation::Applied(attempt));
        }

        let changed = sqlx::query(
            "UPDATE provider_route_attempts
             SET owner_id=?, owner_epoch=?, observed_at=?
             WHERE id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND owner_kind=? AND status='prepared'
               AND output_started=0 AND side_effect_started=0
               AND side_effect_receipt_id IS NULL",
        )
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .bind(now)
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .bind(&permit.owner_kind)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Ok(ProviderMutation::Fenced);
        }
        sqlx::query(
            "UPDATE provider_route_episodes
             SET owner_id=?, owner_epoch=?, last_objective_revision=?, updated_at=?
             WHERE id=? AND objective_id=? AND binding_id=?
               AND resource_generation=? AND status='active'",
        )
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .bind(permit.objective_revision)
        .bind(now)
        .bind(&attempt.episode_id)
        .bind(&permit.objective_id)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .execute(&mut *tx)
        .await?;
        let adopted = load_attempt(&mut tx, attempt_id)
            .await?
            .context("adopted provider attempt disappeared")?;
        tx.commit().await?;
        Ok(ProviderMutation::Applied(adopted))
    }

    /// The returned `Applied` value is the mutation-rung permit for POST.
    pub async fn mark_in_flight(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        now: i64,
    ) -> Result<ProviderMutation<ProviderAttemptSnapshot>> {
        self.transition_attempt(permit, attempt_id, "prepared", "in_flight", now)
            .await
    }

    pub async fn append_partial_output(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        visible_chunk: &str,
        now: i64,
    ) -> Result<ProviderMutation<ProviderOutputCheckpoint>> {
        if visible_chunk.is_empty() {
            bail!("empty provider output chunks are not checkpoints");
        }
        permit.validate()?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        let attempt = load_attempt(&mut tx, attempt_id)
            .await?
            .with_context(|| format!("provider attempt {attempt_id:?} does not exist"))?;
        ensure_attempt_identity(&attempt, permit)?;
        if !attempt_owner_is_current(&attempt, permit) {
            return Ok(ProviderMutation::Fenced);
        }
        if !matches!(attempt.status.as_str(), "in_flight" | "streaming") {
            bail!(
                "provider output cannot be appended from status {:?}",
                attempt.status
            );
        }

        let prior = sqlx::query(
            "SELECT content, chunk_count FROM provider_output_checkpoints WHERE attempt_id=?",
        )
        .bind(attempt_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (mut content, chunk_count) = match prior {
            Some(row) => (
                row.get::<String, _>("content"),
                row.get::<i64, _>("chunk_count"),
            ),
            None => (String::new(), 0),
        };
        content.push_str(visible_chunk);
        let digest = digest_text(&content);
        sqlx::query(
            "INSERT INTO provider_output_checkpoints
             (attempt_id, objective_id, objective_revision, state, content,
              content_digest, chunk_count, created_at, updated_at)
             VALUES (?, ?, ?, 'partial', ?, ?, ?, ?, ?)
             ON CONFLICT(attempt_id) DO UPDATE SET
               state='partial', content=excluded.content,
               content_digest=excluded.content_digest,
               chunk_count=excluded.chunk_count, updated_at=excluded.updated_at",
        )
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&content)
        .bind(&digest)
        .bind(chunk_count + 1)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE provider_route_attempts
             SET status='streaming', output_started=1, observed_at=?
             WHERE id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND owner_kind=? AND owner_id=? AND owner_epoch=?
               AND status IN ('in_flight', 'streaming')",
        )
        .bind(now)
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .bind(&permit.owner_kind)
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .execute(&mut *tx)
        .await?;
        latch_output(&mut tx, permit, &attempt.episode_id, now).await?;
        let checkpoint = ProviderOutputCheckpoint {
            attempt_id: attempt_id.to_string(),
            state: "partial".into(),
            content,
            content_digest: digest,
            chunk_count: chunk_count + 1,
        };
        tx.commit().await?;
        Ok(ProviderMutation::Applied(checkpoint))
    }

    /// Latch non-text model output (for example a streamed tool-call start)
    /// before it is exposed to the loop. The empty checkpoint is deliberate:
    /// it proves output began without inventing visible text that could later
    /// be mistaken for a resumable assistant answer.
    pub async fn latch_output_event(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        now: i64,
    ) -> Result<ProviderMutation<ProviderOutputCheckpoint>> {
        permit.validate()?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        let attempt = load_attempt(&mut tx, attempt_id)
            .await?
            .with_context(|| format!("provider attempt {attempt_id:?} does not exist"))?;
        ensure_attempt_identity(&attempt, permit)?;
        if !attempt_owner_is_current(&attempt, permit) {
            return Ok(ProviderMutation::Fenced);
        }
        if !matches!(attempt.status.as_str(), "in_flight" | "streaming") {
            bail!(
                "provider output event cannot be latched from status {:?}",
                attempt.status
            );
        }
        let digest = digest_text("");
        sqlx::query(
            "INSERT INTO provider_output_checkpoints
             (attempt_id, objective_id, objective_revision, state, content,
              content_digest, chunk_count, created_at, updated_at)
             VALUES (?, ?, ?, 'partial', '', ?, 0, ?, ?)
             ON CONFLICT(attempt_id) DO UPDATE SET state='partial', updated_at=excluded.updated_at",
        )
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&digest)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE provider_route_attempts
             SET status='streaming', output_started=1, observed_at=?
             WHERE id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND owner_kind=? AND owner_id=? AND owner_epoch=?
               AND status IN ('in_flight', 'streaming')",
        )
        .bind(now)
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .bind(&permit.owner_kind)
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .execute(&mut *tx)
        .await?;
        latch_output(&mut tx, permit, &attempt.episode_id, now).await?;
        let checkpoint = ProviderOutputCheckpoint {
            attempt_id: attempt_id.to_string(),
            state: "partial".into(),
            content: String::new(),
            content_digest: digest,
            chunk_count: 0,
        };
        tx.commit().await?;
        Ok(ProviderMutation::Applied(checkpoint))
    }

    /// Persist an unresolved side-effect receipt before the caller is allowed
    /// to invoke a tool or other external mutation.  `Applied` is the rung
    /// permit; after a restart the started receipt projects as observe-only
    /// until a separate reconciler settles it.
    pub async fn begin_side_effect(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        action_fingerprint: &str,
        idempotency_key: &str,
        now: i64,
    ) -> Result<ProviderMutation<ProviderAttemptSnapshot>> {
        validate_identifier("action_fingerprint", action_fingerprint)?;
        validate_identifier("idempotency_key", idempotency_key)?;
        permit.validate()?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        let attempt = load_attempt(&mut tx, attempt_id)
            .await?
            .with_context(|| format!("provider attempt {attempt_id:?} does not exist"))?;
        ensure_attempt_identity(&attempt, permit)?;
        if !attempt_owner_is_current(&attempt, permit) {
            return Ok(ProviderMutation::Fenced);
        }
        if !matches!(attempt.status.as_str(), "in_flight" | "streaming") {
            bail!(
                "provider side effect cannot start from status {:?}",
                attempt.status
            );
        }

        let existing_receipt = sqlx::query(
            "SELECT id FROM side_effect_receipts
             WHERE objective_id=? AND revision=? AND action_fingerprint=?
               AND idempotency_key=?",
        )
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(action_fingerprint)
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing_receipt) = existing_receipt {
            let receipt_id: String = existing_receipt.get("id");
            if attempt.side_effect_receipt_id.as_deref() == Some(receipt_id.as_str()) {
                tx.commit().await?;
                return Ok(ProviderMutation::Applied(attempt));
            }
            bail!("side-effect idempotency key is already bound to another attempt");
        }
        if attempt.side_effect_started || attempt.side_effect_receipt_id.is_some() {
            bail!("provider attempt already has a different unresolved side effect");
        }

        let receipt_id = digest_text(&format!(
            "provider-side-effect\0{}\0{}\0{attempt_id}\0{action_fingerprint}\0{idempotency_key}",
            permit.objective_id, permit.objective_revision
        ));
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES (?, ?, ?, ?, ?, ?, 'started', ?, ?)",
        )
        .bind(&receipt_id)
        .bind(&permit.objective_id)
        .bind(&permit.binding_id)
        .bind(permit.objective_revision)
        .bind(action_fingerprint)
        .bind(idempotency_key)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let changed = sqlx::query(
            "UPDATE provider_route_attempts SET side_effect_started=1,
               side_effect_receipt_id=?, observed_at=?
             WHERE id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND owner_kind=? AND owner_id=? AND owner_epoch=?
               AND status IN ('in_flight', 'streaming') AND side_effect_started=0",
        )
        .bind(&receipt_id)
        .bind(now)
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .bind(&permit.owner_kind)
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Ok(ProviderMutation::Fenced);
        }
        latch_side_effect(&mut tx, permit, &attempt.episode_id, now).await?;
        let updated = load_attempt(&mut tx, attempt_id)
            .await?
            .context("side-effect-latched provider attempt disappeared")?;
        tx.commit().await?;
        Ok(ProviderMutation::Applied(updated))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_failure(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        failure_class: &str,
        failure_code: &str,
        replayable: bool,
        now: i64,
    ) -> Result<ProviderMutation<OverloadBudgetDecision>> {
        validate_identifier("failure_class", failure_class)?;
        validate_identifier("failure_code", failure_code)?;
        permit.validate()?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        let attempt = load_attempt(&mut tx, attempt_id)
            .await?
            .with_context(|| format!("provider attempt {attempt_id:?} does not exist"))?;
        ensure_attempt_identity(&attempt, permit)?;
        if !attempt_owner_is_current(&attempt, permit) {
            return Ok(ProviderMutation::Fenced);
        }
        if !matches!(
            attempt.status.as_str(),
            "prepared" | "in_flight" | "streaming"
        ) {
            bail!("provider failure cannot settle status {:?}", attempt.status);
        }
        let replay_is_proven =
            replayable && !attempt.output_started && !attempt.side_effect_started;
        let status = if replay_is_proven {
            "failed_replayable"
        } else {
            "unknown"
        };
        sqlx::query(
            "UPDATE provider_route_attempts
             SET status=?, failure_class=?, failure_code=?, observed_at=?, completed_at=?
             WHERE id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND owner_kind=? AND owner_id=? AND owner_epoch=?",
        )
        .bind(status)
        .bind(failure_class)
        .bind(failure_code)
        .bind(now)
        .bind(now)
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .bind(&permit.owner_kind)
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .execute(&mut *tx)
        .await?;

        if !replay_is_proven {
            sqlx::query(
                "UPDATE provider_route_episodes SET status='unknown', updated_at=? WHERE id=?",
            )
            .bind(now)
            .bind(&attempt.episode_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            self.persist_provider_waiting_decision(
                permit,
                "provider_external_state_uncertain",
                now,
            )
            .await?;
            return Ok(ProviderMutation::Applied(
                OverloadBudgetDecision::DurableWaiting {
                    next_observation_at: now,
                },
            ));
        }

        let decision = if failure_class == "provider_overload" {
            let failed_attempts: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM provider_route_attempts
                 WHERE episode_id=? AND status='failed_replayable'
                   AND failure_class='provider_overload'",
            )
            .bind(&attempt.episode_id)
            .fetch_one(&mut *tx)
            .await?;
            overload_budget_decision(failed_attempts, now)
        } else {
            OverloadBudgetDecision::RetryAfter { retry_at: now }
        };
        if let OverloadBudgetDecision::DurableWaiting {
            next_observation_at,
        } = &decision
        {
            sqlx::query(
                "UPDATE provider_route_episodes
                 SET status='waiting', next_observation_at=?, updated_at=? WHERE id=?",
            )
            .bind(next_observation_at)
            .bind(now)
            .bind(&attempt.episode_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        if let OverloadBudgetDecision::DurableWaiting {
            next_observation_at,
        } = decision
        {
            self.persist_provider_waiting_decision(
                permit,
                "provider_overload_budget_exhausted",
                next_observation_at,
            )
            .await?;
        }
        Ok(ProviderMutation::Applied(decision))
    }

    #[cfg(not(test))]
    async fn persist_provider_waiting_decision(
        &self,
        permit: &ProviderOwnerPermit,
        routed_failure_code: &str,
        next_observation_at: i64,
    ) -> Result<()> {
        use crate::agent::objective::{
            DecisionRouter, ObjectiveStore, RecoveryDomain, RouteSignal,
        };

        let store = ObjectiveStore::new(self.pool.clone());
        let current = store
            .get(&permit.objective_id)
            .await?
            .with_context(|| format!("objective {:?} disappeared", permit.objective_id))?;
        if current.revision != permit.objective_revision {
            bail!("provider waiting decision lost its Objective revision");
        }
        let decision = DecisionRouter::route(
            &current,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Provider,
                failure_code: routed_failure_code.into(),
                failure_signature: digest_text(&format!(
                    "provider-waiting\0{}\0{}\0{routed_failure_code}",
                    permit.objective_id, permit.objective_revision
                )),
                next_observation_at,
                resume_cursor: current.root_turn_id.clone(),
            },
        )?;
        if permit.owner_kind == "remediation" {
            let mutation_permit = codefactory_agent_loop::tool::MutationPermit {
                objective_id: permit.objective_id.clone(),
                remediation_id: permit
                    .remediation_id
                    .clone()
                    .context("remediation provider permit has no remediation id")?,
                owner: permit.owner_id.clone(),
                claim_epoch: permit.owner_epoch,
                binding_id: Some(permit.binding_id.clone()),
                resource_generation: Some(permit.resource_generation),
            };
            store
                .apply_claimed_decision(current.revision, decision, &mutation_permit)
                .await?;
        } else {
            store.apply_decision(current.revision, decision).await?;
        }
        Ok(())
    }

    // The standalone persistence contract test includes this module directly,
    // outside the desktop crate's private Objective module. Production and
    // desktop integration tests exercise the real DecisionRouter path above.
    #[cfg(test)]
    async fn persist_provider_waiting_decision(
        &self,
        _permit: &ProviderOwnerPermit,
        _routed_failure_code: &str,
        _next_observation_at: i64,
    ) -> Result<()> {
        Ok(())
    }

    pub async fn mark_unknown(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        failure_code: &str,
        now: i64,
    ) -> Result<ProviderMutation<ProviderAttemptSnapshot>> {
        validate_identifier("failure_code", failure_code)?;
        permit.validate()?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        let attempt = load_attempt(&mut tx, attempt_id)
            .await?
            .with_context(|| format!("provider attempt {attempt_id:?} does not exist"))?;
        ensure_attempt_identity(&attempt, permit)?;
        if !attempt_owner_is_current(&attempt, permit) {
            return Ok(ProviderMutation::Fenced);
        }
        if matches!(attempt.status.as_str(), "response_committed" | "cancelled") {
            bail!("terminal provider attempt cannot become unknown");
        }
        let existing = sqlx::query(
            "SELECT content, content_digest, chunk_count, created_at
             FROM provider_output_checkpoints WHERE attempt_id=?",
        )
        .bind(attempt_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (content, digest, chunks, created_at) = match existing {
            Some(row) => (
                row.get::<String, _>("content"),
                row.get::<String, _>("content_digest"),
                row.get::<i64, _>("chunk_count"),
                row.get::<i64, _>("created_at"),
            ),
            None => (String::new(), digest_text(""), 0, now),
        };
        sqlx::query(
            "INSERT INTO provider_output_checkpoints
             (attempt_id, objective_id, objective_revision, state, content,
              content_digest, chunk_count, created_at, updated_at)
             VALUES (?, ?, ?, 'unknown', ?, ?, ?, ?, ?)
             ON CONFLICT(attempt_id) DO UPDATE SET state='unknown', updated_at=excluded.updated_at",
        )
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(content)
        .bind(digest)
        .bind(chunks)
        .bind(created_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE provider_route_attempts
             SET status='unknown', failure_code=?, observed_at=?, completed_at=? WHERE id=?",
        )
        .bind(failure_code)
        .bind(now)
        .bind(now)
        .bind(attempt_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE provider_route_episodes SET status='unknown', updated_at=? WHERE id=?")
            .bind(now)
            .bind(&attempt.episode_id)
            .execute(&mut *tx)
            .await?;
        let updated = load_attempt(&mut tx, attempt_id)
            .await?
            .context("unknown provider attempt disappeared")?;
        tx.commit().await?;
        Ok(ProviderMutation::Applied(updated))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn commit_response(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        response_digest: &str,
        checkpoint_content: &str,
        side_effect_started: bool,
        now: i64,
    ) -> Result<ProviderMutation<ProviderAttemptSnapshot>> {
        validate_digest("response_digest", response_digest)?;
        permit.validate()?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        let attempt = load_attempt(&mut tx, attempt_id)
            .await?
            .with_context(|| format!("provider attempt {attempt_id:?} does not exist"))?;
        ensure_attempt_identity(&attempt, permit)?;
        if !attempt_owner_is_current(&attempt, permit) {
            return Ok(ProviderMutation::Fenced);
        }
        if !matches!(attempt.status.as_str(), "in_flight" | "streaming") {
            bail!(
                "provider response cannot commit from status {:?}",
                attempt.status
            );
        }
        if side_effect_started && !attempt.side_effect_started {
            bail!("call begin_side_effect before committing a response with side effects");
        }
        let side_effect_started = attempt.side_effect_started;
        let content_digest = digest_text(checkpoint_content);
        let existing_chunks: i64 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT chunk_count FROM provider_output_checkpoints
                              WHERE attempt_id=?), 0)",
        )
        .bind(attempt_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO provider_output_checkpoints
             (attempt_id, objective_id, objective_revision, state, content,
              content_digest, chunk_count, created_at, updated_at)
             VALUES (?, ?, ?, 'committed', ?, ?, ?, ?, ?)
             ON CONFLICT(attempt_id) DO UPDATE SET state='committed', content=excluded.content,
               content_digest=excluded.content_digest, updated_at=excluded.updated_at",
        )
        .bind(attempt_id)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(checkpoint_content)
        .bind(&content_digest)
        .bind(existing_chunks)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE provider_route_attempts SET status='response_committed',
               response_digest=?, output_started=?, side_effect_started=?,
               observed_at=?, completed_at=? WHERE id=?",
        )
        .bind(response_digest)
        .bind(1_i64)
        .bind(i64::from(side_effect_started))
        .bind(now)
        .bind(now)
        .bind(attempt_id)
        .execute(&mut *tx)
        .await?;
        // Keep the episode live after a committed provider response. The same
        // fenced owner may issue the next model round after tool results; a
        // restart/takeover sees ResponseCheckpoint and cannot replay it.
        sqlx::query(
            "UPDATE provider_route_episodes SET status=?, output_started=1,
               side_effect_started=?, updated_at=?, completed_at=NULL WHERE id=?",
        )
        .bind(if side_effect_started {
            "unknown"
        } else {
            "active"
        })
        .bind(i64::from(side_effect_started))
        .bind(now)
        .bind(&attempt.episode_id)
        .execute(&mut *tx)
        .await?;
        latch_output(&mut tx, permit, &attempt.episode_id, now).await?;
        let updated = load_attempt(&mut tx, attempt_id)
            .await?
            .context("committed provider attempt disappeared")?;
        tx.commit().await?;
        Ok(ProviderMutation::Applied(updated))
    }

    /// Close the episodes belonging to a finished turn.
    ///
    /// `commit_response` deliberately keeps an episode live so the same fenced
    /// owner can issue the next model round after tool results, but nothing
    /// closed it once the turn itself ended: the only other `completed` write
    /// lives inside `open_episode`, which supersedes a prior episode only when
    /// `replay_safe` proves it never produced output. A turn that ended after
    /// committing a response therefore left an `active` episode that no later
    /// admission could ever supersede — every retry raised the Objective
    /// revision, re-derived a new episode id, hit the same live prior episode
    /// and bailed, so system recovery spun every 10s until the app quit
    /// (2026-08-13 freeze).
    ///
    /// Close only what is certain: no side effect outstanding and every attempt
    /// already terminal. `prepared`, `in_flight`, `streaming` and `unknown`
    /// still need the supervisor to observe them, so they keep the fence shut
    /// on purpose.
    pub async fn settle_finished_turn_episodes(
        &self,
        session_id: &str,
        root_turn_id: &str,
        now: i64,
    ) -> Result<u64> {
        let settled = sqlx::query(
            "UPDATE provider_route_episodes
                SET status='completed', completed_at=?, updated_at=?
              WHERE session_id=? AND root_turn_id=?
                AND status IN ('active', 'waiting')
                AND side_effect_started=0
                AND NOT EXISTS (
                      SELECT 1 FROM provider_route_attempts attempt
                       WHERE attempt.episode_id=provider_route_episodes.id
                         AND (attempt.side_effect_started<>0
                              OR attempt.status IN
                                 ('prepared', 'in_flight', 'streaming', 'unknown')))",
        )
        .bind(now)
        .bind(now)
        .bind(session_id)
        .bind(root_turn_id)
        .execute(&self.pool)
        .await?;
        Ok(settled.rows_affected())
    }

    pub async fn observe(&self, objective_id: &str) -> Result<ProviderRecoveryDisposition> {
        self.observe_at(objective_id, chrono::Utc::now().timestamp_millis())
            .await
    }

    /// Deterministic observation boundary used by the supervisor and fault
    /// tests. A bounded overload wait becomes executable only after its durable
    /// deadline; this does not relax partial, unknown, or side-effect latches.
    pub async fn observe_at(
        &self,
        objective_id: &str,
        now: i64,
    ) -> Result<ProviderRecoveryDisposition> {
        let episode = sqlx::query(
            "SELECT e.id, e.status, e.next_observation_at,
                    e.side_effect_started,
                    b.side_effect_started AS binding_side_effect_started,
                    o.side_effect_started AS objective_side_effect_started,
                    (SELECT COUNT(*) FROM side_effect_receipts receipt
                     WHERE receipt.objective_id=e.objective_id
                       AND receipt.status IN ('started', 'unknown'))
                       AS unresolved_receipt_count,
                    (SELECT COUNT(*) FROM side_effect_receipts receipt
                     WHERE receipt.objective_id=e.objective_id
                       AND receipt.status IN ('committed', 'reconciled', 'cancelled'))
                       AS settled_receipt_count
             FROM provider_route_episodes e
             JOIN objective_bindings b ON b.id=e.binding_id AND b.objective_id=e.objective_id
             JOIN objectives o ON o.id=e.objective_id
             WHERE e.objective_id=?
             ORDER BY e.updated_at DESC, e.created_at DESC LIMIT 1",
        )
        .bind(objective_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(episode) = episode else {
            return Ok(ProviderRecoveryDisposition::NoEpisode);
        };
        let episode_id: String = episode.get("id");
        let episode_status: String = episode.get("status");
        let next_observation_at = episode.try_get::<Option<i64>, _>("next_observation_at")?;
        if episode_status == "waiting" && next_observation_at.is_none_or(|deadline| now < deadline)
        {
            return Ok(ProviderRecoveryDisposition::DurableWaiting {
                episode_id,
                // A malformed legacy waiting row with no deadline remains
                // fail-closed instead of becoming immediately replayable.
                next_observation_at: next_observation_at.unwrap_or(i64::MAX),
            });
        }
        let attempt = sqlx::query(
            "SELECT id, status, output_started, side_effect_started
             FROM provider_route_attempts WHERE episode_id=?
             ORDER BY attempt_order DESC LIMIT 1",
        )
        .bind(&episode_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(attempt) = attempt else {
            return Ok(ProviderRecoveryDisposition::ReadyToAttempt { episode_id });
        };
        let attempt_id: String = attempt.get("id");
        let status: String = attempt.get("status");
        let historical_side_effect = episode.get::<i64, _>("binding_side_effect_started") != 0
            || episode.get::<i64, _>("objective_side_effect_started") != 0;
        let side_effect_started = attempt.get::<i64, _>("side_effect_started") != 0
            || episode.get::<i64, _>("side_effect_started") != 0
            || episode.get::<i64, _>("unresolved_receipt_count") != 0
            || (historical_side_effect && episode.get::<i64, _>("settled_receipt_count") == 0);
        if side_effect_started {
            return Ok(ProviderRecoveryDisposition::ObserveOnlySideEffect {
                episode_id,
                attempt_id,
            });
        }
        match status.as_str() {
            "prepared" => Ok(ProviderRecoveryDisposition::ObserveOnlyPrepared {
                episode_id,
                attempt_id,
            }),
            "in_flight" => Ok(ProviderRecoveryDisposition::ObserveOnlyInFlight {
                episode_id,
                attempt_id,
            }),
            "streaming" => Ok(ProviderRecoveryDisposition::ObserveOnlyPartial {
                episode_id,
                checkpoint_content: load_checkpoint_content(&self.pool, &attempt_id).await?,
                attempt_id,
            }),
            "unknown" => Ok(ProviderRecoveryDisposition::ObserveOnlyUnknown {
                episode_id,
                attempt_id,
            }),
            "failed_replayable" => Ok(ProviderRecoveryDisposition::RetrySafe {
                episode_id,
                attempt_id,
            }),
            "response_committed" => Ok(ProviderRecoveryDisposition::ResponseCheckpoint {
                episode_id,
                checkpoint_content: load_checkpoint_content(&self.pool, &attempt_id).await?,
                attempt_id,
            }),
            "failed_fatal" | "cancelled" => Ok(ProviderRecoveryDisposition::ObserveOnlyUnknown {
                episode_id,
                attempt_id,
            }),
            other => bail!("invalid provider attempt status {other:?}"),
        }
    }

    /// Find provider work whose process owner disappeared while the business
    /// Objective stayed active. This is observation only: the startup control
    /// plane advances each candidate through the normal Objective CAS before
    /// any adapter may resume it.
    pub async fn startup_recovery_candidates(
        &self,
        now: i64,
    ) -> Result<Vec<ProviderStartupRecovery>> {
        let rows = sqlx::query(
            "SELECT o.id AS objective_id, o.revision AS objective_revision,
                    e.root_turn_id, e.status AS episode_status,
                    e.next_observation_at, e.side_effect_started AS episode_side_effect,
                    b.side_effect_started AS binding_side_effect,
                    o.side_effect_started AS objective_side_effect,
                    (SELECT COUNT(*) FROM side_effect_receipts receipt
                     WHERE receipt.objective_id=o.id
                       AND receipt.status IN ('started', 'unknown'))
                       AS unresolved_receipt_count,
                    (SELECT COUNT(*) FROM side_effect_receipts receipt
                     WHERE receipt.objective_id=o.id
                       AND receipt.status IN ('committed', 'reconciled', 'cancelled'))
                       AS settled_receipt_count,
                    a.status AS attempt_status, a.output_started AS attempt_output,
                    a.side_effect_started AS attempt_side_effect
             FROM objectives o
             JOIN provider_route_episodes e ON e.id=(
                 SELECT latest.id FROM provider_route_episodes latest
                 WHERE latest.objective_id=o.id
                 ORDER BY latest.updated_at DESC, latest.created_at DESC LIMIT 1
             )
             JOIN objective_bindings b ON b.id=e.binding_id AND b.objective_id=o.id
             LEFT JOIN provider_route_attempts a ON a.id=(
                 SELECT latest_attempt.id FROM provider_route_attempts latest_attempt
                 WHERE latest_attempt.episode_id=e.id
                 ORDER BY latest_attempt.attempt_order DESC LIMIT 1
             )
             WHERE o.status='active'
               AND NOT EXISTS (
                 SELECT 1 FROM chat_run_controls c
                 WHERE c.objective_id=o.id AND c.objective_revision=o.revision
                   AND c.status IN ('active', 'cancel_requested')
               )
             ORDER BY o.updated_at, o.id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let episode_status: String = row.get("episode_status");
                let attempt_status: Option<String> = row.try_get("attempt_status")?;
                let historical_side_effect = row.get::<i64, _>("binding_side_effect") != 0
                    || row.get::<i64, _>("objective_side_effect") != 0;
                let has_side_effect = row.get::<i64, _>("episode_side_effect") != 0
                    || row.get::<i64, _>("unresolved_receipt_count") != 0
                    || (historical_side_effect
                        && row.get::<i64, _>("settled_receipt_count") == 0)
                    || row
                        .try_get::<Option<i64>, _>("attempt_side_effect")?
                        .unwrap_or_default()
                        != 0;
                let has_output = row
                    .try_get::<Option<i64>, _>("attempt_output")?
                    .unwrap_or_default()
                    != 0;
                let failure_code = if has_side_effect {
                    "provider_side_effect_unresolved"
                } else if episode_status == "waiting" {
                    "provider_overload_budget_exhausted"
                } else {
                    match attempt_status.as_deref() {
                        None | Some("prepared") => "provider_prepared_after_restart",
                        Some("failed_replayable") => "provider_retry_safe_after_restart",
                        Some("in_flight") => "provider_external_state_uncertain",
                        Some("streaming") if has_output => "provider_partial_output_unresolved",
                        Some("response_committed") => "provider_response_checkpoint_unsettled",
                        Some("unknown" | "streaming" | "failed_fatal" | "cancelled") => {
                            "provider_external_state_uncertain"
                        }
                        Some(other) => bail!("invalid provider attempt status {other:?}"),
                    }
                };
                Ok(ProviderStartupRecovery {
                    objective_id: row.get("objective_id"),
                    objective_revision: row.get("objective_revision"),
                    root_turn_id: row.get("root_turn_id"),
                    failure_code: failure_code.into(),
                    next_observation_at: row
                        .get::<Option<i64>, _>("next_observation_at")
                        .unwrap_or(now),
                })
            })
            .collect()
    }

    /// A provider POST has no external mutation semantics of its own. After
    /// its chat-run owner is durably retired, a latest in-flight attempt with
    /// no observed bytes, no tool intent, and no unresolved tool receipt may
    /// be replayed at least once. Any partial output or uncertain receipt keeps
    /// the attempt fenced and observation-only.
    pub async fn reconcile_stale_effect_free_in_flight(&self, now: i64) -> Result<u64> {
        let updated = sqlx::query(
            "UPDATE provider_route_attempts
             SET status='failed_replayable',
                 failure_class='process_restart',
                 failure_code='provider_process_restarted_before_output',
                 observed_at=?, completed_at=?
             WHERE id IN (
                 SELECT attempt.id
                 FROM provider_route_attempts attempt
                 JOIN provider_route_episodes episode ON episode.id=attempt.episode_id
                 JOIN objectives objective ON objective.id=attempt.objective_id
                 JOIN objective_bindings binding
                   ON binding.id=attempt.binding_id
                  AND binding.objective_id=attempt.objective_id
                 WHERE attempt.status='in_flight'
                   AND attempt.output_started=0
                   AND attempt.side_effect_started=0
                   AND attempt.side_effect_receipt_id IS NULL
                   AND objective.status='active'
                   AND attempt.id=(
                       SELECT latest.id FROM provider_route_attempts latest
                       WHERE latest.episode_id=attempt.episode_id
                       ORDER BY latest.attempt_order DESC LIMIT 1)
                   AND NOT EXISTS (
                       SELECT 1 FROM provider_output_checkpoints checkpoint
                       WHERE checkpoint.attempt_id=attempt.id)
                   AND NOT EXISTS (
                       SELECT 1 FROM chat_run_controls control
                       WHERE control.objective_id=objective.id
                         AND control.objective_revision=objective.revision
                         AND control.status IN ('active', 'cancel_requested'))
                   AND NOT EXISTS (
                       SELECT 1 FROM side_effect_receipts receipt
                       WHERE receipt.objective_id=objective.id
                         AND receipt.status IN ('started', 'unknown'))
                   AND (
                       (objective.side_effect_started=0
                        AND binding.side_effect_started=0)
                       OR EXISTS (
                           SELECT 1 FROM side_effect_receipts receipt
                           WHERE receipt.objective_id=objective.id
                             AND receipt.status IN
                                 ('committed', 'reconciled', 'cancelled'))))",
        )
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected())
    }

    async fn transition_attempt(
        &self,
        permit: &ProviderOwnerPermit,
        attempt_id: &str,
        from: &str,
        to: &str,
        now: i64,
    ) -> Result<ProviderMutation<ProviderAttemptSnapshot>> {
        permit.validate()?;
        let mut tx = self.pool.begin().await?;
        if !permit_is_current(&mut tx, permit, now).await? {
            return Ok(ProviderMutation::Fenced);
        }
        let attempt = load_attempt(&mut tx, attempt_id)
            .await?
            .with_context(|| format!("provider attempt {attempt_id:?} does not exist"))?;
        ensure_attempt_identity(&attempt, permit)?;
        if !attempt_owner_is_current(&attempt, permit) {
            return Ok(ProviderMutation::Fenced);
        }
        if attempt.status == to {
            tx.commit().await?;
            return Ok(ProviderMutation::Applied(attempt));
        }
        if attempt.status != from {
            bail!(
                "provider attempt transition requires {from:?}, got {:?}",
                attempt.status
            );
        }
        let changed = sqlx::query(
            "UPDATE provider_route_attempts SET status=?, started_at=COALESCE(started_at, ?),
               observed_at=? WHERE id=? AND status=? AND objective_id=?
               AND objective_revision=? AND binding_id=? AND resource_generation=?
               AND owner_kind=? AND owner_id=? AND owner_epoch=?",
        )
        .bind(to)
        .bind(now)
        .bind(now)
        .bind(attempt_id)
        .bind(from)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .bind(&permit.binding_id)
        .bind(permit.resource_generation)
        .bind(&permit.owner_kind)
        .bind(&permit.owner_id)
        .bind(permit.owner_epoch)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Ok(ProviderMutation::Fenced);
        }
        let updated = load_attempt(&mut tx, attempt_id)
            .await?
            .context("transitioned provider attempt disappeared")?;
        tx.commit().await?;
        Ok(ProviderMutation::Applied(updated))
    }
}

async fn permit_is_current(
    tx: &mut Transaction<'_, Sqlite>,
    permit: &ProviderOwnerPermit,
    now: i64,
) -> Result<bool> {
    let count: i64 = match permit.owner_kind.as_str() {
        "remediation" => {
            let remediation_id = permit
                .remediation_id
                .as_deref()
                .context("remediation provider permit has no remediation id")?;
            sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM objectives o
                 JOIN objective_bindings b ON b.id=? AND b.objective_id=o.id
                 JOIN objective_remediations r ON r.id=? AND r.objective_id=o.id
                 WHERE o.id=? AND o.revision=?
                   AND o.status NOT IN ('completed', 'cancelled', 'legacy_orphan')
                   AND o.remediation_id=r.id
                   AND o.lease_owner=? AND o.lease_expires_at>?
                   AND b.resource_generation=?
                   AND r.binding_id=b.id AND r.status='claimed'
                   AND r.lease_owner=? AND r.lease_expires_at>?
                   AND r.attempt_index=?",
            )
            .bind(&permit.binding_id)
            .bind(remediation_id)
            .bind(&permit.objective_id)
            .bind(permit.objective_revision)
            .bind(&permit.owner_id)
            .bind(now)
            .bind(permit.resource_generation)
            .bind(&permit.owner_id)
            .bind(now)
            .bind(permit.owner_epoch)
            .fetch_one(&mut **tx)
            .await?
        }
        "chat_run" => {
            sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM objectives o
                 JOIN objective_bindings b ON b.id=? AND b.objective_id=o.id
                 JOIN chat_run_controls c ON c.run_instance_id=?
                 WHERE o.id=? AND o.revision=? AND o.status='active'
                   AND b.resource_generation=?
                   AND c.objective_id=o.id AND c.objective_revision=o.revision
                   AND c.session_id=o.session_id AND c.root_turn_id=o.root_turn_id
                   AND c.status='active'",
            )
            .bind(&permit.binding_id)
            .bind(&permit.owner_id)
            .bind(&permit.objective_id)
            .bind(permit.objective_revision)
            .bind(permit.resource_generation)
            .fetch_one(&mut **tx)
            .await?
        }
        _ => return Ok(false),
    };
    Ok(count == 1)
}

async fn ensure_episode_current(
    tx: &mut Transaction<'_, Sqlite>,
    permit: &ProviderOwnerPermit,
    episode_id: &str,
) -> Result<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM provider_route_episodes
         WHERE id=? AND objective_id=? AND admission_revision=?
           AND binding_id=? AND resource_generation=? AND status='active'",
    )
    .bind(episode_id)
    .bind(&permit.objective_id)
    .bind(permit.objective_revision)
    .bind(&permit.binding_id)
    .bind(permit.resource_generation)
    .fetch_one(&mut **tx)
    .await?;
    if count != 1 {
        bail!("provider episode is not current and active");
    }
    Ok(())
}

async fn load_episode(
    tx: &mut Transaction<'_, Sqlite>,
    episode_id: &str,
) -> Result<Option<ProviderEpisodeSnapshot>> {
    let row = sqlx::query(
        "SELECT id, objective_id, admission_revision, binding_id,
                resource_generation, session_id, root_turn_id, policy,
                candidate_snapshot_digest, candidate_snapshot_json,
                resume_cursor, status
         FROM provider_route_episodes WHERE id=?",
    )
    .bind(episode_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| ProviderEpisodeSnapshot {
        id: row.get("id"),
        objective_id: row.get("objective_id"),
        objective_revision: row.get("admission_revision"),
        binding_id: row.get("binding_id"),
        resource_generation: row.get("resource_generation"),
        session_id: row.get("session_id"),
        root_turn_id: row.get("root_turn_id"),
        policy: row.get("policy"),
        candidate_snapshot_digest: row.get("candidate_snapshot_digest"),
        candidate_snapshot_json: row.get("candidate_snapshot_json"),
        resume_cursor: row.get("resume_cursor"),
        status: row.get("status"),
    }))
}

async fn load_attempt(
    tx: &mut Transaction<'_, Sqlite>,
    attempt_id: &str,
) -> Result<Option<ProviderAttemptSnapshot>> {
    let row = sqlx::query(
        "SELECT id, episode_id, objective_id, objective_revision, binding_id,
                resource_generation, attempt_order, endpoint, model, request_digest,
                resume_cursor, status, output_started, side_effect_started,
                side_effect_receipt_id, owner_kind, owner_id, owner_epoch
         FROM provider_route_attempts WHERE id=?",
    )
    .bind(attempt_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| ProviderAttemptSnapshot {
        id: row.get("id"),
        episode_id: row.get("episode_id"),
        objective_id: row.get("objective_id"),
        objective_revision: row.get("objective_revision"),
        binding_id: row.get("binding_id"),
        resource_generation: row.get("resource_generation"),
        attempt_order: row.get("attempt_order"),
        endpoint: row.get("endpoint"),
        model: row.get("model"),
        request_digest: row.get("request_digest"),
        resume_cursor: row.get("resume_cursor"),
        status: row.get("status"),
        output_started: row.get::<i64, _>("output_started") != 0,
        side_effect_started: row.get::<i64, _>("side_effect_started") != 0,
        side_effect_receipt_id: row.get("side_effect_receipt_id"),
        owner_kind: row.get("owner_kind"),
        owner_id: row.get("owner_id"),
        owner_epoch: row.get("owner_epoch"),
    }))
}

fn ensure_existing_episode_matches(
    episode: &ProviderEpisodeSnapshot,
    permit: &ProviderOwnerPermit,
    spec: &ProviderEpisodeSpec,
) -> Result<()> {
    if episode.objective_id != permit.objective_id
        || episode.objective_revision != permit.objective_revision
        || episode.binding_id != permit.binding_id
        || episode.resource_generation != permit.resource_generation
        || episode.session_id != spec.session_id
        || episode.root_turn_id != spec.root_turn_id
        || episode.policy != spec.policy
        || episode.candidate_snapshot_digest != spec.candidate_snapshot_digest
        || episode.candidate_snapshot_json != spec.candidate_snapshot_json
        || episode.resume_cursor != spec.resume_cursor
    {
        bail!("provider episode id is already bound to different durable identity");
    }
    Ok(())
}

fn ensure_existing_attempt_matches(
    attempt: &ProviderAttemptSnapshot,
    permit: &ProviderOwnerPermit,
    spec: &ProviderAttemptSpec,
) -> Result<()> {
    ensure_attempt_identity(attempt, permit)?;
    if attempt.episode_id != spec.episode_id
        || attempt.endpoint != spec.endpoint
        || attempt.model != spec.model
        || attempt.request_digest != spec.request_digest
        || attempt.resume_cursor != spec.resume_cursor
    {
        bail!("provider attempt id is already bound to different route payload");
    }
    Ok(())
}

fn ensure_attempt_identity(
    attempt: &ProviderAttemptSnapshot,
    permit: &ProviderOwnerPermit,
) -> Result<()> {
    if attempt.objective_id != permit.objective_id
        || attempt.objective_revision != permit.objective_revision
        || attempt.binding_id != permit.binding_id
        || attempt.resource_generation != permit.resource_generation
    {
        bail!("provider attempt does not match the current durable identity");
    }
    Ok(())
}

fn attempt_owner_is_current(
    attempt: &ProviderAttemptSnapshot,
    permit: &ProviderOwnerPermit,
) -> bool {
    attempt.owner_kind == permit.owner_kind
        && attempt.owner_id == permit.owner_id
        && attempt.owner_epoch == permit.owner_epoch
}

async fn latch_output(
    tx: &mut Transaction<'_, Sqlite>,
    permit: &ProviderOwnerPermit,
    episode_id: &str,
    now: i64,
) -> Result<()> {
    sqlx::query("UPDATE provider_route_episodes SET output_started=1, updated_at=? WHERE id=?")
        .bind(now)
        .bind(episode_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE objective_bindings SET output_started=1, updated_at=?
         WHERE id=? AND objective_id=? AND resource_generation=?",
    )
    .bind(now)
    .bind(&permit.binding_id)
    .bind(&permit.objective_id)
    .bind(permit.resource_generation)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE objectives SET output_started=1, updated_at=? WHERE id=? AND revision=?")
        .bind(now)
        .bind(&permit.objective_id)
        .bind(permit.objective_revision)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn latch_side_effect(
    tx: &mut Transaction<'_, Sqlite>,
    permit: &ProviderOwnerPermit,
    episode_id: &str,
    now: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE provider_route_episodes SET side_effect_started=1, updated_at=? WHERE id=?",
    )
    .bind(now)
    .bind(episode_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE objective_bindings SET side_effect_started=1, updated_at=?
         WHERE id=? AND objective_id=? AND resource_generation=?",
    )
    .bind(now)
    .bind(&permit.binding_id)
    .bind(&permit.objective_id)
    .bind(permit.resource_generation)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE objectives SET side_effect_started=1, updated_at=? WHERE id=? AND revision=?",
    )
    .bind(now)
    .bind(&permit.objective_id)
    .bind(permit.objective_revision)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn load_checkpoint_content(pool: &SqlitePool, attempt_id: &str) -> Result<String> {
    sqlx::query_scalar("SELECT content FROM provider_output_checkpoints WHERE attempt_id=?")
        .bind(attempt_id)
        .fetch_optional(pool)
        .await?
        .with_context(|| format!("provider attempt {attempt_id:?} has no durable checkpoint"))
}

fn validate_episode_spec(spec: &ProviderEpisodeSpec) -> Result<()> {
    for (label, value) in [
        ("episode id", spec.id.as_str()),
        ("session id", spec.session_id.as_str()),
        ("root turn id", spec.root_turn_id.as_str()),
        ("resume cursor", spec.resume_cursor.as_str()),
    ] {
        validate_identifier(label, value)?;
    }
    if !matches!(spec.policy.as_str(), "fixed" | "prefer" | "auto") {
        bail!("invalid provider routing policy {:?}", spec.policy);
    }
    validate_digest("candidate snapshot digest", &spec.candidate_snapshot_digest)?;
    let snapshot: Value = serde_json::from_str(&spec.candidate_snapshot_json)
        .context("provider candidate snapshot must be valid JSON")?;
    if !snapshot.is_array() {
        bail!("provider candidate snapshot must be an ordered JSON array");
    }
    reject_secret_json(&snapshot)?;
    Ok(())
}

fn validate_attempt_spec(spec: &ProviderAttemptSpec) -> Result<()> {
    for (label, value) in [
        ("attempt id", spec.id.as_str()),
        ("episode id", spec.episode_id.as_str()),
        ("endpoint", spec.endpoint.as_str()),
        ("model", spec.model.as_str()),
        ("resume cursor", spec.resume_cursor.as_str()),
    ] {
        validate_identifier(label, value)?;
    }
    validate_digest("request digest", &spec.request_digest)
}

fn validate_digest(label: &str, value: &str) -> Result<()> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        bail!("{label} must use sha256");
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} is not a full sha256 digest");
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        bail!("{label} must be a non-empty public identifier");
    }
    Ok(())
}

fn reject_secret_json(value: &Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let key = key.to_ascii_lowercase();
                if [
                    "token",
                    "secret",
                    "password",
                    "authorization",
                    "api_key",
                    "apikey",
                ]
                .iter()
                .any(|marker| key.contains(marker))
                {
                    bail!("provider candidate snapshot contains secret-like field {key:?}");
                }
                reject_secret_json(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_secret_json(value)?;
            }
        }
        Value::String(text) => {
            let lowercase = text.to_ascii_lowercase();
            if lowercase.contains("bearer ") || lowercase.contains("access_token=") {
                bail!("provider candidate snapshot contains credential-like content");
            }
        }
        _ => {}
    }
    Ok(())
}

fn digest_text(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}
