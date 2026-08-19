// SPDX-License-Identifier: Apache-2.0
//! Exact-identity recovery adapter for in-app updates.
//!
//! The renderer may request an update Objective, but only this adapter owns the
//! updater mutation. It consumes the supervisor's exact Objective claim,
//! writes the durable install receipt before mutation, and never turns an
//! unknown or conflicting observation into another install.

use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;
use uuid::Uuid;

use super::objective::{
    CompletionArbiter, DecisionRouter, EvidenceKind, ObjectiveEvidence, ObjectiveSnapshot,
    ObjectiveStatus, ObjectiveStore, RecoveryDomain, RouteSignal, UPDATE_SAFE_POINT_PENDING,
};
use crate::commands::tasks::SchedulerHandles;
use crate::commands::terminal::TerminalState;
use crate::commands::update_safety::{
    admit_update_install, current_app_identity, mark_update_install_unknown,
    observe_update_restart_safety_inner, parse_update_target_resource_id,
    release_update_install_reservation_inner, reserve_update_install_inner,
    validate_update_identity, UpdateClaimPermit, UpdateInstallAdmission, UpdateInstallReceiptView,
    UpdateInstallState,
};
use crate::errors::AppError;
use crate::AppState;

const UPDATE_TARGET_RESOURCE_KIND: &str = "app_update_target";
const SAFE_POINT_REOBSERVE_MS: i64 = 5_000;
const UPDATE_INSTALL_PROGRESS_EVENT: &str = "update-install-progress";

#[derive(Debug, Clone, serde::Serialize)]
struct UpdateInstallProgressEvent<'a> {
    target_version: &'a str,
    target_build: &'a str,
    phase: &'a str,
    received: u64,
    total: Option<u64>,
}

fn emit_update_install_progress(
    app: &AppHandle,
    target_version: &str,
    target_build: &str,
    phase: &str,
    received: u64,
    total: Option<u64>,
) {
    if let Err(error) = app.emit(
        UPDATE_INSTALL_PROGRESS_EVENT,
        UpdateInstallProgressEvent {
            target_version,
            target_build,
            phase,
            received,
            total,
        },
    ) {
        tracing::debug!(%error, phase, "could not project updater progress");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaimedUpdateTarget {
    version: String,
    build: String,
}

fn app_error(context: &str, error: impl std::fmt::Display) -> AppError {
    AppError::Other(format!("{context}: {error}"))
}

fn exact_update_claim(
    permit: &codefactory_agent_loop::tool::MutationPermit,
) -> Result<UpdateClaimPermit, AppError> {
    let binding_id = permit.binding_id.clone().ok_or_else(|| {
        AppError::Other("update recovery permit is missing its exact binding".into())
    })?;
    let resource_generation = permit.resource_generation.ok_or_else(|| {
        AppError::Other("update recovery permit is missing its resource generation".into())
    })?;
    if permit.claim_epoch <= 0 {
        return Err(AppError::Other(
            "update recovery permit claim epoch must be positive".into(),
        ));
    }
    Ok(UpdateClaimPermit {
        objective_id: permit.objective_id.clone(),
        remediation_id: permit.remediation_id.clone(),
        owner: permit.owner.clone(),
        claim_epoch: permit.claim_epoch,
        binding_id,
        resource_generation,
    })
}

async fn load_claimed_update_target(
    pool: &SqlitePool,
    objective: &ObjectiveSnapshot,
    permit: &codefactory_agent_loop::tool::MutationPermit,
) -> Result<ClaimedUpdateTarget, AppError> {
    if objective.id != permit.objective_id
        || objective.domain != RecoveryDomain::Update
        || objective.status != ObjectiveStatus::WaitingSystem
    {
        return Err(AppError::Other(
            "update recovery objective and mutation permit do not match".into(),
        ));
    }
    let binding_id = permit.binding_id.as_deref().ok_or_else(|| {
        AppError::Other("update recovery permit is missing its exact binding".into())
    })?;
    let generation = permit.resource_generation.ok_or_else(|| {
        AppError::Other("update recovery permit is missing its resource generation".into())
    })?;
    let row = sqlx::query(
        "SELECT resource_id, resource_generation
         FROM objective_bindings
         WHERE id=? AND objective_id=? AND domain='update'
           AND resource_kind=? AND resource_generation=?",
    )
    .bind(binding_id)
    .bind(&objective.id)
    .bind(UPDATE_TARGET_RESOURCE_KIND)
    .bind(generation)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        AppError::Other("update recovery binding is stale or points to another target".into())
    })?;
    let resource_id: String = row.try_get("resource_id")?;
    let persisted_generation: i64 = row.try_get("resource_generation")?;
    if persisted_generation != generation
        || objective.resume_cursor.as_deref() != Some(resource_id.as_str())
    {
        return Err(AppError::Other(
            "update recovery cursor no longer matches the claimed target".into(),
        ));
    }
    let (version, build) = parse_update_target_resource_id(&resource_id)
        .ok_or_else(|| AppError::Other("update recovery target identity is malformed".into()))?;
    validate_update_identity(&version, &build)?;
    Ok(ClaimedUpdateTarget { version, build })
}

fn manifest_build_identity(raw_json: &serde_json::Value) -> Result<String, AppError> {
    let build = raw_json
        .get("build_git_sha")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Other(
                "update manifest has no exact build_git_sha; install remains fenced".into(),
            )
        })?
        .to_string();
    validate_update_identity("manifest-version-placeholder", &build)?;
    Ok(build)
}

async fn settle_applied_update(
    pool: &SqlitePool,
    objective: &ObjectiveSnapshot,
    permit: &codefactory_agent_loop::tool::MutationPermit,
    receipt: &UpdateInstallReceiptView,
) -> Result<(), AppError> {
    if receipt.objective_id.as_deref() != Some(objective.id.as_str()) {
        return Err(AppError::Other(
            "applied update receipt belongs to another Objective".into(),
        ));
    }
    let exact_receipt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM update_install_receipts
         WHERE id=? AND objective_id=? AND target_version=? AND target_build=?
           AND status='applied'",
    )
    .bind(&receipt.id)
    .bind(&objective.id)
    .bind(&receipt.target_version)
    .bind(&receipt.target_build)
    .fetch_one(pool)
    .await?;
    if exact_receipt != 1 || receipt.state != UpdateInstallState::Applied {
        return Err(AppError::Other(
            "update completion lacks one exact applied receipt".into(),
        ));
    }
    let now = chrono::Utc::now().timestamp_millis();
    let material = format!(
        "{}\0{}\0{}\0{}",
        objective.id, receipt.id, receipt.target_version, receipt.target_build
    );
    let evidence = ObjectiveEvidence {
        id: Uuid::new_v4().to_string(),
        kind: EvidenceKind::CurrentStateAcceptance,
        scope: objective.id.clone(),
        digest: format!("sha256:{:x}", Sha256::digest(material.as_bytes())),
        evidence_ref: format!("update-install-receipt:{}", receipt.id),
        observed_at: now,
        reached_acceptance: objective.requested_acceptance.clone(),
    };
    let decision = CompletionArbiter::decide(objective, &[evidence])
        .map_err(|error| app_error("arbitrate applied update", error))?;
    ObjectiveStore::new(pool.clone())
        .apply_claimed_decision(objective.revision, decision, permit)
        .await
        .map_err(|error| app_error("settle applied update Objective", error))?;
    Ok(())
}

fn deferred_admission_error(state: UpdateInstallState) -> AppError {
    AppError::Other(format!(
        "update install remains {:?}; no updater mutation is authorized",
        state
    ))
}

/// Execute one Update remediation. This is called only after the generic
/// supervisor has observed the typed Update binding and verified the permit is
/// current. The adapter repeats that exact check immediately before install.
pub(crate) async fn resume_update_objective(
    app: AppHandle,
    objective: ObjectiveSnapshot,
    permit: codefactory_agent_loop::tool::MutationPermit,
) -> Result<(), AppError> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Other("application state is not ready".into()))?;
    let schedulers = app
        .try_state::<SchedulerHandles>()
        .ok_or_else(|| AppError::Other("task scheduler handles are not ready".into()))?;
    let terminals = app
        .try_state::<TerminalState>()
        .ok_or_else(|| AppError::Other("terminal state is not ready".into()))?;
    let pool = state.db.read().await.clone();
    let target = load_claimed_update_target(&pool, &objective, &permit).await?;
    let exact_claim = exact_update_claim(&permit)?;
    let (current_version, current_build) = current_app_identity(&app)?;

    // A restarted app that already matches the exact target needs no manifest
    // or installer I/O; the receipt/current-state evidence settles the same
    // opaque Objective. This path deliberately does not require restart-idle
    // admission: it performs no process mutation and must not be held open by
    // unrelated live work.
    if current_version == target.version && current_build == target.build {
        let admission = admit_update_install(
            &pool,
            Some(&exact_claim),
            &target.version,
            &target.build,
            &current_version,
            &current_build,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
        return match admission {
            UpdateInstallAdmission::Applied(receipt) => {
                settle_applied_update(&pool, &objective, &permit, &receipt).await
            }
            other => Err(deferred_admission_error(other.state())),
        };
    }

    let store = ObjectiveStore::new(pool.clone());
    if objective.failure_code.as_deref() == Some(UPDATE_SAFE_POINT_PENDING) {
        let safety = observe_update_restart_safety_inner(
            &target.version,
            &target.build,
            &exact_claim,
            state.inner(),
            schedulers.inner(),
            terminals.inner(),
        )
        .await?;
        if !safety.safe_to_restart {
            store
                .defer_claimed_remediation(
                    &objective.id,
                    &permit.remediation_id,
                    &permit.owner,
                    permit.claim_epoch,
                    SAFE_POINT_REOBSERVE_MS,
                )
                .await
                .map_err(|error| app_error("defer Update safe-point observation", error))?;
            return Ok(());
        }
        let decision = DecisionRouter::route(
            &objective,
            RouteSignal::CapabilityRestored {
                domain: RecoveryDomain::Update,
                reason: "update_safe_point_reached".into(),
                next_observation_at: chrono::Utc::now().timestamp_millis(),
                resume_cursor: objective.resume_cursor.clone(),
            },
        )
        .map_err(|error| app_error("route Update safe-point observation", error))?;
        store
            .apply_claimed_decision(objective.revision, decision, &permit)
            .await
            .map_err(|error| app_error("settle Update safe-point observation", error))?;
        return Ok(());
    }

    let update = app
        .updater()
        .map_err(|error| app_error("initialize updater", error))?
        .check()
        .await
        .map_err(|error| app_error("check update manifest", error))?
        .ok_or_else(|| {
            AppError::Other("claimed update target is absent from the updater manifest".into())
        })?;
    let manifest_build = manifest_build_identity(&update.raw_json)?;
    if update.version != target.version || manifest_build != target.build {
        return Err(AppError::Other(format!(
            "updater manifest identity conflict: claimed {}@{}, observed {}@{}",
            target.version, target.build, update.version, manifest_build
        )));
    }
    let safety = reserve_update_install_inner(
        target.version.clone(),
        target.build.clone(),
        Some(exact_claim),
        &app,
        state.inner(),
        schedulers.inner(),
        terminals.inner(),
    )
    .await?;
    if !safety.safe_to_restart && safety.update_install_state.is_none() {
        let failure_signature = format!(
            "sha256:{:x}",
            Sha256::digest(
                format!("{}\0{}\0restart-safe-point", target.version, target.build).as_bytes()
            )
        );
        let decision = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Update,
                failure_code: UPDATE_SAFE_POINT_PENDING.into(),
                failure_signature,
                next_observation_at: chrono::Utc::now().timestamp_millis()
                    + SAFE_POINT_REOBSERVE_MS,
                resume_cursor: objective.resume_cursor.clone(),
            },
        )
        .map_err(|error| app_error("route Update safe-point wait", error))?;
        store
            .apply_claimed_decision(objective.revision, decision, &permit)
            .await
            .map_err(|error| app_error("persist Update safe-point wait", error))?;
        return Ok(());
    }
    let receipt_id = safety.update_receipt_id.ok_or_else(|| {
        release_update_install_reservation_inner(state.inner());
        AppError::Other("update admission produced no durable receipt".into())
    })?;
    let receipt = UpdateInstallReceiptView {
        id: receipt_id,
        objective_id: safety.update_objective_id,
        target_version: target.version.clone(),
        target_build: target.build.clone(),
        state: safety.update_install_state.ok_or_else(|| {
            release_update_install_reservation_inner(state.inner());
            AppError::Other("update admission produced no typed state".into())
        })?,
        recovery_replay_count: 0,
        observed_at: chrono::Utc::now().timestamp_millis(),
    };

    match receipt.state {
        UpdateInstallState::Applied => {
            settle_applied_update(&pool, &objective, &permit, &receipt).await?;
            release_update_install_reservation_inner(state.inner());
            Ok(())
        }
        UpdateInstallState::InstallPermitted => {
            emit_update_install_progress(
                &app,
                &target.version,
                &target.build,
                "downloading",
                0,
                None,
            );
            let received = Arc::new(AtomicU64::new(0));
            let progress_app = app.clone();
            let progress_version = target.version.clone();
            let progress_build = target.build.clone();
            let progress_received = received.clone();
            let bytes = match update
                .download(
                    move |chunk_length, content_length| {
                        let cumulative = progress_received
                            .fetch_add(chunk_length as u64, Ordering::Relaxed)
                            + chunk_length as u64;
                        emit_update_install_progress(
                            &progress_app,
                            &progress_version,
                            &progress_build,
                            "downloading",
                            cumulative,
                            content_length,
                        );
                    },
                    || {},
                )
                .await
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    mark_update_install_unknown(&pool, &receipt.id).await?;
                    release_update_install_reservation_inner(state.inner());
                    return Err(app_error("download exact update package", error));
                }
            };
            let claim_is_current = store
                .claim_is_current(&permit)
                .await
                .map_err(|error| app_error("recheck update mutation permit", error))?;
            if !claim_is_current {
                mark_update_install_unknown(&pool, &receipt.id).await?;
                release_update_install_reservation_inner(state.inner());
                return Err(AppError::Other(
                    "update mutation permit became stale before install; package discarded".into(),
                ));
            }
            emit_update_install_progress(
                &app,
                &target.version,
                &target.build,
                "installing",
                received.load(Ordering::Relaxed),
                None,
            );
            if let Err(error) = update.install(bytes) {
                mark_update_install_unknown(&pool, &receipt.id).await?;
                release_update_install_reservation_inner(state.inner());
                return Err(app_error("install exact update package", error));
            }
            app.restart()
        }
        UpdateInstallState::DefinitelyNotApplied
        | UpdateInstallState::StillUnknown
        | UpdateInstallState::Conflict
        | UpdateInstallState::Queued => {
            release_update_install_reservation_inner(state.inner());
            Err(deferred_admission_error(receipt.state))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::objective::{ObjectiveKind, ObjectiveStatus};
    use crate::commands::update_safety::{ensure_update_objective, UpdateInstallAdmission};
    use sqlx::sqlite::SqlitePoolOptions;

    const OLD_VERSION: &str = "1.79.0";
    const OLD_BUILD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TARGET_VERSION: &str = "1.80.0";
    const TARGET_BUILD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    async fn claimed_update(
        owner: &str,
    ) -> (
        SqlitePool,
        ObjectiveSnapshot,
        codefactory_agent_loop::tool::MutationPermit,
    ) {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        ensure_update_objective(&pool, TARGET_VERSION, TARGET_BUILD)
            .await
            .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let claim = store
            .claim_due_remediations(owner, 1, 60_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claim.objective.kind, ObjectiveKind::LocalMutation);
        assert_eq!(claim.objective.status, ObjectiveStatus::WaitingSystem);
        let permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id,
            owner: owner.into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id,
            resource_generation: claim.resource_generation,
        };
        (pool, claim.objective, permit)
    }

    #[tokio::test]
    async fn exact_binding_loader_fences_other_target_generation_and_objective() {
        let (pool, objective, permit) = claimed_update("update-loader").await;
        let loaded = load_claimed_update_target(&pool, &objective, &permit)
            .await
            .unwrap();
        assert_eq!(loaded.version, TARGET_VERSION);
        assert_eq!(loaded.build, TARGET_BUILD);

        let mut stale_generation = permit.clone();
        stale_generation.resource_generation = Some(2);
        assert!(
            load_claimed_update_target(&pool, &objective, &stale_generation)
                .await
                .is_err()
        );

        let mut other_objective = permit.clone();
        other_objective.objective_id = "another-objective".into();
        assert!(
            load_claimed_update_target(&pool, &objective, &other_objective)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn exact_installed_identity_settles_the_same_opaque_objective() {
        let (pool, objective, permit) = claimed_update("update-settle").await;
        let claim = exact_update_claim(&permit).unwrap();
        let admission = admit_update_install(
            &pool,
            Some(&claim),
            TARGET_VERSION,
            TARGET_BUILD,
            TARGET_VERSION,
            TARGET_BUILD,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
        let receipt = match admission {
            UpdateInstallAdmission::Applied(receipt) => receipt,
            other => panic!("expected applied receipt, got {:?}", other.state()),
        };
        settle_applied_update(&pool, &objective, &permit, &receipt)
            .await
            .unwrap();

        let settled = ObjectiveStore::new(pool.clone())
            .get(&objective.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.id, objective.id);
        assert_eq!(settled.status, ObjectiveStatus::Completed);
        let evidence_objective: String = sqlx::query_scalar(
            "SELECT objective_id FROM objective_evidence
             WHERE evidence_ref=?",
        )
        .bind(format!("update-install-receipt:{}", receipt.id))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(evidence_objective, objective.id);
    }

    #[test]
    fn manifest_identity_requires_a_full_exact_build() {
        assert_eq!(
            manifest_build_identity(&serde_json::json!({ "build_git_sha": TARGET_BUILD })).unwrap(),
            TARGET_BUILD
        );
        assert!(
            manifest_build_identity(&serde_json::json!({ "build_git_sha": "bbbbbbb" })).is_err()
        );
        assert!(manifest_build_identity(&serde_json::json!({})).is_err());
        validate_update_identity(OLD_VERSION, OLD_BUILD).unwrap();
    }
}
