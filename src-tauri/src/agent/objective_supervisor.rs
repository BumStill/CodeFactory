// SPDX-License-Identifier: Apache-2.0
//! Process-resident lease owner for recoverable business objectives.
//!
//! Domain adapters remain explicit. The supervisor never turns an unknown
//! technical state into a user handoff: it releases the lease with another
//! observation time until an identity-complete adapter is available.

use std::{future::Future, time::Duration};

use sqlx::SqlitePool;
use tauri::AppHandle;

use super::objective::{ClaimedRemediation, ObjectiveStore};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const LEASE_MS: i64 = 60_000;
const LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const UNHANDLED_REOBSERVE_MS: i64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryAdapter {
    Chat,
    Task,
    Deferred,
}

fn adapter_for(claim: &ClaimedRemediation) -> RecoveryAdapter {
    if claim.objective.session_id.is_some() && claim.objective.task_id.is_some() {
        RecoveryAdapter::Task
    } else if claim.objective.session_id.is_some() && claim.objective.root_turn_id.is_some() {
        RecoveryAdapter::Chat
    } else {
        RecoveryAdapter::Deferred
    }
}

async fn run_with_claim_lease<F>(
    store: &ObjectiveStore,
    claim: &ClaimedRemediation,
    owner: &str,
    adapter_future: F,
) -> Result<(), crate::errors::AppError>
where
    F: Future<Output = Result<(), crate::errors::AppError>>,
{
    tokio::pin!(adapter_future);
    let mut heartbeat = tokio::time::interval(LEASE_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Consume the immediate first tick; the claim itself already owns a fresh
    // lease and the next renewal should happen after one heartbeat interval.
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut adapter_future => return result,
            _ = heartbeat.tick() => {
                match store
                    .renew_claimed_remediation(
                        &claim.objective.id,
                        &claim.remediation_id,
                        owner,
                        LEASE_MS,
                    )
                    .await
                {
                    Ok(true) => tracing::debug!(
                        objective_id = %claim.objective.id,
                        remediation_id = %claim.remediation_id,
                        "objective remediation lease renewed"
                    ),
                    Ok(false) => {
                        return Err(crate::errors::AppError::Other(
                            "objective remediation ownership changed; adapter cancelled".into(),
                        ));
                    }
                    Err(error) => {
                        return Err(crate::errors::AppError::Other(format!(
                            "objective remediation lease renewal failed: {error}"
                        )));
                    }
                }
            }
        }
    }
}

async fn process_claim(
    app: AppHandle,
    store: ObjectiveStore,
    owner: String,
    claim: ClaimedRemediation,
) {
    let result = match adapter_for(&claim) {
        RecoveryAdapter::Chat => {
            run_with_claim_lease(
                &store,
                &claim,
                &owner,
                crate::commands::chat::resume_chat_objective(app, claim.objective.clone()),
            )
            .await
        }
        RecoveryAdapter::Task => {
            run_with_claim_lease(
                &store,
                &claim,
                &owner,
                crate::commands::tasks::resume_task_objective(app, claim.objective.clone()),
            )
            .await
        }
        RecoveryAdapter::Deferred => Err(crate::errors::AppError::Other(format!(
            "no identity-complete adapter for {:?}",
            claim.domain
        ))),
    };
    if let Err(error) = result {
        tracing::warn!(
            objective_id = %claim.objective.id,
            remediation_id = %claim.remediation_id,
            domain = ?claim.domain,
            %error,
            "objective remediation deferred"
        );
        // The adapter may already have superseded this remediation while
        // persisting a new decision. A failed defer in that case is benign
        // compare-and-swap evidence.
        if let Err(defer_error) = store
            .defer_claimed_remediation(
                &claim.objective.id,
                &claim.remediation_id,
                &owner,
                UNHANDLED_REOBSERVE_MS,
            )
            .await
        {
            tracing::debug!(
                objective_id = %claim.objective.id,
                %defer_error,
                "remediation was advanced before defer"
            );
        }
    }
}

pub fn spawn_objective_recovery_supervisor(app: AppHandle, pool: SqlitePool) {
    let owner = format!(
        "objective-supervisor:{}:{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    tauri::async_runtime::spawn(async move {
        tracing::info!(owner = %owner, poll_ms = POLL_INTERVAL.as_millis(), "objective supervisor started");
        let store = ObjectiveStore::new(pool);
        loop {
            match store.claim_due_remediations(&owner, 8, LEASE_MS).await {
                Ok(claims) => {
                    for claim in claims {
                        tauri::async_runtime::spawn(process_claim(
                            app.clone(),
                            store.clone(),
                            owner.clone(),
                            claim,
                        ));
                    }
                }
                Err(error) => tracing::warn!(%error, "objective supervisor poll failed"),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::objective::{ObjectiveKind, ObjectiveSnapshot, RecoveryDomain};

    #[test]
    fn adapter_requires_complete_chat_identity_and_never_hands_unknown_work_to_user() {
        let mut objective = ObjectiveSnapshot::new(
            "objective-adapter",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        let claim = |objective| ClaimedRemediation {
            objective,
            remediation_id: "remediation-adapter".into(),
            domain: RecoveryDomain::Chat,
        };
        assert_eq!(
            adapter_for(&claim(objective.clone())),
            RecoveryAdapter::Deferred
        );
        objective.task_id = Some("task-adapter".into());
        assert_eq!(
            adapter_for(&claim(objective.clone())),
            RecoveryAdapter::Deferred
        );
        objective.session_id = Some("session-adapter".into());
        assert_eq!(
            adapter_for(&claim(objective.clone())),
            RecoveryAdapter::Task
        );
        objective.task_id = None;
        objective.root_turn_id = Some("turn-adapter".into());
        assert_eq!(adapter_for(&claim(objective)), RecoveryAdapter::Chat);
    }
}
