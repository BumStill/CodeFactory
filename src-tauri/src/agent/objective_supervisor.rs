// SPDX-License-Identifier: Apache-2.0
//! Process-resident lease owner for recoverable business objectives.
//!
//! Domain adapters remain explicit. The supervisor never turns an unknown
//! technical state into a user handoff: it releases the lease with another
//! observation time until an identity-complete adapter is available.

use std::{future::Future, time::Duration};

use sqlx::SqlitePool;
use tauri::AppHandle;

use super::objective::{ClaimedRemediation, ObjectiveStore, RecoveryDomain};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const LEASE_MS: i64 = 60_000;
const LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const UNHANDLED_REOBSERVE_MS: i64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdapterExecutor {
    Chat,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdapterCapability {
    ChatResume,
    TaskResume,
    /// The domain is deliberately registered, but its domain-specific
    /// observer/reconciler has not yet proved a safe mutation path. Keeping a
    /// distinct capability is what prevents an identity-shaped Chat fallback.
    ObserveOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegisteredDomainAdapter {
    domain: RecoveryDomain,
    capability: AdapterCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DomainObservation {
    Ready(AdapterExecutor),
    IdentityIncomplete(&'static str),
    CapabilityUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconciledAction {
    Execute(AdapterExecutor),
    ReobserveOnly(&'static str),
}

/// Only `ObjectiveDomainAdapter::reconcile` can construct this token. The
/// executor accepts the token rather than a raw domain, so the production path
/// cannot skip its read-only observe/reconcile phases accidentally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReconciledPlan {
    action: ReconciledAction,
}

trait ObjectiveDomainAdapter {
    fn domain(&self) -> RecoveryDomain;
    fn observe(&self, claim: &ClaimedRemediation) -> DomainObservation;
    fn reconcile(&self, observation: DomainObservation) -> ReconciledPlan;
}

impl ObjectiveDomainAdapter for RegisteredDomainAdapter {
    fn domain(&self) -> RecoveryDomain {
        self.domain
    }

    fn observe(&self, claim: &ClaimedRemediation) -> DomainObservation {
        if claim.domain != self.domain {
            return DomainObservation::IdentityIncomplete("adapter_domain_mismatch");
        }
        match self.capability {
            AdapterCapability::ChatResume
                if claim.objective.session_id.is_some()
                    && claim.objective.root_turn_id.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Chat)
            }
            AdapterCapability::ChatResume => {
                DomainObservation::IdentityIncomplete("chat_identity_incomplete")
            }
            AdapterCapability::TaskResume
                if claim.objective.session_id.is_some() && claim.objective.task_id.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Task)
            }
            AdapterCapability::TaskResume => {
                DomainObservation::IdentityIncomplete("task_identity_incomplete")
            }
            AdapterCapability::ObserveOnly => DomainObservation::CapabilityUnavailable,
        }
    }

    fn reconcile(&self, observation: DomainObservation) -> ReconciledPlan {
        let action = match observation {
            DomainObservation::Ready(executor) => ReconciledAction::Execute(executor),
            DomainObservation::IdentityIncomplete(failure_code) => {
                ReconciledAction::ReobserveOnly(failure_code)
            }
            DomainObservation::CapabilityUnavailable => {
                ReconciledAction::ReobserveOnly("domain_adapter_capability_unavailable")
            }
        };
        ReconciledPlan { action }
    }
}

#[derive(Debug, Clone, Copy)]
struct AdapterRegistry<'a> {
    adapters: &'a [RegisteredDomainAdapter],
}

impl<'a> AdapterRegistry<'a> {
    const fn new(adapters: &'a [RegisteredDomainAdapter]) -> Self {
        Self { adapters }
    }

    fn validate(&self) -> Result<(), String> {
        for domain in RecoveryDomain::ALL {
            let count = self
                .adapters
                .iter()
                .filter(|adapter| adapter.domain() == domain)
                .count();
            if count != 1 {
                return Err(format!(
                    "recovery domain {} must be registered exactly once; found {count}",
                    domain.as_str()
                ));
            }
        }
        Ok(())
    }

    fn adapter_for(&self, domain: RecoveryDomain) -> Result<&'a RegisteredDomainAdapter, String> {
        let mut matches = self
            .adapters
            .iter()
            .filter(|adapter| adapter.domain() == domain);
        let adapter = matches.next().ok_or_else(|| {
            format!(
                "recovery domain {} has no registered adapter",
                domain.as_str()
            )
        })?;
        if matches.next().is_some() {
            return Err(format!(
                "recovery domain {} has duplicate registered adapters",
                domain.as_str()
            ));
        }
        Ok(adapter)
    }
}

static DOMAIN_ADAPTERS: [RegisteredDomainAdapter; 12] = [
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Chat,
        capability: AdapterCapability::ChatResume,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Context,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Tool,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Permission,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Task,
        capability: AdapterCapability::TaskResume,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Provider,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Auth,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Browser,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Terminal,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Delivery,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Release,
        capability: AdapterCapability::ObserveOnly,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Update,
        capability: AdapterCapability::ObserveOnly,
    },
];

static DEFAULT_ADAPTER_REGISTRY: AdapterRegistry<'static> = AdapterRegistry::new(&DOMAIN_ADAPTERS);

fn mutation_permit(
    claim: &ClaimedRemediation,
    owner: &str,
) -> codefactory_agent_loop::tool::MutationPermit {
    codefactory_agent_loop::tool::MutationPermit {
        objective_id: claim.objective.id.clone(),
        remediation_id: claim.remediation_id.clone(),
        owner: owner.to_string(),
        claim_epoch: claim.claim_epoch,
        binding_id: claim.binding_id.clone(),
        resource_generation: claim.resource_generation,
    }
}

async fn drive_adapter<A, Execute, ExecuteFuture>(
    adapter: &A,
    store: &ObjectiveStore,
    claim: &ClaimedRemediation,
    permit: &codefactory_agent_loop::tool::MutationPermit,
    execute: Execute,
) -> Result<(), crate::errors::AppError>
where
    A: ObjectiveDomainAdapter + ?Sized,
    Execute: FnOnce(AdapterExecutor) -> ExecuteFuture,
    ExecuteFuture: Future<Output = Result<(), crate::errors::AppError>>,
{
    let observation = adapter.observe(claim);
    let plan = adapter.reconcile(observation);
    let executor = match plan.action {
        ReconciledAction::Execute(executor) => executor,
        ReconciledAction::ReobserveOnly(failure_code) => {
            return Err(crate::errors::AppError::Other(format!(
                "{} adapter remains observe-only: {failure_code}",
                adapter.domain().as_str()
            )));
        }
    };

    let claim_is_current = store.claim_is_current(permit).await.map_err(|error| {
        crate::errors::AppError::Other(format!(
            "{} adapter could not verify its mutation permit: {error}",
            adapter.domain().as_str()
        ))
    })?;
    if !claim_is_current {
        return Err(crate::errors::AppError::Other(format!(
            "{} adapter mutation permit is stale; execution fenced",
            adapter.domain().as_str()
        )));
    }

    execute(executor).await
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
    run_with_claim_lease_timing(
        store,
        claim,
        owner,
        LEASE_MS,
        LEASE_HEARTBEAT_INTERVAL,
        adapter_future,
    )
    .await
}

async fn run_with_claim_lease_timing<F>(
    store: &ObjectiveStore,
    claim: &ClaimedRemediation,
    owner: &str,
    lease_ms: i64,
    heartbeat_interval: Duration,
    adapter_future: F,
) -> Result<(), crate::errors::AppError>
where
    F: Future<Output = Result<(), crate::errors::AppError>>,
{
    tokio::pin!(adapter_future);
    let mut heartbeat = tokio::time::interval(heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Consume the immediate first tick; the claim itself already owns a fresh
    // lease and the next renewal should happen after one heartbeat interval.
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result = &mut adapter_future => return result,
            _ = heartbeat.tick() => {
                let renewal = store.renew_claimed_remediation(
                    &claim.objective.id,
                    &claim.remediation_id,
                    owner,
                    claim.claim_epoch,
                    lease_ms,
                );
                tokio::pin!(renewal);
                // Keep polling the adapter while SQLite admission for the
                // heartbeat is pending. Otherwise a saturated/single-entry
                // pool can deadlock: the adapter holds the connection while
                // this branch waits for it, but the adapter is no longer
                // polled to release it. Prefer a completed settlement when
                // both futures become ready on the same wakeup.
                let renewed = tokio::select! {
                    biased;
                    result = &mut adapter_future => return result,
                    renewed = &mut renewal => renewed,
                };
                match renewed {
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
    let permit = mutation_permit(&claim, &owner);
    let adapter = DEFAULT_ADAPTER_REGISTRY.adapter_for(claim.domain);
    let result = match adapter {
        Ok(adapter) => {
            let objective = claim.objective.clone();
            let execution_permit = permit.clone();
            run_with_claim_lease(
                &store,
                &claim,
                &owner,
                drive_adapter(
                    adapter,
                    &store,
                    &claim,
                    &permit,
                    move |executor| async move {
                        match executor {
                            AdapterExecutor::Chat => {
                                crate::commands::chat::resume_chat_objective(
                                    app,
                                    objective,
                                    execution_permit,
                                )
                                .await
                            }
                            AdapterExecutor::Task => {
                                crate::commands::tasks::resume_task_objective(
                                    app,
                                    objective,
                                    execution_permit,
                                )
                                .await
                            }
                        }
                    },
                ),
            )
            .await
        }
        Err(error) => Err(crate::errors::AppError::Other(error)),
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
                claim.claim_epoch,
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

/// Claim one bounded batch and hand each exact claim to the caller. Keeping
/// the polling boundary injectable lets restart/fault tests observe the real
/// SQLite CAS without constructing a Tauri application or sleeping a loop.
async fn poll_once<Schedule>(
    store: &ObjectiveStore,
    owner: &str,
    limit: i64,
    lease_ms: i64,
    mut schedule: Schedule,
) -> anyhow::Result<usize>
where
    Schedule: FnMut(ClaimedRemediation),
{
    let claims = store.claim_due_remediations(owner, limit, lease_ms).await?;
    let claimed_count = claims.len();
    for claim in claims {
        schedule(claim);
    }
    Ok(claimed_count)
}

pub fn spawn_objective_recovery_supervisor(app: AppHandle, pool: SqlitePool) {
    let owner = format!(
        "objective-supervisor:{}:{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    tauri::async_runtime::spawn(async move {
        tracing::info!(owner = %owner, poll_ms = POLL_INTERVAL.as_millis(), "objective supervisor started");
        if let Err(error) = DEFAULT_ADAPTER_REGISTRY.validate() {
            tracing::error!(%error, "objective supervisor adapter registry is invalid");
            return;
        }
        let store = ObjectiveStore::new(pool);
        loop {
            let claim_app = app.clone();
            let claim_store = store.clone();
            let claim_owner = owner.clone();
            if let Err(error) = poll_once(&store, &owner, 8, LEASE_MS, move |claim| {
                tauri::async_runtime::spawn(process_claim(
                    claim_app.clone(),
                    claim_store.clone(),
                    claim_owner.clone(),
                    claim,
                ));
            })
            .await
            {
                tracing::warn!(%error, "objective supervisor poll failed");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::objective::{
        CreateObjective, DecisionRouter, ObjectiveKind, ObjectiveSnapshot, ObjectiveStatus,
        RecoveryDomain, RouteSignal,
    };
    use sqlx::sqlite::SqlitePoolOptions;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    fn claim_for(domain: RecoveryDomain, objective: ObjectiveSnapshot) -> ClaimedRemediation {
        ClaimedRemediation {
            objective,
            remediation_id: "remediation-adapter".into(),
            domain,
            claim_epoch: 1,
            binding_id: None,
            resource_generation: None,
        }
    }

    async fn claimed_chat_objective(
        suffix: &str,
        owner: &str,
        with_binding: bool,
    ) -> (SqlitePool, ObjectiveStore, ClaimedRemediation) {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: format!("objective-{suffix}"),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(format!("session-{suffix}")),
                root_turn_id: Some(format!("turn-{suffix}")),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        if with_binding {
            let now = chrono::Utc::now().timestamp_millis();
            sqlx::query(
                "INSERT INTO objective_bindings
                 (id, objective_id, domain, resource_kind, resource_id,
                  resource_generation, identity_digest, created_at, updated_at)
                 VALUES (?, ?, 'chat', 'chat_root_turn', ?, 1, ?, ?, ?)",
            )
            .bind(format!("binding-{suffix}"))
            .bind(&objective.id)
            .bind(format!("turn-{suffix}"))
            .bind(format!("sha256:{suffix}"))
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_timeout".into(),
                failure_signature: format!("sha256:{suffix}"),
                next_observation_at: chrono::Utc::now().timestamp_millis() - 1,
                resume_cursor: Some(format!("turn-{suffix}")),
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
        (pool, store, claim)
    }

    #[test]
    fn every_recovery_domain_is_registered_exactly_once() {
        DEFAULT_ADAPTER_REGISTRY.validate().unwrap();
        assert_eq!(DOMAIN_ADAPTERS.len(), RecoveryDomain::ALL.len());
        for domain in RecoveryDomain::ALL {
            assert_eq!(
                DOMAIN_ADAPTERS
                    .iter()
                    .filter(|adapter| adapter.domain() == domain)
                    .count(),
                1,
                "{}",
                domain.as_str()
            );
        }
    }

    #[test]
    fn missing_or_duplicate_domain_registration_fails_closed() {
        let missing_update = AdapterRegistry::new(&DOMAIN_ADAPTERS[..11]);
        assert!(missing_update.validate().unwrap_err().contains("update"));
        assert!(missing_update
            .adapter_for(RecoveryDomain::Update)
            .unwrap_err()
            .contains("no registered adapter"));

        let duplicate_chat = [DOMAIN_ADAPTERS[0], DOMAIN_ADAPTERS[0]];
        let duplicate_registry = AdapterRegistry::new(&duplicate_chat);
        assert!(duplicate_registry
            .adapter_for(RecoveryDomain::Chat)
            .unwrap_err()
            .contains("duplicate"));
        assert!(duplicate_registry.validate().is_err());
    }

    #[test]
    fn adapter_selection_uses_claim_domain_not_identity_shape() {
        let mut objective = ObjectiveSnapshot::new(
            "objective-provider-shaped-like-chat",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        objective.session_id = Some("session-provider".into());
        objective.root_turn_id = Some("turn-provider".into());
        let claim = claim_for(RecoveryDomain::Provider, objective);
        let adapter = DEFAULT_ADAPTER_REGISTRY.adapter_for(claim.domain).unwrap();
        assert_eq!(adapter.domain(), RecoveryDomain::Provider);
        assert_eq!(adapter.capability, AdapterCapability::ObserveOnly);
        assert_eq!(
            adapter.reconcile(adapter.observe(&claim)).action,
            ReconciledAction::ReobserveOnly("domain_adapter_capability_unavailable")
        );
    }

    #[test]
    fn chat_and_task_adapters_require_their_own_complete_identity() {
        let mut objective = ObjectiveSnapshot::new(
            "objective-adapter",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        let chat_adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Chat)
            .unwrap();
        assert_eq!(
            chat_adapter.reconcile(
                chat_adapter.observe(&claim_for(RecoveryDomain::Chat, objective.clone()))
            ),
            ReconciledPlan {
                action: ReconciledAction::ReobserveOnly("chat_identity_incomplete")
            }
        );
        objective.task_id = Some("task-adapter".into());
        assert_eq!(
            chat_adapter.reconcile(
                chat_adapter.observe(&claim_for(RecoveryDomain::Chat, objective.clone()))
            ),
            ReconciledPlan {
                action: ReconciledAction::ReobserveOnly("chat_identity_incomplete")
            }
        );
        objective.session_id = Some("session-adapter".into());
        let task_adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Task)
            .unwrap();
        assert_eq!(
            task_adapter.reconcile(
                task_adapter.observe(&claim_for(RecoveryDomain::Task, objective.clone()))
            ),
            ReconciledPlan {
                action: ReconciledAction::Execute(AdapterExecutor::Task)
            }
        );
        objective.task_id = None;
        objective.root_turn_id = Some("turn-adapter".into());
        assert_eq!(
            chat_adapter
                .reconcile(chat_adapter.observe(&claim_for(RecoveryDomain::Chat, objective))),
            ReconciledPlan {
                action: ReconciledAction::Execute(AdapterExecutor::Chat)
            }
        );
    }

    struct RecordingAdapter {
        stages: Arc<Mutex<Vec<&'static str>>>,
    }

    impl ObjectiveDomainAdapter for RecordingAdapter {
        fn domain(&self) -> RecoveryDomain {
            RecoveryDomain::Chat
        }

        fn observe(&self, _claim: &ClaimedRemediation) -> DomainObservation {
            self.stages.lock().unwrap().push("observe");
            DomainObservation::Ready(AdapterExecutor::Chat)
        }

        fn reconcile(&self, observation: DomainObservation) -> ReconciledPlan {
            self.stages.lock().unwrap().push("reconcile");
            ReconciledPlan {
                action: match observation {
                    DomainObservation::Ready(executor) => ReconciledAction::Execute(executor),
                    _ => ReconciledAction::ReobserveOnly("unexpected_observation"),
                },
            }
        }
    }

    #[tokio::test]
    async fn observe_and_reconcile_always_precede_execute() {
        let (_pool, store, claim) =
            claimed_chat_objective("ordered-adapter", "ordered-owner", false).await;
        let permit = mutation_permit(&claim, "ordered-owner");
        let stages = Arc::new(Mutex::new(Vec::new()));
        let adapter = RecordingAdapter {
            stages: stages.clone(),
        };
        let execute_stages = stages.clone();

        drive_adapter(&adapter, &store, &claim, &permit, move |executor| {
            let execute_stages = execute_stages.clone();
            async move {
                assert_eq!(executor, AdapterExecutor::Chat);
                execute_stages.lock().unwrap().push("execute");
                Ok(())
            }
        })
        .await
        .unwrap();

        assert_eq!(
            stages.lock().unwrap().as_slice(),
            ["observe", "reconcile", "execute"]
        );
    }

    #[tokio::test]
    async fn missing_domain_capability_stays_system_owned_and_reobserves_without_cta() {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-provider-observe-only".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-provider-observe-only".into()),
                root_turn_id: Some("turn-provider-observe-only".into()),
                domain: RecoveryDomain::Provider,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Provider,
                failure_code: "provider_unavailable".into(),
                failure_signature: "sha256:provider-observe-only".into(),
                next_observation_at: chrono::Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-provider-observe-only".into()),
            },
        )
        .unwrap();
        store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        let claim = store
            .claim_due_remediations("provider-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let permit = mutation_permit(&claim, "provider-owner");
        let adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Provider)
            .unwrap();
        let execute_count = Arc::new(AtomicUsize::new(0));
        let attempt_count = execute_count.clone();

        let error = drive_adapter(adapter, &store, &claim, &permit, move |_| {
            let attempt_count = attempt_count.clone();
            async move {
                attempt_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("observe-only"));
        assert_eq!(execute_count.load(Ordering::SeqCst), 0);

        store
            .defer_claimed_remediation(
                &claim.objective.id,
                &claim.remediation_id,
                "provider-owner",
                claim.claim_epoch,
                UNHANDLED_REOBSERVE_MS,
            )
            .await
            .unwrap();
        let current = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(current.status, ObjectiveStatus::WaitingSystem);
        assert!(!current.requires_user_action);
        assert!(current.next_observation_at.unwrap() > chrono::Utc::now().timestamp_millis());
        assert!(current.lease_owner.is_none());
    }

    #[tokio::test]
    async fn stale_claim_epoch_or_binding_generation_cannot_execute() {
        for (suffix, stale_epoch, with_binding) in [
            ("stale-epoch", true, false),
            ("stale-generation", false, true),
        ] {
            let owner = format!("owner-{suffix}");
            let (pool, store, claim) = claimed_chat_objective(suffix, &owner, with_binding).await;
            let permit = mutation_permit(&claim, &owner);
            if stale_epoch {
                sqlx::query(
                    "UPDATE objective_remediations
                     SET attempt_index=attempt_index+1 WHERE id=?",
                )
                .bind(&claim.remediation_id)
                .execute(&pool)
                .await
                .unwrap();
            } else {
                sqlx::query(
                    "UPDATE objective_bindings
                     SET resource_generation=resource_generation+1 WHERE id=?",
                )
                .bind(claim.binding_id.as_deref().unwrap())
                .execute(&pool)
                .await
                .unwrap();
            }
            let adapter = DEFAULT_ADAPTER_REGISTRY
                .adapter_for(RecoveryDomain::Chat)
                .unwrap();
            let execute_count = Arc::new(AtomicUsize::new(0));
            let attempt_count = execute_count.clone();
            let result = drive_adapter(adapter, &store, &claim, &permit, move |_| {
                let attempt_count = attempt_count.clone();
                async move {
                    attempt_count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await;

            assert!(result.is_err(), "{suffix}");
            assert_eq!(execute_count.load(Ordering::SeqCst), 0, "{suffix}");
        }
    }

    #[tokio::test]
    async fn poll_once_claims_a_bounded_batch_without_a_tauri_runtime() {
        let (pool, store, prior_claim) =
            claimed_chat_objective("poll-once", "old-poll-owner", false).await;
        let expired = chrono::Utc::now().timestamp_millis() - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&prior_claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&prior_claim.objective.id)
            .execute(&pool)
            .await
            .unwrap();
        let scheduled = Arc::new(Mutex::new(Vec::new()));
        let scheduled_claims = scheduled.clone();

        let claimed = poll_once(&store, "new-poll-owner", 1, 30_000, move |claim| {
            scheduled_claims.lock().unwrap().push(claim);
        })
        .await
        .unwrap();

        assert_eq!(claimed, 1);
        let scheduled = scheduled.lock().unwrap();
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].domain, RecoveryDomain::Chat);
        assert!(scheduled[0].claim_epoch > prior_claim.claim_epoch);
    }

    #[tokio::test]
    async fn short_claim_lease_stays_live_until_adapter_settlement() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool);
        let objective = store
            .create(CreateObjective {
                id: "objective-short-heartbeat".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-short-heartbeat".into()),
                root_turn_id: Some("turn-short-heartbeat".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_timeout".into(),
                failure_signature: "sha256:short-heartbeat".into(),
                next_observation_at: chrono::Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-short-heartbeat".into()),
            },
        )
        .unwrap();
        store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        let claim = store
            .claim_due_remediations("short-heartbeat-owner", 1, 400)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let permit = mutation_permit(&claim, "short-heartbeat-owner");
        let adapter_store = store.clone();
        let adapter_objective = claim.objective.clone();
        let old_remediation_id = claim.remediation_id.clone();

        run_with_claim_lease_timing(
            &store,
            &claim,
            "short-heartbeat-owner",
            400,
            Duration::from_millis(50),
            async move {
                // Cross more than two original TTLs. Without the adapter being
                // awaited by the heartbeat owner, this claim is necessarily stale.
                tokio::time::sleep(Duration::from_millis(1_000)).await;
                assert!(adapter_store.claim_is_current(&permit).await.unwrap());
                let retry = DecisionRouter::route(
                    &adapter_objective,
                    RouteSignal::TechnicalFailure {
                        domain: RecoveryDomain::Chat,
                        failure_code: "provider_still_unavailable".into(),
                        failure_signature: "sha256:short-heartbeat-retry".into(),
                        next_observation_at: chrono::Utc::now().timestamp_millis() + 1_000,
                        resume_cursor: Some("turn-short-heartbeat".into()),
                    },
                )
                .unwrap();
                adapter_store
                    .apply_claimed_decision(adapter_objective.revision, retry, &permit)
                    .await
                    .unwrap();
                Ok(())
            },
        )
        .await
        .unwrap();
        let current = store
            .get("objective-short-heartbeat")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.status, ObjectiveStatus::WaitingSystem);
        assert_ne!(
            current.remediation_id.as_deref(),
            Some(old_remediation_id.as_str())
        );
    }
}
