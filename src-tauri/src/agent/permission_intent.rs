// SPDX-License-Identifier: Apache-2.0
//! Durable, exact-scope permission intents.
//!
//! Provider `tool_call_id` values are not globally unique and are never an
//! authorization key. A prompt is owned by an opaque Objective revision plus
//! one immutable binding generation and one hashed action. Every allow is a
//! single-consumption receipt for that exact tuple.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionScope {
    pub(crate) objective_id: String,
    pub(crate) objective_revision: i64,
    pub(crate) binding_id: String,
    pub(crate) resource_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionIntentRequest {
    pub(crate) scope: PermissionScope,
    pub(crate) session_id: String,
    pub(crate) provider_tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) args: Value,
    pub(crate) bash_command: Option<String>,
    pub(crate) expires_at: i64,
    pub(crate) created_process_instance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionIntentStatus {
    Pending,
    Allowed,
    Consumed,
    Denied,
    TimedOut,
    ChannelClosed,
    Cancelled,
    Superseded,
}

impl PermissionIntentStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Allowed => "allowed",
            Self::Consumed => "consumed",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::ChannelClosed => "channel_closed",
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "allowed" => Ok(Self::Allowed),
            "consumed" => Ok(Self::Consumed),
            "denied" => Ok(Self::Denied),
            "timed_out" => Ok(Self::TimedOut),
            "channel_closed" => Ok(Self::ChannelClosed),
            "cancelled" => Ok(Self::Cancelled),
            "superseded" => Ok(Self::Superseded),
            _ => bail!("unknown permission intent status: {value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionIntentSnapshot {
    pub(crate) intent_id: String,
    pub(crate) scope: PermissionScope,
    pub(crate) session_id: String,
    pub(crate) provider_tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) prompt_args: Value,
    pub(crate) action_signature: String,
    pub(crate) prompt_generation: i64,
    pub(crate) predecessor_intent_id: Option<String>,
    pub(crate) status: PermissionIntentStatus,
    pub(crate) failure_code: Option<String>,
    pub(crate) expires_at: i64,
    pub(crate) decided_at: Option<i64>,
    pub(crate) consumed_at: Option<i64>,
}

impl PermissionIntentSnapshot {
    pub(crate) fn prompt_key(&self) -> PermissionPromptKey {
        PermissionPromptKey {
            intent_id: self.intent_id.clone(),
            objective_id: self.scope.objective_id.clone(),
            objective_revision: self.scope.objective_revision,
            binding_id: self.scope.binding_id.clone(),
            resource_generation: self.scope.resource_generation,
            action_signature: self.action_signature.clone(),
            prompt_generation: self.prompt_generation,
        }
    }

    fn from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<Self> {
        Ok(Self {
            intent_id: row.try_get("intent_id")?,
            scope: PermissionScope {
                objective_id: row.try_get("objective_id")?,
                objective_revision: row.try_get("objective_revision")?,
                binding_id: row.try_get("binding_id")?,
                resource_generation: row.try_get("resource_generation")?,
            },
            session_id: row.try_get("session_id")?,
            provider_tool_call_id: row.try_get("provider_tool_call_id")?,
            tool_name: row.try_get("tool_name")?,
            prompt_args: serde_json::from_str(&row.try_get::<String, _>("prompt_args_json")?)
                .context("decode persisted permission prompt args")?,
            action_signature: row.try_get("action_signature")?,
            prompt_generation: row.try_get("prompt_generation")?,
            predecessor_intent_id: row.try_get("predecessor_intent_id")?,
            status: PermissionIntentStatus::parse(row.try_get::<String, _>("status")?.as_str())?,
            failure_code: row.try_get("failure_code")?,
            expires_at: row.try_get("expires_at")?,
            decided_at: row.try_get("decided_at")?,
            consumed_at: row.try_get("consumed_at")?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionRecoveryDisposition {
    AwaitingDecision,
    ReprojectInterrupted,
    ConsumeExactAllow,
    AlreadyConsumed,
    ExplicitlyDenied,
    ExplicitlyCancelled,
    StaleScope,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PermissionPromptProjection {
    pub(crate) intent_id: String,
    pub(crate) provider_tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) args: Value,
    pub(crate) expires_at: i64,
    pub(crate) prompt_generation: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PermissionIntentObservation {
    pub(crate) snapshot: PermissionIntentSnapshot,
    pub(crate) disposition: PermissionRecoveryDisposition,
    pub(crate) projection: PermissionPromptProjection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedPermissionSettlement {
    pub(crate) objective_id: String,
    pub(crate) objective_revision: i64,
    pub(crate) recovery_scheduled: bool,
    pub(crate) remediation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PermissionClaimAction {
    ProjectPrompt(PermissionIntentObservation),
    ResumeAuthorizedAction,
}

impl PermissionIntentObservation {
    fn new(snapshot: PermissionIntentSnapshot, disposition: PermissionRecoveryDisposition) -> Self {
        let projection = PermissionPromptProjection {
            intent_id: snapshot.intent_id.clone(),
            provider_tool_call_id: snapshot.provider_tool_call_id.clone(),
            tool_name: snapshot.tool_name.clone(),
            args: snapshot.prompt_args.clone(),
            expires_at: snapshot.expires_at,
            prompt_generation: snapshot.prompt_generation,
        };
        Self {
            snapshot,
            disposition,
            projection,
        }
    }
}

/// Exact in-memory ownership of the one-shot channel. `tool_call_id` is
/// deliberately absent: two providers may emit the same value concurrently.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PermissionPromptKey {
    pub(crate) intent_id: String,
    pub(crate) objective_id: String,
    pub(crate) objective_revision: i64,
    pub(crate) binding_id: String,
    pub(crate) resource_generation: i64,
    pub(crate) action_signature: String,
    pub(crate) prompt_generation: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionPromptResponse {
    Allow,
    Deny,
}

#[derive(Clone, Default)]
pub(crate) struct PendingPermissionRegistry {
    inner: Arc<Mutex<HashMap<PermissionPromptKey, oneshot::Sender<PermissionPromptResponse>>>>,
}

impl PendingPermissionRegistry {
    pub(crate) async fn register(
        &self,
        key: PermissionPromptKey,
        sender: oneshot::Sender<PermissionPromptResponse>,
    ) -> anyhow::Result<()> {
        let mut pending = self.inner.lock().await;
        match pending.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(sender);
                Ok(())
            }
            Entry::Occupied(entry) => {
                bail!(
                    "permission prompt already registered: {}",
                    entry.key().intent_id
                )
            }
        }
    }

    pub(crate) async fn take_exact(
        &self,
        key: &PermissionPromptKey,
    ) -> Option<oneshot::Sender<PermissionPromptResponse>> {
        self.inner.lock().await.remove(key)
    }

    pub(crate) async fn remove_exact(&self, key: &PermissionPromptKey) -> bool {
        self.inner.lock().await.remove(key).is_some()
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

#[derive(Clone)]
pub(crate) struct PermissionIntentStore {
    pool: SqlitePool,
}

impl PermissionIntentStore {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn create_pending(
        &self,
        request: &PermissionIntentRequest,
        now: i64,
    ) -> anyhow::Result<PermissionIntentSnapshot> {
        validate_request(request, now)?;
        let action_signature = permission_action_signature(
            &request.tool_name,
            &request.args,
            request.bash_command.as_deref(),
        );
        let prompt_args_json =
            serde_json::to_string(&crate::trajectory::redact_json(&request.args))
                .context("encode redacted permission prompt args")?;
        let intent_id = Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;
        validate_scope(&mut tx, &request.scope).await?;

        if let Some(existing) =
            latest_action_in_tx(&mut tx, &request.scope, &action_signature).await?
        {
            let reason = match existing.status {
                PermissionIntentStatus::Pending => "is already waiting for a decision",
                PermissionIntentStatus::Allowed => "already has an unconsumed allow receipt",
                PermissionIntentStatus::Consumed => "already consumed its allow receipt",
                PermissionIntentStatus::Denied => "was explicitly denied",
                PermissionIntentStatus::TimedOut | PermissionIntentStatus::ChannelClosed => {
                    "must be rehydrated without replaying the provider call"
                }
                PermissionIntentStatus::Cancelled => "was explicitly cancelled",
                PermissionIntentStatus::Superseded => "has stale authority",
            };
            bail!(
                "permission action {} {reason} (intent {})",
                action_signature,
                existing.intent_id
            );
        }

        let row = sqlx::query(
            "INSERT INTO permission_intents
             (intent_id, objective_id, objective_revision, binding_id,
              resource_generation, session_id, provider_tool_call_id, tool_name,
              prompt_args_json, action_signature, prompt_generation,
              predecessor_intent_id, status, expires_at,
              created_process_instance, created_at, updated_at)
             SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    COALESCE(MAX(prompt_generation), 0) + 1,
                    NULL, 'pending', ?, ?, ?, ?
             FROM permission_intents
             WHERE objective_id=? AND objective_revision=? AND binding_id=?
               AND resource_generation=? AND action_signature=?
             RETURNING *",
        )
        .bind(&intent_id)
        .bind(&request.scope.objective_id)
        .bind(request.scope.objective_revision)
        .bind(&request.scope.binding_id)
        .bind(request.scope.resource_generation)
        .bind(&request.session_id)
        .bind(&request.provider_tool_call_id)
        .bind(&request.tool_name)
        .bind(&prompt_args_json)
        .bind(&action_signature)
        .bind(request.expires_at)
        .bind(&request.created_process_instance)
        .bind(now)
        .bind(now)
        .bind(&request.scope.objective_id)
        .bind(request.scope.objective_revision)
        .bind(&request.scope.binding_id)
        .bind(request.scope.resource_generation)
        .bind(&action_signature)
        .fetch_one(&mut *tx)
        .await
        .context("create exact permission intent")?;
        let snapshot = PermissionIntentSnapshot::from_row(&row)?;
        tx.commit().await?;
        Ok(snapshot)
    }

    /// Observe one exact durable intent without executing a provider or native
    /// action. Pending expiry and stale active authority are reconciled inside
    /// the same transaction so the returned disposition is restart-safe.
    pub(crate) async fn observe_exact(
        &self,
        intent_id: &str,
        now: i64,
    ) -> anyhow::Result<Option<PermissionIntentObservation>> {
        let mut tx = self.pool.begin().await?;
        let Some(initial) = get_in_tx(&mut tx, intent_id).await? else {
            tx.commit().await?;
            return Ok(None);
        };
        expire_exact_if_due(&mut tx, &initial.prompt_key(), now).await?;
        let mut snapshot = get_in_tx(&mut tx, intent_id)
            .await?
            .ok_or_else(|| anyhow!("observed permission intent disappeared"))?;
        let authoritative = scope_is_authoritative(&mut tx, &snapshot.scope).await?;
        if !authoritative
            && matches!(
                snapshot.status,
                PermissionIntentStatus::Pending | PermissionIntentStatus::Allowed
            )
        {
            supersede_exact(&mut tx, &snapshot.prompt_key(), now).await?;
            snapshot = get_in_tx(&mut tx, intent_id)
                .await?
                .ok_or_else(|| anyhow!("superseded permission intent disappeared"))?;
        }
        let disposition = if !authoritative {
            PermissionRecoveryDisposition::StaleScope
        } else {
            match snapshot.status {
                PermissionIntentStatus::Pending => PermissionRecoveryDisposition::AwaitingDecision,
                PermissionIntentStatus::Allowed => PermissionRecoveryDisposition::ConsumeExactAllow,
                PermissionIntentStatus::Consumed => PermissionRecoveryDisposition::AlreadyConsumed,
                PermissionIntentStatus::Denied => PermissionRecoveryDisposition::ExplicitlyDenied,
                PermissionIntentStatus::TimedOut | PermissionIntentStatus::ChannelClosed => {
                    PermissionRecoveryDisposition::ReprojectInterrupted
                }
                PermissionIntentStatus::Cancelled => {
                    PermissionRecoveryDisposition::ExplicitlyCancelled
                }
                PermissionIntentStatus::Superseded => PermissionRecoveryDisposition::StaleScope,
            }
        };
        tx.commit().await?;
        Ok(Some(PermissionIntentObservation::new(
            snapshot,
            disposition,
        )))
    }

    /// Discover the latest prompt generation for each action owned by an
    /// Objective. This is an observation API only; it never invokes the model,
    /// tool backend, or native action.
    pub(crate) async fn observe_objective(
        &self,
        objective_id: &str,
        now: i64,
    ) -> anyhow::Result<Vec<PermissionIntentObservation>> {
        validate_opaque_objective_id(objective_id)?;
        let intent_ids: Vec<String> = sqlx::query_scalar(
            "SELECT intent.intent_id
             FROM permission_intents intent
             WHERE intent.objective_id=?
               AND NOT EXISTS (
                 SELECT 1 FROM permission_intents newer
                 WHERE newer.objective_id=intent.objective_id
                   AND newer.objective_revision=intent.objective_revision
                   AND newer.binding_id=intent.binding_id
                   AND newer.resource_generation=intent.resource_generation
                   AND newer.action_signature=intent.action_signature
                   AND newer.prompt_generation>intent.prompt_generation
               )
             ORDER BY intent.updated_at, intent.intent_id",
        )
        .bind(objective_id)
        .fetch_all(&self.pool)
        .await?;
        let mut observations = Vec::with_capacity(intent_ids.len());
        for intent_id in intent_ids {
            if let Some(observation) = self.observe_exact(&intent_id, now).await? {
                observations.push(observation);
            }
        }
        Ok(observations)
    }

    /// Create one successor prompt for a timeout/channel interruption. The
    /// successor keeps the same provider call id and action signature; this API
    /// only reprojects authorization UI and cannot execute the provider/native
    /// action. A unique predecessor fence makes retries idempotent.
    pub(crate) async fn rehydrate_interrupted(
        &self,
        source_intent_id: &str,
        process_instance: &str,
        expires_at: i64,
        now: i64,
    ) -> anyhow::Result<PermissionIntentSnapshot> {
        if process_instance.trim().is_empty() || expires_at <= now {
            bail!("rehydrated permission prompt requires a live owner and future expiry");
        }
        let mut tx = self.pool.begin().await?;
        let source = get_in_tx(&mut tx, source_intent_id)
            .await?
            .ok_or_else(|| anyhow!("permission intent {source_intent_id} does not exist"))?;

        if !matches!(
            source.status,
            PermissionIntentStatus::TimedOut | PermissionIntentStatus::ChannelClosed
        ) {
            bail!(
                "permission intent {} with status {} is not rehydratable",
                source.intent_id,
                source.status.as_str()
            );
        }
        if let Some(successor) = successor_in_tx(&mut tx, source_intent_id).await? {
            validate_successor(&source, &successor)?;
            tx.commit().await?;
            return Ok(successor);
        }
        validate_scope(&mut tx, &source.scope)
            .await
            .context("interrupted permission scope is no longer authoritative")?;
        if let Some(newer) =
            latest_action_in_tx(&mut tx, &source.scope, &source.action_signature).await?
        {
            if newer.intent_id != source.intent_id {
                bail!(
                    "permission intent {} is not the latest prompt generation",
                    source.intent_id
                );
            }
        }

        let successor_id = Uuid::new_v4().to_string();
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO permission_intents
             (intent_id, objective_id, objective_revision, binding_id,
              resource_generation, session_id, provider_tool_call_id, tool_name,
              prompt_args_json, action_signature, prompt_generation,
              predecessor_intent_id, status, expires_at,
              created_process_instance, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?)",
        )
        .bind(&successor_id)
        .bind(&source.scope.objective_id)
        .bind(source.scope.objective_revision)
        .bind(&source.scope.binding_id)
        .bind(source.scope.resource_generation)
        .bind(&source.session_id)
        .bind(&source.provider_tool_call_id)
        .bind(&source.tool_name)
        .bind(serde_json::to_string(&source.prompt_args)?)
        .bind(&source.action_signature)
        .bind(source.prompt_generation + 1)
        .bind(&source.intent_id)
        .bind(expires_at)
        .bind(process_instance)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let successor = if inserted.rows_affected() == 1 {
            get_in_tx(&mut tx, &successor_id)
                .await?
                .ok_or_else(|| anyhow!("rehydrated permission intent disappeared"))?
        } else {
            successor_in_tx(&mut tx, source_intent_id)
                .await?
                .ok_or_else(|| anyhow!("permission rehydration lost its predecessor CAS"))?
        };
        validate_successor(&source, &successor)?;
        tx.commit().await?;
        Ok(successor)
    }

    /// Move one claimed Permission remediation into a durable, typed user wait
    /// and reproject the original prompt at the new authoritative Objective
    /// revision. The Objective transition, remediation settlement, and prompt
    /// insertion share one transaction, so a crash cannot leave a user wait
    /// without a recoverable prompt (or vice versa).
    pub(crate) async fn project_claimed_interruption(
        &self,
        permit: &codefactory_agent_loop::tool::MutationPermit,
        process_instance: &str,
        expires_at: i64,
        now: i64,
    ) -> anyhow::Result<PermissionIntentObservation> {
        if process_instance.trim().is_empty() || expires_at <= now {
            bail!("projected permission prompt requires a live owner and future expiry");
        }
        let binding_id = permit
            .binding_id
            .as_deref()
            .ok_or_else(|| anyhow!("Permission remediation has no exact binding"))?;
        let resource_generation = permit
            .resource_generation
            .ok_or_else(|| anyhow!("Permission remediation has no binding generation"))?;
        let mut tx = self.pool.begin().await?;
        let claim = claimed_permission_scope_in_tx(&mut tx, permit, now).await?;
        if claim.binding_id != binding_id || claim.resource_generation != resource_generation {
            bail!("Permission remediation binding changed before prompt projection");
        }

        let sources = sqlx::query(
            "SELECT intent.* FROM permission_intents intent
             WHERE intent.objective_id=? AND intent.binding_id=?
               AND intent.resource_generation=?
               AND intent.status IN ('timed_out','channel_closed')
               AND NOT EXISTS (
                 SELECT 1 FROM permission_intents successor
                 WHERE successor.predecessor_intent_id=intent.intent_id
               )
             ORDER BY intent.prompt_generation DESC, intent.updated_at DESC,
                      intent.intent_id DESC LIMIT 2",
        )
        .bind(&permit.objective_id)
        .bind(binding_id)
        .bind(resource_generation)
        .fetch_all(&mut *tx)
        .await?;
        if sources.len() != 1 {
            bail!(
                "Permission remediation requires one interrupted prompt; found {}",
                sources.len()
            );
        }
        let source = PermissionIntentSnapshot::from_row(&sources[0])?;
        let target_revision = claim.objective_revision + 1;
        let successor_id = Uuid::new_v4().to_string();
        let request_key = format!("permission:{successor_id}");

        let objective_update = sqlx::query(
            "UPDATE objectives SET
               revision=?, status='waiting_authorization',
               decision_type='authorization_required', domain='permission',
               requires_user_action=1, request_key=?, decision_key=NULL,
               action_signature=?, failure_code=NULL, failure_signature=NULL,
               recovery_owner=NULL, remediation_id=NULL,
               next_observation_at=NULL, lease_owner=NULL, lease_expires_at=NULL,
               cancellation_provenance=NULL, last_progress_at=?, updated_at=?
             WHERE id=? AND revision=? AND status='waiting_system'
               AND domain='permission' AND remediation_id=?
               AND lease_owner=? AND lease_expires_at>?",
        )
        .bind(target_revision)
        .bind(&request_key)
        .bind(&source.action_signature)
        .bind(now)
        .bind(now)
        .bind(&permit.objective_id)
        .bind(claim.objective_revision)
        .bind(&permit.remediation_id)
        .bind(&permit.owner)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if objective_update.rows_affected() != 1 {
            bail!("Permission remediation claim changed before prompt projection");
        }
        let remediation_update = sqlx::query(
            "UPDATE objective_remediations
             SET status='superseded', lease_owner=NULL, lease_expires_at=NULL,
                 last_progress_at=?, updated_at=?
             WHERE id=? AND objective_id=? AND binding_id=? AND domain='permission'
               AND status='claimed' AND lease_owner=? AND attempt_index=?",
        )
        .bind(now)
        .bind(now)
        .bind(&permit.remediation_id)
        .bind(&permit.objective_id)
        .bind(binding_id)
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .execute(&mut *tx)
        .await?;
        if remediation_update.rows_affected() != 1 {
            bail!("Permission remediation ownership changed during prompt projection");
        }

        sqlx::query(
            "INSERT INTO permission_intents
             (intent_id, objective_id, objective_revision, binding_id,
              resource_generation, session_id, provider_tool_call_id, tool_name,
              prompt_args_json, action_signature, prompt_generation,
              predecessor_intent_id, status, expires_at,
              created_process_instance, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?)",
        )
        .bind(&successor_id)
        .bind(&permit.objective_id)
        .bind(target_revision)
        .bind(binding_id)
        .bind(resource_generation)
        .bind(&source.session_id)
        .bind(&source.provider_tool_call_id)
        .bind(&source.tool_name)
        .bind(serde_json::to_string(&source.prompt_args)?)
        .bind(&source.action_signature)
        .bind(source.prompt_generation + 1)
        .bind(&source.intent_id)
        .bind(expires_at)
        .bind(process_instance)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        insert_permission_objective_audit(
            &mut tx,
            &permit.objective_id,
            target_revision,
            "waiting_authorization",
            "authorization_required",
            true,
            None,
            None,
            serde_json::json!({
                "request_key": request_key,
                "action_signature": source.action_signature,
                "intent_id": successor_id,
                "predecessor_intent_id": source.intent_id,
            }),
            now,
        )
        .await?;
        let successor = get_in_tx(&mut tx, &successor_id)
            .await?
            .ok_or_else(|| anyhow!("projected permission intent disappeared"))?;
        tx.commit().await?;
        Ok(PermissionIntentObservation::new(
            successor,
            PermissionRecoveryDisposition::AwaitingDecision,
        ))
    }

    /// Read-only classification for one claimed Permission remediation. The
    /// executor must project a prompt for transport interruptions and may call
    /// the AgentLoop only when an exact available action receipt is already
    /// bound to this claim.
    pub(crate) async fn observe_claimed_recovery(
        &self,
        permit: &codefactory_agent_loop::tool::MutationPermit,
        process_instance: &str,
        expires_at: i64,
        now: i64,
    ) -> anyhow::Result<PermissionClaimAction> {
        // A prior owner can disappear after reserving permission but before the
        // tool backend records its mutation intent. Once takeover owns a new
        // epoch, reset only when the generic side-effect ledger proves there
        // is no unresolved dispatch. A committed receipt is safe too: the
        // backend will replay its summary instead of mutating again.
        let mut reservation_tx = self.pool.begin().await?;
        claimed_permission_scope_in_tx(&mut reservation_tx, permit, now).await?;
        let reserved: Option<(String, i64, String)> = sqlx::query_as(
            "SELECT receipt.consumer_owner, receipt.consumer_claim_epoch,
                    receipt.binding_id
             FROM permission_action_receipts receipt
             WHERE receipt.objective_id=? AND receipt.remediation_id=?
               AND receipt.status='reserved'",
        )
        .bind(&permit.objective_id)
        .bind(&permit.remediation_id)
        .fetch_optional(&mut *reservation_tx)
        .await?;
        if let Some((reserved_owner, reserved_epoch, binding_id)) = reserved {
            if reserved_owner == permit.owner && reserved_epoch == permit.claim_epoch {
                bail!("Permission allow receipt is already reserved by this claim");
            }
            // Permission signatures hash the prompt surface; the backend
            // fingerprint additionally binds cwd and resource identity. Do
            // not compare those distinct digests. Any unresolved mutation on
            // this exact Objective binding keeps takeover observe-only.
            let unresolved: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM side_effect_receipts
                 WHERE objective_id=? AND binding_id=?
                   AND status IN ('started','unknown')",
            )
            .bind(&permit.objective_id)
            .bind(&binding_id)
            .fetch_one(&mut *reservation_tx)
            .await?;
            if unresolved > 0 {
                bail!("authorized action has unresolved external state");
            }
            sqlx::query(
                "UPDATE permission_action_receipts
                 SET status='available', consumer_owner=NULL,
                     consumer_claim_epoch=NULL, consumed_at=NULL, updated_at=?
                 WHERE objective_id=? AND remediation_id=? AND status='reserved'
                   AND consumer_owner=? AND consumer_claim_epoch=?",
            )
            .bind(now)
            .bind(&permit.objective_id)
            .bind(&permit.remediation_id)
            .bind(&reserved_owner)
            .bind(reserved_epoch)
            .execute(&mut *reservation_tx)
            .await?;
        }
        reservation_tx.commit().await?;
        let available_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM permission_action_receipts receipt
             JOIN objectives objective ON objective.id=receipt.objective_id
             JOIN objective_remediations remediation
               ON remediation.id=receipt.remediation_id
              AND remediation.objective_id=receipt.objective_id
             WHERE receipt.objective_id=? AND receipt.remediation_id=?
               AND receipt.objective_revision=objective.revision
               AND receipt.status='available'
               AND objective.status='waiting_system' AND objective.domain='permission'
               AND objective.lease_owner=? AND objective.lease_expires_at>?
               AND remediation.status='claimed' AND remediation.lease_owner=?
               AND remediation.attempt_index=? AND remediation.lease_expires_at>?
               AND receipt.binding_id=? AND receipt.resource_generation=?",
        )
        .bind(&permit.objective_id)
        .bind(&permit.remediation_id)
        .bind(&permit.owner)
        .bind(now)
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .bind(permit.binding_id.as_deref().unwrap_or(""))
        .bind(permit.resource_generation.unwrap_or_default())
        .fetch_one(&self.pool)
        .await?;
        match available_receipts {
            1 => Ok(PermissionClaimAction::ResumeAuthorizedAction),
            count if count > 1 => bail!("Permission recovery has duplicate available receipts"),
            _ => {
                // A response can win its durable CAS immediately before the
                // process-local receiver disappears. On restart, adopt that
                // exact allow into the newly-claimed remediation instead of
                // prompting again or replaying the provider call.
                let binding_id = permit
                    .binding_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("Permission remediation has no exact binding"))?;
                let resource_generation = permit
                    .resource_generation
                    .ok_or_else(|| anyhow!("Permission remediation has no binding generation"))?;
                let orphaned = sqlx::query(
                    "SELECT intent.* FROM permission_intents intent
                     WHERE intent.objective_id=? AND intent.binding_id=?
                       AND intent.resource_generation=?
                       AND intent.action_signature=COALESCE(
                         (SELECT objective.action_signature FROM objectives objective
                          WHERE objective.id=?), intent.action_signature
                       )
                       AND intent.status IN ('allowed','consumed','denied')
                       AND NOT EXISTS (
                         SELECT 1 FROM permission_intents successor
                         WHERE successor.predecessor_intent_id=intent.intent_id
                       )
                     ORDER BY intent.prompt_generation DESC, intent.updated_at DESC,
                              intent.intent_id DESC LIMIT 2",
                )
                .bind(&permit.objective_id)
                .bind(binding_id)
                .bind(resource_generation)
                .bind(&permit.objective_id)
                .fetch_all(&self.pool)
                .await?;
                if orphaned.len() > 1 {
                    bail!("Permission recovery has ambiguous orphaned decisions");
                }
                if let Some(row) = orphaned.first() {
                    let source = PermissionIntentSnapshot::from_row(row)?;
                    return match source.status {
                        PermissionIntentStatus::Allowed | PermissionIntentStatus::Consumed => {
                            self.adopt_orphaned_allow(permit, &source, now).await?;
                            Ok(PermissionClaimAction::ResumeAuthorizedAction)
                        }
                        PermissionIntentStatus::Denied => {
                            bail!("Permission action was explicitly denied")
                        }
                        _ => unreachable!("orphaned response query is status constrained"),
                    };
                }
                self.project_claimed_interruption(permit, process_instance, expires_at, now)
                    .await
                    .map(PermissionClaimAction::ProjectPrompt)
            }
        }
    }

    async fn adopt_orphaned_allow(
        &self,
        permit: &codefactory_agent_loop::tool::MutationPermit,
        source: &PermissionIntentSnapshot,
        now: i64,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        let claim = claimed_permission_scope_in_tx(&mut tx, permit, now).await?;
        if source.scope.objective_id != permit.objective_id
            || source.scope.binding_id != claim.binding_id
            || source.scope.resource_generation != claim.resource_generation
            || claim
                .action_signature
                .as_deref()
                .is_some_and(|signature| signature != source.action_signature.as_str())
        {
            bail!("orphaned permission allow does not match the live claim scope");
        }
        let current = get_in_tx(&mut tx, &source.intent_id)
            .await?
            .ok_or_else(|| anyhow!("orphaned permission allow disappeared"))?;
        if current != *source
            || !matches!(
                current.status,
                PermissionIntentStatus::Allowed | PermissionIntentStatus::Consumed
            )
        {
            bail!("orphaned permission allow changed before adoption");
        }
        if current.status == PermissionIntentStatus::Consumed {
            // See the takeover check above: backend fingerprints are not
            // permission prompt signatures, so binding scope is the safe
            // common identity fence.
            let unresolved: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM side_effect_receipts
                 WHERE objective_id=? AND binding_id=?
                   AND status IN ('started','unknown')",
            )
            .bind(&permit.objective_id)
            .bind(&claim.binding_id)
            .fetch_one(&mut *tx)
            .await?;
            if unresolved > 0 {
                bail!("authorized action has unresolved external state");
            }
        }
        let receipt_id = Uuid::new_v4().to_string();
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO permission_action_receipts
             (receipt_id, intent_id, objective_id, objective_revision,
              remediation_id, binding_id, resource_generation,
              action_signature, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'available', ?, ?)",
        )
        .bind(&receipt_id)
        .bind(&current.intent_id)
        .bind(&permit.objective_id)
        .bind(claim.objective_revision)
        .bind(&permit.remediation_id)
        .bind(&claim.binding_id)
        .bind(claim.resource_generation)
        .bind(&current.action_signature)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            let exact: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM permission_action_receipts
                 WHERE intent_id=? AND objective_id=? AND objective_revision=?
                   AND remediation_id=? AND binding_id=? AND resource_generation=?
                   AND action_signature=? AND status='available'",
            )
            .bind(&current.intent_id)
            .bind(&permit.objective_id)
            .bind(claim.objective_revision)
            .bind(&permit.remediation_id)
            .bind(&claim.binding_id)
            .bind(claim.resource_generation)
            .bind(&current.action_signature)
            .fetch_one(&mut *tx)
            .await?;
            if exact != 1 {
                bail!("orphaned permission allow conflicts with an existing receipt");
            }
        }
        if current.status == PermissionIntentStatus::Allowed {
            let consumed = sqlx::query(
                "UPDATE permission_intents SET status='consumed', consumed_at=?, updated_at=?
                 WHERE intent_id=? AND status='allowed'",
            )
            .bind(now)
            .bind(now)
            .bind(&current.intent_id)
            .execute(&mut *tx)
            .await?;
            if consumed.rows_affected() != 1 {
                bail!("orphaned permission allow lost its consume CAS");
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Settle a prompt that has no live process-local receiver. An allow is
    /// converted atomically into one exact execution receipt plus a queued
    /// Permission remediation. A deny terminally cancels the same Objective.
    pub(crate) async fn settle_projected_response(
        &self,
        intent_id: &str,
        response: PermissionPromptResponse,
        now: i64,
    ) -> anyhow::Result<ProjectedPermissionSettlement> {
        let mut tx = self.pool.begin().await?;
        let intent = get_in_tx(&mut tx, intent_id)
            .await?
            .ok_or_else(|| anyhow!("permission intent {intent_id} does not exist"))?;

        if intent.status != PermissionIntentStatus::Pending {
            let same_terminal = matches!(
                (intent.status, response),
                (
                    PermissionIntentStatus::Denied,
                    PermissionPromptResponse::Deny
                ) | (
                    PermissionIntentStatus::Consumed,
                    PermissionPromptResponse::Allow
                )
            );
            if !same_terminal {
                bail!("late or conflicting permission response was rejected");
            }
            let receipt: Option<(i64, String)> = sqlx::query_as(
                "SELECT objective_revision, remediation_id
                 FROM permission_action_receipts WHERE intent_id=?",
            )
            .bind(intent_id)
            .fetch_optional(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(ProjectedPermissionSettlement {
                objective_id: intent.scope.objective_id,
                objective_revision: receipt
                    .as_ref()
                    .map_or(intent.scope.objective_revision + 1, |row| row.0),
                recovery_scheduled: receipt.is_some(),
                remediation_id: receipt.map(|row| row.1),
            });
        }
        if intent.expires_at <= now {
            expire_exact_if_due(&mut tx, &intent.prompt_key(), now).await?;
            tx.commit().await?;
            bail!("late permission response arrived after prompt expiry");
        }
        validate_scope(&mut tx, &intent.scope).await?;
        let request_key = format!("permission:{intent_id}");
        let objective: Option<(String, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT status, resume_cursor, output_started, side_effect_started
             FROM objectives
             WHERE id=? AND revision=? AND status='waiting_authorization'
               AND decision_type='authorization_required' AND domain='permission'
               AND request_key=? AND action_signature=?",
        )
        .bind(&intent.scope.objective_id)
        .bind(intent.scope.objective_revision)
        .bind(&request_key)
        .bind(&intent.action_signature)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((_status, resume_cursor, output_started, side_effect_started)) = objective else {
            bail!("permission response no longer owns the waiting Objective revision");
        };
        let target_revision = intent.scope.objective_revision + 1;

        match response {
            PermissionPromptResponse::Deny => {
                let denied = update_pending_intent_response(
                    &mut tx,
                    &intent,
                    PermissionIntentStatus::Denied,
                    Some("permission_denied_by_user"),
                    now,
                )
                .await?;
                if denied != 1 {
                    bail!("permission response lost its prompt CAS");
                }
                let updated = sqlx::query(
                    "UPDATE objectives SET revision=?, status='cancelled',
                       decision_type='cancelled', domain='permission',
                       requires_user_action=0, request_key=NULL, decision_key=NULL,
                       action_signature=NULL, failure_code=NULL, failure_signature=NULL,
                       recovery_owner=NULL, remediation_id=NULL,
                       next_observation_at=NULL, lease_owner=NULL, lease_expires_at=NULL,
                       cancellation_provenance='explicit_deny', last_progress_at=?,
                       updated_at=?, completed_at=?
                     WHERE id=? AND revision=? AND status='waiting_authorization'
                       AND request_key=?",
                )
                .bind(target_revision)
                .bind(now)
                .bind(now)
                .bind(now)
                .bind(&intent.scope.objective_id)
                .bind(intent.scope.objective_revision)
                .bind(&request_key)
                .execute(&mut *tx)
                .await?;
                if updated.rows_affected() != 1 {
                    bail!("permission denial lost its Objective CAS");
                }
                insert_permission_objective_audit(
                    &mut tx,
                    &intent.scope.objective_id,
                    target_revision,
                    "cancelled",
                    "cancelled",
                    false,
                    None,
                    None,
                    serde_json::json!({
                        "intent_id": intent_id,
                        "cancellation_provenance": "explicit_deny",
                        "output_started": output_started,
                        "side_effect_started": side_effect_started,
                    }),
                    now,
                )
                .await?;
                tx.commit().await?;
                Ok(ProjectedPermissionSettlement {
                    objective_id: intent.scope.objective_id,
                    objective_revision: target_revision,
                    recovery_scheduled: false,
                    remediation_id: None,
                })
            }
            PermissionPromptResponse::Allow => {
                let allowed = update_pending_intent_response(
                    &mut tx,
                    &intent,
                    PermissionIntentStatus::Allowed,
                    None,
                    now,
                )
                .await?;
                if allowed != 1 {
                    bail!("permission response lost its prompt CAS");
                }
                let consumed = sqlx::query(
                    "UPDATE permission_intents
                     SET status='consumed', consumed_at=?, updated_at=?
                     WHERE intent_id=? AND status='allowed'",
                )
                .bind(now)
                .bind(now)
                .bind(intent_id)
                .execute(&mut *tx)
                .await?;
                if consumed.rows_affected() != 1 {
                    bail!("permission allow could not transfer into an execution receipt");
                }
                let remediation_id = Uuid::new_v4().to_string();
                let receipt_id = Uuid::new_v4().to_string();
                let failure_signature = format!("permission:{intent_id}:authorized");
                let updated = sqlx::query(
                    "UPDATE objectives SET revision=?, status='waiting_system',
                       decision_type='apply_recommended', domain='permission',
                       requires_user_action=0, request_key=NULL, decision_key=NULL,
                       action_signature=?, failure_code='authorization_restored',
                       failure_signature=?, recovery_owner='objective-supervisor:permission',
                       remediation_id=?, next_observation_at=?,
                       lease_owner=NULL, lease_expires_at=NULL,
                       cancellation_provenance=NULL, last_progress_at=?, updated_at=?,
                       completed_at=NULL
                     WHERE id=? AND revision=? AND status='waiting_authorization'
                       AND request_key=?",
                )
                .bind(target_revision)
                .bind(&intent.action_signature)
                .bind(&failure_signature)
                .bind(&remediation_id)
                .bind(now)
                .bind(now)
                .bind(now)
                .bind(&intent.scope.objective_id)
                .bind(intent.scope.objective_revision)
                .bind(&request_key)
                .execute(&mut *tx)
                .await?;
                if updated.rows_affected() != 1 {
                    bail!("permission allow lost its Objective CAS");
                }
                sqlx::query(
                    "INSERT INTO objective_remediations
                     (id, objective_id, binding_id, domain, status, failure_code,
                      failure_signature, strategy, approach_index, attempt_index,
                      action_fingerprint, resume_cursor, next_observation_at,
                      created_at, updated_at)
                     VALUES (?, ?, ?, 'permission', 'queued',
                             'authorization_restored', ?, 'resume_authorized_action',
                             0, 0, ?, ?, ?, ?, ?)",
                )
                .bind(&remediation_id)
                .bind(&intent.scope.objective_id)
                .bind(&intent.scope.binding_id)
                .bind(&failure_signature)
                .bind(&intent.action_signature)
                .bind(&resume_cursor)
                .bind(now)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO permission_action_receipts
                     (receipt_id, intent_id, objective_id, objective_revision,
                      remediation_id, binding_id, resource_generation,
                      action_signature, status, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'available', ?, ?)",
                )
                .bind(&receipt_id)
                .bind(intent_id)
                .bind(&intent.scope.objective_id)
                .bind(target_revision)
                .bind(&remediation_id)
                .bind(&intent.scope.binding_id)
                .bind(intent.scope.resource_generation)
                .bind(&intent.action_signature)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                insert_permission_objective_audit(
                    &mut tx,
                    &intent.scope.objective_id,
                    target_revision,
                    "waiting_system",
                    "apply_recommended",
                    false,
                    Some("authorization_restored"),
                    Some(&remediation_id),
                    serde_json::json!({
                        "intent_id": intent_id,
                        "receipt_id": receipt_id,
                        "action_signature": intent.action_signature,
                        "resume_cursor": resume_cursor,
                        "output_started": output_started,
                        "side_effect_started": side_effect_started,
                    }),
                    now,
                )
                .await?;
                tx.commit().await?;
                Ok(ProjectedPermissionSettlement {
                    objective_id: intent.scope.objective_id,
                    objective_revision: target_revision,
                    recovery_scheduled: true,
                    remediation_id: Some(remediation_id),
                })
            }
        }
    }

    /// Reserve the one durable allow receipt that matches the currently-live
    /// recovery claim. Reservation is the authorization boundary: a duplicate,
    /// late response, stale revision, changed binding, or reclaimed epoch all
    /// return false and cannot reach the native tool backend.
    pub(crate) async fn reserve_exact_recovery_allow(
        &self,
        scope: &PermissionScope,
        action_signature: &str,
        permit: &codefactory_agent_loop::tool::MutationPermit,
        now: i64,
    ) -> anyhow::Result<bool> {
        if permit.objective_id != scope.objective_id
            || permit.binding_id.as_deref() != Some(scope.binding_id.as_str())
            || permit.resource_generation != Some(scope.resource_generation)
        {
            return Ok(false);
        }
        let updated = sqlx::query(
            "UPDATE permission_action_receipts
             SET status='reserved', consumer_owner=?, consumer_claim_epoch=?,
                 consumed_at=?, updated_at=?
             WHERE objective_id=? AND objective_revision=? AND remediation_id=?
               AND binding_id=? AND resource_generation=? AND action_signature=?
               AND status='available'
               AND EXISTS (
                 SELECT 1 FROM objectives objective
                 WHERE objective.id=permission_action_receipts.objective_id
                   AND objective.revision=permission_action_receipts.objective_revision
                   AND objective.status='waiting_system'
                   AND objective.domain='permission'
                   AND objective.remediation_id=permission_action_receipts.remediation_id
                   AND objective.lease_owner=? AND objective.lease_expires_at>?
               )
               AND EXISTS (
                 SELECT 1 FROM objective_remediations remediation
                 WHERE remediation.id=permission_action_receipts.remediation_id
                   AND remediation.objective_id=permission_action_receipts.objective_id
                   AND remediation.binding_id=permission_action_receipts.binding_id
                   AND remediation.status='claimed' AND remediation.lease_owner=?
                   AND remediation.attempt_index=? AND remediation.lease_expires_at>?
               )
               AND EXISTS (
                 SELECT 1 FROM objective_bindings binding
                 WHERE binding.id=permission_action_receipts.binding_id
                   AND binding.objective_id=permission_action_receipts.objective_id
                   AND binding.resource_generation=permission_action_receipts.resource_generation
               )",
        )
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .bind(now)
        .bind(&scope.objective_id)
        .bind(scope.objective_revision)
        .bind(&permit.remediation_id)
        .bind(&scope.binding_id)
        .bind(scope.resource_generation)
        .bind(action_signature)
        .bind(&permit.owner)
        .bind(now)
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn get(
        &self,
        intent_id: &str,
    ) -> anyhow::Result<Option<PermissionIntentSnapshot>> {
        let row = sqlx::query("SELECT * FROM permission_intents WHERE intent_id=?")
            .bind(intent_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| PermissionIntentSnapshot::from_row(&row))
            .transpose()
    }

    /// Persist an exact user decision. A late answer loses to expiry and may
    /// not revive an old prompt generation.
    pub(crate) async fn record_user_response(
        &self,
        key: &PermissionPromptKey,
        response: PermissionPromptResponse,
        now: i64,
    ) -> anyhow::Result<PermissionIntentSnapshot> {
        let mut tx = self.pool.begin().await?;
        expire_exact_if_due(&mut tx, key, now).await?;
        let scope = PermissionScope {
            objective_id: key.objective_id.clone(),
            objective_revision: key.objective_revision,
            binding_id: key.binding_id.clone(),
            resource_generation: key.resource_generation,
        };
        if let Err(scope_error) = validate_scope(&mut tx, &scope).await {
            supersede_exact(&mut tx, key, now).await?;
            tx.commit().await?;
            return Err(scope_error.context("permission response scope is no longer authoritative"));
        }
        let (status, failure_code) = match response {
            PermissionPromptResponse::Allow => ("allowed", None),
            PermissionPromptResponse::Deny => ("denied", Some("permission_denied_by_user")),
        };
        let updated = sqlx::query(
            "UPDATE permission_intents
             SET status=?, failure_code=?, decided_at=?, updated_at=?
             WHERE intent_id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=? AND action_signature=?
               AND prompt_generation=? AND status='pending' AND expires_at>?",
        )
        .bind(status)
        .bind(failure_code)
        .bind(now)
        .bind(now)
        .bind(&key.intent_id)
        .bind(&key.objective_id)
        .bind(key.objective_revision)
        .bind(&key.binding_id)
        .bind(key.resource_generation)
        .bind(&key.action_signature)
        .bind(key.prompt_generation)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            let current = get_in_tx(&mut tx, &key.intent_id).await?;
            let replayed_same_decision = current.as_ref().is_some_and(|snapshot| {
                snapshot.prompt_key() == *key
                    && matches!(
                        (response, snapshot.status),
                        (
                            PermissionPromptResponse::Allow,
                            PermissionIntentStatus::Allowed
                        ) | (
                            PermissionPromptResponse::Allow,
                            PermissionIntentStatus::Consumed
                        ) | (
                            PermissionPromptResponse::Deny,
                            PermissionIntentStatus::Denied
                        )
                    )
            });
            tx.commit().await?;
            if replayed_same_decision {
                return current.ok_or_else(|| anyhow!("permission decision disappeared"));
            }
            return Err(anyhow!(match current {
                Some(snapshot) => format!(
                    "permission intent {} is not pending for this exact scope (status {})",
                    key.intent_id,
                    snapshot.status.as_str()
                ),
                None => format!("permission intent {} does not exist", key.intent_id),
            }));
        }
        let snapshot = get_in_tx(&mut tx, &key.intent_id)
            .await?
            .ok_or_else(|| anyhow!("updated permission intent disappeared"))?;
        tx.commit().await?;
        Ok(snapshot)
    }

    /// Convert a transport interruption into durable system-owned evidence.
    /// This CAS never overwrites a user decision that won the race.
    pub(crate) async fn record_interruption(
        &self,
        key: &PermissionPromptKey,
        status: PermissionIntentStatus,
        now: i64,
    ) -> anyhow::Result<PermissionIntentSnapshot> {
        let failure_code = match status {
            PermissionIntentStatus::TimedOut => "permission_timed_out",
            PermissionIntentStatus::ChannelClosed => "permission_channel_closed",
            PermissionIntentStatus::Cancelled => "permission_cancelled",
            PermissionIntentStatus::Superseded => "permission_scope_stale",
            _ => bail!("{status:?} is not an interruption status"),
        };
        let updated = sqlx::query(
            "UPDATE permission_intents
             SET status=?, failure_code=?, decided_at=?, updated_at=?
             WHERE intent_id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=? AND action_signature=?
               AND prompt_generation=? AND status='pending'",
        )
        .bind(status.as_str())
        .bind(failure_code)
        .bind(now)
        .bind(now)
        .bind(&key.intent_id)
        .bind(&key.objective_id)
        .bind(key.objective_revision)
        .bind(&key.binding_id)
        .bind(key.resource_generation)
        .bind(&key.action_signature)
        .bind(key.prompt_generation)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            let current = self.get(&key.intent_id).await?;
            if current
                .as_ref()
                .is_some_and(|snapshot| snapshot.prompt_key() == *key && snapshot.status == status)
            {
                return current.ok_or_else(|| anyhow!("permission interruption disappeared"));
            }
            bail!(
                "permission intent {} is not pending for this exact scope",
                key.intent_id
            );
        }
        self.get(&key.intent_id)
            .await?
            .ok_or_else(|| anyhow!("interrupted permission intent disappeared"))
    }

    /// Consume an allow exactly once and only for the action that was shown.
    pub(crate) async fn consume_exact_allow(
        &self,
        key: &PermissionPromptKey,
        now: i64,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let scope = PermissionScope {
            objective_id: key.objective_id.clone(),
            objective_revision: key.objective_revision,
            binding_id: key.binding_id.clone(),
            resource_generation: key.resource_generation,
        };
        if validate_scope(&mut tx, &scope).await.is_err() {
            supersede_exact(&mut tx, key, now).await?;
            tx.commit().await?;
            return Ok(false);
        }
        let updated = sqlx::query(
            "UPDATE permission_intents
             SET status='consumed', consumed_at=?, updated_at=?
             WHERE intent_id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=? AND action_signature=?
               AND prompt_generation=? AND status='allowed'",
        )
        .bind(now)
        .bind(now)
        .bind(&key.intent_id)
        .bind(&key.objective_id)
        .bind(key.objective_revision)
        .bind(&key.binding_id)
        .bind(key.resource_generation)
        .bind(&key.action_signature)
        .bind(key.prompt_generation)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn expire_due(&self, now: i64) -> anyhow::Result<u64> {
        let updated = sqlx::query(
            "UPDATE permission_intents
             SET status='timed_out', failure_code='permission_timed_out',
                 decided_at=?, updated_at=?
             WHERE status='pending' AND expires_at<=?",
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected())
    }

    pub(crate) async fn close_process_channels(
        &self,
        process_instance: &str,
        now: i64,
    ) -> anyhow::Result<u64> {
        let updated = sqlx::query(
            "UPDATE permission_intents
             SET status='channel_closed', failure_code='permission_channel_closed',
                 decided_at=?, updated_at=?
             WHERE status='pending' AND created_process_instance=?",
        )
        .bind(now)
        .bind(now)
        .bind(process_instance)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected())
    }

    /// The durable response may win just before its process-local oneshot
    /// receiver disappears. Adopt that exact response immediately so the user
    /// never has to restart or answer again. This method does not execute the
    /// action: an allow only queues Permission reconciliation, where the exact
    /// action receipt is minted and reserved once.
    pub(crate) async fn reconcile_orphaned_response(
        &self,
        intent_id: &str,
        now: i64,
    ) -> anyhow::Result<ProjectedPermissionSettlement> {
        let intent = self
            .get(intent_id)
            .await?
            .ok_or_else(|| anyhow!("permission intent {intent_id} does not exist"))?;
        if !matches!(
            intent.status,
            PermissionIntentStatus::Allowed
                | PermissionIntentStatus::Consumed
                | PermissionIntentStatus::Denied
        ) {
            bail!("permission response was not durably decided before channel close");
        }
        let objective_store = super::objective::ObjectiveStore::new(self.pool.clone());
        let objective = objective_store
            .get(&intent.scope.objective_id)
            .await?
            .ok_or_else(|| anyhow!("permission Objective disappeared"))?;

        // An identical retry after the first CAS is idempotent. Anything else
        // is a late response to stale authority and must fail closed.
        if objective.revision != intent.scope.objective_revision {
            let same_denial = intent.status == PermissionIntentStatus::Denied
                && objective.status == super::objective::ObjectiveStatus::Cancelled
                && objective.cancellation_provenance.as_deref() == Some("explicit_deny");
            let same_allow = matches!(
                intent.status,
                PermissionIntentStatus::Allowed | PermissionIntentStatus::Consumed
            ) && objective.status
                == super::objective::ObjectiveStatus::WaitingSystem
                && objective.domain == super::objective::RecoveryDomain::Permission
                && objective.action_signature.as_deref() == Some(intent.action_signature.as_str())
                && objective.remediation_id.is_some();
            if same_denial || same_allow {
                return Ok(ProjectedPermissionSettlement {
                    objective_id: objective.id,
                    objective_revision: objective.revision,
                    recovery_scheduled: same_allow,
                    remediation_id: objective.remediation_id,
                });
            }
            bail!("late permission response no longer owns the Objective revision");
        }
        if objective.status != super::objective::ObjectiveStatus::Active {
            bail!("orphaned permission response does not own an active Objective");
        }

        let mut decision = if intent.status == PermissionIntentStatus::Denied {
            super::objective::DecisionRouter::route(
                &objective,
                super::objective::RouteSignal::Cancelled {
                    domain: super::objective::RecoveryDomain::Permission,
                    provenance: "explicit_deny".into(),
                },
            )?
        } else {
            super::objective::DecisionRouter::route(
                &objective,
                super::objective::RouteSignal::TechnicalFailure {
                    domain: super::objective::RecoveryDomain::Permission,
                    failure_code: "permission_response_channel_closed".into(),
                    failure_signature: format!(
                        "permission:{}:response_receiver_closed",
                        intent.intent_id
                    ),
                    next_observation_at: now,
                    resume_cursor: objective.root_turn_id.clone().or(objective.task_id.clone()),
                },
            )?
        };
        if intent.status != PermissionIntentStatus::Denied {
            decision.action_signature = Some(intent.action_signature);
        }
        let updated = objective_store
            .apply_decision(objective.revision, decision)
            .await?;
        Ok(ProjectedPermissionSettlement {
            objective_id: updated.id,
            objective_revision: updated.revision,
            recovery_scheduled: updated.status == super::objective::ObjectiveStatus::WaitingSystem,
            remediation_id: updated.remediation_id,
        })
    }

    /// Adopt permission prompts whose process-local response channel vanished
    /// across an application restart. This runs before the generic active-
    /// Objective reconciliation so a permission interruption cannot be
    /// misclassified as a fresh Chat retry.
    ///
    /// The intent transition is durable first. Each Objective transition then
    /// uses its normal revision CAS, making a repeated startup idempotent. An
    /// explicit denial remains terminal; every other orphaned transport state
    /// becomes a typed Permission remediation for read-only reconciliation.
    pub(crate) async fn reconcile_stale_process_channels(
        &self,
        current_process_instance: &str,
        now: i64,
    ) -> anyhow::Result<usize> {
        if current_process_instance.trim().is_empty() {
            bail!("permission restart reconciliation requires a process identity");
        }
        sqlx::query(
            "UPDATE permission_intents
             SET status='channel_closed', failure_code='permission_channel_closed',
                 decided_at=?, updated_at=?
             WHERE status='pending' AND created_process_instance<>?",
        )
        .bind(now)
        .bind(now)
        .bind(current_process_instance)
        .execute(&self.pool)
        .await?;

        // More than one unchained action at one authoritative revision is an
        // identity ambiguity, never a reason to pick whichever row happens to
        // sort first. Keep the Objective system-owned and let the adapter
        // observe/fail closed without dispatching a tool.
        let rows = sqlx::query(
            "SELECT intent.objective_id, COUNT(*) AS candidate_count,
                    MIN(intent.status) AS only_status,
                    MIN(intent.intent_id) AS only_intent_id,
                    MIN(intent.action_signature) AS only_action_signature
             FROM permission_intents intent
             JOIN objectives objective ON objective.id=intent.objective_id
             WHERE intent.created_process_instance<>?
               AND intent.status IN ('channel_closed','allowed','consumed','denied')
               AND intent.objective_revision=objective.revision
               AND objective.status IN ('active','waiting_authorization')
               AND NOT EXISTS (
                 SELECT 1 FROM permission_intents successor
                 WHERE successor.predecessor_intent_id=intent.intent_id
               )
             GROUP BY intent.objective_id
             ORDER BY MIN(intent.created_at), intent.objective_id",
        )
        .bind(current_process_instance)
        .fetch_all(&self.pool)
        .await?;

        let objective_store = super::objective::ObjectiveStore::new(self.pool.clone());
        let mut reconciled = 0;
        for row in rows {
            let objective_id: String = row.try_get("objective_id")?;
            let candidate_count: i64 = row.try_get("candidate_count")?;
            let status =
                PermissionIntentStatus::parse(row.try_get::<String, _>("only_status")?.as_str())?;
            let intent_id: String = row.try_get("only_intent_id")?;
            let action_signature: String = row.try_get("only_action_signature")?;
            let Some(objective) = objective_store.get(&objective_id).await? else {
                continue;
            };
            let mut decision = if candidate_count == 1 && status == PermissionIntentStatus::Denied {
                super::objective::DecisionRouter::route(
                    &objective,
                    super::objective::RouteSignal::Cancelled {
                        domain: super::objective::RecoveryDomain::Permission,
                        provenance: "explicit_deny".into(),
                    },
                )?
            } else {
                let failure_code = if candidate_count == 1 {
                    match status {
                        PermissionIntentStatus::Allowed | PermissionIntentStatus::Consumed => {
                            "permission_response_channel_closed"
                        }
                        PermissionIntentStatus::ChannelClosed => "permission_channel_closed",
                        // The query excludes these states. Keeping a closed
                        // match makes future schema changes fail safe.
                        _ => "permission_identity_ambiguous",
                    }
                } else {
                    "permission_identity_ambiguous"
                };
                super::objective::DecisionRouter::route(
                    &objective,
                    super::objective::RouteSignal::TechnicalFailure {
                        domain: super::objective::RecoveryDomain::Permission,
                        failure_code: failure_code.into(),
                        failure_signature: format!(
                            "{}:{}:{}:{}",
                            objective.id, intent_id, candidate_count, current_process_instance
                        ),
                        next_observation_at: now,
                        resume_cursor: objective.root_turn_id.clone().or(objective.task_id.clone()),
                    },
                )?
            };
            if candidate_count == 1 && !matches!(status, PermissionIntentStatus::Denied) {
                decision.action_signature = Some(action_signature);
            }
            match objective_store
                .apply_decision(objective.revision, decision)
                .await
            {
                Ok(_) => reconciled += 1,
                Err(error) if error.to_string().contains("revision") => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(reconciled)
    }
}

struct ClaimedPermissionScope {
    objective_revision: i64,
    binding_id: String,
    resource_generation: i64,
    action_signature: Option<String>,
}

async fn claimed_permission_scope_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    permit: &codefactory_agent_loop::tool::MutationPermit,
    now: i64,
) -> anyhow::Result<ClaimedPermissionScope> {
    let row: Option<(i64, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT objective.revision, remediation.binding_id,
                binding.resource_generation, objective.action_signature
         FROM objectives objective
         JOIN objective_remediations remediation
           ON remediation.id=objective.remediation_id
          AND remediation.objective_id=objective.id
         JOIN objective_bindings binding
           ON binding.id=remediation.binding_id
          AND binding.objective_id=objective.id
         WHERE objective.id=? AND objective.status='waiting_system'
           AND objective.domain='permission' AND objective.remediation_id=?
           AND objective.lease_owner=? AND objective.lease_expires_at>?
           AND remediation.status='claimed' AND remediation.lease_owner=?
           AND remediation.attempt_index=? AND remediation.lease_expires_at>?
           AND remediation.binding_id=? AND binding.resource_generation=?",
    )
    .bind(&permit.objective_id)
    .bind(&permit.remediation_id)
    .bind(&permit.owner)
    .bind(now)
    .bind(&permit.owner)
    .bind(permit.claim_epoch)
    .bind(now)
    .bind(permit.binding_id.as_deref().unwrap_or(""))
    .bind(permit.resource_generation.unwrap_or_default())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((objective_revision, binding_id, resource_generation, action_signature)) = row else {
        bail!("Permission remediation mutation permit is stale");
    };
    Ok(ClaimedPermissionScope {
        objective_revision,
        binding_id,
        resource_generation,
        action_signature,
    })
}

async fn update_pending_intent_response(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    intent: &PermissionIntentSnapshot,
    status: PermissionIntentStatus,
    failure_code: Option<&str>,
    now: i64,
) -> anyhow::Result<u64> {
    let updated = sqlx::query(
        "UPDATE permission_intents
         SET status=?, failure_code=?, decided_at=?, updated_at=?
         WHERE intent_id=? AND objective_id=? AND objective_revision=?
           AND binding_id=? AND resource_generation=? AND action_signature=?
           AND prompt_generation=? AND status='pending' AND expires_at>?",
    )
    .bind(status.as_str())
    .bind(failure_code)
    .bind(now)
    .bind(now)
    .bind(&intent.intent_id)
    .bind(&intent.scope.objective_id)
    .bind(intent.scope.objective_revision)
    .bind(&intent.scope.binding_id)
    .bind(intent.scope.resource_generation)
    .bind(&intent.action_signature)
    .bind(intent.prompt_generation)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(updated.rows_affected())
}

#[allow(clippy::too_many_arguments)]
async fn insert_permission_objective_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    objective_id: &str,
    revision: i64,
    status: &str,
    decision_type: &str,
    requires_user_action: bool,
    failure_code: Option<&str>,
    remediation_id: Option<&str>,
    detail: Value,
    now: i64,
) -> anyhow::Result<()> {
    let recovery_owner = remediation_id.map(|_| "objective-supervisor:permission");
    let envelope_json = serde_json::json!({
        "objective_id": objective_id,
        "revision": revision,
        "status": status,
        "decision_type": decision_type,
        "domain": "permission",
        "requires_user_action": requires_user_action,
        "failure_code": failure_code,
        "recovery_owner": recovery_owner,
        "remediation_id": remediation_id,
        "detail": detail.clone(),
    })
    .to_string();
    sqlx::query(
        "INSERT INTO objective_decisions
         (id, objective_id, revision, domain, decision_type, failure_code,
          failure_signature, recovery_owner, remediation_id,
          requires_user_action, output_started, side_effect_started,
          envelope_json, evidence_ref, created_at)
         SELECT ?, id, revision, 'permission', ?, ?, failure_signature, ?, ?,
                ?, output_started, side_effect_started, ?, NULL, ?
         FROM objectives WHERE id=? AND revision=?",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(decision_type)
    .bind(failure_code)
    .bind(recovery_owner)
    .bind(remediation_id)
    .bind(i64::from(requires_user_action))
    .bind(envelope_json)
    .bind(now)
    .bind(objective_id)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO objective_events
         (id, objective_id, revision, event_type, status, decision_type,
          domain, failure_code, recovery_owner, detail_json, created_at)
         VALUES (?, ?, ?, 'decision_applied', ?, ?, 'permission', ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(objective_id)
    .bind(revision)
    .bind(status)
    .bind(decision_type)
    .bind(failure_code)
    .bind(recovery_owner)
    .bind(detail.to_string())
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_request(request: &PermissionIntentRequest, now: i64) -> anyhow::Result<()> {
    validate_opaque_objective_id(&request.scope.objective_id)?;
    if request.scope.objective_revision < 1 || request.scope.resource_generation < 1 {
        bail!("permission intent scope generations must be positive");
    }
    if request.scope.binding_id.trim().is_empty()
        || request.session_id.trim().is_empty()
        || request.provider_tool_call_id.trim().is_empty()
        || request.tool_name.trim().is_empty()
        || request.created_process_instance.trim().is_empty()
    {
        bail!("permission intent scope and correlation fields must be present");
    }
    if request.expires_at <= now {
        bail!("permission intent expiry must be in the future");
    }
    Ok(())
}

fn validate_opaque_objective_id(objective_id: &str) -> anyhow::Result<()> {
    let normalized = objective_id.to_ascii_lowercase();
    if normalized.starts_with("chat:")
        || normalized.starts_with("task:")
        || objective_id.trim().len() < 16
    {
        bail!("permission intent requires an opaque Objective id");
    }
    Ok(())
}

async fn validate_scope(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scope: &PermissionScope,
) -> anyhow::Result<()> {
    let objective_matches: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives
         WHERE id=? AND revision=?
           AND status NOT IN ('completed','cancelled','legacy_orphan')",
    )
    .bind(&scope.objective_id)
    .bind(scope.objective_revision)
    .fetch_one(&mut **tx)
    .await?;
    if objective_matches != 1 {
        bail!("permission Objective scope is stale or terminal");
    }
    let binding_matches: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objective_bindings
         WHERE id=? AND objective_id=? AND resource_generation=?",
    )
    .bind(&scope.binding_id)
    .bind(&scope.objective_id)
    .bind(scope.resource_generation)
    .fetch_one(&mut **tx)
    .await?;
    if binding_matches != 1 {
        bail!("permission Objective binding scope is stale or conflicting");
    }
    Ok(())
}

async fn scope_is_authoritative(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scope: &PermissionScope,
) -> anyhow::Result<bool> {
    let matches: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM objectives objective
         JOIN objective_bindings binding ON binding.objective_id=objective.id
         WHERE objective.id=? AND objective.revision=?
           AND objective.status NOT IN ('completed','cancelled','legacy_orphan')
           AND binding.id=? AND binding.resource_generation=?",
    )
    .bind(&scope.objective_id)
    .bind(scope.objective_revision)
    .bind(&scope.binding_id)
    .bind(scope.resource_generation)
    .fetch_one(&mut **tx)
    .await?;
    Ok(matches == 1)
}

async fn latest_action_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scope: &PermissionScope,
    action_signature: &str,
) -> anyhow::Result<Option<PermissionIntentSnapshot>> {
    let row = sqlx::query(
        "SELECT * FROM permission_intents
         WHERE objective_id=? AND objective_revision=? AND binding_id=?
           AND resource_generation=? AND action_signature=?
         ORDER BY prompt_generation DESC LIMIT 1",
    )
    .bind(&scope.objective_id)
    .bind(scope.objective_revision)
    .bind(&scope.binding_id)
    .bind(scope.resource_generation)
    .bind(action_signature)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| PermissionIntentSnapshot::from_row(&row))
        .transpose()
}

async fn successor_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    predecessor_intent_id: &str,
) -> anyhow::Result<Option<PermissionIntentSnapshot>> {
    let row = sqlx::query("SELECT * FROM permission_intents WHERE predecessor_intent_id=?")
        .bind(predecessor_intent_id)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| PermissionIntentSnapshot::from_row(&row))
        .transpose()
}

fn validate_successor(
    source: &PermissionIntentSnapshot,
    successor: &PermissionIntentSnapshot,
) -> anyhow::Result<()> {
    let exact = successor.predecessor_intent_id.as_deref() == Some(source.intent_id.as_str())
        && successor.scope.objective_id == source.scope.objective_id
        && successor.scope.objective_revision >= source.scope.objective_revision
        && successor.scope.binding_id == source.scope.binding_id
        && successor.scope.resource_generation == source.scope.resource_generation
        && successor.session_id == source.session_id
        && successor.provider_tool_call_id == source.provider_tool_call_id
        && successor.tool_name == source.tool_name
        && successor.prompt_args == source.prompt_args
        && successor.action_signature == source.action_signature
        && successor.prompt_generation == source.prompt_generation + 1;
    if !exact {
        bail!("permission prompt successor conflicts with its durable predecessor");
    }
    Ok(())
}

async fn expire_exact_if_due(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &PermissionPromptKey,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE permission_intents
         SET status='timed_out', failure_code='permission_timed_out',
             decided_at=?, updated_at=?
         WHERE intent_id=? AND objective_id=? AND objective_revision=?
           AND binding_id=? AND resource_generation=? AND action_signature=?
           AND prompt_generation=? AND status='pending' AND expires_at<=?",
    )
    .bind(now)
    .bind(now)
    .bind(&key.intent_id)
    .bind(&key.objective_id)
    .bind(key.objective_revision)
    .bind(&key.binding_id)
    .bind(key.resource_generation)
    .bind(&key.action_signature)
    .bind(key.prompt_generation)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn supersede_exact(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &PermissionPromptKey,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE permission_intents
         SET status='superseded', failure_code='permission_scope_stale',
             decided_at=COALESCE(decided_at, ?), updated_at=?
         WHERE intent_id=? AND objective_id=? AND objective_revision=?
           AND binding_id=? AND resource_generation=? AND action_signature=?
           AND prompt_generation=? AND status IN ('pending','allowed')",
    )
    .bind(now)
    .bind(now)
    .bind(&key.intent_id)
    .bind(&key.objective_id)
    .bind(key.objective_revision)
    .bind(&key.binding_id)
    .bind(key.resource_generation)
    .bind(&key.action_signature)
    .bind(key.prompt_generation)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn get_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    intent_id: &str,
) -> anyhow::Result<Option<PermissionIntentSnapshot>> {
    let row = sqlx::query("SELECT * FROM permission_intents WHERE intent_id=?")
        .bind(intent_id)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| PermissionIntentSnapshot::from_row(&row))
        .transpose()
}

pub(crate) fn permission_action_signature(
    tool_name: &str,
    args: &Value,
    bash_command: Option<&str>,
) -> String {
    let canonical = canonical_json(args);
    let mut digest = Sha256::new();
    for part in [tool_name, canonical.as_str(), bash_command.unwrap_or("")] {
        let bytes = part.as_bytes();
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    format!("{:x}", digest.finalize())
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::objective::{
        CreateObjective, DecisionRouter, ObjectiveKind, ObjectiveStatus, ObjectiveStore,
        RecoveryDomain, RouteSignal,
    };
    use sqlx::sqlite::SqlitePoolOptions;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const OBJECTIVE_ID: &str = "5eea633e-59f9-42bc-91f8-0a19a5c49711";
    const SECOND_OBJECTIVE_ID: &str = "96780477-79ce-4775-83ac-9071225d8dbb";
    const BINDING_ID: &str = "42cfb353-5f4e-4809-8338-9b49b9806894";
    const SECOND_BINDING_ID: &str = "d90ded63-a9a6-490c-b2ea-225fd86bc7e9";

    async fn database() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE objectives (
               id TEXT PRIMARY KEY, revision INTEGER NOT NULL, status TEXT NOT NULL
             );
             CREATE TABLE objective_bindings (
               id TEXT PRIMARY KEY, objective_id TEXT NOT NULL,
               resource_generation INTEGER NOT NULL,
               FOREIGN KEY(objective_id) REFERENCES objectives(id) ON DELETE CASCADE
             );",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (objective_id, binding_id) in [
            (OBJECTIVE_ID, BINDING_ID),
            (SECOND_OBJECTIVE_ID, SECOND_BINDING_ID),
        ] {
            sqlx::query("INSERT INTO objectives VALUES (?, 7, 'active')")
                .bind(objective_id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO objective_bindings VALUES (?, ?, 3)")
                .bind(binding_id)
                .bind(objective_id)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::raw_sql(include_str!("../../migrations/0010_permission_intents.sql"))
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn recovery_database() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .unwrap();
        // Exercise the same ordered fresh-install history as SQLite startup;
        // Objective cancellation consults DeliveryRun attribution even when
        // this fixture itself has no delivery.
        for migration in [
            include_str!("../../migrations/0001_init.sql"),
            include_str!("../../migrations/0002_knowledge.sql"),
            include_str!("../../migrations/0003_session_execution_governance.sql"),
            include_str!("../../migrations/0004_delivery_runs.sql"),
            include_str!("../../migrations/0005_objective_recovery_control_plane.sql"),
            include_str!("../../migrations/0006_session_auto_title.sql"),
            include_str!("../../migrations/0007_unified_objective_control_plane.sql"),
            include_str!("../../migrations/0008_delivery_identity_revisions.sql"),
            include_str!("../../migrations/0009_chat_run_controls.sql"),
            include_str!("../../migrations/0010_permission_intents.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        // Production `storage::db::connect` always follows sqlx migrations
        // with this idempotent compatibility bootstrap. It adds nullable
        // projection columns (including chat_turn_state.objective_id) without
        // making re-run-safe migration files depend on unconditional ALTERs.
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        pool
    }

    async fn pending_active_permission(
        suffix: &str,
    ) -> (
        SqlitePool,
        PermissionIntentStore,
        PermissionIntentSnapshot,
        crate::agent::objective::ObjectiveSnapshot,
    ) {
        let pool = recovery_database().await;
        let objective_id = Uuid::new_v4().to_string();
        let binding_id = Uuid::new_v4().to_string();
        let session_id = format!("session-{suffix}");
        let root_turn_id = format!("turn-{suffix}");
        let objective_store = ObjectiveStore::new(pool.clone());
        let objective = objective_store
            .create(CreateObjective {
                id: objective_id.clone(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(session_id.clone()),
                root_turn_id: Some(root_turn_id.clone()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "permission-test".into(),
            })
            .await
            .unwrap();
        let now = 10_000;
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, created_at, updated_at)
             VALUES (?, ?, 'chat', 'chat_root_turn', ?, 1, ?, ?, ?)",
        )
        .bind(&binding_id)
        .bind(&objective_id)
        .bind(&root_turn_id)
        .bind(format!("sha256:{suffix}"))
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let store = PermissionIntentStore::new(pool.clone());
        let intent = store
            .create_pending(
                &PermissionIntentRequest {
                    scope: PermissionScope {
                        objective_id: objective_id.clone(),
                        objective_revision: objective.revision,
                        binding_id: binding_id.clone(),
                        resource_generation: 1,
                    },
                    session_id,
                    provider_tool_call_id: format!("provider-{suffix}"),
                    tool_name: "browser_session".into(),
                    args: serde_json::json!({
                        "action": "click",
                        "selector": "#publish"
                    }),
                    bash_command: None,
                    expires_at: now + 1_000,
                    created_process_instance: "process-before-crash".into(),
                },
                now,
            )
            .await
            .unwrap();
        (pool, store, intent, objective)
    }

    async fn interrupted_permission_claim(
        suffix: &str,
    ) -> (
        SqlitePool,
        PermissionIntentStore,
        PermissionIntentSnapshot,
        crate::agent::objective::ClaimedRemediation,
        codefactory_agent_loop::tool::MutationPermit,
    ) {
        let (pool, store, intent, objective) = pending_active_permission(suffix).await;
        let objective_store = ObjectiveStore::new(pool.clone());
        let now = 10_000;
        store
            .record_interruption(
                &intent.prompt_key(),
                PermissionIntentStatus::ChannelClosed,
                now + 10,
            )
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Permission,
                failure_code: "permission_channel_closed".into(),
                failure_signature: format!("sha256:permission-{suffix}"),
                next_observation_at: now + 20,
                resume_cursor: objective.root_turn_id.clone(),
            },
        )
        .unwrap();
        objective_store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        let claim = objective_store
            .claim_due_remediations("permission-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id.clone(),
            owner: "permission-owner".into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id.clone(),
            resource_generation: claim.resource_generation,
        };
        (pool, store, intent, claim, permit)
    }

    fn request(
        objective_id: &str,
        binding_id: &str,
        provider_tool_call_id: &str,
    ) -> PermissionIntentRequest {
        PermissionIntentRequest {
            scope: PermissionScope {
                objective_id: objective_id.into(),
                objective_revision: 7,
                binding_id: binding_id.into(),
                resource_generation: 3,
            },
            session_id: "session-a".into(),
            provider_tool_call_id: provider_tool_call_id.into(),
            tool_name: "browser_session".into(),
            args: serde_json::json!({"action":"click","selector":"#publish"}),
            bash_command: None,
            expires_at: 2_000,
            created_process_instance: "process-a".into(),
        }
    }

    #[tokio::test]
    async fn production_bootstrap_adds_objective_projection_after_ordered_migrations() {
        let pool = recovery_database().await;
        let projection_column: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('chat_turn_state')
             WHERE name='objective_id'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let permission_tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table' AND name IN (
               'permission_intents','permission_action_receipts'
             )",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(projection_column, 1);
        assert_eq!(permission_tables, 2);
    }

    #[tokio::test]
    async fn action_signature_is_canonical_and_action_exact() {
        let first = permission_action_signature(
            "browser_session",
            &serde_json::json!({"selector":"#publish","action":"click"}),
            None,
        );
        let reordered = permission_action_signature(
            "browser_session",
            &serde_json::json!({"action":"click","selector":"#publish"}),
            None,
        );
        let different = permission_action_signature(
            "browser_session",
            &serde_json::json!({"action":"click","selector":"#delete"}),
            None,
        );
        assert_eq!(first, reordered);
        assert_ne!(first, different);
        assert_eq!(first.len(), 64);
    }

    #[tokio::test]
    async fn intent_rejects_legacy_or_stale_objective_identity() {
        let store = PermissionIntentStore::new(database().await);
        let mut legacy = request("chat:root-one", BINDING_ID, "call-1");
        assert!(store.create_pending(&legacy, 1_000).await.is_err());

        legacy.scope.objective_id = OBJECTIVE_ID.into();
        legacy.scope.objective_revision = 6;
        assert!(store.create_pending(&legacy, 1_000).await.is_err());
    }

    #[tokio::test]
    async fn same_provider_tool_id_in_distinct_objectives_does_not_collide() {
        let store = PermissionIntentStore::new(database().await);
        let first = store
            .create_pending(&request(OBJECTIVE_ID, BINDING_ID, "call-duplicate"), 1_000)
            .await
            .unwrap();
        let second = store
            .create_pending(
                &request(SECOND_OBJECTIVE_ID, SECOND_BINDING_ID, "call-duplicate"),
                1_000,
            )
            .await
            .unwrap();
        let registry = PendingPermissionRegistry::default();
        let (first_sender, _first_receiver) = oneshot::channel();
        let (second_sender, _second_receiver) = oneshot::channel();
        registry
            .register(first.prompt_key(), first_sender)
            .await
            .unwrap();
        registry
            .register(second.prompt_key(), second_sender)
            .await
            .unwrap();
        assert_eq!(registry.len().await, 2);
    }

    #[tokio::test]
    async fn exact_allow_is_persisted_and_consumed_only_once() {
        let store = PermissionIntentStore::new(database().await);
        let intent = store
            .create_pending(&request(OBJECTIVE_ID, BINDING_ID, "call-allow"), 1_000)
            .await
            .unwrap();
        let key = intent.prompt_key();
        let allowed = store
            .record_user_response(&key, PermissionPromptResponse::Allow, 1_100)
            .await
            .unwrap();
        assert_eq!(allowed.status, PermissionIntentStatus::Allowed);

        let mut wrong_action = key.clone();
        wrong_action.action_signature = "0".repeat(64);
        assert!(!store
            .consume_exact_allow(&wrong_action, 1_101)
            .await
            .unwrap());
        assert!(store.consume_exact_allow(&key, 1_102).await.unwrap());
        assert!(!store.consume_exact_allow(&key, 1_103).await.unwrap());
        assert_eq!(
            store.get(&intent.intent_id).await.unwrap().unwrap().status,
            PermissionIntentStatus::Consumed
        );
    }

    #[tokio::test]
    async fn timeout_and_channel_close_are_durable_and_do_not_overwrite_decisions() {
        let store = PermissionIntentStore::new(database().await);
        let timed_out = store
            .create_pending(&request(OBJECTIVE_ID, BINDING_ID, "call-timeout"), 1_000)
            .await
            .unwrap();
        assert_eq!(store.expire_due(2_000).await.unwrap(), 1);
        assert_eq!(
            store
                .get(&timed_out.intent_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            PermissionIntentStatus::TimedOut
        );
        assert!(store
            .record_user_response(
                &timed_out.prompt_key(),
                PermissionPromptResponse::Allow,
                2_001
            )
            .await
            .is_err());

        let mut second_request = request(OBJECTIVE_ID, BINDING_ID, "call-close");
        second_request.args = serde_json::json!({"action":"fill","selector":"#name"});
        second_request.expires_at = 4_000;
        let closed = store.create_pending(&second_request, 2_100).await.unwrap();
        assert_eq!(
            store
                .close_process_channels("process-a", 2_200)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store.get(&closed.intent_id).await.unwrap().unwrap().status,
            PermissionIntentStatus::ChannelClosed
        );
    }

    #[tokio::test]
    async fn explicit_denial_blocks_same_action_even_with_new_provider_tool_id() {
        let store = PermissionIntentStore::new(database().await);
        let original_request = request(OBJECTIVE_ID, BINDING_ID, "call-first");
        let first = store
            .create_pending(&original_request, 1_000)
            .await
            .unwrap();
        store
            .record_user_response(&first.prompt_key(), PermissionPromptResponse::Deny, 1_100)
            .await
            .unwrap();

        let mut retry = original_request;
        retry.provider_tool_call_id = "call-second".into();
        let retry_error = store.create_pending(&retry, 1_200).await.unwrap_err();
        assert_eq!(first.prompt_generation, 1);
        assert!(retry_error.to_string().contains("explicitly denied"));
        let rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM permission_intents
             WHERE objective_id=? AND action_signature=?",
        )
        .bind(OBJECTIVE_ID)
        .bind(&first.action_signature)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn wrong_prompt_generation_cannot_settle_or_take_memory_channel() {
        let store = PermissionIntentStore::new(database().await);
        let intent = store
            .create_pending(&request(OBJECTIVE_ID, BINDING_ID, "call-generation"), 1_000)
            .await
            .unwrap();
        let registry = PendingPermissionRegistry::default();
        let (sender, _receiver) = oneshot::channel();
        registry
            .register(intent.prompt_key(), sender)
            .await
            .unwrap();

        let mut stale = intent.prompt_key();
        stale.prompt_generation += 1;
        assert!(store
            .record_user_response(&stale, PermissionPromptResponse::Allow, 1_100)
            .await
            .is_err());
        assert!(registry.take_exact(&stale).await.is_none());
        assert!(registry.take_exact(&intent.prompt_key()).await.is_some());
    }

    #[tokio::test]
    async fn duplicate_registry_insert_preserves_the_original_receiver() {
        let store = PermissionIntentStore::new(database().await);
        let intent = store
            .create_pending(&request(OBJECTIVE_ID, BINDING_ID, "call-registry"), 1_000)
            .await
            .unwrap();
        let key = intent.prompt_key();
        let registry = PendingPermissionRegistry::default();
        let (original_sender, original_receiver) = oneshot::channel();
        let (replacement_sender, replacement_receiver) = oneshot::channel();
        registry
            .register(key.clone(), original_sender)
            .await
            .unwrap();
        assert!(registry
            .register(key.clone(), replacement_sender)
            .await
            .is_err());
        registry
            .take_exact(&key)
            .await
            .unwrap()
            .send(PermissionPromptResponse::Allow)
            .unwrap();
        assert_eq!(
            original_receiver.await.unwrap(),
            PermissionPromptResponse::Allow
        );
        assert!(replacement_receiver.await.is_err());
    }

    #[tokio::test]
    async fn stale_objective_revision_supersedes_pending_or_allowed_authority() {
        let store = PermissionIntentStore::new(database().await);
        let pending = store
            .create_pending(
                &request(OBJECTIVE_ID, BINDING_ID, "call-stale-pending"),
                1_000,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET revision=8 WHERE id=?")
            .bind(OBJECTIVE_ID)
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(store
            .record_user_response(
                &pending.prompt_key(),
                PermissionPromptResponse::Allow,
                1_100
            )
            .await
            .is_err());
        assert_eq!(
            store.get(&pending.intent_id).await.unwrap().unwrap().status,
            PermissionIntentStatus::Superseded
        );

        sqlx::query("UPDATE objectives SET revision=7 WHERE id=?")
            .bind(OBJECTIVE_ID)
            .execute(&store.pool)
            .await
            .unwrap();
        let mut allowed_request = request(OBJECTIVE_ID, BINDING_ID, "call-stale-allowed");
        allowed_request.args = serde_json::json!({"action":"press","key":"Enter"});
        let allowed = store.create_pending(&allowed_request, 1_200).await.unwrap();
        store
            .record_user_response(
                &allowed.prompt_key(),
                PermissionPromptResponse::Allow,
                1_300,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET revision=8 WHERE id=?")
            .bind(OBJECTIVE_ID)
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(!store
            .consume_exact_allow(&allowed.prompt_key(), 1_400)
            .await
            .unwrap());
        assert_eq!(
            store.get(&allowed.intent_id).await.unwrap().unwrap().status,
            PermissionIntentStatus::Superseded
        );
    }

    #[tokio::test]
    async fn repeated_same_response_is_idempotent_but_conflicting_response_is_not() {
        let store = PermissionIntentStore::new(database().await);
        let intent = store
            .create_pending(&request(OBJECTIVE_ID, BINDING_ID, "call-idempotent"), 1_000)
            .await
            .unwrap();
        let key = intent.prompt_key();
        store
            .record_user_response(&key, PermissionPromptResponse::Deny, 1_100)
            .await
            .unwrap();
        assert_eq!(
            store
                .record_user_response(&key, PermissionPromptResponse::Deny, 1_101)
                .await
                .unwrap()
                .status,
            PermissionIntentStatus::Denied
        );
        assert!(store
            .record_user_response(&key, PermissionPromptResponse::Allow, 1_102)
            .await
            .is_err());
        assert_eq!(
            store
                .record_interruption(&key, PermissionIntentStatus::TimedOut, 1_103)
                .await
                .unwrap_err()
                .to_string()
                .contains("not pending"),
            true
        );
    }

    #[tokio::test]
    async fn restart_rehydrates_the_same_prompt_without_replaying_provider_or_native_action() {
        let store = PermissionIntentStore::new(database().await);
        let original = store
            .create_pending(
                &request(OBJECTIVE_ID, BINDING_ID, "provider-call-stable"),
                1_000,
            )
            .await
            .unwrap();
        store
            .record_interruption(
                &original.prompt_key(),
                PermissionIntentStatus::ChannelClosed,
                1_100,
            )
            .await
            .unwrap();

        let rehydrated = store
            .rehydrate_interrupted(&original.intent_id, "process-after-restart", 2_500, 1_200)
            .await
            .unwrap();
        assert_eq!(
            rehydrated.predecessor_intent_id.as_deref(),
            Some(original.intent_id.as_str())
        );
        assert_eq!(
            rehydrated.provider_tool_call_id,
            original.provider_tool_call_id
        );
        assert_eq!(rehydrated.action_signature, original.action_signature);
        assert_eq!(rehydrated.prompt_generation, original.prompt_generation + 1);
        assert_eq!(rehydrated.status, PermissionIntentStatus::Pending);
        assert_eq!(
            rehydrated.prompt_args,
            serde_json::json!({"action":"click","selector":"#publish"})
        );

        let (rows, provider_calls, actions, consumed): (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COUNT(DISTINCT provider_tool_call_id),
                        COUNT(DISTINCT action_signature),
                        SUM(CASE WHEN status='consumed' THEN 1 ELSE 0 END)
                 FROM permission_intents WHERE objective_id=?",
        )
        .bind(OBJECTIVE_ID)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!((rows, provider_calls, actions, consumed), (2, 1, 1, 0));
    }

    #[tokio::test]
    async fn interrupted_rehydrate_is_idempotent_and_does_not_fork_prompt_generation() {
        let store = PermissionIntentStore::new(database().await);
        let original = store
            .create_pending(
                &request(OBJECTIVE_ID, BINDING_ID, "call-rehydrate-once"),
                1_000,
            )
            .await
            .unwrap();
        store
            .record_interruption(
                &original.prompt_key(),
                PermissionIntentStatus::TimedOut,
                1_100,
            )
            .await
            .unwrap();

        let first = store
            .rehydrate_interrupted(&original.intent_id, "process-b", 2_500, 1_200)
            .await
            .unwrap();
        let second = store
            .rehydrate_interrupted(&original.intent_id, "process-c", 2_600, 1_300)
            .await
            .unwrap();
        assert_eq!(first.intent_id, second.intent_id);
        assert_eq!(first.prompt_generation, 2);
        let generation_two: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM permission_intents
             WHERE objective_id=? AND action_signature=? AND prompt_generation=2",
        )
        .bind(OBJECTIVE_ID)
        .bind(&original.action_signature)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert_eq!(generation_two, 1);
    }

    #[tokio::test]
    async fn observation_preserves_allow_once_and_explicit_deny_as_non_replayable() {
        let store = PermissionIntentStore::new(database().await);
        let allowed = store
            .create_pending(
                &request(OBJECTIVE_ID, BINDING_ID, "call-observe-allow"),
                1_000,
            )
            .await
            .unwrap();
        store
            .record_user_response(
                &allowed.prompt_key(),
                PermissionPromptResponse::Allow,
                1_100,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .observe_exact(&allowed.intent_id, 1_101)
                .await
                .unwrap()
                .unwrap()
                .disposition,
            PermissionRecoveryDisposition::ConsumeExactAllow
        );
        assert!(store
            .consume_exact_allow(&allowed.prompt_key(), 1_102)
            .await
            .unwrap());
        assert_eq!(
            store
                .observe_exact(&allowed.intent_id, 1_103)
                .await
                .unwrap()
                .unwrap()
                .disposition,
            PermissionRecoveryDisposition::AlreadyConsumed
        );

        let mut denied_request = request(OBJECTIVE_ID, BINDING_ID, "call-observe-deny");
        denied_request.args = serde_json::json!({"action":"click","selector":"#delete"});
        let denied = store.create_pending(&denied_request, 1_200).await.unwrap();
        store
            .record_user_response(&denied.prompt_key(), PermissionPromptResponse::Deny, 1_300)
            .await
            .unwrap();
        assert_eq!(
            store
                .observe_exact(&denied.intent_id, 1_301)
                .await
                .unwrap()
                .unwrap()
                .disposition,
            PermissionRecoveryDisposition::ExplicitlyDenied
        );
        assert!(store
            .rehydrate_interrupted(&denied.intent_id, "process-b", 2_500, 1_400)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn rehydration_refuses_stale_revision_or_binding() {
        let store = PermissionIntentStore::new(database().await);
        let original = store
            .create_pending(
                &request(OBJECTIVE_ID, BINDING_ID, "call-stale-rehydrate"),
                1_000,
            )
            .await
            .unwrap();
        store
            .record_interruption(
                &original.prompt_key(),
                PermissionIntentStatus::ChannelClosed,
                1_100,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE objective_bindings SET resource_generation=4 WHERE id=?")
            .bind(BINDING_ID)
            .execute(&store.pool)
            .await
            .unwrap();

        assert!(store
            .rehydrate_interrupted(&original.intent_id, "process-b", 2_500, 1_200)
            .await
            .is_err());
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM permission_intents WHERE objective_id=?")
                .bind(OBJECTIVE_ID)
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn claimed_interruption_reprojects_original_prompt_and_enters_typed_waiting_authorization(
    ) {
        let (_pool, store, original, claim, permit) =
            interrupted_permission_claim("projection").await;
        let provider_calls = AtomicUsize::new(0);
        let native_actions = AtomicUsize::new(0);

        let projected = store
            .project_claimed_interruption(&permit, "process-after-restart", 12_000, 10_100)
            .await
            .unwrap();

        assert_eq!(projected.snapshot.scope.objective_id, claim.objective.id);
        assert_eq!(
            projected.snapshot.scope.objective_revision,
            claim.objective.revision + 1
        );
        assert_eq!(
            projected.snapshot.predecessor_intent_id.as_deref(),
            Some(original.intent_id.as_str())
        );
        assert_eq!(
            projected.snapshot.provider_tool_call_id,
            original.provider_tool_call_id
        );
        assert_eq!(
            projected.snapshot.action_signature,
            original.action_signature
        );
        assert_eq!(
            projected.disposition,
            PermissionRecoveryDisposition::AwaitingDecision
        );
        let objective = ObjectiveStore::new(store.pool.clone())
            .get(&claim.objective.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(objective.status, ObjectiveStatus::WaitingAuthorization);
        assert_eq!(objective.domain, RecoveryDomain::Permission);
        let expected_request_key = format!("permission:{}", projected.snapshot.intent_id);
        assert_eq!(
            objective.request_key.as_deref(),
            Some(expected_request_key.as_str())
        );
        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        assert_eq!(native_actions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn projected_allow_schedules_same_objective_and_reserves_exact_action_only_once() {
        let (_pool, store, _original, claim, first_permit) =
            interrupted_permission_claim("allow-resume").await;
        let projected = store
            .project_claimed_interruption(&first_permit, "process-after-restart", 12_000, 10_100)
            .await
            .unwrap();
        let settlement = store
            .settle_projected_response(
                &projected.snapshot.intent_id,
                PermissionPromptResponse::Allow,
                10_200,
            )
            .await
            .unwrap();
        assert_eq!(settlement.objective_id, claim.objective.id);
        assert!(settlement.recovery_scheduled);

        let objective_store = ObjectiveStore::new(store.pool.clone());
        let resumed = objective_store
            .get(&claim.objective.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resumed.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(resumed.domain, RecoveryDomain::Permission);
        let next_claim = objective_store
            .claim_due_remediations("permission-resume-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(next_claim.objective.id, claim.objective.id);
        let next_permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: next_claim.objective.id.clone(),
            remediation_id: next_claim.remediation_id.clone(),
            owner: "permission-resume-owner".into(),
            claim_epoch: next_claim.claim_epoch,
            binding_id: next_claim.binding_id.clone(),
            resource_generation: next_claim.resource_generation,
        };
        let exact_scope = PermissionScope {
            objective_id: resumed.id.clone(),
            objective_revision: resumed.revision,
            binding_id: projected.snapshot.scope.binding_id.clone(),
            resource_generation: projected.snapshot.scope.resource_generation,
        };
        let mutations = AtomicUsize::new(0);
        if store
            .reserve_exact_recovery_allow(
                &exact_scope,
                &projected.snapshot.action_signature,
                &next_permit,
                10_300,
            )
            .await
            .unwrap()
        {
            mutations.fetch_add(1, Ordering::SeqCst);
        }
        assert!(!store
            .reserve_exact_recovery_allow(
                &exact_scope,
                &projected.snapshot.action_signature,
                &next_permit,
                10_301,
            )
            .await
            .unwrap());
        assert_eq!(mutations.load(Ordering::SeqCst), 1);

        let mut wrong_scope = exact_scope;
        wrong_scope.objective_revision -= 1;
        assert!(!store
            .reserve_exact_recovery_allow(
                &wrong_scope,
                &projected.snapshot.action_signature,
                &next_permit,
                10_302,
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn receiver_close_after_allow_auto_resumes_same_objective_and_action_once() {
        let (pool, store, intent, objective) = pending_active_permission("orphaned-allow").await;
        store
            .record_user_response(
                &intent.prompt_key(),
                PermissionPromptResponse::Allow,
                10_010,
            )
            .await
            .unwrap();

        let settlement = store
            .reconcile_orphaned_response(&intent.intent_id, 10_020)
            .await
            .unwrap();
        assert_eq!(settlement.objective_id, objective.id);
        assert!(settlement.recovery_scheduled);
        let objective_store = ObjectiveStore::new(pool.clone());
        let waiting = objective_store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(waiting.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(waiting.domain, RecoveryDomain::Permission);
        assert_eq!(
            waiting.action_signature,
            Some(intent.action_signature.clone())
        );

        let claim = objective_store
            .claim_due_remediations("orphaned-allow-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id.clone(),
            owner: "orphaned-allow-owner".into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id.clone(),
            resource_generation: claim.resource_generation,
        };
        assert_eq!(
            store
                .observe_claimed_recovery(&permit, "current-process", 20_000, 10_030)
                .await
                .unwrap(),
            PermissionClaimAction::ResumeAuthorizedAction
        );
        assert_eq!(
            store.get(&intent.intent_id).await.unwrap().unwrap().status,
            PermissionIntentStatus::Consumed
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM permission_action_receipts
                 WHERE intent_id=? AND status='available'",
            )
            .bind(&intent.intent_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        let exact_scope = PermissionScope {
            objective_id: objective.id.clone(),
            objective_revision: waiting.revision,
            binding_id: intent.scope.binding_id.clone(),
            resource_generation: intent.scope.resource_generation,
        };
        let native_actions = AtomicUsize::new(0);
        if store
            .reserve_exact_recovery_allow(&exact_scope, &intent.action_signature, &permit, 10_040)
            .await
            .unwrap()
        {
            native_actions.fetch_add(1, Ordering::SeqCst);
        }
        assert!(!store
            .reserve_exact_recovery_allow(&exact_scope, &intent.action_signature, &permit, 10_041,)
            .await
            .unwrap());
        assert_eq!(native_actions.load(Ordering::SeqCst), 1);

        let repeated = store
            .reconcile_orphaned_response(&intent.intent_id, 10_050)
            .await
            .unwrap();
        assert_eq!(repeated.objective_id, objective.id);
        assert!(repeated.recovery_scheduled);
    }

    #[tokio::test]
    async fn stale_epoch_cannot_reset_new_owner_permission_reservation() {
        let (pool, store, _original, claim, prompt_permit) =
            interrupted_permission_claim("stale-reservation-reset").await;
        let projected = store
            .project_claimed_interruption(&prompt_permit, "process-b", 12_000, 10_100)
            .await
            .unwrap();
        store
            .settle_projected_response(
                &projected.snapshot.intent_id,
                PermissionPromptResponse::Allow,
                10_200,
            )
            .await
            .unwrap();
        let objective_store = ObjectiveStore::new(pool.clone());
        let first_claim = objective_store
            .claim_due_remediations("permission-first-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let first_permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: first_claim.objective.id.clone(),
            remediation_id: first_claim.remediation_id.clone(),
            owner: "permission-first-owner".into(),
            claim_epoch: first_claim.claim_epoch,
            binding_id: first_claim.binding_id.clone(),
            resource_generation: first_claim.resource_generation,
        };
        let scope = PermissionScope {
            objective_id: first_claim.objective.id.clone(),
            objective_revision: first_claim.objective.revision,
            binding_id: first_claim.binding_id.clone().unwrap(),
            resource_generation: first_claim.resource_generation.unwrap(),
        };
        let now = chrono::Utc::now().timestamp_millis();
        assert!(store
            .reserve_exact_recovery_allow(
                &scope,
                &projected.snapshot.action_signature,
                &first_permit,
                now,
            )
            .await
            .unwrap());

        let expired = now - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&first_claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&first_claim.objective.id)
            .execute(&pool)
            .await
            .unwrap();
        let second_claim = objective_store
            .claim_due_remediations("permission-second-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert!(second_claim.claim_epoch > first_claim.claim_epoch);
        let second_permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: second_claim.objective.id.clone(),
            remediation_id: second_claim.remediation_id.clone(),
            owner: "permission-second-owner".into(),
            claim_epoch: second_claim.claim_epoch,
            binding_id: second_claim.binding_id.clone(),
            resource_generation: second_claim.resource_generation,
        };
        assert_eq!(
            store
                .observe_claimed_recovery(&second_permit, "process-c", now + 60_000, now + 1,)
                .await
                .unwrap(),
            PermissionClaimAction::ResumeAuthorizedAction
        );
        assert!(store
            .reserve_exact_recovery_allow(
                &scope,
                &projected.snapshot.action_signature,
                &second_permit,
                now + 2,
            )
            .await
            .unwrap());

        assert!(store
            .observe_claimed_recovery(&first_permit, "process-b", now + 60_000, now + 3)
            .await
            .is_err());
        let reservation: (String, i64, String) = sqlx::query_as(
            "SELECT consumer_owner, consumer_claim_epoch, status
             FROM permission_action_receipts WHERE objective_id=?",
        )
        .bind(&claim.objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            reservation,
            (
                "permission-second-owner".into(),
                second_permit.claim_epoch,
                "reserved".into(),
            )
        );
    }

    #[tokio::test]
    async fn takeover_does_not_reset_authority_when_backend_fingerprint_is_unresolved() {
        let (pool, store, _original, _claim, prompt_permit) =
            interrupted_permission_claim("backend-fingerprint-unresolved").await;
        let projected = store
            .project_claimed_interruption(&prompt_permit, "process-b", 12_000, 10_100)
            .await
            .unwrap();
        store
            .settle_projected_response(
                &projected.snapshot.intent_id,
                PermissionPromptResponse::Allow,
                10_200,
            )
            .await
            .unwrap();
        let objective_store = ObjectiveStore::new(pool.clone());
        let first_claim = objective_store
            .claim_due_remediations("permission-first-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let first_permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: first_claim.objective.id.clone(),
            remediation_id: first_claim.remediation_id.clone(),
            owner: "permission-first-owner".into(),
            claim_epoch: first_claim.claim_epoch,
            binding_id: first_claim.binding_id.clone(),
            resource_generation: first_claim.resource_generation,
        };
        let scope = PermissionScope {
            objective_id: first_claim.objective.id.clone(),
            objective_revision: first_claim.objective.revision,
            binding_id: first_claim.binding_id.clone().unwrap(),
            resource_generation: first_claim.resource_generation.unwrap(),
        };
        let now = chrono::Utc::now().timestamp_millis();
        assert!(store
            .reserve_exact_recovery_allow(
                &scope,
                &projected.snapshot.action_signature,
                &first_permit,
                now,
            )
            .await
            .unwrap());

        // The tool backend deliberately incorporates cwd, binding and
        // generation into this fingerprint, while the prompt signature only
        // hashes what the user saw. They are not interchangeable identities.
        let backend_fingerprint = format!(
            "backend:{}:{}",
            projected.snapshot.action_signature, scope.binding_id
        );
        assert_ne!(
            backend_fingerprint, projected.snapshot.action_signature,
            "fixture must model the production signature/fingerprint split"
        );
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES ('permission-unresolved-backend-receipt', ?, ?, ?, ?,
                     'permission-unresolved-backend-key', 'started', ?, ?)",
        )
        .bind(&scope.objective_id)
        .bind(&scope.binding_id)
        .bind(scope.objective_revision)
        .bind(&backend_fingerprint)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let expired = now - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&first_claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&first_claim.objective.id)
            .execute(&pool)
            .await
            .unwrap();
        let second_claim = objective_store
            .claim_due_remediations("permission-second-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let second_permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: second_claim.objective.id.clone(),
            remediation_id: second_claim.remediation_id.clone(),
            owner: "permission-second-owner".into(),
            claim_epoch: second_claim.claim_epoch,
            binding_id: second_claim.binding_id.clone(),
            resource_generation: second_claim.resource_generation,
        };

        assert!(store
            .observe_claimed_recovery(&second_permit, "process-c", now + 60_000, now + 1)
            .await
            .is_err());
        let reservation: (String, i64, String) = sqlx::query_as(
            "SELECT consumer_owner, consumer_claim_epoch, status
             FROM permission_action_receipts WHERE objective_id=?",
        )
        .bind(&scope.objective_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            reservation,
            (
                "permission-first-owner".into(),
                first_permit.claim_epoch,
                "reserved".into(),
            ),
            "unresolved backend dispatch must preserve the old reservation for observe-only reconciliation"
        );
    }

    #[tokio::test]
    async fn receiver_close_after_explicit_deny_cancels_without_replay_authority() {
        let (pool, store, intent, objective) = pending_active_permission("orphaned-deny").await;
        store
            .record_user_response(&intent.prompt_key(), PermissionPromptResponse::Deny, 10_010)
            .await
            .unwrap();
        let settlement = store
            .reconcile_orphaned_response(&intent.intent_id, 10_020)
            .await
            .unwrap();
        assert!(!settlement.recovery_scheduled);
        let cancelled = ObjectiveStore::new(pool.clone())
            .get(&objective.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.status, ObjectiveStatus::Cancelled);
        assert_eq!(
            cancelled.cancellation_provenance.as_deref(),
            Some("explicit_deny")
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM permission_action_receipts WHERE objective_id=?",
            )
            .bind(&objective.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
        assert!(store
            .record_user_response(
                &intent.prompt_key(),
                PermissionPromptResponse::Allow,
                10_030,
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn projected_explicit_deny_cancels_objective_and_never_creates_execution_receipt() {
        let (pool, store, _original, claim, permit) =
            interrupted_permission_claim("deny-terminal").await;
        let projected = store
            .project_claimed_interruption(&permit, "process-after-restart", 12_000, 10_100)
            .await
            .unwrap();
        let settlement = store
            .settle_projected_response(
                &projected.snapshot.intent_id,
                PermissionPromptResponse::Deny,
                10_200,
            )
            .await
            .unwrap();
        assert!(!settlement.recovery_scheduled);
        let cancelled = ObjectiveStore::new(pool.clone())
            .get(&claim.objective.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.status, ObjectiveStatus::Cancelled);
        assert_eq!(
            cancelled.cancellation_provenance.as_deref(),
            Some("explicit_deny")
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM permission_action_receipts WHERE objective_id=?",
            )
            .bind(&claim.objective.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
        assert!(store
            .settle_projected_response(
                &projected.snapshot.intent_id,
                PermissionPromptResponse::Allow,
                10_201,
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn stale_process_pending_permission_routes_same_active_objective_to_permission_recovery()
    {
        let pool = recovery_database().await;
        let objective_store = ObjectiveStore::new(pool.clone());
        let objective_id = Uuid::new_v4().to_string();
        let binding_id = Uuid::new_v4().to_string();
        let root_turn_id = "turn-stale-permission".to_string();
        let objective = objective_store
            .create(CreateObjective {
                id: objective_id.clone(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-stale-permission".into()),
                root_turn_id: Some(root_turn_id.clone()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "permission-restart-test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, created_at, updated_at)
             VALUES (?, ?, 'chat', 'chat_root_turn', ?, 1,
                     'sha256:permission-restart', 20000, 20000)",
        )
        .bind(&binding_id)
        .bind(&objective_id)
        .bind(&root_turn_id)
        .execute(&pool)
        .await
        .unwrap();
        let store = PermissionIntentStore::new(pool.clone());
        let original = store
            .create_pending(
                &PermissionIntentRequest {
                    scope: PermissionScope {
                        objective_id: objective_id.clone(),
                        objective_revision: objective.revision,
                        binding_id: binding_id.clone(),
                        resource_generation: 1,
                    },
                    session_id: "session-stale-permission".into(),
                    provider_tool_call_id: "provider-call-before-crash".into(),
                    tool_name: "browser_session".into(),
                    args: serde_json::json!({
                        "action": "click",
                        "selector": "#publish"
                    }),
                    bash_command: None,
                    expires_at: 30_000,
                    created_process_instance: "process-before-crash".into(),
                },
                20_000,
            )
            .await
            .unwrap();
        let provider_calls = AtomicUsize::new(0);
        let native_actions = AtomicUsize::new(0);

        assert_eq!(
            store
                .reconcile_stale_process_channels("process-after-restart", 20_100)
                .await
                .unwrap(),
            1
        );

        assert_eq!(
            store
                .get(&original.intent_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            PermissionIntentStatus::ChannelClosed
        );
        let waiting = objective_store.get(&objective_id).await.unwrap().unwrap();
        assert_eq!(waiting.id, objective.id);
        assert_eq!(waiting.root_turn_id, objective.root_turn_id);
        assert_eq!(waiting.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(waiting.domain, RecoveryDomain::Permission);
        assert_eq!(waiting.revision, objective.revision + 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM objective_remediations
                 WHERE objective_id=? AND domain='permission' AND status='queued'",
            )
            .bind(&objective_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );

        let first_claim = objective_store
            .claim_due_remediations("permission-project-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let first_permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: first_claim.objective.id.clone(),
            remediation_id: first_claim.remediation_id.clone(),
            owner: "permission-project-owner".into(),
            claim_epoch: first_claim.claim_epoch,
            binding_id: first_claim.binding_id.clone(),
            resource_generation: first_claim.resource_generation,
        };
        let projected = match store
            .observe_claimed_recovery(&first_permit, "process-after-restart", 40_000, 20_200)
            .await
            .unwrap()
        {
            PermissionClaimAction::ProjectPrompt(observation) => observation,
            other => panic!("expected prompt projection, got {other:?}"),
        };
        assert_eq!(
            projected.snapshot.predecessor_intent_id.as_deref(),
            Some(original.intent_id.as_str())
        );
        assert_eq!(
            projected.snapshot.provider_tool_call_id,
            original.provider_tool_call_id
        );
        assert_eq!(projected.snapshot.prompt_args, original.prompt_args);
        store
            .settle_projected_response(
                &projected.snapshot.intent_id,
                PermissionPromptResponse::Allow,
                20_300,
            )
            .await
            .unwrap();
        let authorized_claim = objective_store
            .claim_due_remediations("permission-action-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let authorized_permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: authorized_claim.objective.id.clone(),
            remediation_id: authorized_claim.remediation_id.clone(),
            owner: "permission-action-owner".into(),
            claim_epoch: authorized_claim.claim_epoch,
            binding_id: authorized_claim.binding_id.clone(),
            resource_generation: authorized_claim.resource_generation,
        };
        assert_eq!(
            store
                .observe_claimed_recovery(
                    &authorized_permit,
                    "process-after-restart",
                    40_000,
                    20_400,
                )
                .await
                .unwrap(),
            PermissionClaimAction::ResumeAuthorizedAction
        );
        let authorized_objective = objective_store.get(&objective_id).await.unwrap().unwrap();
        let exact_scope = PermissionScope {
            objective_id: objective_id.clone(),
            objective_revision: authorized_objective.revision,
            binding_id,
            resource_generation: 1,
        };
        if store
            .reserve_exact_recovery_allow(
                &exact_scope,
                &projected.snapshot.action_signature,
                &authorized_permit,
                20_500,
            )
            .await
            .unwrap()
        {
            native_actions.fetch_add(1, Ordering::SeqCst);
        }
        assert!(!store
            .reserve_exact_recovery_allow(
                &exact_scope,
                &projected.snapshot.action_signature,
                &authorized_permit,
                20_501,
            )
            .await
            .unwrap());
        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        assert_eq!(native_actions.load(Ordering::SeqCst), 1);

        // Restart reconciliation is idempotent and does not fork a second
        // remediation while the first durable recovery is still pending.
        assert_eq!(
            store
                .reconcile_stale_process_channels("process-after-restart", 20_101)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM objective_remediations
                 WHERE objective_id=? AND domain='permission'",
            )
            .bind(&objective_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            2
        );
    }
}
