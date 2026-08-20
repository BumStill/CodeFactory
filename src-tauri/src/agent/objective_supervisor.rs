// SPDX-License-Identifier: Apache-2.0
//! Process-resident lease owner for recoverable business objectives.
//!
//! Domain adapters remain explicit. The supervisor never turns an unknown
//! technical state into a user handoff: it releases the lease with another
//! observation time until an identity-complete adapter is available.

use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager};

use super::objective::{ClaimedRemediation, ObjectiveStore, RecoveryDomain};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const LEASE_MS: i64 = 60_000;
const LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const UNHANDLED_REOBSERVE_MS: i64 = 15_000;

fn restart_reservation_blocks_domain(reserved: bool, domain: RecoveryDomain) -> bool {
    reserved && domain != RecoveryDomain::Update
}

fn update_restart_blocks_claim(app: &AppHandle, domain: RecoveryDomain) -> bool {
    app.try_state::<crate::AppState>().is_some_and(|state| {
        restart_reservation_blocks_domain(
            state.update_restart_reserved.load(Ordering::SeqCst),
            domain,
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdapterExecutor {
    Chat,
    Context,
    Tool,
    Task,
    Permission,
    Provider,
    Auth,
    Browser,
    Update,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdapterCapability {
    ChatResume,
    ContextResume,
    ToolResume,
    TaskResume,
    PermissionResume,
    ProviderResume,
    AuthResume,
    BrowserResume,
    UpdateResume,
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
            AdapterCapability::ContextResume
                if claim.objective.session_id.is_some()
                    && claim.objective.root_turn_id.is_some()
                    && claim
                        .objective
                        .resume_cursor
                        .as_deref()
                        .is_some_and(|cursor| !cursor.trim().is_empty())
                    && claim.binding_id.is_some()
                    && claim.resource_generation.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Context)
            }
            AdapterCapability::ContextResume => {
                DomainObservation::IdentityIncomplete("context_identity_incomplete")
            }
            AdapterCapability::ToolResume
                if claim.objective.session_id.is_some()
                    && (claim.objective.root_turn_id.is_some()
                        || claim.objective.task_id.is_some())
                    && claim.binding_id.is_some()
                    && claim.resource_generation.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Tool)
            }
            AdapterCapability::ToolResume => {
                DomainObservation::IdentityIncomplete("tool_identity_incomplete")
            }
            AdapterCapability::TaskResume
                if claim.objective.session_id.is_some() && claim.objective.task_id.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Task)
            }
            AdapterCapability::TaskResume => {
                DomainObservation::IdentityIncomplete("task_identity_incomplete")
            }
            AdapterCapability::PermissionResume
                if claim.objective.session_id.is_some()
                    && (claim.objective.root_turn_id.is_some()
                        || claim.objective.task_id.is_some())
                    && claim.binding_id.is_some()
                    && claim.resource_generation.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Permission)
            }
            AdapterCapability::PermissionResume => {
                DomainObservation::IdentityIncomplete("permission_identity_incomplete")
            }
            AdapterCapability::ProviderResume
                if claim.objective.session_id.is_some()
                    && claim.objective.root_turn_id.is_some()
                    && claim.binding_id.is_some()
                    && claim.resource_generation.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Provider)
            }
            AdapterCapability::ProviderResume => {
                DomainObservation::IdentityIncomplete("provider_identity_incomplete")
            }
            AdapterCapability::AuthResume
                if claim.objective.session_id.is_some()
                    && claim.objective.root_turn_id.is_some()
                    && claim.objective.request_key.is_some()
                    && claim.binding_id.is_some()
                    && claim.resource_generation.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Auth)
            }
            AdapterCapability::AuthResume => {
                DomainObservation::IdentityIncomplete("auth_identity_incomplete")
            }
            AdapterCapability::BrowserResume
                if claim.objective.session_id.is_some()
                    && (claim.objective.root_turn_id.is_some()
                        || claim.objective.task_id.is_some())
                    && claim.binding_id.is_some()
                    && claim.resource_generation.is_some() =>
            {
                DomainObservation::Ready(AdapterExecutor::Browser)
            }
            AdapterCapability::BrowserResume => {
                DomainObservation::IdentityIncomplete("browser_identity_incomplete")
            }
            AdapterCapability::UpdateResume
                if claim.binding_id.is_some()
                    && claim.resource_generation.is_some()
                    && claim
                        .objective
                        .resume_cursor
                        .as_deref()
                        .is_some_and(|cursor| !cursor.trim().is_empty()) =>
            {
                DomainObservation::Ready(AdapterExecutor::Update)
            }
            AdapterCapability::UpdateResume => {
                DomainObservation::IdentityIncomplete("update_identity_incomplete")
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
        capability: AdapterCapability::ContextResume,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Tool,
        capability: AdapterCapability::ToolResume,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Permission,
        capability: AdapterCapability::PermissionResume,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Task,
        capability: AdapterCapability::TaskResume,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Provider,
        capability: AdapterCapability::ProviderResume,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Auth,
        capability: AdapterCapability::AuthResume,
    },
    RegisteredDomainAdapter {
        domain: RecoveryDomain::Browser,
        capability: AdapterCapability::BrowserResume,
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
        capability: AdapterCapability::UpdateResume,
    },
];

static DEFAULT_ADAPTER_REGISTRY: AdapterRegistry<'static> = AdapterRegistry::new(&DOMAIN_ADAPTERS);

pub(crate) async fn reconcile_provider_recovery_on_startup(
    pool: &SqlitePool,
) -> anyhow::Result<usize> {
    use super::objective::{DecisionRouter, ObjectiveStatus, RouteSignal};

    let now = chrono::Utc::now().timestamp_millis();
    let provider = super::provider_recovery::ProviderRecoveryStore::new(pool.clone());
    provider
        .reconcile_stale_effect_free_in_flight(now)
        .await?;
    let candidates = provider.startup_recovery_candidates(now).await?;
    let store = ObjectiveStore::new(pool.clone());
    let mut reconciled = 0;
    for candidate in candidates {
        let Some(current) = store.get(&candidate.objective_id).await? else {
            continue;
        };
        if current.revision != candidate.objective_revision
            || current.status != ObjectiveStatus::Active
        {
            continue;
        }
        let signature = format!(
            "sha256:{:x}",
            Sha256::digest(
                format!(
                    "provider-startup\0{}\0{}\0{}",
                    candidate.objective_id, candidate.objective_revision, candidate.failure_code
                )
                .as_bytes()
            )
        );
        let decision = DecisionRouter::route(
            &current,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Provider,
                failure_code: candidate.failure_code,
                failure_signature: signature,
                next_observation_at: candidate.next_observation_at,
                resume_cursor: Some(candidate.root_turn_id),
            },
        )?;
        match store.apply_decision(current.revision, decision).await {
            Ok(_) => reconciled += 1,
            Err(error) if error.to_string().contains("revision") => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(reconciled)
}

/// Move crash-left active Browser operations into the Browser domain before
/// the generic stale-active sweep can classify them as Chat recovery. This is
/// routing only; the Browser adapter remains responsible for read-only
/// observation and never treats a contract row as proof of success.
pub(crate) async fn reconcile_browser_recovery_on_startup(
    pool: &SqlitePool,
) -> anyhow::Result<usize> {
    use super::objective::{DecisionRouter, ObjectiveStatus, RouteSignal};

    let candidates: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT contract.objective_id
         FROM browser_recovery_contracts contract
         JOIN objectives objective ON objective.id=contract.objective_id
         WHERE objective.status='active'
           AND contract.state NOT IN (
                 'settled_committed','settled_reconciled','cancelled'
               )",
    )
    .fetch_all(pool)
    .await?;
    let now = chrono::Utc::now().timestamp_millis();
    let store = ObjectiveStore::new(pool.clone());
    let mut reconciled = 0;
    for objective_id in candidates {
        let Some(current) = store.get(&objective_id).await? else {
            continue;
        };
        if current.status != ObjectiveStatus::Active {
            continue;
        }
        let signature = format!(
            "sha256:{:x}",
            Sha256::digest(
                format!("browser-startup\0{}\0{}", current.id, current.revision).as_bytes()
            )
        );
        let decision = DecisionRouter::route(
            &current,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Browser,
                failure_code: "browser_external_state_uncertain".into(),
                failure_signature: signature,
                next_observation_at: now,
                resume_cursor: current
                    .resume_cursor
                    .clone()
                    .or_else(|| current.root_turn_id.clone())
                    .or_else(|| current.task_id.clone()),
            },
        )?;
        match store.apply_decision(current.revision, decision).await {
            Ok(_) => reconciled += 1,
            Err(error) if error.to_string().contains("revision") => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(reconciled)
}

/// Single source of truth for health/release gates. A registered ObserveOnly
/// placeholder is intentionally not an executable recovery capability.
pub(crate) fn domain_has_executable_adapter(domain: RecoveryDomain) -> bool {
    DEFAULT_ADAPTER_REGISTRY
        .adapter_for(domain)
        .is_ok_and(|adapter| adapter.capability != AdapterCapability::ObserveOnly)
}

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
    pool: SqlitePool,
    store: ObjectiveStore,
    owner: String,
    claim: ClaimedRemediation,
) {
    if update_restart_blocks_claim(&app, claim.domain) {
        tracing::info!(
            objective_id = %claim.objective.id,
            remediation_id = %claim.remediation_id,
            domain = ?claim.domain,
            "objective remediation deferred behind update restart reservation"
        );
        if let Err(error) = store
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
                %error,
                "reserved update observed an already-advanced remediation"
            );
        }
        return;
    }
    let permit = mutation_permit(&claim, &owner);
    let adapter = DEFAULT_ADAPTER_REGISTRY.adapter_for(claim.domain);
    let result = match adapter {
        Ok(adapter) => {
            let objective = claim.objective.clone();
            let execution_claim = claim.clone();
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
                            AdapterExecutor::Context => {
                                let authorization = reserve_context_resume_authorization(
                                    &pool,
                                    &execution_claim,
                                    &execution_permit,
                                )
                                .await?;
                                crate::commands::chat::resume_context_objective(
                                    app,
                                    objective,
                                    execution_permit,
                                    authorization,
                                )
                                .await
                            }
                            AdapterExecutor::Tool => {
                                resume_tool_objective(
                                    app,
                                    pool,
                                    execution_claim,
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
                            AdapterExecutor::Permission => {
                                crate::commands::chat::reconcile_permission_objective(
                                    app,
                                    pool,
                                    objective,
                                    execution_permit,
                                )
                                .await
                            }
                            AdapterExecutor::Provider => {
                                require_provider_resume_evidence(&pool, &objective.id, false)
                                    .await?;
                                crate::commands::chat::resume_chat_objective(
                                    app,
                                    objective,
                                    execution_permit,
                                )
                                .await
                            }
                            AdapterExecutor::Auth => {
                                require_provider_resume_evidence(&pool, &objective.id, true)
                                    .await?;
                                crate::commands::chat::resume_chat_objective(
                                    app,
                                    objective,
                                    execution_permit,
                                )
                                .await
                            }
                            AdapterExecutor::Browser => {
                                resume_browser_objective(
                                    app,
                                    pool,
                                    execution_claim,
                                    objective,
                                    execution_permit,
                                )
                                .await
                            }
                            AdapterExecutor::Update => {
                                super::update_recovery::resume_update_objective(
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

/// Reconcile the exact durable Tool side effect before creating another model
/// run.  The store may grant one same-receipt retry, settle a changed resource
/// for replanning, or prove that the prior failure happened before any receipt.
/// It never guesses from the domain or falls back to Chat identity alone.
async fn resume_tool_objective(
    app: AppHandle,
    pool: SqlitePool,
    claim: ClaimedRemediation,
    objective: super::objective::ObjectiveSnapshot,
    permit: codefactory_agent_loop::tool::MutationPermit,
) -> Result<(), crate::errors::AppError> {
    use super::tool_recovery::ToolRecoveryDisposition;

    let disposition = super::tool_recovery::ToolRecoveryStore::new(pool.clone())
        .reconcile_claimed(&claim, &permit)
        .await
        .map_err(|error| crate::errors::AppError::Other(error.to_string()))?;
    match disposition {
        ToolRecoveryDisposition::RetryExact
        | ToolRecoveryDisposition::ReplanCurrentState
        | ToolRecoveryDisposition::ResumeWithoutReceipt => {
            if !super::objective::ObjectiveStore::new(pool)
                .claim_is_current(&permit)
                .await
                .map_err(|error| crate::errors::AppError::Other(error.to_string()))?
            {
                return Err(crate::errors::AppError::Other(
                    "Tool recovery claim changed before resume".into(),
                ));
            }
            if objective.task_id.is_some() {
                crate::commands::tasks::resume_task_objective(app, objective, permit).await
            } else if objective.root_turn_id.is_some() {
                crate::commands::chat::resume_chat_objective(app, objective, permit).await
            } else {
                Err(crate::errors::AppError::Other(
                    "Tool recovery objective has no exact Chat or Task cursor".into(),
                ))
            }
        }
        ToolRecoveryDisposition::ObserveOnly => Err(crate::errors::AppError::Other(
            "Tool recovery evidence remains externally uncertain".into(),
        )),
    }
}

/// Context recovery may construct a fresh agent only after the domain store
/// proves that the exact current chat binding has one effect-free terminal
/// Context attempt. All other dispositions remain system-owned observations.
async fn reserve_context_resume_authorization(
    pool: &SqlitePool,
    claim: &ClaimedRemediation,
    permit: &codefactory_agent_loop::tool::MutationPermit,
) -> Result<super::context_recovery::ContextRecoveryAuthorization, crate::errors::AppError> {
    let reservation = super::context_recovery::ContextRecoveryStore::new(pool.clone())
        .reserve_claimed_recovery(claim, permit)
        .await
        .map_err(|error| {
            crate::errors::AppError::Other(format!("context recovery observation failed: {error}"))
        })?;
    match reservation {
        super::context_recovery::ContextRecoveryReservation::Authorized(authorization) => {
            Ok(authorization)
        }
        reservation => Err(crate::errors::AppError::Other(format!(
            "context recovery remains observe-only: {reservation:?}"
        ))),
    }
}

async fn resume_browser_objective(
    app: AppHandle,
    pool: SqlitePool,
    claim: ClaimedRemediation,
    objective: super::objective::ObjectiveSnapshot,
    permit: codefactory_agent_loop::tool::MutationPermit,
) -> Result<(), crate::errors::AppError> {
    use super::browser_recovery::{BrowserRecoveryDisposition as Disposition, BrowserSettlement};

    let binding_id = claim.binding_id.as_deref().ok_or_else(|| {
        crate::errors::AppError::Other("browser recovery lacks binding identity".into())
    })?;
    let generation = claim.resource_generation.ok_or_else(|| {
        crate::errors::AppError::Other("browser recovery lacks resource generation".into())
    })?;
    let observer_pool = pool.clone();
    let store = super::browser_recovery::BrowserRecoveryStore::new(pool);
    let receipt = store
        .receipt_for_scope(&objective.id, objective.revision, binding_id, generation)
        .await
        .map_err(|error| {
            crate::errors::AppError::Other(format!("browser recovery lookup failed: {error}"))
        })?;

    match receipt {
        Some(receipt_id) => match {
            let initial = store.disposition(&receipt_id).await.map_err(|error| {
                crate::errors::AppError::Other(format!(
                    "browser recovery observation failed: {error}"
                ))
            })?;
            if matches!(
                initial,
                Disposition::ObserveOnlyUncertain | Disposition::Conflict
            ) {
                crate::tools::browser_session::observe_browser_receipt(observer_pool, &receipt_id)
                    .await
                    .map_err(|error| {
                        crate::errors::AppError::Other(format!(
                            "browser runtime observation failed: {error}"
                        ))
                    })?
            } else {
                initial
            }
        } {
            Disposition::AwaitingSettlement | Disposition::ObservedApplied => {
                let settlement = store
                    .settle(&receipt_id, chrono::Utc::now().timestamp_millis())
                    .await
                    .map_err(|error| {
                        crate::errors::AppError::Other(format!(
                            "browser recovery settlement failed: {error}"
                        ))
                    })?;
                if !matches!(
                    settlement,
                    BrowserSettlement::Committed | BrowserSettlement::Reconciled
                ) {
                    return Err(crate::errors::AppError::Other(format!(
                        "browser recovery remains system-owned: {settlement:?}"
                    )));
                }
            }
            Disposition::Prepared
            | Disposition::ReplayableExactGeneration
            | Disposition::ReplayableDigestCas
            | Disposition::SettledCommitted
            | Disposition::SettledReconciled => {}
            disposition => {
                return Err(crate::errors::AppError::Other(format!(
                    "browser recovery remains observe-only: {disposition:?}"
                )))
            }
        },
        None if objective.failure_code.as_deref()
            == Some("browser_observation_contract_required") => {}
        None => {
            return Err(crate::errors::AppError::Other(
                "browser recovery has no exact durable operation contract".into(),
            ))
        }
    }

    if objective.task_id.is_some() {
        crate::commands::tasks::resume_task_objective(app, objective, permit).await
    } else {
        crate::commands::chat::resume_chat_objective(app, objective, permit).await
    }
}

/// Domain-specific read-only reconciliation. Only durable evidence that no
/// unresolved provider side effect/output can be replayed reaches the Chat
/// executor. Auth additionally requires a current-revision capability receipt;
/// neither path treats an identity-shaped Objective as sufficient proof.
pub(crate) async fn require_provider_resume_evidence(
    pool: &SqlitePool,
    objective_id: &str,
    require_auth_receipt: bool,
) -> Result<(), crate::errors::AppError> {
    if require_auth_receipt {
        match super::auth_recovery::AuthRecoveryStore::new(pool.clone())
            .observe(objective_id)
            .await
            .map_err(|error| {
                crate::errors::AppError::Other(format!("auth recovery observation failed: {error}"))
            })? {
            super::auth_recovery::AuthRecoveryDisposition::QueueProvider { .. } => {}
            disposition => {
                return Err(crate::errors::AppError::Other(format!(
                    "auth recovery remains observe-only: {disposition:?}"
                )));
            }
        }
    }

    let disposition = super::provider_recovery::ProviderRecoveryStore::new(pool.clone())
        .observe(objective_id)
        .await
        .map_err(|error| {
            crate::errors::AppError::Other(format!("provider recovery observation failed: {error}"))
        })?;
    match disposition {
        super::provider_recovery::ProviderRecoveryDisposition::ReadyToAttempt { .. }
        | super::provider_recovery::ProviderRecoveryDisposition::ObserveOnlyPrepared { .. }
        | super::provider_recovery::ProviderRecoveryDisposition::RetrySafe { .. } => Ok(()),
        other => Err(crate::errors::AppError::Other(format!(
            "provider recovery remains observe-only: {other:?}"
        ))),
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

async fn poll_once_with_restart_admission<Schedule>(
    restart_admission: &tokio::sync::Mutex<()>,
    restart_reserved: &AtomicBool,
    store: &ObjectiveStore,
    owner: &str,
    limit: i64,
    lease_ms: i64,
    schedule: Schedule,
) -> anyhow::Result<usize>
where
    Schedule: FnMut(ClaimedRemediation),
{
    let _admission = restart_admission.lock().await;
    if restart_reserved.load(Ordering::SeqCst) {
        return Ok(0);
    }
    poll_once(store, owner, limit, lease_ms, schedule).await
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
        // This runs exactly once per backend process, before any Update claim
        // is admitted. A crash-left `install_started` receipt can therefore be
        // classified against the newly-installed exact version+build without
        // mistaking a live same-process installer for a replay opportunity.
        match crate::commands::update_safety::current_app_identity(&app) {
            Ok((current_version, current_build)) => {
                match crate::commands::update_safety::observe_latest_update_install(
                    &pool,
                    &current_version,
                    &current_build,
                    chrono::Utc::now().timestamp_millis(),
                )
                .await
                {
                    Ok(Some(receipt)) => tracing::info!(
                        receipt_id = %receipt.id,
                        state = ?receipt.state,
                        "startup: reconciled updater write-ahead receipt before recovery admission"
                    ),
                    Ok(None) => {}
                    Err(error) => tracing::warn!(
                        %error,
                        "startup: updater receipt observation deferred"
                    ),
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "startup: exact installed build identity unavailable; Update remains fenced"
            ),
        }
        let store = ObjectiveStore::new(pool.clone());
        loop {
            let Some(state) = app.try_state::<crate::AppState>() else {
                tracing::error!("objective supervisor stopped: application state is unavailable");
                return;
            };
            let restart_admission = state.update_restart_admission.clone();
            let restart_reserved = state.update_restart_reserved.clone();
            let claim_app = app.clone();
            let claim_pool = pool.clone();
            let claim_store = store.clone();
            let claim_owner = owner.clone();
            if let Err(error) = poll_once_with_restart_admission(
                &restart_admission,
                &restart_reserved,
                &store,
                &owner,
                8,
                LEASE_MS,
                move |claim| {
                    tauri::async_runtime::spawn(process_claim(
                        claim_app.clone(),
                        claim_pool.clone(),
                        claim_store.clone(),
                        claim_owner.clone(),
                        claim,
                    ));
                },
            )
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

    #[test]
    fn update_restart_reservation_blocks_every_non_update_recovery_domain() {
        for domain in RecoveryDomain::ALL {
            assert_eq!(
                restart_reservation_blocks_domain(true, domain),
                domain != RecoveryDomain::Update,
                "unexpected update reservation policy for {domain:?}"
            );
            assert!(!restart_reservation_blocks_domain(false, domain));
        }
    }

    fn claim_for(domain: RecoveryDomain, objective: ObjectiveSnapshot) -> ClaimedRemediation {
        ClaimedRemediation {
            objective,
            remediation_id: "remediation-adapter".into(),
            domain,
            failure_code: "platform_incident".into(),
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

    async fn claimed_context_recovery_authorization(
        suffix: &str,
    ) -> (
        SqlitePool,
        ObjectiveStore,
        ClaimedRemediation,
        codefactory_agent_loop::tool::MutationPermit,
        crate::agent::context_recovery::ContextRecoveryAuthorization,
    ) {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective_id = format!("objective-context-{suffix}");
        let session_id = format!("session-context-{suffix}");
        let anchor_root = format!("turn-context-anchor-{suffix}");
        let active_root = format!("turn-context-active-{suffix}");
        let binding_id = format!("binding-context-{suffix}");
        let attempt_id = format!("attempt-context-{suffix}");
        let owner = format!("owner-context-{suffix}");
        let objective = store
            .create(CreateObjective {
                id: objective_id.clone(),
                kind: ObjectiveKind::Informational,
                session_id: Some(session_id.clone()),
                root_turn_id: Some(anchor_root),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "context-p0-test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES (?, ?, ?)",
        )
        .bind(&active_root)
        .bind(&session_id)
        .bind(&objective_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES (?, ?, 'chat', 'chat_root_turn', ?, 2, ?, ?, ?, ?)",
        )
        .bind(&binding_id)
        .bind(&objective_id)
        .bind(&active_root)
        .bind(format!("sha256:{suffix}"))
        .bind(&active_root)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Context,
                failure_code: "context_overflow_after_compaction".into(),
                failure_signature: format!("sha256:context-{suffix}"),
                next_observation_at: now - 1,
                resume_cursor: Some(active_root.clone()),
            },
        )
        .unwrap();
        store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO objective_recovery_attempts
             (id, objective_id, root_turn_id, delivery_run_id, domain, attempt_index,
              failure_code, failure_class, output_started, side_effect_started,
              queue_wait_ms, runtime_ms, process_instance, resume_owner,
              terminal_decision, created_at)
             VALUES (?, ?, ?, NULL, 'context', 1,
                     'CONTEXT_OVERFLOW_AFTER_COMPACTION', 'context_capacity', 0, 0,
                     NULL, NULL, 'prior-process', 'agent_loop', 'waiting_system', ?)",
        )
        .bind(&attempt_id)
        .bind(&objective_id)
        .bind(&active_root)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let claim = store
            .claim_due_remediations(&owner, 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let permit = mutation_permit(&claim, &owner);
        let authorization =
            match crate::agent::context_recovery::ContextRecoveryStore::new(pool.clone())
                .reserve_claimed_recovery(&claim, &permit)
                .await
                .unwrap()
            {
                crate::agent::context_recovery::ContextRecoveryReservation::Authorized(value) => {
                    value
                }
                other => panic!("expected durable Context authorization, got {other:?}"),
            };
        (pool, store, claim, permit, authorization)
    }

    struct AdvancingContextPolicy {
        pool: SqlitePool,
        objective_id: String,
        remediation_id: String,
        advanced: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl codefactory_agent_loop::services::ContextPolicy for AdvancingContextPolicy {
        async fn context_window(&self, _estimated_tokens: u32) -> (u32, u32) {
            let now = chrono::Utc::now().timestamp_millis();
            sqlx::query(
                "UPDATE objectives
                 SET revision=revision+1, domain='chat', status='active',
                     decision_type='continue', resume_cursor='turn-context-steered',
                     remediation_id=NULL, lease_owner=NULL, lease_expires_at=NULL,
                     updated_at=? WHERE id=?",
            )
            .bind(now)
            .bind(&self.objective_id)
            .execute(&self.pool)
            .await
            .unwrap();
            sqlx::query(
                "UPDATE objective_remediations
                 SET status='superseded', lease_owner=NULL, lease_expires_at=NULL,
                     updated_at=? WHERE id=?",
            )
            .bind(now)
            .bind(&self.remediation_id)
            .execute(&self.pool)
            .await
            .unwrap();
            self.advanced.fetch_add(1, Ordering::SeqCst);
            (100_000, 100_000)
        }

        async fn supports_vision(&self) -> bool {
            true
        }

        async fn round_reasoning_effort(&self) -> String {
            String::new()
        }
    }

    #[derive(Default)]
    struct NoCallContextTransport {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl codefactory_agent_loop::transport::ModelTransport for NoCallContextTransport {
        async fn complete(
            &self,
            _messages: &[codefactory_agent_loop::types::ChatMessage],
            _tools: &[codefactory_agent_loop::types::ToolDefinition],
            _opts: &codefactory_agent_loop::transport::RoundOptions,
        ) -> std::result::Result<
            codefactory_agent_loop::transport::ModelResponse,
            codefactory_agent_loop::transport::TransportError,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(codefactory_agent_loop::transport::TransportError::Fatal(
                "provider must not be called by a stale Context runner".into(),
            ))
        }
    }

    #[derive(Default)]
    struct CountingContextCompactor {
        calls: AtomicUsize,
    }

    impl codefactory_agent_loop::services::ContextCompactor for CountingContextCompactor {
        fn compact(
            &self,
            messages: Vec<codefactory_agent_loop::types::ChatMessage>,
            _system_prompt: &str,
            _context_limit: u32,
            _tool_definitions: &[codefactory_agent_loop::types::ToolDefinition],
        ) -> codefactory_agent_loop::services::CompactionOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            codefactory_agent_loop::services::CompactionOutcome {
                messages,
                ..Default::default()
            }
        }
    }

    struct NoCallContextTools;

    #[async_trait::async_trait]
    impl codefactory_agent_loop::tool::ToolBackend for NoCallContextTools {
        async fn list_schemas(&self) -> Vec<codefactory_agent_loop::types::ToolDefinition> {
            Vec::new()
        }

        async fn execute(
            &self,
            _call: &codefactory_agent_loop::types::ToolCall,
            _args: &serde_json::Value,
            _ctx: &codefactory_agent_loop::tool::ToolCtx,
        ) -> std::result::Result<
            codefactory_agent_loop::tool::ToolInvocationResult,
            codefactory_agent_loop::tool::ToolError,
        > {
            panic!("tool backend must not be called by a stale Context runner")
        }
    }

    #[derive(Default)]
    struct CountingContextPersistence {
        recovery_attempts: AtomicUsize,
        notices: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl codefactory_agent_loop::journal::Persistence for CountingContextPersistence {
        async fn record_recovery_attempt(
            &self,
            _attempt: &codefactory_agent_loop::journal::RecoveryAttemptRow,
        ) -> codefactory_agent_loop::journal::PersistResult<()> {
            self.recovery_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn persist_message(
            &self,
            _role: &str,
            _content: &str,
            _input_tokens: Option<i64>,
            _output_tokens: Option<i64>,
            _tool_calls: Option<&[codefactory_agent_loop::types::ToolCall]>,
            _reasoning_content: Option<&str>,
            _endpoint_id: Option<&str>,
            _model_id: Option<&str>,
            _usage_request_id: Option<&str>,
        ) -> codefactory_agent_loop::journal::PersistResult<Option<String>> {
            Ok(None)
        }

        async fn persist_gate_message(
            &self,
            _content: &str,
            _state: &str,
        ) -> codefactory_agent_loop::journal::PersistResult<()> {
            self.notices.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn persist_gate_message_once(
            &self,
            _marker: &str,
            _content: &str,
            _state: &str,
        ) -> codefactory_agent_loop::journal::PersistResult<bool> {
            self.notices.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }

        async fn mark_rejected_candidate(
            &self,
            _message_id: Option<&str>,
        ) -> codefactory_agent_loop::journal::PersistResult<()> {
            Ok(())
        }

        async fn record_tool_call_started(
            &self,
            _message_id: &str,
            _tool_call: &codefactory_agent_loop::types::ToolCall,
        ) -> codefactory_agent_loop::journal::PersistResult<()> {
            Ok(())
        }

        async fn record_tool_call_outcome(
            &self,
            _tool_call: &codefactory_agent_loop::types::ToolCall,
            _status: &str,
            _result: Option<&str>,
            _error: Option<&str>,
            _duration_ms: u64,
        ) -> codefactory_agent_loop::journal::PersistResult<()> {
            Ok(())
        }

        async fn persist_cancelled_tool_batch(
            &self,
            _remaining: &[codefactory_agent_loop::types::ToolCall],
        ) -> codefactory_agent_loop::journal::PersistResult<Vec<String>> {
            Ok(Vec::new())
        }

        async fn record_usage(
            &self,
            _row: codefactory_agent_loop::journal::UsageRow<'_>,
        ) -> codefactory_agent_loop::journal::PersistResult<bool> {
            Ok(false)
        }
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
    fn provider_and_auth_are_executable_only_through_domain_specific_reconcilers() {
        assert!(domain_has_executable_adapter(RecoveryDomain::Provider));
        assert!(domain_has_executable_adapter(RecoveryDomain::Auth));
        assert_ne!(
            DEFAULT_ADAPTER_REGISTRY
                .adapter_for(RecoveryDomain::Provider)
                .unwrap()
                .capability,
            AdapterCapability::ChatResume,
            "provider recovery must not be an identity-shaped Chat fallback",
        );
        assert_ne!(
            DEFAULT_ADAPTER_REGISTRY
                .adapter_for(RecoveryDomain::Auth)
                .unwrap()
                .capability,
            AdapterCapability::ChatResume,
            "auth recovery must require a current capability receipt",
        );
    }

    #[test]
    fn update_is_executable_only_through_its_exact_target_reconciler() {
        assert!(domain_has_executable_adapter(RecoveryDomain::Update));
        let mut objective = ObjectiveSnapshot::new(
            "objective-update-adapter",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Update,
            "installed_exact_update",
        );
        objective.resume_cursor = Some(crate::commands::update_safety::update_target_resource_id(
            "1.80.0",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ));
        let mut claim = claim_for(RecoveryDomain::Update, objective);
        claim.binding_id = Some("binding-update-adapter".into());
        claim.resource_generation = Some(1);
        let adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Update)
            .unwrap();
        assert_eq!(
            adapter.reconcile(adapter.observe(&claim)),
            ReconciledPlan {
                action: ReconciledAction::Execute(AdapterExecutor::Update)
            }
        );
    }

    #[test]
    fn permission_is_executable_only_through_opaque_bound_reconciler() {
        assert!(domain_has_executable_adapter(RecoveryDomain::Permission));
        let mut objective = ObjectiveSnapshot::new(
            "6afc1ef3-48f6-4b2e-9650-6c66de11d16e",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Permission,
            "validated_change",
        );
        objective.session_id = Some("session-permission-adapter".into());
        objective.root_turn_id = Some("turn-permission-adapter".into());
        let adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Permission)
            .unwrap();

        let incomplete = claim_for(RecoveryDomain::Permission, objective.clone());
        assert_eq!(
            adapter.reconcile(adapter.observe(&incomplete)),
            ReconciledPlan {
                action: ReconciledAction::ReobserveOnly("permission_identity_incomplete")
            }
        );
        let mut complete = incomplete;
        complete.binding_id = Some("binding-permission-adapter".into());
        complete.resource_generation = Some(1);
        assert_eq!(
            adapter.reconcile(adapter.observe(&complete)),
            ReconciledPlan {
                action: ReconciledAction::Execute(AdapterExecutor::Permission)
            }
        );
        assert_ne!(
            adapter.capability,
            AdapterCapability::ChatResume,
            "Permission recovery must inspect a durable exact intent before Chat/Task resume",
        );
    }

    #[tokio::test]
    async fn allowed_permission_claim_auto_reaches_exact_action_once_through_registry() {
        use crate::agent::permission_intent::{
            PermissionClaimAction, PermissionIntentRequest, PermissionIntentStatus,
            PermissionIntentStore, PermissionPromptResponse, PermissionScope,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "7d3be932-1a8b-4654-8ead-84d19f2d0b68".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-permission-auto-resume".into()),
                root_turn_id: Some("turn-permission-auto-resume".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "permission-supervisor-test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let binding_id = "binding-permission-auto-resume";
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, created_at, updated_at)
             VALUES (?, ?, 'chat', 'chat_root_turn', ?, 1,
                     'sha256:permission-auto-resume', ?, ?)",
        )
        .bind(binding_id)
        .bind(&objective.id)
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let permission_store = PermissionIntentStore::new(pool.clone());
        let interrupted = permission_store
            .create_pending(
                &PermissionIntentRequest {
                    scope: PermissionScope {
                        objective_id: objective.id.clone(),
                        objective_revision: objective.revision,
                        binding_id: binding_id.into(),
                        resource_generation: 1,
                    },
                    session_id: objective.session_id.clone().unwrap(),
                    provider_tool_call_id: "provider-permission-auto-resume".into(),
                    tool_name: "browser_session".into(),
                    args: serde_json::json!({"action":"click","selector":"#publish"}),
                    bash_command: None,
                    expires_at: now + 60_000,
                    created_process_instance: "prior-process".into(),
                },
                now,
            )
            .await
            .unwrap();
        permission_store
            .record_interruption(
                &interrupted.prompt_key(),
                PermissionIntentStatus::ChannelClosed,
                now + 1,
            )
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Permission,
                failure_code: "permission_channel_closed".into(),
                failure_signature: "sha256:permission-auto-resume".into(),
                next_observation_at: now,
                resume_cursor: objective.root_turn_id.clone(),
            },
        )
        .unwrap();
        store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        let prompt_claim = store
            .claim_due_remediations("permission-prompt-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let prompt_permit = mutation_permit(&prompt_claim, "permission-prompt-owner");
        let projected = match permission_store
            .observe_claimed_recovery(&prompt_permit, "current-process", now + 60_000, now + 2)
            .await
            .unwrap()
        {
            PermissionClaimAction::ProjectPrompt(observation) => observation,
            other => panic!("expected prompt projection, got {other:?}"),
        };
        permission_store
            .settle_projected_response(
                &projected.snapshot.intent_id,
                PermissionPromptResponse::Allow,
                now + 3,
            )
            .await
            .unwrap();
        let authorized_claim = store
            .claim_due_remediations("permission-action-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(authorized_claim.objective.id, objective.id);
        let permit = mutation_permit(&authorized_claim, "permission-action-owner");
        let adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Permission)
            .unwrap();
        let mutation_count = Arc::new(AtomicUsize::new(0));
        let observed_mutations = mutation_count.clone();
        let observed_store = permission_store.clone();
        let observed_permit = permit.clone();
        let observed_action = projected.snapshot.action_signature.clone();
        let observed_objective = authorized_claim.objective.clone();

        drive_adapter(
            adapter,
            &store,
            &authorized_claim,
            &permit,
            move |executor| async move {
                assert_eq!(executor, AdapterExecutor::Permission);
                assert_eq!(
                    observed_store
                        .observe_claimed_recovery(
                            &observed_permit,
                            "current-process",
                            now + 60_000,
                            now + 4,
                        )
                        .await
                        .unwrap(),
                    PermissionClaimAction::ResumeAuthorizedAction
                );
                let scope = PermissionScope {
                    objective_id: observed_objective.id,
                    objective_revision: observed_objective.revision,
                    binding_id: observed_permit.binding_id.clone().unwrap(),
                    resource_generation: observed_permit.resource_generation.unwrap(),
                };
                if observed_store
                    .reserve_exact_recovery_allow(
                        &scope,
                        &observed_action,
                        &observed_permit,
                        now + 5,
                    )
                    .await
                    .unwrap()
                {
                    observed_mutations.fetch_add(1, Ordering::SeqCst);
                }
                assert!(!observed_store
                    .reserve_exact_recovery_allow(
                        &scope,
                        &observed_action,
                        &observed_permit,
                        now + 6,
                    )
                    .await
                    .unwrap());
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(mutation_count.load(Ordering::SeqCst), 1);
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
        let mut claim = claim_for(RecoveryDomain::Provider, objective);
        claim.binding_id = Some("binding-provider".into());
        claim.resource_generation = Some(1);
        let adapter = DEFAULT_ADAPTER_REGISTRY.adapter_for(claim.domain).unwrap();
        assert_eq!(adapter.domain(), RecoveryDomain::Provider);
        assert_eq!(adapter.capability, AdapterCapability::ProviderResume);
        assert_eq!(
            adapter.reconcile(adapter.observe(&claim)).action,
            ReconciledAction::Execute(AdapterExecutor::Provider)
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

    #[test]
    fn context_recovery_is_an_executable_registered_capability() {
        assert!(domain_has_executable_adapter(RecoveryDomain::Context));
        let adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Context)
            .unwrap();
        assert_eq!(adapter.capability, AdapterCapability::ContextResume);
    }

    #[test]
    fn browser_recovery_is_an_executable_domain_specific_capability() {
        assert!(domain_has_executable_adapter(RecoveryDomain::Browser));
        let adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Browser)
            .unwrap();
        assert_ne!(
            adapter.capability,
            AdapterCapability::ChatResume,
            "Browser recovery must inspect its exact durable operation before resuming Chat",
        );
    }

    #[test]
    fn tool_recovery_is_executable_only_with_exact_objective_binding_identity() {
        assert!(domain_has_executable_adapter(RecoveryDomain::Tool));
        let mut objective = ObjectiveSnapshot::new(
            "objective-tool-adapter",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Tool,
            "validated_change",
        );
        objective.session_id = Some("session-tool-adapter".into());
        objective.root_turn_id = Some("turn-tool-adapter".into());
        let adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Tool)
            .unwrap();

        let incomplete = claim_for(RecoveryDomain::Tool, objective);
        assert!(matches!(
            adapter.reconcile(adapter.observe(&incomplete)).action,
            ReconciledAction::ReobserveOnly(_)
        ));

        let mut complete = incomplete;
        complete.binding_id = Some("binding-tool-adapter".into());
        complete.resource_generation = Some(1);
        assert_eq!(
            adapter.reconcile(adapter.observe(&complete)),
            ReconciledPlan {
                action: ReconciledAction::Execute(AdapterExecutor::Tool)
            }
        );
        assert_ne!(
            adapter.capability,
            AdapterCapability::ChatResume,
            "Tool recovery must reconcile the exact durable receipt before Chat/Task resume",
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
                domain: RecoveryDomain::Terminal,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Terminal,
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
            .adapter_for(RecoveryDomain::Terminal)
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
    async fn update_restart_admission_blocks_then_releases_objective_claims() {
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
        let restart_admission = tokio::sync::Mutex::new(());
        let restart_reserved = AtomicBool::new(true);

        let blocked = poll_once_with_restart_admission(
            &restart_admission,
            &restart_reserved,
            &store,
            "blocked-poll-owner",
            1,
            30_000,
            |_| panic!("restart reservation must prevent Objective claims"),
        )
        .await
        .unwrap();
        assert_eq!(blocked, 0);
        restart_reserved.store(false, Ordering::SeqCst);

        let claimed = poll_once_with_restart_admission(
            &restart_admission,
            &restart_reserved,
            &store,
            "new-poll-owner",
            1,
            30_000,
            move |claim| scheduled_claims.lock().unwrap().push(claim),
        )
        .await
        .unwrap();

        assert_eq!(claimed, 1);
        let scheduled = scheduled.lock().unwrap();
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].domain, RecoveryDomain::Chat);
        assert!(scheduled[0].claim_epoch > prior_claim.claim_epoch);
    }

    #[tokio::test]
    async fn restart_context_claim_reuses_same_objective_and_authorizes_one_recompaction() {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-context-restart".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-context-restart".into()),
                root_turn_id: Some("turn-context-anchor".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "context-restart-test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES ('turn-context-active', 'session-context-restart', ?)",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES ('binding-context-restart', ?, 'chat', 'chat_root_turn',
                     'turn-context-active', 2, 'sha256:context-restart',
                     'turn-context-active', ?, ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Context,
                failure_code: "context_overflow_after_compaction".into(),
                failure_signature: "sha256:context-restart".into(),
                next_observation_at: now - 1,
                resume_cursor: Some("turn-context-active".into()),
            },
        )
        .unwrap();
        store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO objective_recovery_attempts
             (id, objective_id, root_turn_id, delivery_run_id, domain, attempt_index,
              failure_code, failure_class, output_started, side_effect_started,
              queue_wait_ms, runtime_ms, process_instance, resume_owner,
              terminal_decision, created_at)
             VALUES ('attempt-context-restart', ?, 'turn-context-active', NULL, 'context', 1,
                     'CONTEXT_OVERFLOW_AFTER_COMPACTION', 'context_capacity', 0, 0,
                     NULL, NULL, 'prior-process', 'agent_loop', 'waiting_system', ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let prior_claim = store
            .claim_due_remediations("prior-process", 1, 1_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&prior_claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&objective.id)
            .execute(&pool)
            .await
            .unwrap();

        let scheduled = Arc::new(Mutex::new(Vec::new()));
        let scheduled_claims = scheduled.clone();
        assert_eq!(
            poll_once(&store, "replacement-process", 1, 30_000, move |claim| {
                scheduled_claims.lock().unwrap().push(claim)
            },)
            .await
            .unwrap(),
            1,
        );
        let claim = scheduled.lock().unwrap().pop().unwrap();
        assert_eq!(claim.objective.id, objective.id);
        assert_eq!(
            claim.objective.root_turn_id.as_deref(),
            Some("turn-context-anchor")
        );
        assert_eq!(
            claim.objective.resume_cursor.as_deref(),
            Some("turn-context-active")
        );
        assert_eq!(claim.binding_id.as_deref(), Some("binding-context-restart"));
        assert_eq!(claim.resource_generation, Some(2));
        assert!(claim.claim_epoch > prior_claim.claim_epoch);

        let permit = mutation_permit(&claim, "replacement-process");
        let adapter = DEFAULT_ADAPTER_REGISTRY
            .adapter_for(RecoveryDomain::Context)
            .unwrap();
        let execution_count = Arc::new(AtomicUsize::new(0));
        let observed_count = execution_count.clone();
        let observed_pool = pool.clone();
        let observed_claim = claim.clone();
        let observed_objective = claim.objective.clone();
        let observed_permit = permit.clone();
        drive_adapter(
            adapter,
            &store,
            &claim,
            &permit,
            move |executor| async move {
                assert_eq!(executor, AdapterExecutor::Context);
                let authorization = match crate::agent::context_recovery::ContextRecoveryStore::new(
                    observed_pool.clone(),
                )
                .reserve_claimed_recovery(&observed_claim, &observed_permit)
                .await
                .map_err(|error| crate::errors::AppError::Other(error.to_string()))?
                {
                    crate::agent::context_recovery::ContextRecoveryReservation::Authorized(
                        authorization,
                    ) => authorization,
                    other => {
                        return Err(crate::errors::AppError::Other(format!(
                            "expected durable Context authorization, got {other:?}"
                        )))
                    }
                };
                assert!(crate::agent::claimed_context_compression_authorization(
                    &observed_objective,
                    &observed_permit,
                    authorization,
                )
                .is_some());
                observed_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(execution_count.load(Ordering::SeqCst), 1);

        let expired = chrono::Utc::now().timestamp_millis() - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&objective.id)
            .execute(&pool)
            .await
            .unwrap();
        let replacement = store
            .claim_due_remediations("competing-process", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let replacement_permit = mutation_permit(&replacement, "competing-process");
        assert!(matches!(
            crate::agent::context_recovery::ContextRecoveryStore::new(pool.clone())
                .reserve_claimed_recovery(&replacement, &replacement_permit)
                .await
                .unwrap(),
            crate::agent::context_recovery::ContextRecoveryReservation::ObserveOnly(
                crate::agent::context_recovery::ContextRecoveryDisposition::ObserveOnlyAuthorizationConsumed
            )
        ));
        assert_eq!(
            poll_once(&store, "third-process", 1, 30_000, |_| {})
                .await
                .unwrap(),
            0,
            "a live replacement claim must fence duplicate restart execution",
        );
    }

    #[tokio::test]
    async fn context_authorization_is_fenced_when_steer_advances_cursor_before_provider() {
        let (pool, store, claim, permit, authorization) =
            claimed_context_recovery_authorization("stale-before-provider").await;
        assert!(
            crate::agent::context_recovery::ContextRecoveryStore::new(pool.clone())
                .authorization_is_current(&authorization, &permit)
                .await
                .unwrap()
        );

        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE objectives SET revision=revision+1, domain='chat', status='active',
                    decision_type='continue', resume_cursor='turn-context-new',
                    remediation_id=NULL, lease_owner=NULL, lease_expires_at=NULL, updated_at=?
             WHERE id=?",
        )
        .bind(now)
        .bind(&claim.objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objective_remediations SET status='superseded', lease_owner=NULL,
                    lease_expires_at=NULL, updated_at=? WHERE id=?",
        )
        .bind(now)
        .bind(&claim.remediation_id)
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            !crate::agent::context_recovery::ContextRecoveryStore::new(pool)
                .authorization_is_current(&authorization, &permit)
                .await
                .unwrap()
        );
        assert!(!store.claim_is_current(&permit).await.unwrap());
    }

    #[tokio::test]
    async fn real_agent_loop_revalidates_durable_context_claim_after_window_lookup() {
        let (pool, _store, claim, permit, authorization) =
            claimed_context_recovery_authorization("loop-final-fence").await;
        let policy = Arc::new(AdvancingContextPolicy {
            pool: pool.clone(),
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id.clone(),
            advanced: AtomicUsize::new(0),
        });
        let transport = Arc::new(NoCallContextTransport::default());
        let compactor = Arc::new(CountingContextCompactor::default());
        let persistence = Arc::new(CountingContextPersistence::default());
        let events = Arc::new(codefactory_agent_loop::events::CollectingEventSink::new());
        let services = codefactory_agent_loop::run::LoopServices {
            transport: transport.clone(),
            tools: Arc::new(NoCallContextTools),
            persistence: persistence.clone(),
            events: events.clone(),
            budget: Arc::new(codefactory_agent_loop::journal::NullBudget),
            compactor: compactor.clone(),
            context_compaction_gate: Arc::new(
                crate::agent::context_recovery::DurableContextCompactionGate::new(
                    pool,
                    authorization,
                    permit.clone(),
                ),
            ),
            permission: Arc::new(codefactory_agent_loop::services::AllowAllPermissions),
            hooks: Arc::new(codefactory_agent_loop::services::NoOpHooks),
            context_policy: policy.clone(),
            fact_checker: Arc::new(codefactory_agent_loop::services::NoOpFactChecker),
            steer: Arc::new(codefactory_agent_loop::services::NoSteering),
        };
        let inputs = codefactory_agent_loop::run::LoopInputs {
            messages: vec![codefactory_agent_loop::types::ChatMessage {
                role: "user".into(),
                content: codefactory_agent_loop::types::MessageContent::Text(
                    "continue the exact objective".into(),
                ),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            }],
            system_prompt: "system".into(),
            tool_defs: Vec::new(),
            completion_instruction: "continue the exact objective".into(),
            fact_check_instruction: String::new(),
            audit_session_id: "audit-context-loop".into(),
            root_turn_id: claim.objective.resume_cursor.clone(),
            mutation_permit: Some(permit),
            knowledge_library_ids: None,
            cancel: None,
        };
        let config = codefactory_agent_loop::run::RunConfig {
            finalization: codefactory_agent_loop::run::FinalizationPolicy::ReleaseWithWarning,
            turn_capability: codefactory_agent_loop::run::TurnCapability::Implement,
            gate_benchmark: false,
            progress_window: 8,
            recovery_limit: 0,
            max_iterations: 1,
            wall_budget_applies: false,
            context_compression: true,
            overload_backoff: false,
            overload_retry_delays: [std::time::Duration::ZERO; 2],
            inspection_budget: false,
            replay_rejected_draft: false,
            tool_heartbeat_interval: None,
            long_tool_wait_threshold: std::time::Duration::from_secs(60),
            tool_amplification_threshold: None,
            session_id: "session-context-loop".into(),
            endpoint_name: "test".into(),
            model_id: "model".into(),
            base_url: "http://example.invalid".into(),
            usage_run_id: "usage-context-loop".into(),
            surface: "interactive".into(),
            task_id: None,
            anonymous: false,
            is_chatgpt: false,
            cwd: std::path::PathBuf::from("/tmp"),
        };

        let error = codefactory_agent_loop::run::run_agent_loop(inputs, config, services)
            .await
            .expect_err("stale Objective must be fenced before compaction");

        assert!(error.to_string().contains("CONTEXT_RECOVERY_FENCED"));
        assert_eq!(policy.advanced.load(Ordering::SeqCst), 1);
        assert_eq!(compactor.calls.load(Ordering::SeqCst), 0);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
        assert_eq!(persistence.recovery_attempts.load(Ordering::SeqCst), 0);
        assert_eq!(persistence.notices.load(Ordering::SeqCst), 0);
        assert!(!events.events().iter().any(|event| matches!(
            event,
            codefactory_agent_loop::types::StreamEvent::ContextCompressed { .. }
        )));
    }

    #[tokio::test]
    async fn startup_partial_provider_attempt_is_routed_before_generic_chat_resume() {
        use crate::agent::provider_recovery::{
            ProviderAttemptSpec, ProviderEpisodeSpec, ProviderOwnerPermit, ProviderRecoveryStore,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-provider-startup-partial".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-provider-startup-partial".into()),
                root_turn_id: Some("turn-provider-startup-partial".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES ('binding-provider-startup-partial', ?, 'chat', 'chat_root_turn', ?,
                     1, 'sha256:provider-startup-partial', ?, ?, ?)",
        )
        .bind(&objective.id)
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id,
              objective_revision, status, created_process_instance, created_at, updated_at)
             VALUES ('run-provider-startup-partial', ?, ?, ?, 1, 'active',
                     'prior-process', ?, ?)",
        )
        .bind(objective.session_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let permit = ProviderOwnerPermit::chat_run(
            &objective.id,
            1,
            "binding-provider-startup-partial",
            1,
            "run-provider-startup-partial",
            1,
        );
        let provider = ProviderRecoveryStore::new(pool.clone());
        provider
            .open_episode(
                &permit,
                &ProviderEpisodeSpec {
                    id: "episode-provider-startup-partial".into(),
                    session_id: objective.session_id.clone().unwrap(),
                    root_turn_id: objective.root_turn_id.clone().unwrap(),
                    policy: "fixed".into(),
                    candidate_snapshot_digest:
                        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .into(),
                    candidate_snapshot_json: r#"[{"endpoint":"test","model":"test-model"}]"#.into(),
                    resume_cursor: objective.root_turn_id.clone().unwrap(),
                },
                now,
            )
            .await
            .unwrap();
        provider
            .begin_attempt(
                &permit,
                &ProviderAttemptSpec {
                    id: "attempt-provider-startup-partial".into(),
                    episode_id: "episode-provider-startup-partial".into(),
                    endpoint: "test".into(),
                    model: "test-model".into(),
                    request_digest:
                        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .into(),
                    resume_cursor: objective.root_turn_id.clone().unwrap(),
                },
                now + 1,
            )
            .await
            .unwrap();
        provider
            .mark_in_flight(&permit, "attempt-provider-startup-partial", now + 2)
            .await
            .unwrap();
        provider
            .append_partial_output(
                &permit,
                "attempt-provider-startup-partial",
                "visible fragment",
                now + 3,
            )
            .await
            .unwrap();
        sqlx::query(
            "UPDATE chat_run_controls SET status='completed', settled_at=?, updated_at=?
             WHERE run_instance_id='run-provider-startup-partial'",
        )
        .bind(now + 4)
        .bind(now + 4)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            reconcile_provider_recovery_on_startup(&pool).await.unwrap(),
            1
        );
        let current = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(current.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(current.domain, RecoveryDomain::Provider);
        assert_eq!(
            current.failure_code.as_deref(),
            Some("provider_partial_output_unresolved")
        );
        assert!(
            require_provider_resume_evidence(&pool, &objective.id, false)
                .await
                .unwrap_err()
                .to_string()
                .contains("ObserveOnlyPartial")
        );
    }

    /// Production-shaped restart boundary for an unattended coding turn:
    /// round one produced a tool call whose exact side-effect receipt is
    /// committed, then the process died after POSTing round two but before any
    /// bytes or tool intent were observed. Historical turn latches must not
    /// make that latest effect-free request permanently unrecoverable.
    #[tokio::test]
    async fn startup_effect_free_provider_request_after_committed_tool_is_retry_safe() {
        use crate::agent::provider_recovery::{
            ProviderAttemptSpec, ProviderEpisodeSpec, ProviderOwnerPermit,
            ProviderRecoveryDisposition, ProviderRecoveryStore,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-provider-restart-safe".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-provider-restart-safe".into()),
                root_turn_id: Some("turn-provider-restart-safe".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES ('binding-provider-restart-safe', ?, 'chat', 'chat_root_turn', ?,
                     1, 'sha256:provider-restart-safe', ?, ?, ?)",
        )
        .bind(&objective.id)
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id,
              objective_revision, status, created_process_instance, created_at, updated_at)
             VALUES ('run-provider-restart-safe', ?, ?, ?, 1, 'active',
                     'dead-process', ?, ?)",
        )
        .bind(objective.session_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let permit = ProviderOwnerPermit::chat_run(
            &objective.id,
            1,
            "binding-provider-restart-safe",
            1,
            "run-provider-restart-safe",
            1,
        );
        let provider = ProviderRecoveryStore::new(pool.clone());
        provider
            .open_episode(
                &permit,
                &ProviderEpisodeSpec {
                    id: "episode-provider-restart-safe".into(),
                    session_id: objective.session_id.clone().unwrap(),
                    root_turn_id: objective.root_turn_id.clone().unwrap(),
                    policy: "fixed".into(),
                    candidate_snapshot_digest:
                        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .into(),
                    candidate_snapshot_json:
                        r#"[{"endpoint":"test","model":"test-model"}]"#.into(),
                    resume_cursor: objective.root_turn_id.clone().unwrap(),
                },
                now,
            )
            .await
            .unwrap();
        provider
            .begin_attempt(
                &permit,
                &ProviderAttemptSpec {
                    id: "attempt-provider-tool-call".into(),
                    episode_id: "episode-provider-restart-safe".into(),
                    endpoint: "test".into(),
                    model: "test-model".into(),
                    request_digest:
                        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .into(),
                    resume_cursor: objective.root_turn_id.clone().unwrap(),
                },
                now + 1,
            )
            .await
            .unwrap();
        provider
            .mark_in_flight(&permit, "attempt-provider-tool-call", now + 2)
            .await
            .unwrap();
        provider
            .commit_response(
                &permit,
                "attempt-provider-tool-call",
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "tool call",
                false,
                now + 3,
            )
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES ('receipt-provider-restart-safe', ?, 'binding-provider-restart-safe', 1,
                     'tool:write_file', 'write-once', 'started', ?, ?)",
        )
        .bind(&objective.id)
        .bind(now + 4)
        .bind(now + 4)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objectives SET side_effect_started=1 WHERE id=?;
             UPDATE objective_bindings SET side_effect_started=1
              WHERE id='binding-provider-restart-safe'",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        provider
            .begin_attempt(
                &permit,
                &ProviderAttemptSpec {
                    id: "attempt-provider-crash-left".into(),
                    episode_id: "episode-provider-restart-safe".into(),
                    endpoint: "test".into(),
                    model: "test-model".into(),
                    request_digest:
                        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                            .into(),
                    resume_cursor: objective.root_turn_id.clone().unwrap(),
                },
                now + 5,
            )
            .await
            .unwrap();
        provider
            .mark_in_flight(&permit, "attempt-provider-crash-left", now + 6)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE chat_run_controls SET status='completed', settled_at=?, updated_at=?
             WHERE run_instance_id='run-provider-restart-safe'",
        )
        .bind(now + 7)
        .bind(now + 7)
        .execute(&pool)
        .await
        .unwrap();

        // A receipt that was only started is still uncertain. The startup
        // reconciler must leave the provider request fenced until that exact
        // side effect has a terminal observation.
        assert_eq!(
            provider
                .reconcile_stale_effect_free_in_flight(now + 8)
                .await
                .unwrap(),
            0
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM provider_route_attempts
             WHERE id='attempt-provider-crash-left'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "in_flight");
        sqlx::query(
            "UPDATE side_effect_receipts
             SET status='committed', observed_at=?
             WHERE id='receipt-provider-restart-safe'",
        )
        .bind(now + 9)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            reconcile_provider_recovery_on_startup(&pool).await.unwrap(),
            1
        );
        let current = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(current.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(current.domain, RecoveryDomain::Provider);
        assert_eq!(
            current.failure_code.as_deref(),
            Some("provider_retry_safe_after_restart")
        );
        assert!(matches!(
            provider.observe(&objective.id).await.unwrap(),
            ProviderRecoveryDisposition::RetrySafe { attempt_id, .. }
                if attempt_id == "attempt-provider-crash-left"
        ));
    }

    #[tokio::test]
    async fn oauth_receipt_and_replay_safe_provider_attempt_unlock_same_objective() {
        use crate::agent::auth_recovery::{AuthCapabilityProbe, AuthObservationSource};
        use crate::agent::provider_recovery::{
            ProviderAttemptSpec, ProviderEpisodeSpec, ProviderOwnerPermit, ProviderRecoveryStore,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-auth-provider-resume".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-auth-provider-resume".into()),
                root_turn_id: Some("turn-auth-provider-resume".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES ('binding-auth-provider-resume', ?, 'chat', 'chat_root_turn', ?, 1,
                     'sha256:auth-provider-resume', ?, ?, ?)",
        )
        .bind(&objective.id)
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id,
              objective_revision, status, created_process_instance, created_at, updated_at)
             VALUES ('run-auth-provider-resume', ?, ?, ?, 1, 'active',
                     'test-process', ?, ?)",
        )
        .bind(objective.session_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let permit = ProviderOwnerPermit::chat_run(
            &objective.id,
            1,
            "binding-auth-provider-resume",
            1,
            "run-auth-provider-resume",
            1,
        );
        let provider = ProviderRecoveryStore::new(pool.clone());
        provider
            .open_episode(
                &permit,
                &ProviderEpisodeSpec {
                    id: "episode-auth-provider-resume".into(),
                    session_id: objective.session_id.clone().unwrap(),
                    root_turn_id: objective.root_turn_id.clone().unwrap(),
                    policy: "fixed".into(),
                    candidate_snapshot_digest:
                        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .into(),
                    candidate_snapshot_json: r#"[{"endpoint":"chatgpt","model":"gpt-test"}]"#
                        .into(),
                    resume_cursor: objective.root_turn_id.clone().unwrap(),
                },
                now,
            )
            .await
            .unwrap();
        provider
            .begin_attempt(
                &permit,
                &ProviderAttemptSpec {
                    id: "attempt-auth-provider-resume".into(),
                    episode_id: "episode-auth-provider-resume".into(),
                    endpoint: "chatgpt".into(),
                    model: "gpt-test".into(),
                    request_digest:
                        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                            .into(),
                    resume_cursor: objective.root_turn_id.clone().unwrap(),
                },
                now + 1,
            )
            .await
            .unwrap();
        provider
            .record_failure(
                &permit,
                "attempt-auth-provider-resume",
                "provider_auth",
                "provider_auth_unavailable",
                true,
                now + 2,
            )
            .await
            .unwrap();
        let authorization = DecisionRouter::route(
            &objective,
            RouteSignal::AuthorizationRequired {
                domain: RecoveryDomain::Auth,
                request_key: format!("chatgpt-auth:{}", objective.id),
                action_signature: format!("oauth:chatgpt:resume:{}", objective.id),
                resume_cursor: objective.root_turn_id.clone(),
            },
        )
        .unwrap();
        store
            .apply_decision(objective.revision, authorization)
            .await
            .unwrap();
        let observed = crate::codex_auth::observe_chatgpt_auth_capability(
            &pool,
            AuthObservationSource::Callback,
            AuthCapabilityProbe::Ready {
                identity_material: b"secret-never-persisted",
            },
        )
        .await
        .unwrap();
        assert_eq!(observed.queued_objectives, 1);
        assert_eq!(observed.receipts_recorded, 1);
        let current = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(current.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(current.domain, RecoveryDomain::Auth);
        require_provider_resume_evidence(&pool, &objective.id, true)
            .await
            .unwrap();
        let serialized: String = sqlx::query_scalar(
            "SELECT group_concat(capability_digest || request_key || credential_ref, '|')
             FROM auth_capability_receipts",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!serialized.contains("secret-never-persisted"));
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
            .claim_due_remediations("short-heartbeat-owner", 1, 2_000)
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
            2_000,
            Duration::from_millis(100),
            async move {
                // Cross more than two original TTLs. Without the adapter being
                // awaited by the heartbeat owner, this claim is necessarily stale.
                tokio::time::sleep(Duration::from_millis(4_500)).await;
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
