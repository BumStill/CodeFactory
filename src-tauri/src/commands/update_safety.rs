// SPDX-License-Identifier: Apache-2.0
//! Fail-closed restart admission for in-app updates.
//!
//! Downloading an updater payload is harmless, but installing it replaces the
//! running bundle and `relaunch()` terminates every in-process agent future.
//! A single backend snapshot keeps all update entry points on the same rule:
//! no install or relaunch while CodeFactory owns live work.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, State};

use crate::commands::tasks::SchedulerHandles;
use crate::commands::terminal::TerminalState;
use crate::errors::AppError;
use crate::AppState;

const UPDATE_RECEIPT_STATES: &str = "'install_started','unknown','applied'";
const UPDATE_TARGET_RESOURCE_KIND: &str = "app_update_target";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateInstallState {
    Queued,
    InstallPermitted,
    DefinitelyNotApplied,
    StillUnknown,
    Conflict,
    Applied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateInstallReceiptView {
    pub id: String,
    pub objective_id: Option<String>,
    pub target_version: String,
    pub target_build: String,
    pub state: UpdateInstallState,
    pub recovery_replay_count: i64,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateInstallAdmission {
    InstallPermitted(UpdateInstallReceiptView),
    DefinitelyNotApplied(UpdateInstallReceiptView),
    StillUnknown(UpdateInstallReceiptView),
    Conflict(UpdateInstallReceiptView),
    Applied(UpdateInstallReceiptView),
}

impl UpdateInstallAdmission {
    pub(crate) fn view(&self) -> &UpdateInstallReceiptView {
        match self {
            Self::InstallPermitted(view)
            | Self::DefinitelyNotApplied(view)
            | Self::StillUnknown(view)
            | Self::Conflict(view)
            | Self::Applied(view) => view,
        }
    }

    pub(crate) const fn state(&self) -> UpdateInstallState {
        match self {
            Self::InstallPermitted(_) => UpdateInstallState::InstallPermitted,
            Self::DefinitelyNotApplied(_) => UpdateInstallState::DefinitelyNotApplied,
            Self::StillUnknown(_) => UpdateInstallState::StillUnknown,
            Self::Conflict(_) => UpdateInstallState::Conflict,
            Self::Applied(_) => UpdateInstallState::Applied,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateClaimPermit {
    pub objective_id: String,
    pub remediation_id: String,
    pub owner: String,
    pub claim_epoch: i64,
    pub binding_id: String,
    pub resource_generation: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ObjectiveBlockers {
    count: i64,
    owners: Vec<String>,
    exempt_objective_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UpdateSafetyStatus {
    pub safe_to_restart: bool,
    pub restart_reserved: bool,
    pub active_chat_turns: usize,
    pub active_task_schedulers: usize,
    pub active_delivery_leases: i64,
    pub active_objective_leases: i64,
    pub objective_blocker_owners: Vec<String>,
    pub pending_permissions: usize,
    pub managed_browser_sessions: usize,
    pub terminal_sessions: usize,
    pub update_objective_id: Option<String>,
    pub update_install_state: Option<UpdateInstallState>,
    pub update_receipt_id: Option<String>,
    pub target_version: Option<String>,
    pub target_build: Option<String>,
}

impl UpdateSafetyStatus {
    fn evaluate(mut self) -> Self {
        self.safe_to_restart = self.active_chat_turns == 0
            && self.active_task_schedulers == 0
            && self.active_delivery_leases == 0
            && self.active_objective_leases == 0
            && self.pending_permissions == 0
            && self.managed_browser_sessions == 0
            && self.terminal_sessions == 0;
        self
    }
}

#[cfg(test)]
async fn count_nonterminal_objectives(pool: &sqlx::SqlitePool) -> Result<i64, sqlx::Error> {
    // Keep this aligned with ObjectiveStatus::is_terminal: waiting states are
    // durable resumable work, not completion, and must survive before restart.
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives
         WHERE status NOT IN ('completed', 'cancelled')",
    )
    .fetch_one(pool)
    .await
}

fn objective_owner(row: &sqlx::sqlite::SqliteRow) -> Result<String, sqlx::Error> {
    let status: String = row.try_get("status")?;
    let domain: String = row.try_get("domain")?;
    let recovery_owner: Option<String> = row.try_get("recovery_owner")?;
    Ok(recovery_owner
        .filter(|owner| !owner.trim().is_empty())
        .unwrap_or_else(|| match status.as_str() {
            "waiting_core_input" => format!("core-input:{domain}"),
            "waiting_authorization" => format!("authorization:{domain}"),
            "waiting_business_decision" => format!("business-decision:{domain}"),
            "legacy_orphan" => "legacy-orphan:system".into(),
            _ => format!("objective-supervisor:{domain}"),
        }))
}

const CURRENT_UPDATE_CLAIM_SQL: &str = "SELECT COUNT(*)
     FROM objectives objective
     JOIN objective_remediations remediation
       ON remediation.id=objective.remediation_id
      AND remediation.objective_id=objective.id
     JOIN objective_bindings binding
       ON binding.id=remediation.binding_id
      AND binding.objective_id=objective.id
     WHERE objective.id=? AND objective.domain='update'
       AND objective.status='waiting_system'
       AND objective.remediation_id=? AND objective.lease_owner=?
       AND objective.lease_expires_at>?
       AND remediation.domain='update'
       AND remediation.status='claimed' AND remediation.lease_owner=?
       AND remediation.lease_expires_at>? AND remediation.attempt_index=?
       AND binding.id=? AND binding.domain='update'
       AND binding.resource_kind=? AND binding.resource_id=?
       AND binding.resource_generation=?";

pub(crate) fn update_target_resource_id(target_version: &str, target_build: &str) -> String {
    format!(
        "v{}:{}b{}:{}",
        target_version.len(),
        target_version,
        target_build.len(),
        target_build
    )
}

pub(crate) fn parse_update_target_resource_id(resource_id: &str) -> Option<(String, String)> {
    let remainder = resource_id.strip_prefix('v')?;
    let version_len_end = remainder.find(':')?;
    let version_len = remainder[..version_len_end].parse::<usize>().ok()?;
    let version_start = version_len_end + 1;
    let version_end = version_start.checked_add(version_len)?;
    let version = remainder.get(version_start..version_end)?.to_string();
    let build_remainder = remainder.get(version_end..)?.strip_prefix('b')?;
    let build_len_end = build_remainder.find(':')?;
    let build_len = build_remainder[..build_len_end].parse::<usize>().ok()?;
    let build_start = build_len_end + 1;
    let build_end = build_start.checked_add(build_len)?;
    if build_end != build_remainder.len() {
        return None;
    }
    let build = build_remainder.get(build_start..build_end)?.to_string();
    Some((version, build))
}

pub(crate) fn validate_update_identity(
    target_version: &str,
    target_build: &str,
) -> Result<(), AppError> {
    if target_version.trim().is_empty() {
        return Err(AppError::Other("update target version is required".into()));
    }
    let build = target_build.trim();
    if build.len() != 40
        || !build
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::Other(
            "update target build_git_sha must be an exact lowercase 40-character commit SHA".into(),
        ));
    }
    Ok(())
}

async fn update_claim_matches_target(
    pool: &sqlx::SqlitePool,
    now: i64,
    claim: &UpdateClaimPermit,
    target_version: &str,
    target_build: &str,
) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(CURRENT_UPDATE_CLAIM_SQL)
        .bind(&claim.objective_id)
        .bind(&claim.remediation_id)
        .bind(&claim.owner)
        .bind(now)
        .bind(&claim.owner)
        .bind(now)
        .bind(claim.claim_epoch)
        .bind(&claim.binding_id)
        .bind(UPDATE_TARGET_RESOURCE_KIND)
        .bind(update_target_resource_id(target_version, target_build))
        .bind(claim.resource_generation)
        .fetch_one(pool)
        .await?;
    Ok(count == 1)
}

async fn update_claim_matches_target_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now: i64,
    claim: &UpdateClaimPermit,
    target_version: &str,
    target_build: &str,
) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(CURRENT_UPDATE_CLAIM_SQL)
        .bind(&claim.objective_id)
        .bind(&claim.remediation_id)
        .bind(&claim.owner)
        .bind(now)
        .bind(&claim.owner)
        .bind(now)
        .bind(claim.claim_epoch)
        .bind(&claim.binding_id)
        .bind(UPDATE_TARGET_RESOURCE_KIND)
        .bind(update_target_resource_id(target_version, target_build))
        .bind(claim.resource_generation)
        .fetch_one(&mut **tx)
        .await?;
    Ok(count == 1)
}

/// Count only currently leased Objective work except a caller-held,
/// target-bound Update mutation permit. Durable user waits and unleased
/// recovery state survive restart, but they do not represent executing work.
/// Merely finding a unique Update claim in SQLite is never enough to infer
/// that it owns this updater request.
async fn load_objective_blockers(
    pool: &sqlx::SqlitePool,
    now: i64,
    exact_update_claim: Option<&UpdateClaimPermit>,
    target_version: &str,
    target_build: &str,
) -> Result<ObjectiveBlockers, sqlx::Error> {
    let exempt_objective_id = if let Some(claim) = exact_update_claim {
        update_claim_matches_target(pool, now, claim, target_version, target_build)
            .await?
            .then_some(claim.objective_id.as_str())
    } else {
        None
    };

    let rows = sqlx::query(
        "SELECT id, status, domain, recovery_owner
         FROM objectives
         WHERE status IN ('active','waiting_system')
           AND lease_owner IS NOT NULL
           AND lease_expires_at>?
           AND (? IS NULL OR id<>?)
         ORDER BY id",
    )
    .bind(now)
    .bind(exempt_objective_id)
    .bind(exempt_objective_id)
    .fetch_all(pool)
    .await?;
    let mut owners = rows
        .iter()
        .map(objective_owner)
        .collect::<Result<Vec<_>, _>>()?;
    owners.sort();
    owners.dedup();
    Ok(ObjectiveBlockers {
        count: rows.len() as i64,
        owners,
        exempt_objective_id: exempt_objective_id.map(str::to_string),
    })
}

async fn ensure_update_receipt_schema(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    let ddl = format!(
        "CREATE TABLE IF NOT EXISTS update_install_receipts (
            id TEXT PRIMARY KEY,
            objective_id TEXT,
            target_version TEXT NOT NULL,
            target_build TEXT NOT NULL,
            pre_install_version TEXT,
            pre_install_build TEXT,
            recovery_replay_count INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL CHECK(status IN ({UPDATE_RECEIPT_STATES})),
            created_at INTEGER NOT NULL,
            observed_at INTEGER NOT NULL,
            UNIQUE(target_version, target_build)
        )"
    );
    sqlx::query(&ddl).execute(pool).await?;
    for (column, declaration) in [
        ("pre_install_version", "TEXT"),
        ("pre_install_build", "TEXT"),
        ("recovery_replay_count", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        let exists = sqlx::query("PRAGMA table_info(update_install_receipts)")
            .fetch_all(pool)
            .await?
            .iter()
            .any(|row| {
                row.try_get::<String, _>("name")
                    .is_ok_and(|name| name == column)
            });
        if !exists {
            sqlx::query(&format!(
                "ALTER TABLE update_install_receipts ADD COLUMN {column} {declaration}"
            ))
            .execute(pool)
            .await?;
        }
    }
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_update_install_receipts_status
         ON update_install_receipts(status, observed_at)",
    )
    .execute(pool)
    .await?;
    // One process-independent unresolved slot fences every target. A second
    // target must observe/reconcile the first attempt before it can install.
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_update_install_receipts_one_unresolved
         ON update_install_receipts ((1))
         WHERE status IN ('install_started', 'unknown')",
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn update_receipt_view(
    row: &sqlx::sqlite::SqliteRow,
    state: UpdateInstallState,
) -> Result<UpdateInstallReceiptView, sqlx::Error> {
    Ok(UpdateInstallReceiptView {
        id: row.try_get("id")?,
        objective_id: row.try_get("objective_id")?,
        target_version: row.try_get("target_version")?,
        target_build: row.try_get("target_build")?,
        state,
        recovery_replay_count: row.try_get("recovery_replay_count")?,
        observed_at: row.try_get("observed_at")?,
    })
}

fn classify_update_receipt(
    row: &sqlx::sqlite::SqliteRow,
    current_version: &str,
    current_build: &str,
) -> Result<UpdateInstallState, sqlx::Error> {
    let target_version: String = row.try_get("target_version")?;
    let target_build: String = row.try_get("target_build")?;
    if current_version == target_version && current_build == target_build {
        return Ok(UpdateInstallState::Applied);
    }
    let pre_install_version: Option<String> = row.try_get("pre_install_version")?;
    let pre_install_build: Option<String> = row.try_get("pre_install_build")?;
    Ok(match (pre_install_version, pre_install_build) {
        (Some(version), Some(build)) if current_version == version && current_build == build => {
            UpdateInstallState::DefinitelyNotApplied
        }
        (Some(_), Some(_)) => UpdateInstallState::Conflict,
        _ => UpdateInstallState::StillUnknown,
    })
}

fn admission_from_observation(
    view: UpdateInstallReceiptView,
    state: UpdateInstallState,
) -> UpdateInstallAdmission {
    match state {
        UpdateInstallState::Applied => UpdateInstallAdmission::Applied(view),
        UpdateInstallState::DefinitelyNotApplied => {
            UpdateInstallAdmission::DefinitelyNotApplied(view)
        }
        UpdateInstallState::Conflict => UpdateInstallAdmission::Conflict(view),
        UpdateInstallState::StillUnknown => UpdateInstallAdmission::StillUnknown(view),
        UpdateInstallState::Queued | UpdateInstallState::InstallPermitted => {
            UpdateInstallAdmission::StillUnknown(view)
        }
    }
}

/// Write-ahead admission for the updater plugin. The partial unique index is a
/// process-independent CAS: at most one target may be unresolved, and every
/// later caller must observe/reconcile it instead of starting another install.
pub(crate) async fn admit_update_install(
    pool: &sqlx::SqlitePool,
    claim: Option<&UpdateClaimPermit>,
    target_version: &str,
    target_build: &str,
    current_version: &str,
    current_build: &str,
    now: i64,
) -> Result<UpdateInstallAdmission, sqlx::Error> {
    ensure_update_receipt_schema(pool).await?;
    let mut tx = pool.begin().await?;

    if let Some(claim) = claim {
        if !update_claim_matches_target_in_tx(&mut tx, now, claim, target_version, target_build)
            .await?
        {
            tx.rollback().await?;
            return Err(sqlx::Error::Protocol(
                "update mutation permit is stale or bound to another target".into(),
            ));
        }
    }

    // Reconcile the single global unresolved slot before considering the
    // requested target. `install_started` itself is not proof that the prior
    // installer stopped: only startup/error observation may move it to
    // `unknown`, preventing a concurrent caller from replaying an in-flight
    // install.
    if let Some(unresolved) = sqlx::query(
        "SELECT id, objective_id, target_version, target_build, pre_install_version,
                pre_install_build, recovery_replay_count, status, observed_at
         FROM update_install_receipts
         WHERE status IN ('install_started', 'unknown')
         ORDER BY created_at, id LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?
    {
        let unresolved_id: String = unresolved.try_get("id")?;
        let unresolved_version: String = unresolved.try_get("target_version")?;
        let unresolved_build: String = unresolved.try_get("target_build")?;
        let unresolved_status: String = unresolved.try_get("status")?;
        let observation = classify_update_receipt(&unresolved, current_version, current_build)?;
        if observation == UpdateInstallState::Applied {
            sqlx::query(
                "UPDATE update_install_receipts
                 SET status='applied', observed_at=? WHERE id=?",
            )
            .bind(now)
            .bind(&unresolved_id)
            .execute(&mut *tx)
            .await?;
            if unresolved_version == target_version && unresolved_build == target_build {
                let row = sqlx::query(
                    "SELECT id, objective_id, target_version, target_build, pre_install_version,
                            pre_install_build, recovery_replay_count, status, observed_at
                     FROM update_install_receipts WHERE id=?",
                )
                .bind(&unresolved_id)
                .fetch_one(&mut *tx)
                .await?;
                let view = update_receipt_view(&row, UpdateInstallState::Applied)?;
                tx.commit().await?;
                return Ok(UpdateInstallAdmission::Applied(view));
            }
        } else {
            let unresolved_objective_id: Option<String> = unresolved.try_get("objective_id")?;
            let replay_count: i64 = unresolved.try_get("recovery_replay_count")?;
            let exact_owned_target = unresolved_version == target_version
                && unresolved_build == target_build
                && claim.is_some_and(|claim| {
                    unresolved_objective_id.as_deref() == Some(claim.objective_id.as_str())
                });
            if unresolved_status == "unknown"
                && observation == UpdateInstallState::DefinitelyNotApplied
                && exact_owned_target
                && replay_count == 0
            {
                let replayed = sqlx::query(
                    "UPDATE update_install_receipts
                     SET status='install_started', recovery_replay_count=1, observed_at=?
                     WHERE id=? AND objective_id=? AND status='unknown'
                       AND recovery_replay_count=0",
                )
                .bind(now)
                .bind(&unresolved_id)
                .bind(
                    &claim
                        .expect("exact_owned_target requires claim")
                        .objective_id,
                )
                .execute(&mut *tx)
                .await?;
                if replayed.rows_affected() == 1 {
                    let row = sqlx::query(
                        "SELECT id, objective_id, target_version, target_build,
                                pre_install_version, pre_install_build,
                                recovery_replay_count, status, observed_at
                         FROM update_install_receipts WHERE id=?",
                    )
                    .bind(&unresolved_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    let view = update_receipt_view(&row, UpdateInstallState::InstallPermitted)?;
                    tx.commit().await?;
                    return Ok(UpdateInstallAdmission::InstallPermitted(view));
                }
            }
            let row = sqlx::query(
                "SELECT id, objective_id, target_version, target_build, pre_install_version,
                        pre_install_build, recovery_replay_count, status, observed_at
                 FROM update_install_receipts WHERE id=?",
            )
            .bind(&unresolved_id)
            .fetch_one(&mut *tx)
            .await?;
            let state = if unresolved_status == "install_started" {
                UpdateInstallState::StillUnknown
            } else {
                observation
            };
            let view = update_receipt_view(&row, state)?;
            tx.commit().await?;
            return Ok(admission_from_observation(view, state));
        }
    }

    if let Some(existing) = sqlx::query(
        "SELECT id, objective_id, target_version, target_build, pre_install_version,
                pre_install_build, recovery_replay_count, status, observed_at
         FROM update_install_receipts
         WHERE target_version=? AND target_build=?",
    )
    .bind(target_version)
    .bind(target_build)
    .fetch_optional(&mut *tx)
    .await?
    {
        let existing_id: String = existing.try_get("id")?;
        let observation = classify_update_receipt(&existing, current_version, current_build)?;
        if observation == UpdateInstallState::Applied {
            sqlx::query(
                "UPDATE update_install_receipts
                 SET status='applied', observed_at=? WHERE id=?",
            )
            .bind(now)
            .bind(&existing_id)
            .execute(&mut *tx)
            .await?;
            let row = sqlx::query(
                "SELECT id, objective_id, target_version, target_build, pre_install_version,
                        pre_install_build, recovery_replay_count, status, observed_at
                 FROM update_install_receipts WHERE id=?",
            )
            .bind(&existing_id)
            .fetch_one(&mut *tx)
            .await?;
            let view = update_receipt_view(&row, UpdateInstallState::Applied)?;
            tx.commit().await?;
            return Ok(UpdateInstallAdmission::Applied(view));
        }

        // Applied history, ownerless legacy rows, and rows owned by a different
        // Objective are immutable evidence. None may be rebound or replayed.
        let state = if existing.try_get::<String, _>("status")? == "applied" {
            UpdateInstallState::Conflict
        } else {
            observation
        };
        let view = update_receipt_view(&existing, state)?;
        tx.commit().await?;
        return Ok(admission_from_observation(view, state));
    }

    let Some(claim) = claim else {
        tx.rollback().await?;
        return Err(sqlx::Error::Protocol(
            "update install requires an exact current supervisor claim".into(),
        ));
    };
    let receipt_id = uuid::Uuid::new_v4().to_string();
    let already_applied = current_version == target_version && current_build == target_build;
    let initial_status = if already_applied {
        "applied"
    } else {
        "install_started"
    };
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO update_install_receipts
         (id, objective_id, target_version, target_build,
          pre_install_version, pre_install_build, recovery_replay_count,
          status, created_at, observed_at)
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?)",
    )
    .bind(&receipt_id)
    .bind(&claim.objective_id)
    .bind(target_version)
    .bind(target_build)
    .bind(current_version)
    .bind(current_build)
    .bind(initial_status)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;

    if inserted {
        let row = sqlx::query(
            "SELECT id, objective_id, target_version, target_build, pre_install_version,
                    pre_install_build, recovery_replay_count, status, observed_at
             FROM update_install_receipts WHERE id=?",
        )
        .bind(&receipt_id)
        .fetch_one(&mut *tx)
        .await?;
        let initial_state = if already_applied {
            UpdateInstallState::Applied
        } else {
            UpdateInstallState::InstallPermitted
        };
        let view = update_receipt_view(&row, initial_state)?;
        tx.commit().await?;
        return Ok(if already_applied {
            UpdateInstallAdmission::Applied(view)
        } else {
            UpdateInstallAdmission::InstallPermitted(view)
        });
    }

    // A concurrent process won the single unresolved slot. Observe whichever
    // target it admitted; never convert the loser into another mutation.
    let row = sqlx::query(
        "SELECT id, objective_id, target_version, target_build, pre_install_version,
                pre_install_build, recovery_replay_count, status, observed_at
         FROM update_install_receipts
         WHERE status IN ('install_started', 'unknown')
            OR (target_version=? AND target_build=?)
         ORDER BY CASE WHEN status IN ('install_started', 'unknown') THEN 0 ELSE 1 END,
                  created_at, id
         LIMIT 1",
    )
    .bind(target_version)
    .bind(target_build)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        sqlx::Error::Protocol("update receipt admission lost without observable winner".into())
    })?;
    let state = classify_update_receipt(&row, current_version, current_build)?;
    let state = if row.try_get::<String, _>("status")? == "install_started" {
        UpdateInstallState::StillUnknown
    } else {
        state
    };
    let view = update_receipt_view(&row, state)?;
    tx.commit().await?;
    Ok(admission_from_observation(view, state))
}

pub(crate) async fn observe_latest_update_install(
    pool: &sqlx::SqlitePool,
    current_version: &str,
    current_build: &str,
    now: i64,
) -> Result<Option<UpdateInstallReceiptView>, sqlx::Error> {
    ensure_update_receipt_schema(pool).await?;
    let Some(row) = sqlx::query(
        "SELECT id, objective_id, target_version, target_build, pre_install_version,
                pre_install_build, recovery_replay_count, status, observed_at
         FROM update_install_receipts
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let id: String = row.try_get("id")?;
    let state = classify_update_receipt(&row, current_version, current_build)?;
    if state == UpdateInstallState::Applied {
        sqlx::query(
            "UPDATE update_install_receipts
             SET status='applied', observed_at=? WHERE id=?",
        )
        .bind(now)
        .bind(&id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE update_install_receipts
             SET status='unknown', observed_at=?
             WHERE id=? AND status='install_started'",
        )
        .bind(now)
        .bind(&id)
        .execute(pool)
        .await?;
    }
    let row = sqlx::query(
        "SELECT id, objective_id, target_version, target_build, pre_install_version,
                pre_install_build, recovery_replay_count, status, observed_at
         FROM update_install_receipts WHERE id=?",
    )
    .bind(&id)
    .fetch_one(pool)
    .await?;
    update_receipt_view(&row, state).map(Some)
}

/// Synthetic previous-install state consumed by the exact release executable.
///
/// The fixture is deliberately file-backed and reopened before observation so
/// an in-memory happy path cannot stand in for the updater restart boundary.
/// The previous version/build comes from the prior public release manifest in
/// the release workflow; the current identity comes from the candidate binary.
pub(crate) async fn run_update_upgrade_smoke_fixture(
    state_path: &std::path::Path,
    previous_version: &str,
    previous_build: &str,
    current_version: &str,
    current_build: &str,
) -> Result<serde_json::Value, AppError> {
    validate_update_identity(previous_version, previous_build)?;
    validate_update_identity(current_version, current_build)?;
    if previous_version == current_version || previous_build == current_build {
        return Err(AppError::Other(
            "update upgrade smoke requires distinct previous and current identities".into(),
        ));
    }

    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(state_path)
        .create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options.clone())
        .await?;
    crate::agent::objective::ensure_schema(&pool).await?;
    let store = crate::agent::objective::ObjectiveStore::new(pool.clone());
    let fixture_key = uuid::Uuid::new_v4().to_string();
    let historical_id = format!("upgrade-history-{fixture_key}");
    store
        .create(crate::agent::objective::CreateObjective {
            id: historical_id.clone(),
            kind: crate::agent::objective::ObjectiveKind::LocalMutation,
            session_id: Some(format!("upgrade-session-{fixture_key}")),
            root_turn_id: None,
            domain: crate::agent::objective::RecoveryDomain::Chat,
            requested_acceptance: "historical_work_remains_durable".into(),
            created_surface: "update_upgrade_smoke".into(),
        })
        .await
        .map_err(|error| AppError::Other(format!("create upgrade fixture objective: {error}")))?;
    let update_objective_id =
        ensure_update_objective(&pool, current_version, current_build).await?;
    ensure_update_receipt_schema(&pool).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let receipt_id = format!("upgrade-receipt-{fixture_key}");
    sqlx::query(
        "INSERT INTO update_install_receipts
         (id, objective_id, target_version, target_build,
          pre_install_version, pre_install_build, recovery_replay_count,
          status, created_at, observed_at)
         VALUES (?, ?, ?, ?, ?, ?, 0, 'install_started', ?, ?)",
    )
    .bind(&receipt_id)
    .bind(&update_objective_id)
    .bind(current_version)
    .bind(current_build)
    .bind(previous_version)
    .bind(previous_build)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await?;
    pool.close().await;

    let reopened = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    crate::agent::objective::ensure_schema(&reopened).await?;
    let first = observe_latest_update_install(&reopened, current_version, current_build, now + 1)
        .await?
        .ok_or_else(|| AppError::Other("upgrade receipt disappeared after restart".into()))?;
    let second = observe_latest_update_install(&reopened, current_version, current_build, now + 2)
        .await?
        .ok_or_else(|| AppError::Other("upgrade receipt disappeared on second restart".into()))?;
    let update_receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM update_install_receipts WHERE id=?")
            .bind(&receipt_id)
            .fetch_one(&reopened)
            .await?;
    let historical_objective_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives WHERE id=? AND status NOT IN ('completed','failed','cancelled')",
    )
    .bind(&historical_id)
    .fetch_one(&reopened)
    .await?;
    let persisted_objective: Option<String> =
        sqlx::query_scalar("SELECT objective_id FROM update_install_receipts WHERE id=?")
            .bind(&receipt_id)
            .fetch_optional(&reopened)
            .await?
            .flatten();
    reopened.close().await;

    Ok(serde_json::json!({
        "scenario_id": "E2E-006",
        "status": if first.state == UpdateInstallState::Applied
            && second.state == UpdateInstallState::Applied
            && update_receipt_count == 1
            && historical_objective_count == 1
            && persisted_objective.as_deref() == Some(update_objective_id.as_str())
        { "pass" } else { "fail" },
        "previous_version": previous_version,
        "previous_build": previous_build,
        "current_version": current_version,
        "current_build": current_build,
        "first_observation": first.state,
        "second_observation": second.state,
        "update_receipt_count": update_receipt_count,
        "historical_objective_count": historical_objective_count,
        "same_update_objective": persisted_objective.as_deref() == Some(update_objective_id.as_str()),
        "database_reopened": true,
    }))
}

pub(crate) async fn mark_update_install_unknown(
    pool: &sqlx::SqlitePool,
    receipt_id: &str,
) -> Result<(), sqlx::Error> {
    ensure_update_receipt_schema(pool).await?;
    sqlx::query(
        "UPDATE update_install_receipts
         SET status='unknown', observed_at=?
         WHERE id=? AND status='install_started'",
    )
    .bind(chrono::Utc::now().timestamp_millis())
    .bind(receipt_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) fn current_app_identity(app: &AppHandle) -> Result<(String, String), AppError> {
    let version = app.package_info().version.to_string();
    let build = option_env!("CODEFACTORY_BUILD_GIT_SHA")
        .map(str::trim)
        .filter(|build| !build.is_empty())
        .ok_or_else(|| {
            AppError::Other(
                "current app build_git_sha is unavailable; update remains observe-only".into(),
            )
        })?
        .to_string();
    validate_update_identity(&version, &build)?;
    Ok((version, build))
}

async fn count_active_delivery_leases(
    pool: &sqlx::SqlitePool,
    now: i64,
) -> Result<i64, sqlx::Error> {
    // A takeover claim is observe-only until its positive epoch has been
    // reconciled. Its lease serializes observers, but it cannot authorize a
    // local or remote mutation and is safe to interrupt for an app restart.
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_runs
         WHERE status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
           AND lease_owner IS NOT NULL
           AND lease_expires_at IS NOT NULL
           AND lease_expires_at > ?
           AND claim_epoch > 0
           AND reconciled_claim_epoch = claim_epoch",
    )
    .bind(now)
    .fetch_one(pool)
    .await
}

pub(crate) async fn ensure_update_objective(
    pool: &sqlx::SqlitePool,
    target_version: &str,
    target_build: &str,
) -> Result<String, AppError> {
    use crate::agent::objective::{
        current_process_instance, DecisionRouter, ObjectiveStatus, ObjectiveStore, RecoveryDomain,
        RouteSignal,
    };

    validate_update_identity(target_version, target_build)?;
    crate::agent::objective::ensure_schema(pool).await?;
    let resource_id = update_target_resource_id(target_version, target_build);
    if let Some(objective_id) = sqlx::query_scalar::<_, String>(
        "SELECT binding.objective_id
         FROM objective_bindings binding
         JOIN objectives objective ON objective.id=binding.objective_id
         WHERE binding.domain='update' AND binding.resource_kind=?
           AND binding.resource_id=?
         ORDER BY binding.resource_generation DESC LIMIT 1",
    )
    .bind(UPDATE_TARGET_RESOURCE_KIND)
    .bind(&resource_id)
    .fetch_optional(pool)
    .await?
    {
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .get(&objective_id)
            .await
            .map_err(|error| AppError::Other(error.to_string()))?
            .ok_or_else(|| AppError::Other("bound update objective disappeared".into()))?;
        if objective.status == ObjectiveStatus::WaitingCoreInput
            && objective.failure_code.as_deref()
                == Some(crate::agent::objective::TECHNICAL_RECOVERY_EXHAUSTED)
        {
            let mut decision = DecisionRouter::route(
                &objective,
                RouteSignal::CapabilityRestored {
                    domain: RecoveryDomain::Update,
                    reason: "update_objective_rearmed".into(),
                    next_observation_at: chrono::Utc::now().timestamp_millis(),
                    resume_cursor: Some(resource_id.clone()),
                },
            )
            .map_err(|error| AppError::Other(error.to_string()))?;
            decision.request_key = None;
            match store.apply_decision(objective.revision, decision).await {
                Ok(_) => {}
                Err(error) if error.to_string().contains("revision") => {}
                Err(error) => return Err(AppError::Other(error.to_string())),
            }
        }
        return Ok(objective_id);
    }

    if let Some((objective_id, existing_resource)) = sqlx::query_as::<_, (String, String)>(
        "SELECT objective.id, binding.resource_id
         FROM objectives objective
         JOIN objective_bindings binding ON binding.objective_id=objective.id
         WHERE objective.domain='update'
           AND objective.status NOT IN ('completed','cancelled','legacy_orphan')
           AND binding.domain='update' AND binding.resource_kind=?
         ORDER BY objective.created_at, objective.id LIMIT 1",
    )
    .bind(UPDATE_TARGET_RESOURCE_KIND)
    .fetch_optional(pool)
    .await?
    {
        let store = ObjectiveStore::new(pool.clone());
        let existing = store
            .get(&objective_id)
            .await
            .map_err(|error| AppError::Other(error.to_string()))?
            .ok_or_else(|| AppError::Other("conflicting update objective disappeared".into()))?;
        if existing.status == ObjectiveStatus::WaitingCoreInput
            && existing.failure_code.as_deref()
                == Some(crate::agent::objective::TECHNICAL_RECOVERY_EXHAUSTED)
        {
            let mut decision = existing.as_decision();
            decision.decision_type = crate::agent::objective::DecisionType::FailedInternal;
            decision.status = ObjectiveStatus::LegacyOrphan;
            decision.failure_code = Some("update_target_superseded".into());
            decision.failure_signature = Some(format!(
                "sha256:{:x}",
                Sha256::digest(
                    format!("{}\0{}\0{}", objective_id, existing_resource, resource_id).as_bytes()
                )
            ));
            decision.recovery_owner = None;
            decision.remediation_id = None;
            decision.next_observation_at = None;
            decision.next_action_authorized = false;
            decision.requires_user_action = false;
            decision.request_key = None;
            decision.decision_key = None;
            match store.apply_decision(existing.revision, decision).await {
                Ok(_) => {}
                Err(error) if error.to_string().contains("revision") => {
                    return Err(AppError::Other(
                        "update target changed while reconciling the exhausted target".into(),
                    ));
                }
                Err(error) => return Err(AppError::Other(error.to_string())),
            }
        } else {
            return Err(AppError::Other(format!(
                "update target {resource_id} cannot replace the system-owned target {existing_resource} on objective {objective_id}"
            )));
        }
    }

    let objective_id = uuid::Uuid::new_v4().to_string();
    let binding_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let process_instance = current_process_instance();
    let identity_digest = format!(
        "sha256:{:x}",
        Sha256::digest(
            format!(
                "{}\0{}\0{}",
                objective_id, UPDATE_TARGET_RESOURCE_KIND, resource_id
            )
            .as_bytes()
        )
    );
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO objectives
         (id, revision, kind, status, decision_type, domain, requested_acceptance,
          requires_user_action, recovery_owner, created_surface,
          created_process_instance, last_observed_process_instance,
          last_progress_at, created_at, updated_at)
         VALUES (?, 1, 'local_mutation', 'active', 'continue', 'update',
                 'installed_exact_update', 0, 'objective-supervisor:update',
                 'updater', ?, ?, ?, ?, ?)",
    )
    .bind(&objective_id)
    .bind(&process_instance)
    .bind(&process_instance)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let bound = sqlx::query(
        "INSERT OR IGNORE INTO objective_bindings
         (id, objective_id, domain, resource_kind, resource_id,
          resource_generation, identity_digest, resume_cursor, created_at, updated_at)
         VALUES (?, ?, 'update', ?, ?, 1, ?, ?, ?, ?)",
    )
    .bind(&binding_id)
    .bind(&objective_id)
    .bind(UPDATE_TARGET_RESOURCE_KIND)
    .bind(&resource_id)
    .bind(&identity_digest)
    .bind(&resource_id)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    if bound.rows_affected() != 1 {
        tx.rollback().await?;
        return sqlx::query_scalar::<_, String>(
            "SELECT objective_id FROM objective_bindings
             WHERE domain='update' AND resource_kind=? AND resource_id=?
             ORDER BY resource_generation DESC LIMIT 1",
        )
        .bind(UPDATE_TARGET_RESOURCE_KIND)
        .bind(&resource_id)
        .fetch_one(pool)
        .await
        .map_err(AppError::from);
    }
    tx.commit().await?;

    let store = ObjectiveStore::new(pool.clone());
    let objective = store
        .get(&objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other("queued update objective disappeared".into()))?;
    if objective.status == ObjectiveStatus::Active {
        let decision = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Update,
                failure_code: "update_install_requested".into(),
                failure_signature: format!(
                    "sha256:{:x}",
                    Sha256::digest(format!("{}\0{}", objective_id, resource_id).as_bytes())
                ),
                next_observation_at: now,
                resume_cursor: Some(resource_id),
            },
        )
        .map_err(|error| AppError::Other(error.to_string()))?;
        match store.apply_decision(objective.revision, decision).await {
            Ok(_) => {}
            Err(error) if error.to_string().contains("revision") => {}
            Err(error) => return Err(AppError::Other(error.to_string())),
        }
    }
    Ok(objective_id)
}

/// Observe whether a claimed Update remediation has reached a restart-safe
/// point without reserving the process or writing an install receipt. The
/// mutating admission path repeats the same checks under the same runtime
/// locks, so a later race still fails closed.
pub(crate) async fn observe_update_restart_safety_inner(
    target_version: &str,
    target_build: &str,
    claim_permit: &UpdateClaimPermit,
    state: &AppState,
    schedulers: &SchedulerHandles,
    terminals: &TerminalState,
) -> Result<UpdateSafetyStatus, AppError> {
    validate_update_identity(target_version.trim(), target_build.trim())?;
    let target_version = target_version.trim();
    let target_build = target_build.trim();
    let chat_turns = state.chat_cancels.lock().await;
    let task_schedulers = schedulers.lock().await;
    let permissions = state.pending_permissions.lock().await;
    let terminal_map = terminals.0.lock().await;
    let now = chrono::Utc::now().timestamp_millis();
    let pool = state.db.read().await;
    let objective_blockers =
        load_objective_blockers(&pool, now, Some(claim_permit), target_version, target_build)
            .await?;
    if objective_blockers.exempt_objective_id.is_none() {
        return Err(AppError::Other(
            "update mutation permit is stale or bound to another target".into(),
        ));
    }
    let restart_reserved = state.update_restart_reserved.load(Ordering::SeqCst);
    let mut status = UpdateSafetyStatus {
        safe_to_restart: false,
        restart_reserved,
        active_chat_turns: chat_turns.len(),
        active_task_schedulers: task_schedulers.len(),
        active_delivery_leases: count_active_delivery_leases(&pool, now).await?,
        active_objective_leases: objective_blockers.count,
        objective_blocker_owners: objective_blockers.owners,
        pending_permissions: permissions.len(),
        managed_browser_sessions: crate::tools::browser_session::list_managed_sessions().len(),
        terminal_sessions: terminal_map.len(),
        update_objective_id: objective_blockers.exempt_objective_id,
        update_install_state: None,
        update_receipt_id: None,
        target_version: Some(target_version.to_string()),
        target_build: Some(target_build.to_string()),
    }
    .evaluate();
    if restart_reserved {
        status.safe_to_restart = false;
    }
    Ok(status)
}

pub(crate) async fn reserve_update_install_inner(
    target_version: String,
    target_build: String,
    claim_permit: Option<UpdateClaimPermit>,
    app: &AppHandle,
    state: &AppState,
    schedulers: &SchedulerHandles,
    terminals: &TerminalState,
) -> Result<UpdateSafetyStatus, AppError> {
    validate_update_identity(target_version.trim(), target_build.trim())?;
    let target_version = target_version.trim().to_string();
    let target_build = target_build.trim().to_string();
    let queued_objective_id = if claim_permit.is_none() {
        let pool = state.db.read().await;
        Some(ensure_update_objective(&pool, &target_version, &target_build).await?)
    } else {
        None
    };
    // The Objective supervisor takes this same gate around claim admission.
    // Once this guard is held, its already-admitted work is visible through
    // durable leases, and no new remediation can slip between our safety
    // snapshot and the restart reservation bit.
    let _restart_admission = state.update_restart_admission.lock().await;
    // Hold every admission map until the reservation bit is set. Each producer
    // rechecks that bit while holding its own map, closing the check/install
    // race instead of relying on a best-effort snapshot.
    let chat_turns = state.chat_cancels.lock().await;
    let task_schedulers = schedulers.lock().await;
    let permissions = state.pending_permissions.lock().await;
    let terminal_map = terminals.0.lock().await;
    let active_chat_turns = chat_turns.len();
    let active_task_schedulers = task_schedulers.len();
    let pending_permissions = permissions.len();
    let terminal_sessions = terminal_map.len();
    let managed_browser_sessions = crate::tools::browser_session::list_managed_sessions().len();

    // A delivery worker can outlive the chat future that started it. Its
    // unexpired durable lease is therefore independent restart-blocking work.
    let now = chrono::Utc::now().timestamp_millis();
    let pool = state.db.read().await;
    ensure_update_receipt_schema(&pool).await?;
    let active_delivery_leases = count_active_delivery_leases(&pool, now).await?;
    let objective_blockers = load_objective_blockers(
        &pool,
        now,
        claim_permit.as_ref(),
        &target_version,
        &target_build,
    )
    .await?;
    if claim_permit.is_some() && objective_blockers.exempt_objective_id.is_none() {
        return Err(AppError::Other(
            "update mutation permit is stale or bound to another target".into(),
        ));
    }

    let mut status = UpdateSafetyStatus {
        safe_to_restart: false,
        restart_reserved: false,
        active_chat_turns,
        active_task_schedulers,
        active_delivery_leases,
        active_objective_leases: objective_blockers.count,
        objective_blocker_owners: objective_blockers.owners,
        pending_permissions,
        managed_browser_sessions,
        terminal_sessions,
        update_objective_id: objective_blockers
            .exempt_objective_id
            .clone()
            .or_else(|| queued_objective_id.clone()),
        update_install_state: None,
        update_receipt_id: None,
        target_version: Some(target_version.clone()),
        target_build: Some(target_build.clone()),
    }
    .evaluate();
    if queued_objective_id.is_some() {
        status.safe_to_restart = false;
        status.restart_reserved = false;
        status.update_install_state = Some(UpdateInstallState::Queued);
        return Ok(status);
    }
    if status.safe_to_restart {
        // Resolve both proof dimensions before taking the process reservation;
        // missing build identity must fail closed without leaving it stuck.
        let (current_version, current_build) = current_app_identity(app)?;
        status.restart_reserved = state
            .update_restart_reserved
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        status.safe_to_restart = status.restart_reserved;
        if status.restart_reserved {
            let admission_now = chrono::Utc::now().timestamp_millis();
            let admission = admit_update_install(
                &pool,
                claim_permit.as_ref(),
                &target_version,
                &target_build,
                &current_version,
                &current_build,
                admission_now,
            )
            .await;
            match admission {
                Ok(admission) => {
                    status.update_objective_id = admission.view().objective_id.clone();
                    status.update_install_state = Some(admission.state());
                    status.update_receipt_id = Some(admission.view().id.clone());
                    if !matches!(admission, UpdateInstallAdmission::InstallPermitted(_)) {
                        state.update_restart_reserved.store(false, Ordering::SeqCst);
                        status.restart_reserved = false;
                        status.safe_to_restart = false;
                    }
                }
                Err(error) => {
                    state.update_restart_reserved.store(false, Ordering::SeqCst);
                    return Err(error.into());
                }
            }
        }
    }
    Ok(status)
}

#[tauri::command]
pub async fn reserve_update_install(
    target_version: String,
    target_build: String,
    claim_permit: Option<UpdateClaimPermit>,
    app: AppHandle,
    state: State<'_, AppState>,
    schedulers: State<'_, SchedulerHandles>,
    terminals: State<'_, TerminalState>,
) -> Result<UpdateSafetyStatus, AppError> {
    reserve_update_install_inner(
        target_version,
        target_build,
        claim_permit,
        &app,
        &state,
        &schedulers,
        &terminals,
    )
    .await
}

/// Startup observation for a prior updater write-ahead receipt. A matching
/// package version *and* build proves the install applied; any other state is
/// retained as observe-only and never authorizes another install.
#[tauri::command]
pub async fn observe_update_install(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<UpdateInstallReceiptView>, AppError> {
    let (current_version, current_build) = current_app_identity(&app)?;
    let pool = state.db.read().await;
    Ok(observe_latest_update_install(
        &pool,
        &current_version,
        &current_build,
        chrono::Utc::now().timestamp_millis(),
    )
    .await?)
}

#[tauri::command]
pub async fn release_update_install_reservation(
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    release_update_install_reservation_inner(&state);
    Ok(())
}

pub(crate) fn release_update_install_reservation_inner(state: &AppState) {
    state.update_restart_reserved.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::{
        admit_update_install, count_active_delivery_leases, count_nonterminal_objectives,
        ensure_update_objective, load_objective_blockers, observe_latest_update_install,
        run_update_upgrade_smoke_fixture, update_target_resource_id, UpdateClaimPermit,
        UpdateInstallAdmission, UpdateInstallState, UpdateSafetyStatus,
    };
    use crate::agent::objective::{ObjectiveStatus, ObjectiveStore, TECHNICAL_RECOVERY_EXHAUSTED};

    const CURRENT_VERSION: &str = "1.79.0";
    const CURRENT_BUILD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TARGET_VERSION: &str = "1.80.0";
    const TARGET_BUILD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[tokio::test]
    async fn previous_to_current_fixture_reconciles_once_with_historical_stuck_objective() {
        let fixture = tempfile::tempdir().unwrap();
        let evidence = run_update_upgrade_smoke_fixture(
            &fixture.path().join("upgrade.db"),
            CURRENT_VERSION,
            CURRENT_BUILD,
            TARGET_VERSION,
            TARGET_BUILD,
        )
        .await
        .unwrap();

        assert_eq!(evidence["scenario_id"], "E2E-006");
        assert_eq!(evidence["status"], "pass");
        assert_eq!(evidence["update_receipt_count"], 1);
        assert_eq!(evidence["historical_objective_count"], 1);
        assert_eq!(evidence["first_observation"], "applied");
        assert_eq!(evidence["second_observation"], "applied");
        assert_eq!(evidence["same_update_objective"], true);
    }

    async fn claimed_update_permit(
        pool: &sqlx::SqlitePool,
        objective_id: &str,
        owner: &str,
        target_version: &str,
        target_build: &str,
    ) -> UpdateClaimPermit {
        use crate::agent::objective::{
            CreateObjective, DecisionRouter, ObjectiveKind, ObjectiveStore, RecoveryDomain,
            RouteSignal,
        };

        crate::agent::objective::ensure_schema(pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: objective_id.into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: None,
                root_turn_id: None,
                domain: RecoveryDomain::Update,
                requested_acceptance: "installed_exact_update".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let resource_id = update_target_resource_id(target_version, target_build);
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES (?, ?, 'update', 'app_update_target', ?, 1, ?, ?, ?, ?)",
        )
        .bind(format!("binding-{objective_id}"))
        .bind(objective_id)
        .bind(&resource_id)
        .bind(format!("sha256:{objective_id}"))
        .bind(&resource_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Update,
                failure_code: "update_install_requested".into(),
                failure_signature: format!("sha256:{objective_id}:update"),
                next_observation_at: now,
                resume_cursor: Some(resource_id),
            },
        )
        .unwrap();
        store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        let claim = store
            .claim_due_remediations(owner, 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        UpdateClaimPermit {
            objective_id: claim.objective.id,
            remediation_id: claim.remediation_id,
            owner: owner.into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id.unwrap(),
            resource_generation: claim.resource_generation.unwrap(),
        }
    }

    #[test]
    fn restart_is_safe_only_when_every_runtime_owner_is_idle() {
        assert!(UpdateSafetyStatus::default().evaluate().safe_to_restart);

        for active_owner in [
            UpdateSafetyStatus {
                active_chat_turns: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                active_task_schedulers: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                active_delivery_leases: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                active_objective_leases: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                pending_permissions: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                managed_browser_sessions: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                terminal_sessions: 1,
                ..UpdateSafetyStatus::default()
            },
        ] {
            assert!(!active_owner.evaluate().safe_to_restart);
        }
    }

    #[tokio::test]
    async fn only_mutation_capable_unexpired_delivery_leases_block_restart() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE delivery_runs (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                lease_owner TEXT,
                lease_expires_at INTEGER,
                claim_epoch INTEGER NOT NULL,
                reconciled_claim_epoch INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (id, status, owner, expiry, claim_epoch, reconciled_claim_epoch) in [
            ("active", "waiting", Some("worker"), Some(2_000_i64), 3, 3),
            (
                "observe-only-takeover",
                "platform_incident",
                Some("observer"),
                Some(2_000_i64),
                708,
                2,
            ),
            (
                "legacy-zero-epoch",
                "waiting",
                Some("legacy-owner"),
                Some(2_000_i64),
                0,
                0,
            ),
            ("expired", "waiting", Some("worker"), Some(999_i64), 3, 3),
            ("done", "completed", Some("worker"), Some(2_000_i64), 3, 3),
            ("failed", "failed", Some("worker"), Some(2_000_i64), 3, 3),
            (
                "cancelled",
                "cancelled",
                Some("worker"),
                Some(2_000_i64),
                3,
                3,
            ),
            (
                "rejected",
                "rejected",
                Some("worker"),
                Some(2_000_i64),
                3,
                3,
            ),
            ("unowned", "waiting", None, Some(2_000_i64), 3, 3),
        ] {
            sqlx::query(
                "INSERT INTO delivery_runs
                 (id,status,lease_owner,lease_expires_at,claim_epoch,reconciled_claim_epoch)
                 VALUES (?,?,?,?,?,?)",
            )
            .bind(id)
            .bind(status)
            .bind(owner)
            .bind(expiry)
            .bind(claim_epoch)
            .bind(reconciled_claim_epoch)
            .execute(&pool)
            .await
            .unwrap();
        }

        assert_eq!(count_active_delivery_leases(&pool, 1_000).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn durable_unleased_waits_do_not_block_restart() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE objectives (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                domain TEXT NOT NULL,
                recovery_owner TEXT,
                remediation_id TEXT,
                lease_owner TEXT,
                lease_expires_at INTEGER
            );
            CREATE TABLE objective_remediations (
                id TEXT PRIMARY KEY,
                objective_id TEXT NOT NULL,
                binding_id TEXT,
                domain TEXT NOT NULL,
                status TEXT NOT NULL,
                lease_owner TEXT,
                lease_expires_at INTEGER,
                attempt_index INTEGER NOT NULL
            );
            CREATE TABLE objective_bindings (
                id TEXT PRIMARY KEY,
                objective_id TEXT NOT NULL,
                domain TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                resource_generation INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (id, status) in [
            ("active-without-runtime-owner", "active"),
            ("expired-system-lease", "waiting_system"),
            ("waiting-core-input", "waiting_core_input"),
            ("waiting-authorization", "waiting_authorization"),
            ("waiting-business", "waiting_business_decision"),
            ("legacy-orphan", "legacy_orphan"),
            ("completed", "completed"),
            ("cancelled", "cancelled"),
        ] {
            sqlx::query(
                "INSERT INTO objectives
                 (id,status,domain,recovery_owner,remediation_id,lease_owner,lease_expires_at)
                 VALUES (?,?,'tool',NULL,NULL,NULL,NULL)",
            )
            .bind(id)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "UPDATE objectives
             SET recovery_owner='objective-supervisor:tool', lease_owner='expired-owner',
                 lease_expires_at=999
             WHERE id='expired-system-lease'",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(count_nonterminal_objectives(&pool).await.unwrap(), 6);
        let blockers = load_objective_blockers(&pool, 1_000, None, TARGET_VERSION, TARGET_BUILD)
            .await
            .unwrap();
        assert_eq!(blockers.count, 0);
        assert!(blockers.owners.is_empty());
        assert!(
            UpdateSafetyStatus {
                active_objective_leases: blockers.count,
                ..UpdateSafetyStatus::default()
            }
            .evaluate()
            .safe_to_restart
        );
    }

    #[tokio::test]
    async fn exact_claimed_update_is_exempt_but_other_live_objective_leases_block() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE objectives (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                domain TEXT NOT NULL,
                recovery_owner TEXT,
                remediation_id TEXT,
                lease_owner TEXT,
                lease_expires_at INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE objective_remediations (
                id TEXT PRIMARY KEY,
                objective_id TEXT NOT NULL,
                binding_id TEXT,
                domain TEXT NOT NULL,
                status TEXT NOT NULL,
                lease_owner TEXT,
                lease_expires_at INTEGER,
                attempt_index INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE objective_bindings (
                id TEXT PRIMARY KEY,
                objective_id TEXT NOT NULL,
                domain TEXT NOT NULL,
                resource_kind TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                resource_generation INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (id, status, domain, recovery_owner, remediation_id, lease_owner) in [
            (
                "update-self",
                "waiting_system",
                "update",
                Some("objective-supervisor:update"),
                Some("remediation-update"),
                Some("update-owner"),
            ),
            (
                "chat-active-stream-closed",
                "active",
                "chat",
                Some("objective-supervisor:chat"),
                None,
                None,
            ),
            (
                "chat-active-with-lease",
                "active",
                "chat",
                Some("objective-supervisor:chat"),
                None,
                Some("chat-owner"),
            ),
            (
                "provider-waiting",
                "waiting_system",
                "provider",
                Some("objective-supervisor:provider"),
                Some("remediation-provider"),
                Some("provider-owner"),
            ),
            ("core-input", "waiting_core_input", "auth", None, None, None),
        ] {
            sqlx::query(
                "INSERT INTO objectives
                 (id,status,domain,recovery_owner,remediation_id,lease_owner,lease_expires_at)
                 VALUES (?,?,?,?,?,?,?)",
            )
            .bind(id)
            .bind(status)
            .bind(domain)
            .bind(recovery_owner)
            .bind(remediation_id)
            .bind(lease_owner)
            .bind(lease_owner.map(|_| 2_000_i64))
            .execute(&pool)
            .await
            .unwrap();
        }
        for (id, objective_id, binding_id, domain, owner, epoch) in [
            (
                "remediation-update",
                "update-self",
                Some("binding-update"),
                "update",
                "update-owner",
                7_i64,
            ),
            (
                "remediation-provider",
                "provider-waiting",
                None,
                "provider",
                "provider-owner",
                2_i64,
            ),
        ] {
            sqlx::query(
                "INSERT INTO objective_remediations
                 (id,objective_id,binding_id,domain,status,lease_owner,lease_expires_at,attempt_index)
                 VALUES (?,?,?,?, 'claimed', ?, 2000, ?)",
            )
            .bind(id)
            .bind(objective_id)
            .bind(binding_id)
            .bind(domain)
            .bind(owner)
            .bind(epoch)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO objective_bindings
             (id,objective_id,domain,resource_kind,resource_id,resource_generation)
             VALUES ('binding-update','update-self','update','app_update_target',?,3)",
        )
        .bind(update_target_resource_id("1.80.0", "build-18000"))
        .execute(&pool)
        .await
        .unwrap();

        let permit = UpdateClaimPermit {
            objective_id: "update-self".into(),
            remediation_id: "remediation-update".into(),
            owner: "update-owner".into(),
            claim_epoch: 7,
            binding_id: "binding-update".into(),
            resource_generation: 3,
        };

        let mismatched_target =
            load_objective_blockers(&pool, 1_000, Some(&permit), "1.80.1", "build-18001")
                .await
                .unwrap();
        assert_eq!(
            mismatched_target.count, 3,
            "all three unexpired Objective leases are live restart blockers"
        );

        let blockers =
            load_objective_blockers(&pool, 1_000, Some(&permit), "1.80.0", "build-18000")
                .await
                .unwrap();

        assert_eq!(blockers.count, 2);
        assert_eq!(
            blockers.owners,
            vec![
                "objective-supervisor:chat".to_string(),
                "objective-supervisor:provider".to_string(),
            ]
        );
    }

    async fn force_exhausted_update(pool: &sqlx::SqlitePool, objective_id: &str) {
        sqlx::query(
            "UPDATE objective_remediations
             SET status='superseded', lease_owner=NULL, lease_expires_at=NULL
             WHERE objective_id=?",
        )
        .bind(objective_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objectives
             SET status='waiting_core_input', decision_type='core_input_required',
                 requires_user_action=1, request_key=?, failure_code=?,
                 recovery_owner=NULL, remediation_id=NULL,
                 next_observation_at=NULL, lease_owner=NULL, lease_expires_at=NULL
             WHERE id=?",
        )
        .bind(format!("{TECHNICAL_RECOVERY_EXHAUSTED}:{objective_id}"))
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(objective_id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn exact_exhausted_update_is_rearmed_in_a_fresh_recovery_generation() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let objective_id = ensure_update_objective(&pool, TARGET_VERSION, TARGET_BUILD)
            .await
            .unwrap();
        force_exhausted_update(&pool, &objective_id).await;

        let recovered_id = ensure_update_objective(&pool, TARGET_VERSION, TARGET_BUILD)
            .await
            .unwrap();
        assert_eq!(recovered_id, objective_id);
        let recovered = ObjectiveStore::new(pool.clone())
            .get(&objective_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(recovered.recovery_generation, 1);
        assert!(recovered.remediation_id.is_some());
        assert!(!recovered.requires_user_action);
    }

    #[tokio::test]
    async fn exhausted_old_update_target_is_audibly_superseded_by_the_new_target() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let old_id = ensure_update_objective(&pool, TARGET_VERSION, TARGET_BUILD)
            .await
            .unwrap();
        force_exhausted_update(&pool, &old_id).await;

        let next_build = "cccccccccccccccccccccccccccccccccccccccc";
        let next_id = ensure_update_objective(&pool, "1.81.0", next_build)
            .await
            .unwrap();
        assert_ne!(next_id, old_id);
        let store = ObjectiveStore::new(pool.clone());
        assert_eq!(
            store.get(&old_id).await.unwrap().unwrap().status,
            ObjectiveStatus::LegacyOrphan
        );
        assert_eq!(
            store.get(&next_id).await.unwrap().unwrap().status,
            ObjectiveStatus::WaitingSystem
        );
    }

    #[tokio::test]
    async fn expired_update_claim_is_neither_exempt_nor_a_live_blocker() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE objectives (
                id TEXT PRIMARY KEY, status TEXT NOT NULL, domain TEXT NOT NULL,
                recovery_owner TEXT, remediation_id TEXT, lease_owner TEXT,
                lease_expires_at INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE objective_remediations (
                id TEXT PRIMARY KEY, objective_id TEXT NOT NULL, binding_id TEXT,
                domain TEXT NOT NULL,
                status TEXT NOT NULL,
                lease_owner TEXT, lease_expires_at INTEGER, attempt_index INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE objective_bindings (
                id TEXT PRIMARY KEY, objective_id TEXT NOT NULL, domain TEXT NOT NULL,
                resource_kind TEXT NOT NULL, resource_id TEXT NOT NULL,
                resource_generation INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objectives VALUES
             ('update-self','waiting_system','update','objective-supervisor:update',
              'remediation-update','update-owner',900)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_remediations VALUES
             ('remediation-update','update-self','binding-update','update',
              'claimed','update-owner',900,7)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings VALUES
             ('binding-update','update-self','update','app_update_target',?,3)",
        )
        .bind(update_target_resource_id("1.80.0", "build-18000"))
        .execute(&pool)
        .await
        .unwrap();

        let blockers = load_objective_blockers(
            &pool,
            1_000,
            Some(&UpdateClaimPermit {
                objective_id: "update-self".into(),
                remediation_id: "remediation-update".into(),
                owner: "update-owner".into(),
                claim_epoch: 7,
                binding_id: "binding-update".into(),
                resource_generation: 3,
            }),
            "1.80.0",
            "build-18000",
        )
        .await
        .unwrap();
        assert_eq!(blockers.exempt_objective_id, None);
        assert_eq!(
            blockers.count, 0,
            "an expired claim cannot exempt itself or impersonate live work"
        );
    }

    #[tokio::test]
    async fn startup_observation_reconciles_exact_build_without_blind_replay() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let permit = claimed_update_permit(
            &pool,
            "objective-update-observe",
            "update-observe-owner",
            TARGET_VERSION,
            TARGET_BUILD,
        )
        .await;

        let first = admit_update_install(
            &pool,
            Some(&permit),
            TARGET_VERSION,
            TARGET_BUILD,
            CURRENT_VERSION,
            CURRENT_BUILD,
            1_000,
        )
        .await
        .unwrap();
        assert!(matches!(
            &first,
            UpdateInstallAdmission::InstallPermitted(_)
        ));
        assert_eq!(
            first.view().objective_id.as_deref(),
            Some(permit.objective_id.as_str())
        );

        let in_flight = admit_update_install(
            &pool,
            Some(&permit),
            TARGET_VERSION,
            TARGET_BUILD,
            CURRENT_VERSION,
            CURRENT_BUILD,
            2_000,
        )
        .await
        .unwrap();
        assert!(matches!(in_flight, UpdateInstallAdmission::StillUnknown(_)));

        let reconciled = observe_latest_update_install(&pool, TARGET_VERSION, TARGET_BUILD, 3_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reconciled.state, UpdateInstallState::Applied);
        assert_eq!(reconciled.target_version, TARGET_VERSION);
        assert_eq!(reconciled.target_build, TARGET_BUILD);
    }

    #[tokio::test]
    async fn definitely_not_applied_reauthorizes_only_the_current_exact_claim_once() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let permit = claimed_update_permit(
            &pool,
            "objective-update-retry",
            "update-owner",
            TARGET_VERSION,
            TARGET_BUILD,
        )
        .await;

        let first = admit_update_install(
            &pool,
            Some(&permit),
            TARGET_VERSION,
            TARGET_BUILD,
            CURRENT_VERSION,
            CURRENT_BUILD,
            1_000,
        )
        .await
        .unwrap();
        assert!(matches!(first, UpdateInstallAdmission::InstallPermitted(_)));

        let observed = observe_latest_update_install(&pool, CURRENT_VERSION, CURRENT_BUILD, 1_500)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.state, UpdateInstallState::DefinitelyNotApplied);

        let one_recovery = admit_update_install(
            &pool,
            Some(&permit),
            TARGET_VERSION,
            TARGET_BUILD,
            CURRENT_VERSION,
            CURRENT_BUILD,
            2_000,
        )
        .await
        .unwrap();
        assert!(
            matches!(one_recovery, UpdateInstallAdmission::InstallPermitted(_)),
            "an exact live claim may retry once when the installed identity is still exactly the pre-install build"
        );

        let exhausted = admit_update_install(
            &pool,
            Some(&permit),
            TARGET_VERSION,
            TARGET_BUILD,
            CURRENT_VERSION,
            CURRENT_BUILD,
            3_000,
        )
        .await
        .unwrap();
        assert!(!matches!(
            exhausted,
            UpdateInstallAdmission::InstallPermitted(_)
        ));
    }

    #[tokio::test]
    async fn a_third_installed_identity_is_conflict_and_never_reauthorized() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let permit = claimed_update_permit(
            &pool,
            "objective-update-conflict",
            "update-owner",
            TARGET_VERSION,
            TARGET_BUILD,
        )
        .await;
        admit_update_install(
            &pool,
            Some(&permit),
            TARGET_VERSION,
            TARGET_BUILD,
            CURRENT_VERSION,
            CURRENT_BUILD,
            1_000,
        )
        .await
        .unwrap();

        let observed = observe_latest_update_install(
            &pool,
            "1.79.5",
            "cccccccccccccccccccccccccccccccccccccccc",
            1_500,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(observed.state, UpdateInstallState::Conflict);

        let conflict = admit_update_install(
            &pool,
            Some(&permit),
            TARGET_VERSION,
            TARGET_BUILD,
            "1.79.5",
            "cccccccccccccccccccccccccccccccccccccccc",
            2_000,
        )
        .await
        .unwrap();
        assert_eq!(format!("{:?}", conflict.state()), "Conflict");
        assert!(!matches!(
            conflict,
            UpdateInstallAdmission::InstallPermitted(_)
        ));
    }
}
