// SPDX-License-Identifier: Apache-2.0
//! Durable Browser-domain mutation intent and observation boundary.

use anyhow::{bail, Result};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

const DONE_SUMMARY: &str = r#"{"status":"done"}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserAction {
    Click,
    Fill,
    Press,
    Open,
    Attach,
    SelectTab,
    Close,
    Screenshot,
}

impl BrowserAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Fill => "fill",
            Self::Press => "press",
            Self::Open => "open",
            Self::Attach => "attach",
            Self::SelectTab => "select_tab",
            Self::Close => "close",
            Self::Screenshot => "screenshot",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        Ok(match value {
            "click" => Self::Click,
            "fill" => Self::Fill,
            "press" => Self::Press,
            "open" => Self::Open,
            "attach" => Self::Attach,
            "select_tab" => Self::SelectTab,
            "close" => Self::Close,
            "screenshot" => Self::Screenshot,
            _ => bail!("unsupported browser recovery action: {value}"),
        })
    }

    fn replay_policy(self) -> BrowserReplayPolicy {
        match self {
            Self::Click | Self::Fill | Self::Press => BrowserReplayPolicy::NeverAfterDispatch,
            Self::Open | Self::Attach | Self::SelectTab | Self::Close => {
                BrowserReplayPolicy::ExactGeneration
            }
            Self::Screenshot => BrowserReplayPolicy::DigestCas,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserReplayPolicy {
    NeverAfterDispatch,
    ExactGeneration,
    DigestCas,
}

impl BrowserReplayPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::NeverAfterDispatch => "never_after_dispatch",
            Self::ExactGeneration => "exact_generation",
            Self::DigestCas => "digest_cas",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        Ok(match value {
            "never_after_dispatch" => Self::NeverAfterDispatch,
            "exact_generation" => Self::ExactGeneration,
            "digest_cas" => Self::DigestCas,
            _ => bail!("unsupported browser replay policy: {value}"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserObserverKind {
    SessionPresence,
    PageDigest,
    ElementDigest,
    TabDigest,
    WorkspaceFileSha256,
}

impl BrowserObserverKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::SessionPresence => "session_presence_v1",
            Self::PageDigest => "page_digest_v1",
            Self::ElementDigest => "element_digest_v1",
            Self::TabDigest => "tab_digest_v1",
            Self::WorkspaceFileSha256 => "workspace_file_sha256_v1",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        Ok(match value {
            "session_presence_v1" => Self::SessionPresence,
            "page_digest_v1" => Self::PageDigest,
            "element_digest_v1" => Self::ElementDigest,
            "tab_digest_v1" => Self::TabDigest,
            "workspace_file_sha256_v1" => Self::WorkspaceFileSha256,
            _ => bail!("unsupported browser observer kind: {value}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserPreparedOperation {
    pub(crate) receipt_id: String,
    pub(crate) objective_id: String,
    pub(crate) objective_revision: i64,
    pub(crate) binding_id: String,
    pub(crate) resource_generation: i64,
    pub(crate) action_fingerprint: String,
    pub(crate) tool_call_id: String,
    pub(crate) action: BrowserAction,
    pub(crate) session_id: String,
    pub(crate) session_generation: i64,
    pub(crate) observer_kind: BrowserObserverKind,
    pub(crate) safe_locator_json: String,
    pub(crate) precondition_digest: Option<String>,
    pub(crate) expected_postcondition_digest: Option<String>,
    pub(crate) now: i64,
}

/// Immutable identity handed to the tool backend. A driver must receive a
/// derived [`BrowserDispatchPermit`]; it must never reconstruct this scope from
/// browser arguments or a lease file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserOperationPermit {
    pub(crate) receipt_id: String,
    pub(crate) objective_id: String,
    pub(crate) objective_revision: i64,
    pub(crate) binding_id: String,
    pub(crate) resource_generation: i64,
    pub(crate) action_fingerprint: String,
    pub(crate) action: BrowserAction,
    pub(crate) session_id: String,
    pub(crate) session_generation: i64,
}

/// Exact authorization for one driver event. `dispatch_generation` fences a
/// stale future from acknowledging or settling a later retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserDispatchPermit {
    pub(crate) operation: BrowserOperationPermit,
    pub(crate) dispatch_owner: Option<String>,
    pub(crate) dispatch_claim_epoch: i64,
    pub(crate) dispatch_generation: i64,
}

/// Browser-domain authority carried from the outer receipt transaction to the
/// native driver. The operation identity is immutable; the optional generic
/// mutation permit is present only for a claimed recovery runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserExecutionPermit {
    pub(crate) operation: BrowserOperationPermit,
    pub(crate) recovery: Option<codefactory_agent_loop::tool::MutationPermit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserContractState {
    Prepared,
    Dispatching,
    Acknowledged,
    Unknown,
    ObservedApplied,
    ObservedNotApplied,
    Conflict,
    SettledCommitted,
    SettledReconciled,
    Cancelled,
}

impl BrowserContractState {
    fn from_str(value: &str) -> Result<Self> {
        Ok(match value {
            "prepared" => Self::Prepared,
            "dispatching" => Self::Dispatching,
            "acknowledged" => Self::Acknowledged,
            "unknown" => Self::Unknown,
            "observed_applied" => Self::ObservedApplied,
            "observed_not_applied" => Self::ObservedNotApplied,
            "conflict" => Self::Conflict,
            "settled_committed" => Self::SettledCommitted,
            "settled_reconciled" => Self::SettledReconciled,
            "cancelled" => Self::Cancelled,
            _ => bail!("unsupported browser recovery state: {value}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserRecoveryContract {
    pub(crate) receipt_id: String,
    pub(crate) objective_id: String,
    pub(crate) objective_revision: i64,
    pub(crate) binding_id: String,
    pub(crate) resource_generation: i64,
    pub(crate) action_fingerprint: String,
    pub(crate) tool_call_id: String,
    pub(crate) action: BrowserAction,
    pub(crate) session_id: String,
    pub(crate) session_generation: i64,
    pub(crate) observer_kind: BrowserObserverKind,
    pub(crate) safe_locator_json: String,
    pub(crate) precondition_digest: Option<String>,
    pub(crate) expected_postcondition_digest: Option<String>,
    state: BrowserContractState,
    replay_policy: BrowserReplayPolicy,
    dispatch_owner: Option<String>,
    dispatch_claim_epoch: i64,
    dispatch_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserObservation {
    Applied {
        observed_digest: Option<String>,
    },
    DefinitelyNotApplied {
        observed_digest: Option<String>,
        /// Negative evidence is authoritative only after the old process/event
        /// future is known unable to send another browser event.
        dispatcher_quiesced: bool,
    },
    StillUnknown {
        observed_digest: Option<String>,
    },
    Conflict {
        observed_digest: Option<String>,
    },
}

#[async_trait::async_trait]
pub(crate) trait BrowserObserver: Send + Sync {
    async fn observe(&self, contract: &BrowserRecoveryContract) -> Result<BrowserObservation>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserRecoveryDisposition {
    Prepared,
    AwaitingSettlement,
    ObservedApplied,
    ReplayableExactGeneration,
    ReplayableDigestCas,
    ObserveOnlyUncertain,
    Conflict,
    SettledCommitted,
    SettledReconciled,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserSettlement {
    Committed,
    Reconciled,
    ReplayableExactGeneration,
    ReplayableDigestCas,
    ObserveOnlyUncertain,
    Conflict,
    Cancelled,
}

#[derive(Clone)]
pub(crate) struct BrowserRecoveryStore {
    pool: SqlitePool,
}

impl BrowserRecoveryStore {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert the browser domain contract in the caller's transaction after
    /// the caller has inserted the outer receipt. The SQLite trigger verifies
    /// exact scope/fingerprint equality and the receipt-id primary key enforces
    /// one-to-one linkage.
    pub(crate) async fn create_prepared_in_tx(
        tx: &mut Transaction<'_, Sqlite>,
        operation: BrowserPreparedOperation,
    ) -> Result<BrowserOperationPermit> {
        validate_prepared_operation(&operation)?;
        sqlx::query(
            "INSERT INTO browser_recovery_contracts
             (receipt_id, objective_id, objective_revision, binding_id,
              resource_generation, action_fingerprint, tool_call_id, action,
              replay_policy, session_id, session_generation, observer_kind,
              safe_locator_json, precondition_digest,
              expected_postcondition_digest, state, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     'prepared', ?, ?)",
        )
        .bind(&operation.receipt_id)
        .bind(&operation.objective_id)
        .bind(operation.objective_revision)
        .bind(&operation.binding_id)
        .bind(operation.resource_generation)
        .bind(&operation.action_fingerprint)
        .bind(&operation.tool_call_id)
        .bind(operation.action.as_str())
        .bind(operation.action.replay_policy().as_str())
        .bind(&operation.session_id)
        .bind(operation.session_generation)
        .bind(operation.observer_kind.as_str())
        .bind(&operation.safe_locator_json)
        .bind(&operation.precondition_digest)
        .bind(&operation.expected_postcondition_digest)
        .bind(operation.now)
        .bind(operation.now)
        .execute(&mut **tx)
        .await?;

        Ok(BrowserOperationPermit {
            receipt_id: operation.receipt_id,
            objective_id: operation.objective_id,
            objective_revision: operation.objective_revision,
            binding_id: operation.binding_id,
            resource_generation: operation.resource_generation,
            action_fingerprint: operation.action_fingerprint,
            action: operation.action,
            session_id: operation.session_id,
            session_generation: operation.session_generation,
        })
    }

    /// CAS the exact prepared (or authoritatively not-applied replay-safe)
    /// operation into `dispatching`. Foreground dispatch has no recovery
    /// permit and is admitted only from an active Objective. Every later retry
    /// requires a live remediation owner/epoch and a higher claim epoch.
    pub(crate) async fn mark_dispatching(
        &self,
        operation: &BrowserOperationPermit,
        recovery_permit: Option<&codefactory_agent_loop::tool::MutationPermit>,
        now: i64,
    ) -> Result<Option<BrowserDispatchPermit>> {
        let (owner, claim_epoch, recovery_id) = match recovery_permit {
            Some(permit)
                if permit.objective_id == operation.objective_id
                    && permit.binding_id.as_deref() == Some(operation.binding_id.as_str())
                    && permit.resource_generation == Some(operation.resource_generation)
                    && permit.claim_epoch >= 1 =>
            {
                (
                    Some(permit.owner.as_str()),
                    permit.claim_epoch,
                    Some(permit.remediation_id.as_str()),
                )
            }
            Some(_) => return Ok(None),
            None => (None, 0, None),
        };

        let updated = if let Some(remediation_id) = recovery_id {
            sqlx::query(
                "UPDATE browser_recovery_contracts AS contract
                 SET state='dispatching', dispatch_owner=?,
                     dispatch_claim_epoch=?,
                     dispatch_generation=dispatch_generation+1,
                     dispatch_started_at=?, acknowledged_at=NULL,
                     observed_at=NULL, updated_at=?
                 WHERE contract.receipt_id=? AND contract.objective_id=?
                   AND contract.objective_revision=? AND contract.binding_id=?
                   AND contract.resource_generation=?
                   AND contract.action_fingerprint=? AND contract.action=?
                   AND contract.session_id=? AND contract.session_generation=?
                   AND contract.state IN ('prepared','observed_not_applied')
                   AND (contract.action NOT IN ('click','press')
                        OR (contract.precondition_digest IS NOT NULL
                            AND contract.expected_postcondition_digest IS NOT NULL
                            AND contract.precondition_digest<>
                                contract.expected_postcondition_digest))
                   AND (contract.state='prepared'
                        OR contract.replay_policy IN ('exact_generation','digest_cas'))
                   AND contract.dispatch_claim_epoch<?
                   AND EXISTS (
                     SELECT 1 FROM side_effect_receipts receipt
                     WHERE receipt.id=contract.receipt_id
                       AND receipt.objective_id=contract.objective_id
                       AND receipt.revision=contract.objective_revision
                       AND receipt.binding_id=contract.binding_id
                       AND receipt.action_fingerprint=contract.action_fingerprint
                       AND receipt.status IN ('started','unknown'))
                   AND EXISTS (
                     SELECT 1 FROM objectives objective
                     JOIN objective_bindings binding
                       ON binding.id=contract.binding_id
                      AND binding.objective_id=objective.id
                     JOIN objective_remediations remediation
                       ON remediation.id=? AND remediation.objective_id=objective.id
                      AND remediation.binding_id=binding.id
                     WHERE objective.id=contract.objective_id
                       AND objective.status='waiting_system'
                       AND objective.remediation_id=remediation.id
                       AND objective.lease_owner=? AND objective.lease_expires_at>?
                       AND binding.resource_generation=contract.resource_generation
                       AND remediation.status='claimed'
                       AND remediation.lease_owner=?
                       AND remediation.attempt_index=?
                       AND remediation.lease_expires_at>?)",
            )
            .bind(owner)
            .bind(claim_epoch)
            .bind(now)
            .bind(now)
            .bind(&operation.receipt_id)
            .bind(&operation.objective_id)
            .bind(operation.objective_revision)
            .bind(&operation.binding_id)
            .bind(operation.resource_generation)
            .bind(&operation.action_fingerprint)
            .bind(operation.action.as_str())
            .bind(&operation.session_id)
            .bind(operation.session_generation)
            .bind(claim_epoch)
            .bind(remediation_id)
            .bind(owner)
            .bind(now)
            .bind(owner)
            .bind(claim_epoch)
            .bind(now)
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                "UPDATE browser_recovery_contracts AS contract
                 SET state='dispatching', dispatch_owner=NULL,
                     dispatch_claim_epoch=0,
                     dispatch_generation=dispatch_generation+1,
                     dispatch_started_at=?, acknowledged_at=NULL,
                     observed_at=NULL, updated_at=?
                 WHERE contract.receipt_id=? AND contract.objective_id=?
                   AND contract.objective_revision=? AND contract.binding_id=?
                   AND contract.resource_generation=?
                   AND contract.action_fingerprint=? AND contract.action=?
                   AND contract.session_id=? AND contract.session_generation=?
                   AND contract.state='prepared'
                   AND (contract.action NOT IN ('click','press')
                        OR (contract.precondition_digest IS NOT NULL
                            AND contract.expected_postcondition_digest IS NOT NULL
                            AND contract.precondition_digest<>
                                contract.expected_postcondition_digest))
                   AND EXISTS (
                     SELECT 1 FROM side_effect_receipts receipt
                     WHERE receipt.id=contract.receipt_id
                       AND receipt.objective_id=contract.objective_id
                       AND receipt.revision=contract.objective_revision
                       AND receipt.binding_id=contract.binding_id
                       AND receipt.action_fingerprint=contract.action_fingerprint
                       AND receipt.status='started')
                   AND EXISTS (
                     SELECT 1 FROM objectives objective
                     JOIN objective_bindings binding
                       ON binding.id=contract.binding_id
                      AND binding.objective_id=objective.id
                     WHERE objective.id=contract.objective_id
                       AND objective.revision=contract.objective_revision
                       AND objective.status='active'
                       AND binding.resource_generation=contract.resource_generation)",
            )
            .bind(now)
            .bind(now)
            .bind(&operation.receipt_id)
            .bind(&operation.objective_id)
            .bind(operation.objective_revision)
            .bind(&operation.binding_id)
            .bind(operation.resource_generation)
            .bind(&operation.action_fingerprint)
            .bind(operation.action.as_str())
            .bind(&operation.session_id)
            .bind(operation.session_generation)
            .execute(&self.pool)
            .await?
        };
        if updated.rows_affected() != 1 {
            return Ok(None);
        }
        let dispatch_generation: i64 = sqlx::query_scalar(
            "SELECT dispatch_generation FROM browser_recovery_contracts
             WHERE receipt_id=?",
        )
        .bind(&operation.receipt_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(Some(BrowserDispatchPermit {
            operation: operation.clone(),
            dispatch_owner: owner.map(str::to_string),
            dispatch_claim_epoch: claim_epoch,
            dispatch_generation,
        }))
    }

    /// Freeze the page state immediately before a dangerous navigation event.
    /// The update is one-shot and intentionally precedes dispatch admission;
    /// an already-satisfied expected URL cannot prove that click/press ran.
    pub(crate) async fn prepare_precondition_digest(
        &self,
        operation: &BrowserOperationPermit,
        digest: &str,
        now: i64,
    ) -> Result<bool> {
        validate_digest("browser dispatch precondition", digest)?;
        let updated = sqlx::query(
            "UPDATE browser_recovery_contracts
             SET precondition_digest=?, updated_at=?
             WHERE receipt_id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND action_fingerprint=? AND action IN ('click','press')
               AND session_id=? AND session_generation=?
               AND state='prepared' AND dispatch_generation=0
               AND precondition_digest IS NULL
               AND expected_postcondition_digest IS NOT NULL
               AND expected_postcondition_digest<>?",
        )
        .bind(digest)
        .bind(now)
        .bind(&operation.receipt_id)
        .bind(&operation.objective_id)
        .bind(operation.objective_revision)
        .bind(&operation.binding_id)
        .bind(operation.resource_generation)
        .bind(&operation.action_fingerprint)
        .bind(&operation.session_id)
        .bind(operation.session_generation)
        .bind(digest)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn record_ack(
        &self,
        permit: &BrowserDispatchPermit,
        evidence_digest: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        if let Some(digest) = evidence_digest {
            validate_digest("browser acknowledgement", digest)?;
        }
        let updated = sqlx::query(
            "UPDATE browser_recovery_contracts
             SET state='acknowledged', observed_digest=COALESCE(?, observed_digest),
                 acknowledged_at=?, updated_at=?
             WHERE receipt_id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND action_fingerprint=? AND action=?
               AND session_id=? AND session_generation=?
               AND state='dispatching' AND dispatch_owner IS ?
               AND dispatch_claim_epoch=? AND dispatch_generation=?",
        )
        .bind(evidence_digest)
        .bind(now)
        .bind(now)
        .bind(&permit.operation.receipt_id)
        .bind(&permit.operation.objective_id)
        .bind(permit.operation.objective_revision)
        .bind(&permit.operation.binding_id)
        .bind(permit.operation.resource_generation)
        .bind(&permit.operation.action_fingerprint)
        .bind(permit.operation.action.as_str())
        .bind(&permit.operation.session_id)
        .bind(permit.operation.session_generation)
        .bind(&permit.dispatch_owner)
        .bind(permit.dispatch_claim_epoch)
        .bind(permit.dispatch_generation)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn prepare_digest_postcondition(
        &self,
        permit: &BrowserDispatchPermit,
        expected_digest: &str,
        now: i64,
    ) -> Result<bool> {
        validate_digest("browser expected postcondition", expected_digest)?;
        let updated = sqlx::query(
            "UPDATE browser_recovery_contracts
             SET expected_postcondition_digest=?, updated_at=?
             WHERE receipt_id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND action_fingerprint=? AND action='screenshot'
               AND replay_policy='digest_cas'
               AND session_id=? AND session_generation=?
               AND state='dispatching' AND dispatch_owner IS ?
               AND dispatch_claim_epoch=? AND dispatch_generation=?
               AND expected_postcondition_digest IS NULL",
        )
        .bind(expected_digest)
        .bind(now)
        .bind(&permit.operation.receipt_id)
        .bind(&permit.operation.objective_id)
        .bind(permit.operation.objective_revision)
        .bind(&permit.operation.binding_id)
        .bind(permit.operation.resource_generation)
        .bind(&permit.operation.action_fingerprint)
        .bind(&permit.operation.session_id)
        .bind(permit.operation.session_generation)
        .bind(&permit.dispatch_owner)
        .bind(permit.dispatch_claim_epoch)
        .bind(permit.dispatch_generation)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Final read-only fence for a dispatch already admitted by
    /// [`mark_dispatching`]. Long local setup (notably launching Chromium)
    /// must call this immediately before the external browser event.
    pub(crate) async fn dispatch_is_current(
        &self,
        permit: &BrowserDispatchPermit,
        now: i64,
    ) -> Result<bool> {
        let current: i64 = if let Some(owner) = permit.dispatch_owner.as_deref() {
            sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM browser_recovery_contracts contract
                 JOIN objectives objective ON objective.id=contract.objective_id
                 JOIN objective_bindings binding
                   ON binding.id=contract.binding_id
                  AND binding.objective_id=objective.id
                 JOIN objective_remediations remediation
                   ON remediation.id=objective.remediation_id
                  AND remediation.objective_id=objective.id
                  AND remediation.binding_id=binding.id
                 WHERE contract.receipt_id=? AND contract.objective_id=?
                   AND contract.binding_id=? AND contract.resource_generation=?
                   AND contract.action_fingerprint=? AND contract.action=?
                   AND contract.session_id=? AND contract.session_generation=?
                   AND contract.state='dispatching' AND contract.dispatch_owner=?
                   AND contract.dispatch_claim_epoch=?
                   AND contract.dispatch_generation=?
                   AND objective.status='waiting_system'
                   AND objective.lease_owner=? AND objective.lease_expires_at>?
                   AND binding.resource_generation=contract.resource_generation
                   AND remediation.status='claimed'
                   AND remediation.lease_owner=?
                   AND remediation.attempt_index=?
                   AND remediation.lease_expires_at>?",
            )
            .bind(&permit.operation.receipt_id)
            .bind(&permit.operation.objective_id)
            .bind(&permit.operation.binding_id)
            .bind(permit.operation.resource_generation)
            .bind(&permit.operation.action_fingerprint)
            .bind(permit.operation.action.as_str())
            .bind(&permit.operation.session_id)
            .bind(permit.operation.session_generation)
            .bind(owner)
            .bind(permit.dispatch_claim_epoch)
            .bind(permit.dispatch_generation)
            .bind(owner)
            .bind(now)
            .bind(owner)
            .bind(permit.dispatch_claim_epoch)
            .bind(now)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM browser_recovery_contracts contract
                 JOIN objectives objective ON objective.id=contract.objective_id
                 JOIN objective_bindings binding
                   ON binding.id=contract.binding_id
                  AND binding.objective_id=objective.id
                 WHERE contract.receipt_id=? AND contract.objective_id=?
                   AND contract.objective_revision=? AND contract.binding_id=?
                   AND contract.resource_generation=?
                   AND contract.action_fingerprint=? AND contract.action=?
                   AND contract.session_id=? AND contract.session_generation=?
                   AND contract.state='dispatching' AND contract.dispatch_owner IS NULL
                   AND contract.dispatch_claim_epoch=0
                   AND contract.dispatch_generation=?
                   AND objective.revision=contract.objective_revision
                   AND objective.status='active'
                   AND binding.resource_generation=contract.resource_generation",
            )
            .bind(&permit.operation.receipt_id)
            .bind(&permit.operation.objective_id)
            .bind(permit.operation.objective_revision)
            .bind(&permit.operation.binding_id)
            .bind(permit.operation.resource_generation)
            .bind(&permit.operation.action_fingerprint)
            .bind(permit.operation.action.as_str())
            .bind(&permit.operation.session_id)
            .bind(permit.operation.session_generation)
            .bind(permit.dispatch_generation)
            .fetch_one(&self.pool)
            .await?
        };
        Ok(current == 1)
    }

    pub(crate) async fn unresolved_session_ids(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT contract.session_id
             FROM browser_recovery_contracts contract
             JOIN objectives objective ON objective.id=contract.objective_id
             WHERE contract.state NOT IN (
                     'settled_committed','settled_reconciled','cancelled'
                   )
               AND objective.status NOT IN ('completed','cancelled')",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub(crate) async fn record_unknown(
        &self,
        permit: &BrowserDispatchPermit,
        now: i64,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE browser_recovery_contracts
             SET state='unknown', updated_at=?
             WHERE receipt_id=? AND objective_id=? AND objective_revision=?
               AND binding_id=? AND resource_generation=?
               AND action_fingerprint=? AND action=?
               AND session_id=? AND session_generation=?
               AND state='dispatching' AND dispatch_owner IS ?
               AND dispatch_claim_epoch=? AND dispatch_generation=?",
        )
        .bind(now)
        .bind(&permit.operation.receipt_id)
        .bind(&permit.operation.objective_id)
        .bind(permit.operation.objective_revision)
        .bind(&permit.operation.binding_id)
        .bind(permit.operation.resource_generation)
        .bind(&permit.operation.action_fingerprint)
        .bind(permit.operation.action.as_str())
        .bind(&permit.operation.session_id)
        .bind(permit.operation.session_generation)
        .bind(&permit.dispatch_owner)
        .bind(permit.dispatch_claim_epoch)
        .bind(permit.dispatch_generation)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        let outer = sqlx::query(
            "UPDATE side_effect_receipts
             SET status='unknown', observed_at=?
             WHERE id=? AND status IN ('started','unknown')",
        )
        .bind(now)
        .bind(&permit.operation.receipt_id)
        .execute(&mut *tx)
        .await?;
        if outer.rows_affected() != 1 {
            tx.rollback().await?;
            bail!("browser outer receipt changed before unknown settlement");
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Run a read-only observer, then persist only its digest result. A
    /// `DefinitelyNotApplied` observation cannot make click/fill/press/open
    /// replayable even when the old dispatcher is known quiesced.
    pub(crate) async fn observe<O: BrowserObserver + ?Sized>(
        &self,
        receipt_id: &str,
        observer: &O,
        now: i64,
    ) -> Result<BrowserRecoveryDisposition> {
        let contract = self.load_contract(receipt_id).await?;
        match disposition_for_state(contract.state, contract.replay_policy) {
            Some(disposition) => return Ok(disposition),
            None => {}
        }
        let observation = observer.observe(&contract).await?;
        let (observed_label, observed_digest, next_state, disposition) = match observation {
            BrowserObservation::Applied { observed_digest } => (
                "applied",
                observed_digest,
                "observed_applied",
                BrowserRecoveryDisposition::ObservedApplied,
            ),
            BrowserObservation::DefinitelyNotApplied {
                observed_digest,
                dispatcher_quiesced,
            } if dispatcher_quiesced
                && contract.replay_policy == BrowserReplayPolicy::ExactGeneration =>
            {
                (
                    "definitely_not_applied",
                    observed_digest,
                    "observed_not_applied",
                    BrowserRecoveryDisposition::ReplayableExactGeneration,
                )
            }
            BrowserObservation::DefinitelyNotApplied {
                observed_digest,
                dispatcher_quiesced,
            } if dispatcher_quiesced
                && contract.replay_policy == BrowserReplayPolicy::DigestCas =>
            {
                (
                    "definitely_not_applied",
                    observed_digest,
                    "observed_not_applied",
                    BrowserRecoveryDisposition::ReplayableDigestCas,
                )
            }
            BrowserObservation::DefinitelyNotApplied {
                observed_digest, ..
            } => (
                "definitely_not_applied",
                observed_digest,
                "unknown",
                BrowserRecoveryDisposition::ObserveOnlyUncertain,
            ),
            BrowserObservation::StillUnknown { observed_digest } => (
                "still_unknown",
                observed_digest,
                "unknown",
                BrowserRecoveryDisposition::ObserveOnlyUncertain,
            ),
            BrowserObservation::Conflict { observed_digest } => (
                "conflict",
                observed_digest,
                "conflict",
                BrowserRecoveryDisposition::Conflict,
            ),
        };
        if let Some(digest) = observed_digest.as_deref() {
            validate_digest("browser observation", digest)?;
        }

        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE browser_recovery_contracts
             SET state=?, last_observation=?, observed_digest=?,
                 observation_count=observation_count+1,
                 observed_at=?, updated_at=?
             WHERE receipt_id=? AND state=? AND dispatch_generation=?",
        )
        .bind(next_state)
        .bind(observed_label)
        .bind(&observed_digest)
        .bind(now)
        .bind(now)
        .bind(receipt_id)
        .bind(state_as_str(contract.state))
        .bind(contract.dispatch_generation)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return self.current_disposition(receipt_id).await;
        }
        let outer = sqlx::query(
            "UPDATE side_effect_receipts
             SET status='unknown', observed_at=?
             WHERE id=? AND status IN ('started','unknown')",
        )
        .bind(now)
        .bind(receipt_id)
        .execute(&mut *tx)
        .await?;
        if outer.rows_affected() != 1 {
            tx.rollback().await?;
            bail!("browser outer receipt changed during observation");
        }
        tx.commit().await?;
        Ok(disposition)
    }

    /// Atomically settle the domain contract and its outer receipt. Unknown,
    /// conflicting and not-applied states remain unresolved fences.
    pub(crate) async fn settle(&self, receipt_id: &str, now: i64) -> Result<BrowserSettlement> {
        let contract = self.load_contract(receipt_id).await?;
        let (next_state, outer_status, result) = match contract.state {
            BrowserContractState::Acknowledged => (
                Some("settled_committed"),
                Some("committed"),
                BrowserSettlement::Committed,
            ),
            BrowserContractState::ObservedApplied => (
                Some("settled_reconciled"),
                Some("reconciled"),
                BrowserSettlement::Reconciled,
            ),
            BrowserContractState::SettledCommitted => return Ok(BrowserSettlement::Committed),
            BrowserContractState::SettledReconciled => return Ok(BrowserSettlement::Reconciled),
            BrowserContractState::ObservedNotApplied => {
                return Ok(match contract.replay_policy {
                    BrowserReplayPolicy::ExactGeneration => {
                        BrowserSettlement::ReplayableExactGeneration
                    }
                    BrowserReplayPolicy::DigestCas => BrowserSettlement::ReplayableDigestCas,
                    BrowserReplayPolicy::NeverAfterDispatch => {
                        BrowserSettlement::ObserveOnlyUncertain
                    }
                })
            }
            BrowserContractState::Conflict => return Ok(BrowserSettlement::Conflict),
            BrowserContractState::Cancelled => return Ok(BrowserSettlement::Cancelled),
            BrowserContractState::Prepared
            | BrowserContractState::Dispatching
            | BrowserContractState::Unknown => return Ok(BrowserSettlement::ObserveOnlyUncertain),
        };

        let mut tx = self.pool.begin().await?;
        let contract_updated = sqlx::query(
            "UPDATE browser_recovery_contracts
             SET state=?, settled_at=?, updated_at=?
             WHERE receipt_id=? AND state=? AND dispatch_generation=?",
        )
        .bind(next_state)
        .bind(now)
        .bind(now)
        .bind(receipt_id)
        .bind(state_as_str(contract.state))
        .bind(contract.dispatch_generation)
        .execute(&mut *tx)
        .await?;
        if contract_updated.rows_affected() != 1 {
            tx.rollback().await?;
            bail!("browser contract changed before settlement");
        }
        let outer_updated = sqlx::query(
            "UPDATE side_effect_receipts
             SET status=?, summary_json=?, observed_at=?
             WHERE id=? AND status IN ('started','unknown')",
        )
        .bind(outer_status)
        .bind(DONE_SUMMARY)
        .bind(now)
        .bind(receipt_id)
        .execute(&mut *tx)
        .await?;
        if outer_updated.rows_affected() != 1 {
            tx.rollback().await?;
            bail!("browser outer receipt changed before settlement");
        }
        let result_json = serde_json::json!({
            "status": "done",
            "browser_receipt_id": contract.receipt_id,
            "reconciled_after_observation": outer_status == Some("reconciled"),
        })
        .to_string();
        let tool_call = sqlx::query(
            "UPDATE tool_calls
             SET status='done', result=?, error=NULL,
                 duration_ms=COALESCE(duration_ms, 0)
             WHERE id=? AND objective_id=? AND binding_id=?
               AND action_signature=? AND resource_generation=?
               AND status IN ('pending','waiting','error')",
        )
        .bind(result_json)
        .bind(&contract.tool_call_id)
        .bind(&contract.objective_id)
        .bind(&contract.binding_id)
        .bind(&contract.action_fingerprint)
        .bind(contract.resource_generation)
        .execute(&mut *tx)
        .await?;
        if tool_call.rows_affected() > 1 {
            tx.rollback().await?;
            bail!("browser settlement matched multiple normalized tool calls");
        }
        tx.commit().await?;
        Ok(result)
    }

    pub(crate) async fn operation_permit(
        &self,
        receipt_id: &str,
    ) -> Result<BrowserOperationPermit> {
        let contract = self.load_contract(receipt_id).await?;
        Ok(BrowserOperationPermit {
            receipt_id: contract.receipt_id,
            objective_id: contract.objective_id,
            objective_revision: contract.objective_revision,
            binding_id: contract.binding_id,
            resource_generation: contract.resource_generation,
            action_fingerprint: contract.action_fingerprint,
            action: contract.action,
            session_id: contract.session_id,
            session_generation: contract.session_generation,
        })
    }

    pub(crate) async fn disposition(&self, receipt_id: &str) -> Result<BrowserRecoveryDisposition> {
        self.current_disposition(receipt_id).await
    }

    pub(crate) async fn receipt_for_scope(
        &self,
        objective_id: &str,
        current_objective_revision: i64,
        binding_id: &str,
        resource_generation: i64,
    ) -> Result<Option<String>> {
        let receipts: Vec<String> = sqlx::query_scalar(
            "SELECT receipt_id FROM browser_recovery_contracts
             WHERE objective_id=? AND objective_revision<=? AND binding_id=?
               AND resource_generation=?
               AND state NOT IN ('settled_committed','settled_reconciled','cancelled')
             ORDER BY created_at DESC LIMIT 2",
        )
        .bind(objective_id)
        .bind(current_objective_revision)
        .bind(binding_id)
        .bind(resource_generation)
        .fetch_all(&self.pool)
        .await?;
        match receipts.as_slice() {
            [] => Ok(None),
            [receipt] => Ok(Some(receipt.clone())),
            _ => bail!("multiple unresolved browser contracts share the current binding"),
        }
    }

    async fn current_disposition(&self, receipt_id: &str) -> Result<BrowserRecoveryDisposition> {
        let contract = self.load_contract(receipt_id).await?;
        Ok(
            disposition_for_state(contract.state, contract.replay_policy)
                .unwrap_or(BrowserRecoveryDisposition::ObserveOnlyUncertain),
        )
    }

    async fn load_contract(&self, receipt_id: &str) -> Result<BrowserRecoveryContract> {
        let row = sqlx::query(
            "SELECT receipt_id, objective_id, objective_revision, binding_id,
                    resource_generation, action_fingerprint, tool_call_id,
                    action, replay_policy, session_id, session_generation,
                    observer_kind, safe_locator_json, precondition_digest,
                    expected_postcondition_digest, state, dispatch_owner,
                    dispatch_claim_epoch, dispatch_generation
             FROM browser_recovery_contracts WHERE receipt_id=?",
        )
        .bind(receipt_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(BrowserRecoveryContract {
            receipt_id: row.try_get("receipt_id")?,
            objective_id: row.try_get("objective_id")?,
            objective_revision: row.try_get("objective_revision")?,
            binding_id: row.try_get("binding_id")?,
            resource_generation: row.try_get("resource_generation")?,
            action_fingerprint: row.try_get("action_fingerprint")?,
            tool_call_id: row.try_get("tool_call_id")?,
            action: BrowserAction::from_str(row.try_get::<String, _>("action")?.as_str())?,
            session_id: row.try_get("session_id")?,
            session_generation: row.try_get("session_generation")?,
            observer_kind: BrowserObserverKind::from_str(
                row.try_get::<String, _>("observer_kind")?.as_str(),
            )?,
            safe_locator_json: row.try_get("safe_locator_json")?,
            precondition_digest: row.try_get("precondition_digest")?,
            expected_postcondition_digest: row.try_get("expected_postcondition_digest")?,
            state: BrowserContractState::from_str(row.try_get::<String, _>("state")?.as_str())?,
            replay_policy: BrowserReplayPolicy::from_str(
                row.try_get::<String, _>("replay_policy")?.as_str(),
            )?,
            dispatch_owner: row.try_get("dispatch_owner")?,
            dispatch_claim_epoch: row.try_get("dispatch_claim_epoch")?,
            dispatch_generation: row.try_get("dispatch_generation")?,
        })
    }
}

fn validate_prepared_operation(operation: &BrowserPreparedOperation) -> Result<()> {
    if operation.receipt_id.is_empty()
        || operation.objective_id.is_empty()
        || operation.binding_id.is_empty()
        || operation.tool_call_id.is_empty()
        || operation.session_id.is_empty()
        || operation.session_id.contains("://")
        || operation.objective_revision < 1
        || operation.resource_generation < 1
        || operation.session_generation < 1
    {
        bail!("browser prepared operation has incomplete or unsafe identity");
    }
    validate_outer_fingerprint(&operation.action_fingerprint)?;
    if let Some(digest) = operation.precondition_digest.as_deref() {
        validate_digest("browser precondition", digest)?;
    }
    if let Some(digest) = operation.expected_postcondition_digest.as_deref() {
        validate_digest("browser expected postcondition", digest)?;
    }
    validate_safe_locator(&operation.safe_locator_json)?;
    Ok(())
}

fn validate_safe_locator(value: &str) -> Result<()> {
    const ALLOWED: [&str; 6] = [
        "session_digest",
        "document_digest",
        "target_digest",
        "tab_digest",
        "path_digest",
        "focus_digest",
    ];
    let parsed: serde_json::Value = serde_json::from_str(value)?;
    let Some(object) = parsed.as_object() else {
        bail!("browser safe locator must be a JSON object");
    };
    for (key, value) in object {
        if !ALLOWED.contains(&key.as_str()) {
            bail!("browser safe locator key is not digest-only: {key}");
        }
        let Some(digest) = value.as_str() else {
            bail!("browser safe locator values must be digests");
        };
        validate_digest("browser safe locator", digest)?;
    }
    Ok(())
}

fn validate_digest(label: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("{label} must be a lowercase SHA-256 digest");
    }
    Ok(())
}

fn validate_outer_fingerprint(value: &str) -> Result<()> {
    let Some(digest) = value.strip_prefix("sha256:") else {
        bail!("browser action fingerprint must use the outer sha256: digest format");
    };
    validate_digest("browser action fingerprint", digest)
}

fn state_as_str(state: BrowserContractState) -> &'static str {
    match state {
        BrowserContractState::Prepared => "prepared",
        BrowserContractState::Dispatching => "dispatching",
        BrowserContractState::Acknowledged => "acknowledged",
        BrowserContractState::Unknown => "unknown",
        BrowserContractState::ObservedApplied => "observed_applied",
        BrowserContractState::ObservedNotApplied => "observed_not_applied",
        BrowserContractState::Conflict => "conflict",
        BrowserContractState::SettledCommitted => "settled_committed",
        BrowserContractState::SettledReconciled => "settled_reconciled",
        BrowserContractState::Cancelled => "cancelled",
    }
}

fn disposition_for_state(
    state: BrowserContractState,
    replay_policy: BrowserReplayPolicy,
) -> Option<BrowserRecoveryDisposition> {
    match state {
        BrowserContractState::Prepared => Some(BrowserRecoveryDisposition::Prepared),
        BrowserContractState::Dispatching | BrowserContractState::Unknown => None,
        BrowserContractState::Acknowledged => Some(BrowserRecoveryDisposition::AwaitingSettlement),
        BrowserContractState::ObservedApplied => Some(BrowserRecoveryDisposition::ObservedApplied),
        BrowserContractState::ObservedNotApplied => Some(match replay_policy {
            BrowserReplayPolicy::ExactGeneration => {
                BrowserRecoveryDisposition::ReplayableExactGeneration
            }
            BrowserReplayPolicy::DigestCas => BrowserRecoveryDisposition::ReplayableDigestCas,
            BrowserReplayPolicy::NeverAfterDispatch => {
                BrowserRecoveryDisposition::ObserveOnlyUncertain
            }
        }),
        BrowserContractState::Conflict => Some(BrowserRecoveryDisposition::Conflict),
        BrowserContractState::SettledCommitted => {
            Some(BrowserRecoveryDisposition::SettledCommitted)
        }
        BrowserContractState::SettledReconciled => {
            Some(BrowserRecoveryDisposition::SettledReconciled)
        }
        BrowserContractState::Cancelled => Some(BrowserRecoveryDisposition::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    const ACTION_FP_A: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ACTION_FP_C: &str =
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DIGEST_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    struct FakeObserver(BrowserObservation);

    #[async_trait::async_trait]
    impl BrowserObserver for FakeObserver {
        async fn observe(
            &self,
            _contract: &BrowserRecoveryContract,
        ) -> anyhow::Result<BrowserObservation> {
            Ok(self.0.clone())
        }
    }

    async fn fixture(action: BrowserAction) -> (SqlitePool, BrowserOperationPermit) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE objectives (
               id TEXT PRIMARY KEY, revision INTEGER NOT NULL,
               status TEXT NOT NULL, remediation_id TEXT,
               lease_owner TEXT, lease_expires_at INTEGER
             );
             CREATE TABLE objective_bindings (
               id TEXT PRIMARY KEY, objective_id TEXT NOT NULL,
               resource_generation INTEGER NOT NULL
             );
             CREATE TABLE objective_remediations (
               id TEXT PRIMARY KEY, objective_id TEXT NOT NULL,
               binding_id TEXT, status TEXT NOT NULL,
               lease_owner TEXT, lease_expires_at INTEGER,
               attempt_index INTEGER NOT NULL
             );
             CREATE TABLE side_effect_receipts (
               id TEXT PRIMARY KEY, objective_id TEXT NOT NULL,
               binding_id TEXT, revision INTEGER NOT NULL,
               action_fingerprint TEXT NOT NULL, idempotency_key TEXT NOT NULL,
               status TEXT NOT NULL, external_identity_digest TEXT,
               summary_json TEXT, created_at INTEGER NOT NULL,
               observed_at INTEGER NOT NULL
             );
             CREATE TABLE tool_calls (
               id TEXT PRIMARY KEY, objective_id TEXT, binding_id TEXT,
               action_signature TEXT, resource_generation INTEGER,
               status TEXT NOT NULL, result TEXT, error TEXT, duration_ms INTEGER
             );",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/0014_browser_recovery_contracts.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objectives
             VALUES ('objective-browser', 7, 'active', NULL, NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             VALUES ('binding-browser', 'objective-browser', 3)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             VALUES ('tool-call-browser', 'objective-browser', 'binding-browser',
                     ?, 3, 'pending', NULL, NULL, NULL)",
        )
        .bind(ACTION_FP_A)
        .execute(&pool)
        .await
        .unwrap();

        let receipt_id = format!("receipt-{}", action.as_str());
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES (?, 'objective-browser', 'binding-browser', 7, ?, ?,
                     'started', 100, 100)",
        )
        .bind(&receipt_id)
        .bind(ACTION_FP_A)
        .bind(format!("idempotency-{}", action.as_str()))
        .execute(&mut *tx)
        .await
        .unwrap();
        let permit = BrowserRecoveryStore::create_prepared_in_tx(
            &mut tx,
            BrowserPreparedOperation {
                receipt_id,
                objective_id: "objective-browser".into(),
                objective_revision: 7,
                binding_id: "binding-browser".into(),
                resource_generation: 3,
                action_fingerprint: ACTION_FP_A.into(),
                tool_call_id: "tool-call-browser".into(),
                action,
                session_id: "codefactory-session-opaque".into(),
                session_generation: 2,
                observer_kind: if action == BrowserAction::Screenshot {
                    BrowserObserverKind::WorkspaceFileSha256
                } else {
                    BrowserObserverKind::ElementDigest
                },
                safe_locator_json: if action == BrowserAction::Screenshot {
                    format!(r#"{{"path_digest":"{DIGEST_B}"}}"#)
                } else {
                    format!(r#"{{"target_digest":"{DIGEST_B}"}}"#)
                },
                precondition_digest: Some(DIGEST_B.into()),
                expected_postcondition_digest: (action != BrowserAction::Screenshot)
                    .then(|| DIGEST_C.into()),
                now: 100,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (pool, permit)
    }

    fn recovery_permit(epoch: i64) -> codefactory_agent_loop::tool::MutationPermit {
        codefactory_agent_loop::tool::MutationPermit {
            objective_id: "objective-browser".into(),
            remediation_id: "remediation-browser".into(),
            owner: "browser-supervisor".into(),
            claim_epoch: epoch,
            binding_id: Some("binding-browser".into()),
            resource_generation: Some(3),
        }
    }

    async fn move_to_recovery(pool: &SqlitePool, epoch: i64) {
        sqlx::query(
            "UPDATE objectives
             SET status='waiting_system', remediation_id='remediation-browser',
                 lease_owner='browser-supervisor', lease_expires_at=999999
             WHERE id='objective-browser'",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_remediations
             VALUES ('remediation-browser', 'objective-browser', 'binding-browser',
                     'claimed', 'browser-supervisor', 999999, ?)",
        )
        .bind(epoch)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn outer_receipt_and_browser_contract_are_one_exact_scope() {
        let (pool, permit) = fixture(BrowserAction::Click).await;
        let row: (String, i64, String, i64, String) = sqlx::query_as(
            "SELECT objective_id, objective_revision, binding_id,
                    resource_generation, action_fingerprint
             FROM browser_recovery_contracts WHERE receipt_id=?",
        )
        .bind(&permit.receipt_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                permit.objective_id,
                permit.objective_revision,
                permit.binding_id,
                permit.resource_generation,
                permit.action_fingerprint,
            )
        );

        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES ('receipt-scope-mismatch', 'objective-browser',
                     'binding-browser', 7, ?, 'idempotency-scope-mismatch',
                     'started', 101, 101)",
        )
        .bind(ACTION_FP_A)
        .execute(&mut *tx)
        .await
        .unwrap();
        let mismatched = BrowserRecoveryStore::create_prepared_in_tx(
            &mut tx,
            BrowserPreparedOperation {
                receipt_id: "receipt-scope-mismatch".into(),
                objective_id: "objective-browser".into(),
                objective_revision: 7,
                binding_id: "binding-browser".into(),
                resource_generation: 3,
                action_fingerprint: ACTION_FP_C.into(),
                tool_call_id: "other-tool-call".into(),
                action: BrowserAction::Click,
                session_id: "other-opaque-session".into(),
                session_generation: 2,
                observer_kind: BrowserObserverKind::ElementDigest,
                safe_locator_json: format!(r#"{{"target_digest":"{DIGEST_B}"}}"#),
                precondition_digest: None,
                expected_postcondition_digest: None,
                now: 101,
            },
        )
        .await;
        assert!(mismatched.is_err());
    }

    #[tokio::test]
    async fn prepared_dispatch_is_a_single_generation_cas() {
        let (pool, permit) = fixture(BrowserAction::Click).await;
        let store = BrowserRecoveryStore::new(pool);
        let first = store
            .mark_dispatching(&permit, None, 101)
            .await
            .unwrap()
            .expect("first dispatcher owns the operation");
        assert_eq!(first.dispatch_generation, 1);
        assert!(store
            .mark_dispatching(&permit, None, 102)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn click_cannot_use_an_already_satisfied_url_as_completion_evidence() {
        let (pool, _) = fixture(BrowserAction::Attach).await;
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES ('receipt-click-precondition', 'objective-browser',
                     'binding-browser', 7, ?, 'click-precondition',
                     'started', 101, 101)",
        )
        .bind(ACTION_FP_A)
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let permit = BrowserRecoveryStore::create_prepared_in_tx(
            &mut tx,
            BrowserPreparedOperation {
                receipt_id: "receipt-click-precondition".into(),
                objective_id: "objective-browser".into(),
                objective_revision: 7,
                binding_id: "binding-browser".into(),
                resource_generation: 3,
                action_fingerprint: ACTION_FP_A.into(),
                tool_call_id: "tool-call-browser".into(),
                action: BrowserAction::Click,
                session_id: "codefactory-click-precondition".into(),
                session_generation: 1,
                observer_kind: BrowserObserverKind::PageDigest,
                safe_locator_json: format!(r#"{{"target_digest":"{DIGEST_B}"}}"#),
                precondition_digest: None,
                expected_postcondition_digest: Some(DIGEST_C.into()),
                now: 101,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let store = BrowserRecoveryStore::new(pool);

        assert!(store
            .mark_dispatching(&permit, None, 102)
            .await
            .unwrap()
            .is_none());
        assert!(!store
            .prepare_precondition_digest(&permit, DIGEST_C, 103)
            .await
            .unwrap());
        assert!(store
            .prepare_precondition_digest(&permit, DIGEST_B, 104)
            .await
            .unwrap());
        assert!(store
            .mark_dispatching(&permit, None, 105)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn final_dispatch_fence_rejects_revision_or_lease_takeover() {
        let (foreground_pool, foreground_operation) = fixture(BrowserAction::Click).await;
        let foreground_store = BrowserRecoveryStore::new(foreground_pool.clone());
        let foreground = foreground_store
            .mark_dispatching(&foreground_operation, None, 101)
            .await
            .unwrap()
            .unwrap();
        assert!(foreground_store
            .dispatch_is_current(&foreground, 102)
            .await
            .unwrap());
        sqlx::query("UPDATE objectives SET revision=8 WHERE id='objective-browser'")
            .execute(&foreground_pool)
            .await
            .unwrap();
        assert!(!foreground_store
            .dispatch_is_current(&foreground, 103)
            .await
            .unwrap());

        let (recovery_pool, recovery_operation) = fixture(BrowserAction::Close).await;
        let recovery_store = BrowserRecoveryStore::new(recovery_pool.clone());
        let first = recovery_store
            .mark_dispatching(&recovery_operation, None, 101)
            .await
            .unwrap()
            .unwrap();
        recovery_store.record_unknown(&first, 102).await.unwrap();
        move_to_recovery(&recovery_pool, 2).await;
        recovery_store
            .observe(
                &recovery_operation.receipt_id,
                &FakeObserver(BrowserObservation::DefinitelyNotApplied {
                    observed_digest: Some(DIGEST_B.into()),
                    dispatcher_quiesced: true,
                }),
                103,
            )
            .await
            .unwrap();
        let recovery = recovery_store
            .mark_dispatching(&recovery_operation, Some(&recovery_permit(2)), 104)
            .await
            .unwrap()
            .unwrap();
        assert!(recovery_store
            .dispatch_is_current(&recovery, 105)
            .await
            .unwrap());
        sqlx::query(
            "UPDATE objectives SET lease_expires_at=105 WHERE id='objective-browser';
             UPDATE objective_remediations SET lease_expires_at=105
             WHERE id='remediation-browser'",
        )
        .execute(&recovery_pool)
        .await
        .unwrap();
        assert!(!recovery_store
            .dispatch_is_current(&recovery, 106)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn unresolved_contract_protects_its_session_until_durable_settlement() {
        let (pool, operation) = fixture(BrowserAction::Click).await;
        let store = BrowserRecoveryStore::new(pool);
        assert_eq!(
            store.unresolved_session_ids().await.unwrap(),
            vec![operation.session_id.clone()]
        );
        let dispatch = store
            .mark_dispatching(&operation, None, 101)
            .await
            .unwrap()
            .unwrap();
        store
            .record_ack(&dispatch, Some(DIGEST_C), 102)
            .await
            .unwrap();
        store.settle(&operation.receipt_id, 103).await.unwrap();
        assert!(store.unresolved_session_ids().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dangerous_actions_never_become_replayable_after_dispatch() {
        for action in [
            BrowserAction::Click,
            BrowserAction::Fill,
            BrowserAction::Press,
        ] {
            let (pool, permit) = fixture(action).await;
            let store = BrowserRecoveryStore::new(pool.clone());
            let dispatch = store
                .mark_dispatching(&permit, None, 101)
                .await
                .unwrap()
                .unwrap();
            store.record_unknown(&dispatch, 102).await.unwrap();
            move_to_recovery(&pool, 2).await;

            let disposition = store
                .observe(
                    &permit.receipt_id,
                    &FakeObserver(BrowserObservation::DefinitelyNotApplied {
                        observed_digest: Some(DIGEST_B.into()),
                        dispatcher_quiesced: true,
                    }),
                    103,
                )
                .await
                .unwrap();
            assert_eq!(
                disposition,
                BrowserRecoveryDisposition::ObserveOnlyUncertain
            );
            assert!(store
                .mark_dispatching(&permit, Some(&recovery_permit(2)), 104)
                .await
                .unwrap()
                .is_none());
        }
    }

    #[tokio::test]
    async fn managed_open_replays_only_after_the_old_browser_is_proven_absent() {
        let (pool, permit) = fixture(BrowserAction::Open).await;
        let store = BrowserRecoveryStore::new(pool.clone());
        let dispatch = store
            .mark_dispatching(&permit, None, 101)
            .await
            .unwrap()
            .unwrap();
        store.record_unknown(&dispatch, 102).await.unwrap();
        move_to_recovery(&pool, 2).await;

        let disposition = store
            .observe(
                &permit.receipt_id,
                &FakeObserver(BrowserObservation::DefinitelyNotApplied {
                    observed_digest: None,
                    dispatcher_quiesced: true,
                }),
                103,
            )
            .await
            .unwrap();
        assert_eq!(
            disposition,
            BrowserRecoveryDisposition::ReplayableExactGeneration
        );
        assert!(store
            .mark_dispatching(&permit, Some(&recovery_permit(2)), 104)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn exact_generation_close_can_retry_only_after_authoritative_negative_observation() {
        let (pool, permit) = fixture(BrowserAction::Close).await;
        let store = BrowserRecoveryStore::new(pool.clone());
        let first = store
            .mark_dispatching(&permit, None, 101)
            .await
            .unwrap()
            .unwrap();
        store.record_unknown(&first, 102).await.unwrap();
        move_to_recovery(&pool, 2).await;
        assert_eq!(
            store
                .observe(
                    &permit.receipt_id,
                    &FakeObserver(BrowserObservation::DefinitelyNotApplied {
                        observed_digest: Some(DIGEST_B.into()),
                        dispatcher_quiesced: true,
                    }),
                    103,
                )
                .await
                .unwrap(),
            BrowserRecoveryDisposition::ReplayableExactGeneration
        );
        let second = store
            .mark_dispatching(&permit, Some(&recovery_permit(2)), 104)
            .await
            .unwrap()
            .expect("exact generation retry is admitted once");
        assert_eq!(second.dispatch_generation, 2);
        assert!(store
            .mark_dispatching(&permit, Some(&recovery_permit(2)), 105)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn screenshot_postcondition_is_write_ahead_once_and_reconciles_without_replay() {
        let (pool, operation) = fixture(BrowserAction::Screenshot).await;
        let store = BrowserRecoveryStore::new(pool.clone());
        let dispatch = store
            .mark_dispatching(&operation, None, 101)
            .await
            .unwrap()
            .unwrap();
        assert!(store
            .prepare_digest_postcondition(&dispatch, DIGEST_C, 102)
            .await
            .unwrap());
        assert!(!store
            .prepare_digest_postcondition(&dispatch, DIGEST_B, 103)
            .await
            .unwrap());
        store.record_unknown(&dispatch, 104).await.unwrap();
        assert_eq!(
            store
                .observe(
                    &operation.receipt_id,
                    &FakeObserver(BrowserObservation::Applied {
                        observed_digest: Some(DIGEST_C.into()),
                    }),
                    105,
                )
                .await
                .unwrap(),
            BrowserRecoveryDisposition::ObservedApplied
        );
        assert_eq!(
            store.settle(&operation.receipt_id, 106).await.unwrap(),
            BrowserSettlement::Reconciled
        );
        assert!(store
            .mark_dispatching(&operation, Some(&recovery_permit(2)), 107)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn ack_and_applied_observation_settle_the_outer_receipt() {
        let (ack_pool, ack_permit) = fixture(BrowserAction::Click).await;
        let ack_store = BrowserRecoveryStore::new(ack_pool.clone());
        let dispatch = ack_store
            .mark_dispatching(&ack_permit, None, 101)
            .await
            .unwrap()
            .unwrap();
        ack_store
            .record_ack(&dispatch, Some(DIGEST_C), 102)
            .await
            .unwrap();
        assert_eq!(
            ack_store.settle(&ack_permit.receipt_id, 103).await.unwrap(),
            BrowserSettlement::Committed
        );
        let ack_status: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE id=?")
                .bind(&ack_permit.receipt_id)
                .fetch_one(&ack_pool)
                .await
                .unwrap();
        assert_eq!(ack_status, "committed");
        let ack_tool_status: String =
            sqlx::query_scalar("SELECT status FROM tool_calls WHERE id='tool-call-browser'")
                .fetch_one(&ack_pool)
                .await
                .unwrap();
        assert_eq!(ack_tool_status, "done");

        let (observed_pool, observed_permit) = fixture(BrowserAction::Fill).await;
        let observed_store = BrowserRecoveryStore::new(observed_pool.clone());
        let dispatch = observed_store
            .mark_dispatching(&observed_permit, None, 101)
            .await
            .unwrap()
            .unwrap();
        observed_store.record_unknown(&dispatch, 102).await.unwrap();
        assert_eq!(
            observed_store
                .observe(
                    &observed_permit.receipt_id,
                    &FakeObserver(BrowserObservation::Applied {
                        observed_digest: Some(DIGEST_C.into()),
                    }),
                    103,
                )
                .await
                .unwrap(),
            BrowserRecoveryDisposition::ObservedApplied
        );
        assert_eq!(
            observed_store
                .settle(&observed_permit.receipt_id, 104)
                .await
                .unwrap(),
            BrowserSettlement::Reconciled
        );
        let observed_status: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE id=?")
                .bind(&observed_permit.receipt_id)
                .fetch_one(&observed_pool)
                .await
                .unwrap();
        assert_eq!(observed_status, "reconciled");
        let observed_tool_status: String =
            sqlx::query_scalar("SELECT status FROM tool_calls WHERE id='tool-call-browser'")
                .fetch_one(&observed_pool)
                .await
                .unwrap();
        assert_eq!(observed_tool_status, "done");
    }

    #[tokio::test]
    async fn stale_session_generation_and_recovery_scope_fail_closed() {
        let (pool, mut permit) = fixture(BrowserAction::Close).await;
        let store = BrowserRecoveryStore::new(pool.clone());
        permit.session_generation += 1;
        assert!(store
            .mark_dispatching(&permit, None, 101)
            .await
            .unwrap()
            .is_none());

        permit.session_generation -= 1;
        move_to_recovery(&pool, 3).await;
        let mut stale = recovery_permit(3);
        stale.resource_generation = Some(4);
        assert!(store
            .mark_dispatching(&permit, Some(&stale), 102)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn raw_browser_identifiers_are_rejected_by_sqlite() {
        let (pool, _) = fixture(BrowserAction::Click).await;
        for (index, unsafe_locator) in [
            r#"{"url":"https://example.com/private"}"#,
            r#"{"tab_id":"123"}"#,
            r#"{"target_digest":"raw fill text"}"#,
            r#"{"pairing_token":"secret"}"#,
        ]
        .into_iter()
        .enumerate()
        {
            let receipt_id = format!("receipt-privacy-{index}");
            sqlx::query(
                "INSERT INTO side_effect_receipts
                 (id, objective_id, binding_id, revision, action_fingerprint,
                  idempotency_key, status, created_at, observed_at)
                 VALUES (?, 'objective-browser', 'binding-browser', 7, ?, ?,
                         'started', 200, 200)",
            )
            .bind(&receipt_id)
            .bind(ACTION_FP_A)
            .bind(format!("idempotency-privacy-{index}"))
            .execute(&pool)
            .await
            .unwrap();
            let result = sqlx::query(
                "INSERT INTO browser_recovery_contracts
                 (receipt_id, objective_id, objective_revision, binding_id,
                  resource_generation, action_fingerprint, tool_call_id,
                  action, replay_policy, session_id, session_generation,
                  observer_kind, safe_locator_json, state, created_at, updated_at)
                 VALUES (?, 'objective-browser', 7, 'binding-browser', 3, ?,
                         'tool-call-privacy', 'click', 'never_after_dispatch',
                         'opaque-session', 2, 'element_digest_v1', ?,
                         'prepared', 200, 200)",
            )
            .bind(&receipt_id)
            .bind(ACTION_FP_A)
            .bind(unsafe_locator)
            .execute(&pool)
            .await;
            assert!(result.is_err());
        }

        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES ('receipt-raw-url-session', 'objective-browser',
                     'binding-browser', 7, ?, 'idempotency-raw-url-session',
                     'started', 200, 200)",
        )
        .bind(ACTION_FP_A)
        .execute(&pool)
        .await
        .unwrap();
        let raw_url_session = sqlx::query(
            "INSERT INTO browser_recovery_contracts
             (receipt_id, objective_id, objective_revision, binding_id,
              resource_generation, action_fingerprint, tool_call_id,
              action, replay_policy, session_id, session_generation,
              observer_kind, safe_locator_json, state, created_at, updated_at)
             VALUES ('receipt-raw-url-session', 'objective-browser', 7,
                     'binding-browser', 3, ?, 'tool-call-privacy', 'click',
                     'never_after_dispatch', 'https://example.com/private', 2,
                     'element_digest_v1', ?, 'prepared', 200, 200)",
        )
        .bind(ACTION_FP_A)
        .bind(format!(r#"{{"target_digest":"{DIGEST_B}"}}"#))
        .execute(&pool)
        .await;
        assert!(raw_url_session.is_err());
    }
}
