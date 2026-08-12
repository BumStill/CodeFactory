// SPDX-License-Identifier: Apache-2.0
//! Durable, cross-domain business-objective control plane.
//!
//! Turn, task, tool and delivery rows remain compatibility projections. This
//! module owns the additive truth tables and the typed transitions that decide
//! whether work remains system-owned, needs genuinely new user input, or has
//! enough evidence to complete.

use anyhow::{anyhow, bail, Context};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }

            fn parse(value: &str) -> anyhow::Result<Self> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => bail!("unknown {} value: {value}", stringify!($name)),
                }
            }
        }
    };
}

string_enum!(ObjectiveKind {
    Informational => "informational",
    LocalMutation => "local_mutation",
    Delivery => "delivery",
    Live => "live",
    LegacyOrphan => "legacy_orphan",
});

const fn objective_kind_rank(kind: ObjectiveKind) -> u8 {
    match kind {
        ObjectiveKind::LegacyOrphan => 0,
        ObjectiveKind::Informational => 1,
        ObjectiveKind::LocalMutation => 2,
        ObjectiveKind::Delivery => 3,
        ObjectiveKind::Live => 4,
    }
}

pub fn current_process_instance() -> String {
    format!(
        "{}:{}",
        std::process::id(),
        crate::storage::db::current_process_start_token()
            .unwrap_or_else(|| "unknown-process-start".into())
    )
}

string_enum!(ObjectiveStatus {
    Active => "active",
    WaitingSystem => "waiting_system",
    WaitingCoreInput => "waiting_core_input",
    WaitingAuthorization => "waiting_authorization",
    WaitingBusinessDecision => "waiting_business_decision",
    Completed => "completed",
    Cancelled => "cancelled",
    LegacyOrphan => "legacy_orphan",
});

impl ObjectiveStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    pub const fn is_system_owned(self) -> bool {
        matches!(self, Self::Active | Self::WaitingSystem)
    }
}

string_enum!(RecoveryDomain {
    Chat => "chat",
    Context => "context",
    Tool => "tool",
    Permission => "permission",
    Task => "task",
    Provider => "provider",
    Auth => "auth",
    Browser => "browser",
    Terminal => "terminal",
    Delivery => "delivery",
    Release => "release",
    Update => "update",
});

impl RecoveryDomain {
    /// Closed world used by the recovery adapter registry and its conformance
    /// tests. Adding a domain without registering an adapter must fail review
    /// and tests instead of silently falling back to another domain's runner.
    pub const ALL: [Self; 12] = [
        Self::Chat,
        Self::Context,
        Self::Tool,
        Self::Permission,
        Self::Task,
        Self::Provider,
        Self::Auth,
        Self::Browser,
        Self::Terminal,
        Self::Delivery,
        Self::Release,
        Self::Update,
    ];
}

string_enum!(DecisionType {
    Continue => "continue",
    Waiting => "waiting",
    ApplyRecommended => "apply_recommended",
    PlatformIncident => "platform_incident",
    FailedInternal => "failed_internal",
    CoreInputRequired => "core_input_required",
    AuthorizationRequired => "authorization_required",
    NeedsBusinessDecision => "needs_business_decision",
    Complete => "complete",
    Cancelled => "cancelled",
});

string_enum!(EvidenceKind {
    InformationalAnswer => "informational_answer",
    CurrentStateAcceptance => "current_state_acceptance",
    ChangeSet => "change_set",
    PostChangeValidation => "post_change_validation",
    DeliveryReceipt => "delivery_receipt",
    LiveVerification => "live_verification",
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveSnapshot {
    pub id: String,
    pub revision: i64,
    pub kind: ObjectiveKind,
    pub session_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub task_id: Option<String>,
    pub delivery_run_id: Option<String>,
    pub status: ObjectiveStatus,
    pub decision_type: DecisionType,
    pub domain: RecoveryDomain,
    pub requested_acceptance: String,
    pub reached_acceptance: Option<String>,
    pub requires_user_action: bool,
    pub request_key: Option<String>,
    pub decision_key: Option<String>,
    pub action_signature: Option<String>,
    pub failure_code: Option<String>,
    pub failure_signature: Option<String>,
    pub recovery_owner: Option<String>,
    pub remediation_id: Option<String>,
    pub resume_cursor: Option<String>,
    pub output_started: bool,
    pub side_effect_started: bool,
    pub next_observation_at: Option<i64>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub evidence_ref: Option<String>,
    pub last_progress_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

impl ObjectiveSnapshot {
    pub fn new(
        id: impl Into<String>,
        kind: ObjectiveKind,
        domain: RecoveryDomain,
        requested_acceptance: impl Into<String>,
    ) -> Self {
        let now = Utc::now().timestamp_millis();
        Self {
            id: id.into(),
            revision: 1,
            kind,
            session_id: None,
            root_turn_id: None,
            task_id: None,
            delivery_run_id: None,
            status: ObjectiveStatus::Active,
            decision_type: DecisionType::Continue,
            domain,
            requested_acceptance: requested_acceptance.into(),
            reached_acceptance: None,
            requires_user_action: false,
            request_key: None,
            decision_key: None,
            action_signature: None,
            failure_code: None,
            failure_signature: None,
            recovery_owner: Some("objective-supervisor".into()),
            remediation_id: None,
            resume_cursor: None,
            output_started: false,
            side_effect_started: false,
            next_observation_at: None,
            lease_owner: None,
            lease_expires_at: None,
            evidence_ref: None,
            last_progress_at: Some(now),
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }

    pub fn as_decision(&self) -> DecisionEnvelope {
        DecisionEnvelope {
            objective_id: self.id.clone(),
            revision: self.revision + 1,
            domain: self.domain,
            decision_type: self.decision_type,
            status: self.status,
            failure_code: self.failure_code.clone(),
            failure_signature: self.failure_signature.clone(),
            recovery_owner: self.recovery_owner.clone(),
            remediation_id: self.remediation_id.clone(),
            next_observation_at: self.next_observation_at,
            next_action_authorized: self.status.is_system_owned(),
            requires_user_action: self.requires_user_action,
            request_key: self.request_key.clone(),
            decision_key: self.decision_key.clone(),
            action_signature: self.action_signature.clone(),
            output_started: self.output_started,
            side_effect_started: self.side_effect_started,
            resume_cursor: self.resume_cursor.clone(),
            reached_acceptance: self.reached_acceptance.clone(),
            evidence: None,
            cancellation_provenance: None,
        }
    }

    fn from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            revision: row.try_get("revision")?,
            kind: ObjectiveKind::parse(row.try_get::<String, _>("kind")?.as_str())?,
            session_id: row.try_get("session_id")?,
            root_turn_id: row.try_get("root_turn_id")?,
            task_id: row.try_get("task_id")?,
            delivery_run_id: row.try_get("delivery_run_id")?,
            status: ObjectiveStatus::parse(row.try_get::<String, _>("status")?.as_str())?,
            decision_type: DecisionType::parse(
                row.try_get::<String, _>("decision_type")?.as_str(),
            )?,
            domain: RecoveryDomain::parse(row.try_get::<String, _>("domain")?.as_str())?,
            requested_acceptance: row.try_get("requested_acceptance")?,
            reached_acceptance: row.try_get("reached_acceptance")?,
            requires_user_action: row.try_get::<i64, _>("requires_user_action")? != 0,
            request_key: row.try_get("request_key")?,
            decision_key: row.try_get("decision_key")?,
            action_signature: row.try_get("action_signature")?,
            failure_code: row.try_get("failure_code")?,
            failure_signature: row.try_get("failure_signature")?,
            recovery_owner: row.try_get("recovery_owner")?,
            remediation_id: row.try_get("remediation_id")?,
            resume_cursor: row.try_get("resume_cursor")?,
            output_started: row.try_get::<i64, _>("output_started")? != 0,
            side_effect_started: row.try_get::<i64, _>("side_effect_started")? != 0,
            next_observation_at: row.try_get("next_observation_at")?,
            lease_owner: row.try_get("lease_owner")?,
            lease_expires_at: row.try_get("lease_expires_at")?,
            evidence_ref: row.try_get("evidence_ref")?,
            last_progress_at: row.try_get("last_progress_at")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            completed_at: row.try_get("completed_at")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CreateObjective {
    pub id: String,
    pub kind: ObjectiveKind,
    pub session_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub domain: RecoveryDomain,
    pub requested_acceptance: String,
    pub created_surface: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveEvidence {
    pub id: String,
    pub kind: EvidenceKind,
    pub scope: String,
    pub digest: String,
    pub evidence_ref: String,
    pub observed_at: i64,
    pub reached_acceptance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEnvelope {
    pub objective_id: String,
    pub revision: i64,
    pub domain: RecoveryDomain,
    pub decision_type: DecisionType,
    pub status: ObjectiveStatus,
    pub failure_code: Option<String>,
    pub failure_signature: Option<String>,
    pub recovery_owner: Option<String>,
    pub remediation_id: Option<String>,
    pub next_observation_at: Option<i64>,
    pub next_action_authorized: bool,
    pub requires_user_action: bool,
    pub request_key: Option<String>,
    pub decision_key: Option<String>,
    pub action_signature: Option<String>,
    pub output_started: bool,
    pub side_effect_started: bool,
    pub resume_cursor: Option<String>,
    pub reached_acceptance: Option<String>,
    pub evidence: Option<ObjectiveEvidence>,
    pub cancellation_provenance: Option<String>,
}

impl DecisionEnvelope {
    fn validate(&self, objective: &ObjectiveSnapshot) -> anyhow::Result<()> {
        if self.objective_id != objective.id || self.revision != objective.revision + 1 {
            bail!("objective decision revision/identity mismatch");
        }
        let user_state = matches!(
            self.status,
            ObjectiveStatus::WaitingCoreInput
                | ObjectiveStatus::WaitingAuthorization
                | ObjectiveStatus::WaitingBusinessDecision
        );
        let user_decision = matches!(
            self.decision_type,
            DecisionType::CoreInputRequired
                | DecisionType::AuthorizationRequired
                | DecisionType::NeedsBusinessDecision
        );
        if self.requires_user_action != (user_state && user_decision) {
            bail!("requires_user_action does not match typed attention state");
        }
        if self.status == ObjectiveStatus::WaitingSystem
            && (self
                .recovery_owner
                .as_deref()
                .unwrap_or_default()
                .is_empty()
                || self
                    .remediation_id
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
                || self.next_observation_at.is_none())
        {
            bail!("system wait requires owner, remediation and next observation");
        }
        if matches!(
            self.decision_type,
            DecisionType::CoreInputRequired | DecisionType::AuthorizationRequired
        ) && self.request_key.as_deref().unwrap_or_default().is_empty()
        {
            bail!("core input/authorization requires a stable request_key");
        }
        if self.decision_type == DecisionType::AuthorizationRequired
            && self
                .action_signature
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        {
            bail!("authorization requires an action signature");
        }
        if self.decision_type == DecisionType::NeedsBusinessDecision
            && self.decision_key.as_deref().unwrap_or_default().is_empty()
        {
            bail!("business decision requires a stable decision_key");
        }
        if self.decision_type == DecisionType::Complete
            && (self.status != ObjectiveStatus::Completed || self.evidence.is_none())
        {
            bail!("completion requires typed evidence from CompletionArbiter");
        }
        if self.decision_type == DecisionType::Cancelled
            && !matches!(
                self.cancellation_provenance.as_deref(),
                Some("explicit_cancel" | "explicit_deny")
            )
        {
            bail!("cancellation requires explicit user provenance");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum RouteSignal {
    TechnicalFailure {
        domain: RecoveryDomain,
        failure_code: String,
        failure_signature: String,
        next_observation_at: i64,
        resume_cursor: Option<String>,
    },
    CapabilityRestored {
        domain: RecoveryDomain,
        reason: String,
        next_observation_at: i64,
        resume_cursor: Option<String>,
    },
    CoreInputRequired {
        domain: RecoveryDomain,
        request_key: String,
        missing_inputs: Vec<String>,
        attempted_routes: Vec<String>,
        resume_cursor: Option<String>,
    },
    AuthorizationRequired {
        domain: RecoveryDomain,
        request_key: String,
        action_signature: String,
        resume_cursor: Option<String>,
    },
    BusinessDecisionRequired {
        domain: RecoveryDomain,
        decision_key: String,
        resume_cursor: Option<String>,
    },
    Cancelled {
        domain: RecoveryDomain,
        provenance: String,
    },
}

pub struct DecisionRouter;

impl DecisionRouter {
    pub fn route(
        objective: &ObjectiveSnapshot,
        signal: RouteSignal,
    ) -> anyhow::Result<DecisionEnvelope> {
        let mut decision = objective.as_decision();
        decision.failure_code = None;
        decision.failure_signature = None;
        decision.request_key = None;
        decision.decision_key = None;
        decision.action_signature = None;
        decision.evidence = None;
        decision.cancellation_provenance = None;
        match signal {
            RouteSignal::TechnicalFailure {
                domain,
                failure_code,
                failure_signature,
                next_observation_at,
                resume_cursor,
            } => {
                decision.domain = domain;
                decision.decision_type = match failure_code.as_str() {
                    "platform_incident" | "permission_timeout" | "permission_channel_closed" => {
                        DecisionType::PlatformIncident
                    }
                    "failed_internal" | "panic" => DecisionType::FailedInternal,
                    _ => DecisionType::Waiting,
                };
                decision.status = ObjectiveStatus::WaitingSystem;
                decision.failure_code = Some(failure_code);
                decision.failure_signature = Some(failure_signature);
                decision.recovery_owner = Some(format!("objective-supervisor:{}", domain.as_str()));
                decision.remediation_id = Some(Uuid::new_v4().to_string());
                decision.next_observation_at = Some(next_observation_at);
                decision.next_action_authorized = true;
                decision.requires_user_action = false;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::CapabilityRestored {
                domain,
                reason,
                next_observation_at,
                resume_cursor,
            } => {
                decision.domain = domain;
                decision.decision_type = DecisionType::ApplyRecommended;
                decision.status = ObjectiveStatus::WaitingSystem;
                decision.failure_code = Some(reason.clone());
                decision.failure_signature = Some(reason);
                decision.recovery_owner = Some(format!("objective-supervisor:{}", domain.as_str()));
                decision.remediation_id = Some(Uuid::new_v4().to_string());
                decision.next_observation_at = Some(next_observation_at);
                decision.next_action_authorized = true;
                decision.requires_user_action = false;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::CoreInputRequired {
                domain,
                request_key,
                missing_inputs,
                attempted_routes,
                resume_cursor,
            } => {
                if missing_inputs.is_empty() || attempted_routes.is_empty() {
                    bail!("core input requires missing_inputs and attempted_routes");
                }
                decision.domain = domain;
                decision.decision_type = DecisionType::CoreInputRequired;
                decision.status = ObjectiveStatus::WaitingCoreInput;
                decision.request_key = Some(request_key);
                decision.requires_user_action = true;
                decision.next_action_authorized = false;
                decision.recovery_owner = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::AuthorizationRequired {
                domain,
                request_key,
                action_signature,
                resume_cursor,
            } => {
                decision.domain = domain;
                decision.decision_type = DecisionType::AuthorizationRequired;
                decision.status = ObjectiveStatus::WaitingAuthorization;
                decision.request_key = Some(request_key);
                decision.action_signature = Some(action_signature);
                decision.requires_user_action = true;
                decision.next_action_authorized = false;
                decision.recovery_owner = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::BusinessDecisionRequired {
                domain,
                decision_key,
                resume_cursor,
            } => {
                decision.domain = domain;
                decision.decision_type = DecisionType::NeedsBusinessDecision;
                decision.status = ObjectiveStatus::WaitingBusinessDecision;
                decision.decision_key = Some(decision_key);
                decision.requires_user_action = true;
                decision.next_action_authorized = false;
                decision.recovery_owner = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::Cancelled { domain, provenance } => {
                decision.domain = domain;
                decision.decision_type = DecisionType::Cancelled;
                decision.status = ObjectiveStatus::Cancelled;
                decision.requires_user_action = false;
                decision.next_action_authorized = false;
                decision.recovery_owner = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.cancellation_provenance = Some(provenance);
            }
        }
        decision.validate(objective)?;
        Ok(decision)
    }
}

pub struct CompletionArbiter;

impl CompletionArbiter {
    pub fn decide(
        objective: &ObjectiveSnapshot,
        evidence: &[ObjectiveEvidence],
    ) -> anyhow::Result<DecisionEnvelope> {
        let has = |kind| evidence.iter().any(|item| item.kind == kind);
        let satisfied = match objective.kind {
            ObjectiveKind::Informational => {
                has(EvidenceKind::InformationalAnswer) || has(EvidenceKind::CurrentStateAcceptance)
            }
            ObjectiveKind::LocalMutation => {
                has(EvidenceKind::CurrentStateAcceptance)
                    || (has(EvidenceKind::ChangeSet) && has(EvidenceKind::PostChangeValidation))
            }
            ObjectiveKind::Delivery => has(EvidenceKind::DeliveryReceipt),
            ObjectiveKind::Live => {
                has(EvidenceKind::DeliveryReceipt) && has(EvidenceKind::LiveVerification)
            }
            ObjectiveKind::LegacyOrphan => false,
        };
        if !satisfied {
            bail!("completion evidence does not satisfy objective kind");
        }
        let terminal_evidence = evidence
            .last()
            .cloned()
            .ok_or_else(|| anyhow!("completion evidence is empty"))?;
        let mut decision = objective.as_decision();
        decision.decision_type = DecisionType::Complete;
        decision.status = ObjectiveStatus::Completed;
        decision.reached_acceptance = Some(terminal_evidence.reached_acceptance.clone());
        decision.evidence = Some(terminal_evidence);
        decision.recovery_owner = None;
        decision.remediation_id = None;
        decision.next_observation_at = None;
        decision.next_action_authorized = false;
        decision.requires_user_action = false;
        decision.validate(objective)?;
        Ok(decision)
    }
}

/// Convert a transport-level terminal into one Objective decision. A model
/// reply, a closed stream, or an exhausted run budget never decides business
/// completion by itself; the objective kind and durable evidence do.
pub fn decision_for_run_outcome(
    objective: &ObjectiveSnapshot,
    outcome: &codefactory_agent_loop::run::RunOutcome,
) -> anyhow::Result<DecisionEnvelope> {
    decision_for_run_outcome_with_reason(objective, outcome, None)
}

pub fn decision_for_run_outcome_with_reason(
    objective: &ObjectiveSnapshot,
    outcome: &codefactory_agent_loop::run::RunOutcome,
    terminal_reason: Option<&str>,
) -> anyhow::Result<DecisionEnvelope> {
    use codefactory_agent_loop::run::StopReason;

    if outcome.stop_reason == StopReason::Cancelled
        || (outcome.stop_reason == StopReason::Blocked
            && terminal_reason == Some("permission_denied_by_user"))
    {
        return DecisionRouter::route(
            objective,
            RouteSignal::Cancelled {
                domain: RecoveryDomain::Chat,
                provenance: if terminal_reason == Some("permission_denied_by_user") {
                    "explicit_deny".into()
                } else {
                    "explicit_cancel".into()
                },
            },
        );
    }

    if outcome.stop_reason == StopReason::Finished {
        let now = Utc::now().timestamp_millis();
        let scope = objective
            .root_turn_id
            .as_deref()
            .or(objective.task_id.as_deref())
            .unwrap_or(&objective.id)
            .to_string();
        let evidence_ref = format!("objective-run:{}:{}", objective.id, objective.revision + 1);
        let digest = |material: &str| format!("sha256:{:x}", Sha256::digest(material.as_bytes()));
        let mut evidence = Vec::new();
        let completion = &outcome.completion_evidence;

        match objective.kind {
            ObjectiveKind::Informational if !outcome.final_text.trim().is_empty() => {
                evidence.push(ObjectiveEvidence {
                    id: Uuid::new_v4().to_string(),
                    kind: EvidenceKind::InformationalAnswer,
                    scope: scope.clone(),
                    digest: digest(&outcome.final_text),
                    evidence_ref: evidence_ref.clone(),
                    observed_at: now,
                    reached_acceptance: objective.requested_acceptance.clone(),
                });
            }
            ObjectiveKind::LocalMutation if completion.completed => {
                let mutation = completion
                    .last_source_mutation_sequence
                    .or(completion.last_mutation_sequence);
                let validation = completion
                    .last_successful_project_test_sequence
                    .or(completion.last_successful_verification_sequence);
                if let Some(sequence) = mutation {
                    evidence.push(ObjectiveEvidence {
                        id: Uuid::new_v4().to_string(),
                        kind: EvidenceKind::ChangeSet,
                        scope: scope.clone(),
                        digest: digest(&format!("mutation:{sequence}")),
                        evidence_ref: evidence_ref.clone(),
                        observed_at: now,
                        reached_acceptance: objective.requested_acceptance.clone(),
                    });
                }
                if let Some(sequence) = validation {
                    evidence.push(ObjectiveEvidence {
                        id: Uuid::new_v4().to_string(),
                        kind: EvidenceKind::PostChangeValidation,
                        scope: scope.clone(),
                        digest: digest(&format!("validation:{sequence}")),
                        evidence_ref: evidence_ref.clone(),
                        observed_at: now,
                        reached_acceptance: objective.requested_acceptance.clone(),
                    });
                }
            }
            ObjectiveKind::Delivery | ObjectiveKind::Live
                if completion.delivery_completion_satisfied =>
            {
                evidence.push(ObjectiveEvidence {
                    id: Uuid::new_v4().to_string(),
                    kind: EvidenceKind::DeliveryReceipt,
                    scope: scope.clone(),
                    digest: digest(&format!(
                        "delivery:{:?}:{:?}",
                        completion.delivery_requested_ceiling, completion.delivery_reached_ceiling
                    )),
                    evidence_ref: evidence_ref.clone(),
                    observed_at: now,
                    reached_acceptance: objective.requested_acceptance.clone(),
                });
                if objective.kind == ObjectiveKind::Live
                    && !completion.required_observable_states.is_empty()
                    && completion
                        .required_observable_states
                        .iter()
                        .all(|required| completion.observed_observable_states.contains(required))
                {
                    evidence.push(ObjectiveEvidence {
                        id: Uuid::new_v4().to_string(),
                        kind: EvidenceKind::LiveVerification,
                        scope: scope.clone(),
                        digest: digest(&completion.observed_observable_states.join("\n")),
                        evidence_ref: evidence_ref.clone(),
                        observed_at: now,
                        reached_acceptance: objective.requested_acceptance.clone(),
                    });
                }
            }
            _ => {}
        }

        if let Ok(decision) = CompletionArbiter::decide(objective, &evidence) {
            return Ok(decision);
        }
    }

    let (failure_code, domain) = match outcome.stop_reason {
        StopReason::PlatformIncident => ("platform_incident", RecoveryDomain::Tool),
        StopReason::FailedInternal => ("failed_internal", RecoveryDomain::Chat),
        StopReason::BudgetExhausted => ("run_budget_exhausted", RecoveryDomain::Chat),
        StopReason::IterationCeiling => ("iteration_ceiling", RecoveryDomain::Chat),
        StopReason::Incomplete => ("objective_incomplete", RecoveryDomain::Chat),
        StopReason::Blocked => ("run_blocked", RecoveryDomain::Chat),
        StopReason::Finished => ("completion_evidence_incomplete", RecoveryDomain::Chat),
        StopReason::Cancelled => unreachable!("explicit cancellation handled above"),
    };
    DecisionRouter::route(
        objective,
        RouteSignal::TechnicalFailure {
            domain,
            failure_code: failure_code.into(),
            failure_signature: format!(
                "{}:{:?}:{}",
                objective.id,
                outcome.stop_reason,
                terminal_reason.unwrap_or("none")
            ),
            next_observation_at: Utc::now().timestamp_millis() + 5_000,
            resume_cursor: objective.root_turn_id.clone().or(objective.task_id.clone()),
        },
    )
}

#[derive(Clone)]
pub struct ObjectiveStore {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
struct DeliveryCompletionCandidate {
    run_id: String,
    binding_id: String,
    resource_generation: i64,
    stage: String,
    claim_epoch: i64,
    repo_identity: String,
    worktree_identity: String,
    expected_head_sha: String,
    change_set_digest: String,
    requested_ceiling: String,
    reached_ceiling: String,
    canonical_pr_number: i64,
    canonical_pr_url: String,
    action_signature: String,
    reconciliation_receipt_id: String,
    already_terminal: bool,
    takeover_lease_owner: Option<String>,
}

#[derive(Debug, Clone)]
struct DeliveryTakeoverSettlement {
    run_id: String,
    claim_epoch: i64,
    lease_owner: String,
    binding_id: String,
    resource_generation: i64,
    action_signature: String,
}

fn delivery_ceiling_rank(value: &str) -> Option<i64> {
    match value {
        "local" => Some(0),
        "committed" => Some(1),
        "pushed" => Some(2),
        "pr_open" => Some(3),
        "ci_green" => Some(4),
        "merge_queued" => Some(5),
        "merged" => Some(6),
        "release_triggered" => Some(7),
        "deployment_succeeded" => Some(8),
        "live_verified" => Some(9),
        _ => None,
    }
}

fn requested_delivery_ceiling_rank(value: &str) -> Option<i64> {
    match value {
        "pr_only" => Some(3),
        "through_ci_green" => Some(4),
        "through_merge" => Some(6),
        "through_release" => Some(9),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct ClaimedRemediation {
    pub objective: ObjectiveSnapshot,
    pub remediation_id: String,
    pub domain: RecoveryDomain,
    /// Monotonic claim generation. Owner strings are process-scoped and can
    /// reclaim the same row after expiry; the epoch is what fences an older
    /// future owned by that same process.
    pub claim_epoch: i64,
    pub binding_id: Option<String>,
    pub resource_generation: Option<i64>,
}

fn objective_binding_digest(objective_id: &str, resource_kind: &str, resource_id: &str) -> String {
    let material = format!("{objective_id}\0{resource_kind}\0{resource_id}");
    format!("sha256:{:x}", Sha256::digest(material.as_bytes()))
}

/// Idempotently bind one opaque persisted resource to its Objective. The
/// digest contains only opaque ids; tool arguments, user content and secrets
/// are never copied into the identity ledger.
async fn ensure_objective_binding(
    tx: &mut Transaction<'_, Sqlite>,
    objective_id: &str,
    domain: RecoveryDomain,
    resource_kind: &str,
    resource_id: &str,
    now: i64,
) -> anyhow::Result<(String, i64)> {
    let digest = objective_binding_digest(objective_id, resource_kind, resource_id);
    sqlx::query(
        "INSERT OR IGNORE INTO objective_bindings
         (id, objective_id, domain, resource_kind, resource_id,
          resource_generation, identity_digest, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(objective_id)
    .bind(domain.as_str())
    .bind(resource_kind)
    .bind(resource_id)
    .bind(&digest)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    let (binding_id, bound_objective_id, generation, stored_digest): (String, String, i64, String) =
        sqlx::query_as(
            "SELECT id, objective_id, resource_generation, identity_digest
         FROM objective_bindings
         WHERE domain=? AND resource_kind=? AND resource_id=?
           AND resource_generation=1",
        )
        .bind(domain.as_str())
        .bind(resource_kind)
        .bind(resource_id)
        .fetch_one(&mut **tx)
        .await?;
    if bound_objective_id != objective_id || stored_digest != digest {
        bail!("objective binding identity conflict for {resource_kind}:{resource_id}");
    }
    Ok((binding_id, generation))
}

impl ObjectiveStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, create: CreateObjective) -> anyhow::Result<ObjectiveSnapshot> {
        let now = Utc::now().timestamp_millis();
        let process_instance = current_process_instance();
        sqlx::query(
            "INSERT INTO objectives
             (id, revision, kind, session_id, root_turn_id, status, decision_type,
              domain, requested_acceptance, requires_user_action, recovery_owner,
              created_surface, created_process_instance,
              last_observed_process_instance, last_progress_at, created_at, updated_at)
             VALUES (?, 1, ?, ?, ?, 'active', 'continue', ?, ?, 0,
                     'objective-supervisor', ?, ?, ?, ?, ?, ?)",
        )
        .bind(&create.id)
        .bind(create.kind.as_str())
        .bind(&create.session_id)
        .bind(&create.root_turn_id)
        .bind(create.domain.as_str())
        .bind(&create.requested_acceptance)
        .bind(&create.created_surface)
        .bind(&process_instance)
        .bind(&process_instance)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("create objective")?;
        self.get(&create.id)
            .await?
            .ok_or_else(|| anyhow!("created objective disappeared"))
    }

    /// Idempotently materialize the business objective for a persisted chat
    /// root turn and bind the legacy transport projection to it. The partial
    /// unique index is the cross-process guard; `INSERT OR IGNORE` plus a
    /// read-back makes concurrent command/setup recovery safe.
    pub async fn ensure_chat_objective(
        &self,
        session_id: &str,
        root_turn_id: &str,
        kind: ObjectiveKind,
        requested_acceptance: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        self.ensure_or_continue_chat_objective(
            session_id,
            root_turn_id,
            None,
            kind,
            requested_acceptance,
        )
        .await
    }

    /// Atomically bind a new chat root either to its one authoritative open
    /// Objective or to a newly-created Objective. A contextual continuation
    /// must never manufacture a second identity: missing legacy bindings and
    /// multiple candidates both fail closed for explicit reconciliation.
    pub async fn ensure_or_continue_chat_objective(
        &self,
        session_id: &str,
        root_turn_id: &str,
        continuation_root_turn_id: Option<&str>,
        kind: ObjectiveKind,
        requested_acceptance: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let now = Utc::now().timestamp_millis();
        let process_instance = current_process_instance();
        let mut tx = self.pool.begin().await?;
        let mut legacy_root_to_reconcile = None;

        let current_binding = sqlx::query_scalar::<_, Option<String>>(
            "SELECT objective_id FROM chat_turn_state WHERE root_turn_id=? AND session_id=?",
        )
        .bind(root_turn_id)
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow!("chat root turn missing while ensuring objective"))?
        .filter(|value| !value.is_empty());

        let mut candidates = Vec::new();
        if let Some(objective_id) = current_binding.as_ref() {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE id=?")
                .bind(objective_id)
                .fetch_one(&mut *tx)
                .await?;
            if exists != 1 {
                bail!("chat objective binding points to missing objective");
            }
            candidates.push(objective_id.clone());
        }

        if let Some(continuation_root_turn_id) = continuation_root_turn_id {
            let continuation_binding = sqlx::query_scalar::<_, Option<String>>(
                "SELECT objective_id FROM chat_turn_state
                 WHERE root_turn_id=? AND session_id=?",
            )
            .bind(continuation_root_turn_id)
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| anyhow!("contextual continuation root turn is missing"))?
            .filter(|value| !value.is_empty());
            if let Some(continuation_binding) = continuation_binding {
                let status =
                    sqlx::query_scalar::<_, String>("SELECT status FROM objectives WHERE id=?")
                        .bind(&continuation_binding)
                        .fetch_optional(&mut *tx)
                        .await?
                        .ok_or_else(|| anyhow!("contextual continuation objective is missing"))?;
                if !matches!(status.as_str(), "completed" | "cancelled" | "legacy_orphan") {
                    candidates.push(continuation_binding);
                }
            } else {
                // A pre-0006 root is still an authoritative continuation
                // anchor, but it has no safe synthesized identity. Reconcile
                // that exact root inside this transaction so both transport
                // projections receive one opaque Objective id.
                legacy_root_to_reconcile = Some(continuation_root_turn_id.to_string());
            }
        }

        candidates.sort();
        candidates.dedup();
        if candidates.len() > 1 {
            bail!("multiple open objectives match contextual chat continuation");
        }

        if let Some(objective_id) = candidates.pop() {
            if current_binding.as_deref() == Some(objective_id.as_str())
                && legacy_root_to_reconcile.is_none()
            {
                ensure_objective_binding(
                    &mut tx,
                    &objective_id,
                    RecoveryDomain::Chat,
                    "chat_root_turn",
                    root_turn_id,
                    now,
                )
                .await?;
                tx.commit().await?;
                return self
                    .get(&objective_id)
                    .await?
                    .ok_or_else(|| anyhow!("bound chat objective disappeared"));
            }
            let row = sqlx::query("SELECT * FROM objectives WHERE id=?")
                .bind(&objective_id)
                .fetch_one(&mut *tx)
                .await?;
            let objective = ObjectiveSnapshot::from_row(&row)?;
            let target_kind = if objective_kind_rank(kind) > objective_kind_rank(objective.kind) {
                kind
            } else {
                objective.kind
            };
            let target_acceptance = if target_kind == kind {
                requested_acceptance
            } else {
                objective.requested_acceptance.as_str()
            };
            let next_revision = objective.revision + 1;
            let updated = sqlx::query(
                "UPDATE objectives SET revision=?, kind=?, requested_acceptance=?,
                   status='active', decision_type='continue', domain='chat',
                   requires_user_action=0, request_key=NULL, decision_key=NULL,
                   failure_code=NULL, failure_signature=NULL,
                   recovery_owner='chat-foreground', remediation_id=NULL,
                   resume_cursor=?, next_observation_at=NULL,
                   lease_owner=NULL, lease_expires_at=NULL,
                   last_observed_process_instance=?,
                   last_progress_at=?, updated_at=?, completed_at=NULL
                 WHERE id=? AND revision=?
                   AND status NOT IN ('completed','cancelled','legacy_orphan')",
            )
            .bind(next_revision)
            .bind(target_kind.as_str())
            .bind(target_acceptance)
            .bind(root_turn_id)
            .bind(&process_instance)
            .bind(now)
            .bind(now)
            .bind(&objective_id)
            .bind(objective.revision)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                bail!("objective changed while binding contextual continuation");
            }
            sqlx::query(
                "UPDATE objective_remediations SET status='superseded',
                   lease_owner=NULL, lease_expires_at=NULL,
                   last_progress_at=?, updated_at=?
                 WHERE objective_id=?
                   AND status NOT IN ('completed','cancelled','superseded')",
            )
            .bind(now)
            .bind(now)
            .bind(&objective_id)
            .execute(&mut *tx)
            .await?;
            if current_binding.is_none() {
                let linked = sqlx::query(
                    "UPDATE chat_turn_state SET objective_id=?
                     WHERE root_turn_id=? AND session_id=? AND objective_id IS NULL",
                )
                .bind(&objective_id)
                .bind(root_turn_id)
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
                if linked.rows_affected() != 1 {
                    bail!("chat root objective binding changed concurrently");
                }
            }
            if let Some(legacy_root_turn_id) = legacy_root_to_reconcile.as_deref() {
                let linked = sqlx::query(
                    "UPDATE chat_turn_state SET objective_id=?
                     WHERE root_turn_id=? AND session_id=? AND objective_id IS NULL",
                )
                .bind(&objective_id)
                .bind(legacy_root_turn_id)
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
                if linked.rows_affected() != 1 {
                    bail!("legacy chat root objective binding changed concurrently");
                }
            }
            ensure_objective_binding(
                &mut tx,
                &objective_id,
                RecoveryDomain::Chat,
                "chat_root_turn",
                root_turn_id,
                now,
            )
            .await?;
            if let Some(legacy_root_turn_id) = legacy_root_to_reconcile.as_deref() {
                ensure_objective_binding(
                    &mut tx,
                    &objective_id,
                    RecoveryDomain::Chat,
                    "chat_root_turn",
                    legacy_root_turn_id,
                    now,
                )
                .await?;
            }
            sqlx::query(
                "INSERT INTO objective_events
                 (id, objective_id, revision, event_type, status, decision_type,
                  domain, recovery_owner, detail_json, created_at)
                 VALUES (?, ?, ?, ?, 'active', 'continue',
                         'chat', 'chat-foreground', ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&objective_id)
            .bind(next_revision)
            .bind(if legacy_root_to_reconcile.is_some() {
                "legacy_root_reconciled"
            } else {
                "contextual_root_bound"
            })
            .bind(
                serde_json::json!({
                    "root_turn_id": root_turn_id,
                    "legacy_root_turn_id": legacy_root_to_reconcile,
                })
                .to_string(),
            )
            .bind(now)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return self
                .get(&objective_id)
                .await?
                .ok_or_else(|| anyhow!("continued chat objective disappeared"));
        }

        let candidate_id = Uuid::new_v4().to_string();
        let objective_root_turn_id = legacy_root_to_reconcile.as_deref().unwrap_or(root_turn_id);
        sqlx::query(
            "INSERT OR IGNORE INTO objectives
             (id, revision, kind, session_id, root_turn_id, status, decision_type,
              domain, requested_acceptance, requires_user_action, recovery_owner,
              created_surface, created_process_instance,
              last_observed_process_instance, last_progress_at, created_at, updated_at)
             VALUES (?, 1, ?, ?, ?, 'active', 'continue', 'chat', ?, 0,
                     'objective-supervisor:chat', 'project_chat', ?, ?, ?, ?, ?)",
        )
        .bind(&candidate_id)
        .bind(kind.as_str())
        .bind(session_id)
        .bind(objective_root_turn_id)
        .bind(requested_acceptance)
        .bind(&process_instance)
        .bind(&process_instance)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .context("ensure chat objective")?;
        let objective_id: String = sqlx::query_scalar(
            "SELECT id FROM objectives WHERE root_turn_id=?
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(objective_root_turn_id)
        .fetch_one(&mut *tx)
        .await
        .context("read ensured chat objective")?;
        let linked = sqlx::query(
            "UPDATE chat_turn_state SET objective_id=? WHERE root_turn_id=? AND session_id=?",
        )
        .bind(&objective_id)
        .bind(root_turn_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await
        .context("bind chat objective")?;
        if linked.rows_affected() != 1 {
            bail!("chat root turn missing while binding objective");
        }
        if let Some(legacy_root_turn_id) = legacy_root_to_reconcile.as_deref() {
            let legacy_linked = sqlx::query(
                "UPDATE chat_turn_state SET objective_id=?
                 WHERE root_turn_id=? AND session_id=? AND objective_id IS NULL",
            )
            .bind(&objective_id)
            .bind(legacy_root_turn_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
            if legacy_linked.rows_affected() != 1 {
                bail!("legacy chat root turn changed while reconciling objective");
            }
            sqlx::query(
                "INSERT INTO objective_events
                 (id, objective_id, revision, event_type, status, decision_type,
                  domain, recovery_owner, detail_json, created_at)
                 VALUES (?, ?, 1, 'legacy_root_reconciled', 'active', 'continue',
                         'chat', 'objective-supervisor:chat', ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&objective_id)
            .bind(
                serde_json::json!({
                    "root_turn_id": root_turn_id,
                    "legacy_root_turn_id": legacy_root_turn_id,
                })
                .to_string(),
            )
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        ensure_objective_binding(
            &mut tx,
            &objective_id,
            RecoveryDomain::Chat,
            "chat_root_turn",
            root_turn_id,
            now,
        )
        .await?;
        if let Some(legacy_root_turn_id) = legacy_root_to_reconcile.as_deref() {
            ensure_objective_binding(
                &mut tx,
                &objective_id,
                RecoveryDomain::Chat,
                "chat_root_turn",
                legacy_root_turn_id,
                now,
            )
            .await?;
        }
        tx.commit().await?;
        self.get(&objective_id)
            .await?
            .ok_or_else(|| anyhow!("ensured chat objective disappeared"))
    }

    pub async fn ensure_task_objective(
        &self,
        session_id: &str,
        task_id: &str,
        requested_acceptance: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let now = Utc::now().timestamp_millis();
        let process_instance = current_process_instance();
        let candidate_id = Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT OR IGNORE INTO objectives
             (id, revision, kind, session_id, task_id, status, decision_type,
              domain, requested_acceptance, requires_user_action, recovery_owner,
              created_surface, created_process_instance,
              last_observed_process_instance, last_progress_at, created_at, updated_at)
             VALUES (?, 1, 'local_mutation', ?, ?, 'active', 'continue', 'task', ?, 0,
                     'objective-supervisor:task', 'task_scheduler', ?, ?, ?, ?, ?)",
        )
        .bind(&candidate_id)
        .bind(session_id)
        .bind(task_id)
        .bind(requested_acceptance)
        .bind(&process_instance)
        .bind(&process_instance)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .context("ensure task objective")?;
        let objective_id: String = sqlx::query_scalar(
            "SELECT id FROM objectives WHERE task_id=? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        let linked = sqlx::query("UPDATE task_runs SET objective_id=? WHERE id=? AND session_id=?")
            .bind(&objective_id)
            .bind(task_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        if linked.rows_affected() != 1 {
            bail!("task row missing while binding objective");
        }
        ensure_objective_binding(
            &mut tx,
            &objective_id,
            RecoveryDomain::Task,
            "task_run",
            task_id,
            now,
        )
        .await?;
        tx.commit().await?;
        self.get(&objective_id)
            .await?
            .ok_or_else(|| anyhow!("ensured task objective disappeared"))
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<ObjectiveSnapshot>> {
        let row = sqlx::query("SELECT * FROM objectives WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| ObjectiveSnapshot::from_row(&row)).transpose()
    }

    pub async fn get_by_root_turn(
        &self,
        root_turn_id: &str,
    ) -> anyhow::Result<Option<ObjectiveSnapshot>> {
        let row = sqlx::query(
            "SELECT * FROM objectives WHERE root_turn_id=?
             ORDER BY created_at DESC, revision DESC LIMIT 1",
        )
        .bind(root_turn_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| ObjectiveSnapshot::from_row(&row)).transpose()
    }

    /// Lease due recovery work with compare-and-swap semantics. A stale
    /// process can observe candidates but cannot claim a row after another
    /// supervisor has advanced its lease or status.
    pub async fn claim_due_remediations(
        &self,
        owner: &str,
        limit: i64,
        lease_ms: i64,
    ) -> anyhow::Result<Vec<ClaimedRemediation>> {
        let now = Utc::now().timestamp_millis();
        let rows = sqlx::query(
            "SELECT remediation.id AS remediation_id,
                    remediation.objective_id, remediation.domain
             FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE objective.status='waiting_system'
               AND objective.remediation_id=remediation.id
               AND remediation.status IN ('queued','waiting','claimed')
               AND remediation.next_observation_at<=?
               AND (remediation.lease_expires_at IS NULL OR remediation.lease_expires_at<=?)
             ORDER BY remediation.next_observation_at, remediation.created_at
             LIMIT ?",
        )
        .bind(now)
        .bind(now)
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await?;
        let mut claimed = Vec::new();
        for row in rows {
            let remediation_id: String = row.try_get("remediation_id")?;
            let objective_id: String = row.try_get("objective_id")?;
            let domain = RecoveryDomain::parse(row.try_get::<String, _>("domain")?.as_str())?;
            let lease_expires_at = now + lease_ms.max(1);
            let mut tx = self.pool.begin().await?;
            let updated = sqlx::query(
                "UPDATE objective_remediations
                 SET status='claimed', attempt_index=attempt_index+1,
                     lease_owner=?, lease_expires_at=?, updated_at=?
                 WHERE id=? AND objective_id=?
                   AND status IN ('queued','waiting','claimed')
                   AND next_observation_at<=?
                   AND (lease_expires_at IS NULL OR lease_expires_at<=?)",
            )
            .bind(owner)
            .bind(lease_expires_at)
            .bind(now)
            .bind(&remediation_id)
            .bind(&objective_id)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            let objective_updated = sqlx::query(
                "UPDATE objectives SET lease_owner=?, lease_expires_at=?, updated_at=?
                 WHERE id=? AND status='waiting_system' AND remediation_id=?
                   AND (lease_expires_at IS NULL OR lease_expires_at<=?)",
            )
            .bind(owner)
            .bind(lease_expires_at)
            .bind(now)
            .bind(&objective_id)
            .bind(&remediation_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            if objective_updated.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            let (claim_epoch, binding_id): (i64, Option<String>) = sqlx::query_as(
                "SELECT attempt_index, binding_id FROM objective_remediations
                 WHERE id=? AND objective_id=? AND status='claimed'
                   AND lease_owner=?",
            )
            .bind(&remediation_id)
            .bind(&objective_id)
            .bind(owner)
            .fetch_one(&mut *tx)
            .await?;
            let resource_generation = if let Some(binding_id) = binding_id.as_deref() {
                Some(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT resource_generation FROM objective_bindings
                         WHERE id=? AND objective_id=?",
                    )
                    .bind(binding_id)
                    .bind(&objective_id)
                    .fetch_one(&mut *tx)
                    .await?,
                )
            } else {
                None
            };
            tx.commit().await?;
            if let Some(objective) = self.get(&objective_id).await? {
                claimed.push(ClaimedRemediation {
                    objective,
                    remediation_id,
                    domain,
                    claim_epoch,
                    binding_id,
                    resource_generation,
                });
            }
        }
        Ok(claimed)
    }

    pub async fn defer_claimed_remediation(
        &self,
        objective_id: &str,
        remediation_id: &str,
        owner: &str,
        claim_epoch: i64,
        delay_ms: i64,
    ) -> anyhow::Result<()> {
        let now = Utc::now().timestamp_millis();
        let next = now + delay_ms.max(1_000);
        let mut tx = self.pool.begin().await?;
        let remediation = sqlx::query(
            "UPDATE objective_remediations
             SET status='waiting',
                 next_observation_at=?, lease_owner=NULL, lease_expires_at=NULL,
                 updated_at=?
             WHERE id=? AND objective_id=? AND status='claimed' AND lease_owner=?
               AND attempt_index=? AND lease_expires_at>?",
        )
        .bind(next)
        .bind(now)
        .bind(remediation_id)
        .bind(objective_id)
        .bind(owner)
        .bind(claim_epoch)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if remediation.rows_affected() != 1 {
            bail!("remediation claim ownership changed before defer");
        }
        let objective = sqlx::query(
            "UPDATE objectives SET next_observation_at=?, lease_owner=NULL,
               lease_expires_at=NULL, updated_at=?
             WHERE id=? AND status='waiting_system' AND remediation_id=? AND lease_owner=?
               AND lease_expires_at>?",
        )
        .bind(next)
        .bind(now)
        .bind(objective_id)
        .bind(remediation_id)
        .bind(owner)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if objective.rows_affected() != 1 {
            bail!("objective claim ownership changed before defer");
        }
        tx.commit().await?;
        Ok(())
    }

    /// Extend both halves of a claimed remediation lease atomically. Returning
    /// `false` means ownership changed (or the remediation was superseded), so
    /// the caller must stop rather than execute another side effect.
    pub async fn renew_claimed_remediation(
        &self,
        objective_id: &str,
        remediation_id: &str,
        owner: &str,
        claim_epoch: i64,
        lease_ms: i64,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp_millis();
        let lease_expires_at = now + lease_ms.max(1);
        let mut tx = self.pool.begin().await?;
        let remediation = sqlx::query(
            "UPDATE objective_remediations
             SET lease_expires_at=?, last_progress_at=?, updated_at=?
             WHERE id=? AND objective_id=? AND status='claimed' AND lease_owner=?
               AND attempt_index=? AND lease_expires_at>?",
        )
        .bind(lease_expires_at)
        .bind(now)
        .bind(now)
        .bind(remediation_id)
        .bind(objective_id)
        .bind(owner)
        .bind(claim_epoch)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if remediation.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        let objective = sqlx::query(
            "UPDATE objectives
             SET lease_expires_at=?, last_progress_at=?, updated_at=?
             WHERE id=? AND status='waiting_system' AND remediation_id=? AND lease_owner=?
               AND lease_expires_at>?",
        )
        .bind(lease_expires_at)
        .bind(now)
        .bind(now)
        .bind(objective_id)
        .bind(remediation_id)
        .bind(owner)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if objective.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Observe whether an adapter still owns the exact durable claim it was
    /// launched for. This is intentionally stronger than comparing only the
    /// owner string: a same-process reclaim increments `attempt_index`, and a
    /// resource rebind invalidates the generation carried by the old permit.
    pub async fn claim_is_current(
        &self,
        permit: &codefactory_agent_loop::tool::MutationPermit,
    ) -> anyhow::Result<bool> {
        if permit.binding_id.is_some() != permit.resource_generation.is_some() {
            bail!("mutation permit binding and resource generation must be paired");
        }
        let now = Utc::now().timestamp_millis();
        let claimed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM objective_remediations remediation
             JOIN objectives objective
               ON objective.id=remediation.objective_id
              AND objective.remediation_id=remediation.id
             WHERE remediation.id=? AND remediation.objective_id=?
               AND remediation.status='claimed' AND remediation.lease_owner=?
               AND remediation.attempt_index=? AND remediation.lease_expires_at>?
               AND COALESCE(remediation.binding_id, '')=COALESCE(?, '')
               AND objective.status='waiting_system' AND objective.lease_owner=?
               AND objective.lease_expires_at>?",
        )
        .bind(&permit.remediation_id)
        .bind(&permit.objective_id)
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .bind(&permit.binding_id)
        .bind(&permit.owner)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        if claimed != 1 {
            return Ok(false);
        }
        if let Some(binding_id) = permit.binding_id.as_deref() {
            let generation = sqlx::query_scalar::<_, i64>(
                "SELECT resource_generation FROM objective_bindings
                 WHERE id=? AND objective_id=?",
            )
            .bind(binding_id)
            .bind(&permit.objective_id)
            .fetch_optional(&self.pool)
            .await?;
            if generation != permit.resource_generation {
                bail!("mutation permit binding generation changed while claim was live");
            }
        }
        Ok(true)
    }

    /// On process start, convert objectives that were active in an older
    /// process into durable system-owned recovery. Transport interruption is
    /// not completion and never becomes a user handoff.
    pub async fn reconcile_stale_active_objectives(
        &self,
        current_process_instance: &str,
    ) -> anyhow::Result<usize> {
        let rows = sqlx::query(
            "SELECT * FROM objectives
             WHERE status='active'
               AND COALESCE(last_observed_process_instance,
                            created_process_instance, '') <> ?
             ORDER BY created_at",
        )
        .bind(current_process_instance)
        .fetch_all(&self.pool)
        .await?;
        let mut reconciled = 0;
        for row in rows {
            let prior_process = row
                .try_get::<Option<String>, _>("last_observed_process_instance")?
                .or(row.try_get::<Option<String>, _>("created_process_instance")?)
                .unwrap_or_else(|| "legacy-process".into());
            let objective = ObjectiveSnapshot::from_row(&row)?;
            let decision = DecisionRouter::route(
                &objective,
                RouteSignal::TechnicalFailure {
                    domain: objective.domain,
                    failure_code: "process_restarted".into(),
                    failure_signature: format!(
                        "{}:{}:{}",
                        objective.id, prior_process, current_process_instance
                    ),
                    next_observation_at: Utc::now().timestamp_millis(),
                    resume_cursor: objective
                        .root_turn_id
                        .clone()
                        .or(objective.task_id.clone())
                        .or(objective.delivery_run_id.clone()),
                },
            )?;
            match self.apply_decision(objective.revision, decision).await {
                Ok(_) => reconciled += 1,
                Err(error) if error.to_string().contains("revision") => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(reconciled)
    }

    /// Convert satisfied authorization requests into immediate system-owned
    /// recovery. The original objective/root turn is preserved; no user
    /// message is replayed or synthesized.
    pub async fn resume_waiting_authorizations(
        &self,
        domain: RecoveryDomain,
        request_key_prefix: &str,
    ) -> anyhow::Result<usize> {
        let rows = sqlx::query(
            "SELECT * FROM objectives
             WHERE status='waiting_authorization' AND domain=?
               AND request_key LIKE ?
             ORDER BY created_at",
        )
        .bind(domain.as_str())
        .bind(format!("{request_key_prefix}%"))
        .fetch_all(&self.pool)
        .await?;
        let mut resumed = 0;
        for row in rows {
            let objective = ObjectiveSnapshot::from_row(&row)?;
            let decision = DecisionRouter::route(
                &objective,
                RouteSignal::CapabilityRestored {
                    domain,
                    reason: "authorization_restored".into(),
                    next_observation_at: Utc::now().timestamp_millis(),
                    resume_cursor: objective.resume_cursor.clone(),
                },
            )?;
            match self.apply_decision(objective.revision, decision).await {
                Ok(_) => resumed += 1,
                Err(error) if error.to_string().contains("revision") => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(resumed)
    }

    pub async fn apply_decision(
        &self,
        expected_revision: i64,
        decision: DecisionEnvelope,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        self.apply_decision_inner(expected_revision, decision, None, None)
            .await
    }

    /// Settle a recovery attempt only while its exact owner+epoch lease is
    /// still live. A same-process reclaim deliberately keeps the owner string,
    /// so omitting the epoch here would let the stale future supersede the new
    /// remediation after takeover.
    pub async fn apply_claimed_decision(
        &self,
        expected_revision: i64,
        decision: DecisionEnvelope,
        permit: &codefactory_agent_loop::tool::MutationPermit,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        self.apply_decision_inner(expected_revision, decision, Some(permit), None)
            .await
    }

    /// Combine newly-observed evidence with the typed durable evidence already
    /// accepted for this Objective. This is what lets a Live Objective resume
    /// after DeliveryRun settlement and later complete from a real live
    /// observation without replaying delivery or fabricating that observation
    /// from `reached_ceiling`.
    pub async fn completion_decision_with_persisted_evidence(
        &self,
        objective: &ObjectiveSnapshot,
        mut newly_observed: Vec<ObjectiveEvidence>,
    ) -> anyhow::Result<DecisionEnvelope> {
        let current = self
            .get(&objective.id)
            .await?
            .ok_or_else(|| anyhow!("objective not found"))?;
        if current.revision != objective.revision || current.status != objective.status {
            bail!("objective changed before persisted completion evidence was combined");
        }
        let rows = sqlx::query(
            "SELECT id, kind, scope, digest, evidence_ref, observed_at
             FROM objective_evidence WHERE objective_id=? ORDER BY observed_at, created_at, id",
        )
        .bind(&objective.id)
        .fetch_all(&self.pool)
        .await?;
        let mut evidence = Vec::with_capacity(rows.len() + newly_observed.len());
        for row in rows {
            evidence.push(ObjectiveEvidence {
                id: row.try_get("id")?,
                kind: EvidenceKind::parse(row.try_get::<String, _>("kind")?.as_str())?,
                scope: row.try_get("scope")?,
                digest: row.try_get("digest")?,
                evidence_ref: row.try_get("evidence_ref")?,
                observed_at: row.try_get("observed_at")?,
                reached_acceptance: objective.requested_acceptance.clone(),
            });
        }
        evidence.append(&mut newly_observed);
        CompletionArbiter::decide(objective, &evidence)
    }

    /// Settle a DeliveryRun whose external effects were already positively
    /// reconciled by takeover. This function performs no provider or git I/O;
    /// it only consumes the exact pointed run/epoch ledger and advances the
    /// provider-replay rows, generic receipt, DeliveryRun and Objective in one
    /// SQLite transaction.
    pub async fn settle_reconciled_delivery_after_takeover(
        &self,
        run_id: &str,
        claim_epoch: i64,
        lease_owner: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        if claim_epoch <= 0 {
            bail!("delivery takeover claim epoch must be positive");
        }
        if lease_owner.trim().is_empty() {
            bail!("delivery takeover lease owner must be explicit");
        }
        let objective_id = sqlx::query_scalar::<_, String>(
            "SELECT objective.id
             FROM objectives objective
             JOIN delivery_runs run
               ON run.id=objective.delivery_run_id AND run.objective_id=objective.id
             WHERE run.id=?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow!("delivery takeover has no exact pointed Objective"))?;
        let current = self
            .get(&objective_id)
            .await?
            .ok_or_else(|| anyhow!("pointed delivery Objective disappeared"))?;

        if current.status.is_terminal()
            || (current.kind == ObjectiveKind::Live
                && current.status == ObjectiveStatus::WaitingSystem)
        {
            let settled: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM delivery_runs run
                 JOIN objectives objective
                   ON objective.id=run.objective_id AND objective.delivery_run_id=run.id
                 WHERE run.id=? AND run.claim_epoch=?
                   AND run.reconciled_claim_epoch=run.claim_epoch
                   AND run.status='completed'
                   AND EXISTS (
                     SELECT 1 FROM side_effect_receipts receipt
                     WHERE receipt.objective_id=objective.id
                       AND receipt.status='reconciled'
                       AND json_extract(receipt.summary_json, '$.delivery_run_id')=run.id
                   )
                   AND EXISTS (
                     SELECT 1 FROM objective_evidence evidence
                     WHERE evidence.objective_id=objective.id
                       AND evidence.kind='delivery_receipt'
                   )",
            )
            .bind(run_id)
            .bind(claim_epoch)
            .fetch_one(&self.pool)
            .await?;
            if settled == 1 {
                return Ok(current);
            }
            if current.status.is_terminal() {
                bail!("terminal Objective has no exact reconciled DeliveryRun receipt");
            }
        }

        if !matches!(current.kind, ObjectiveKind::Delivery | ObjectiveKind::Live) {
            bail!("pointed DeliveryRun cannot settle a non-delivery Objective kind");
        }
        if !current.status.is_system_owned() {
            bail!("DeliveryRun settlement requires a system-owned Objective");
        }
        let run = sqlx::query(
            "SELECT run.status, run.stage, run.claim_epoch, run.reconciled_claim_epoch,
                    run.next_action_authorized, run.autonomous_completion,
                    run.requested_ceiling, run.reached_ceiling,
                    run.repo_identity, run.worktree_identity,
                    run.expected_head_sha, run.change_set_digest,
                    run.canonical_pr_number, run.canonical_pr_url,
                    run.canonical_head_sha, run.root_turn_id, run.task_id,
                    run.lease_owner, run.lease_expires_at
             FROM delivery_runs run
             JOIN objectives objective
               ON objective.id=run.objective_id AND objective.delivery_run_id=run.id
             WHERE run.id=? AND run.objective_id=?",
        )
        .bind(run_id)
        .bind(&current.id)
        .fetch_one(&self.pool)
        .await?;
        let run_status: String = run.try_get("status")?;
        if run_status != "awaiting_completion_arbitration" {
            bail!("delivery takeover is not awaiting completion arbitration");
        }
        let stage: String = run.try_get("stage")?;
        if stage != "complete" {
            bail!("delivery takeover has not reached its complete stage");
        }
        let persisted_claim_epoch: i64 = run.try_get("claim_epoch")?;
        let reconciled_claim_epoch: i64 = run.try_get("reconciled_claim_epoch")?;
        if persisted_claim_epoch != claim_epoch || reconciled_claim_epoch != claim_epoch {
            bail!("delivery takeover claim epoch is stale or unreconciled");
        }
        let next_action_authorized: i64 = run.try_get("next_action_authorized")?;
        let autonomous_completion: i64 = run.try_get("autonomous_completion")?;
        if next_action_authorized != 1 || autonomous_completion != 1 {
            bail!("delivery takeover lacks autonomous completion authority");
        }
        let persisted_lease_owner: Option<String> = run.try_get("lease_owner")?;
        let lease_expires_at: Option<i64> = run.try_get("lease_expires_at")?;
        if persisted_lease_owner.as_deref() != Some(lease_owner)
            || lease_expires_at.is_none_or(|expires_at| expires_at <= Utc::now().timestamp_millis())
        {
            bail!("delivery takeover lease expired or changed before settlement");
        }
        let requested_ceiling: String = run.try_get("requested_ceiling")?;
        let reached_ceiling: String = run.try_get("reached_ceiling")?;
        let requested_rank = requested_delivery_ceiling_rank(&requested_ceiling)
            .ok_or_else(|| anyhow!("delivery takeover has an invalid requested ceiling"))?;
        let reached_rank = delivery_ceiling_rank(&reached_ceiling)
            .ok_or_else(|| anyhow!("delivery takeover has an invalid reached ceiling"))?;
        if reached_rank < requested_rank {
            bail!("delivery takeover has not reached its requested ceiling");
        }
        let repo_identity: String = run.try_get("repo_identity")?;
        let worktree_identity: String = run.try_get("worktree_identity")?;
        let expected_head_sha: String = run.try_get("expected_head_sha")?;
        let change_set_digest: String = run.try_get("change_set_digest")?;
        if [
            repo_identity.as_str(),
            worktree_identity.as_str(),
            expected_head_sha.as_str(),
            change_set_digest.as_str(),
        ]
        .iter()
        .any(|value| value.trim().is_empty())
        {
            bail!("delivery takeover has incomplete durable identity");
        }
        let canonical_pr_number: Option<i64> = run.try_get("canonical_pr_number")?;
        let canonical_pr_url: Option<String> = run.try_get("canonical_pr_url")?;
        let canonical_head_sha: Option<String> = run.try_get("canonical_head_sha")?;
        if canonical_pr_number.is_none()
            || canonical_pr_url
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            || canonical_head_sha.as_deref() != Some(expected_head_sha.as_str())
        {
            bail!("delivery takeover canonical PR/head identity is incomplete or inconsistent");
        }
        let unresolved_intents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_mutation_intents
             WHERE run_id=? AND status IN ('started','unknown')",
        )
        .bind(run_id)
        .fetch_one(&self.pool)
        .await?;
        if unresolved_intents != 0 {
            bail!("delivery takeover has an unresolved mutation intent");
        }
        let (resource_kind, resource_id) = match (
            run.try_get::<Option<String>, _>("root_turn_id")?,
            run.try_get::<Option<String>, _>("task_id")?,
        ) {
            (Some(root_turn_id), None) => ("chat_root_turn", root_turn_id),
            (None, Some(task_id)) => ("task_run", task_id),
            _ => bail!("delivery takeover cannot prove one authoritative resource"),
        };
        let binding = sqlx::query(
            "SELECT id, resource_generation FROM objective_bindings
             WHERE objective_id=? AND resource_kind=? AND resource_id=?
             ORDER BY resource_generation DESC, updated_at DESC, id DESC
             LIMIT 1",
        )
        .bind(&current.id)
        .bind(resource_kind)
        .bind(&resource_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow!("delivery takeover has no current binding"))?;
        let binding_id: String = binding.try_get("id")?;
        let resource_generation: i64 = binding.try_get("resource_generation")?;
        let action_signatures: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT action_signature FROM tool_calls
             WHERE objective_id=? AND binding_id=? AND resource_generation=?
               AND tool_name='deliver_changes'
               AND NULLIF(TRIM(action_signature), '') IS NOT NULL
             ORDER BY action_signature",
        )
        .bind(&current.id)
        .bind(&binding_id)
        .bind(resource_generation)
        .fetch_all(&self.pool)
        .await?;
        if action_signatures.len() != 1 {
            bail!("delivery takeover requires one stable action signature on its current binding");
        }
        let action_signature = action_signatures[0].clone();
        let tool_call_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tool_calls
             WHERE objective_id=? AND binding_id=? AND resource_generation=?
               AND tool_name='deliver_changes' AND action_signature=?",
        )
        .bind(&current.id)
        .bind(&binding_id)
        .bind(resource_generation)
        .bind(&action_signature)
        .fetch_one(&self.pool)
        .await?;
        if tool_call_count == 0 {
            bail!("delivery takeover has no exact normalized action to settle");
        }
        let now = Utc::now().timestamp_millis();
        let scope = current
            .root_turn_id
            .clone()
            .or(current.task_id.clone())
            .unwrap_or_else(|| current.id.clone());
        let delivery_evidence = ObjectiveEvidence {
            id: format!(
                "sha256:{:x}",
                Sha256::digest(
                    format!(
                        "delivery_takeover_evidence\0{}\0{}\0{}",
                        current.id, run_id, claim_epoch
                    )
                    .as_bytes()
                )
            ),
            kind: EvidenceKind::DeliveryReceipt,
            scope,
            digest: format!(
                "sha256:{:x}",
                Sha256::digest(format!("{run_id}\0{claim_epoch}").as_bytes())
            ),
            evidence_ref: format!("delivery-run:{run_id}:epoch:{claim_epoch}"),
            observed_at: now,
            reached_acceptance: current.requested_acceptance.clone(),
        };
        let mut decision = if current.kind == ObjectiveKind::Delivery {
            CompletionArbiter::decide(&current, &[delivery_evidence.clone()])?
        } else {
            let continuation_domain = if current.task_id.is_some() {
                RecoveryDomain::Task
            } else if current.root_turn_id.is_some() {
                RecoveryDomain::Chat
            } else {
                RecoveryDomain::Delivery
            };
            DecisionRouter::route(
                &current,
                RouteSignal::TechnicalFailure {
                    domain: continuation_domain,
                    failure_code: "live_verification_pending_after_delivery".into(),
                    failure_signature: format!(
                        "sha256:{:x}",
                        Sha256::digest(
                            format!("{}\0{}\0{}", current.id, run_id, claim_epoch).as_bytes()
                        )
                    ),
                    next_observation_at: now,
                    resume_cursor: current.root_turn_id.clone().or(current.task_id.clone()),
                },
            )?
        };
        // Waiting decisions may carry accepted non-terminal evidence. It is
        // persisted now, while CompletionArbiter still requires a separate
        // LiveVerification before it can ever emit `complete`.
        decision.evidence = Some(delivery_evidence);
        self.apply_decision_inner(
            current.revision,
            decision,
            None,
            Some(DeliveryTakeoverSettlement {
                run_id: run_id.to_string(),
                claim_epoch,
                lease_owner: lease_owner.to_string(),
                binding_id,
                resource_generation,
                action_signature,
            }),
        )
        .await
    }

    async fn apply_decision_inner(
        &self,
        expected_revision: i64,
        decision: DecisionEnvelope,
        permit: Option<&codefactory_agent_loop::tool::MutationPermit>,
        delivery_takeover: Option<DeliveryTakeoverSettlement>,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let current = self
            .get(&decision.objective_id)
            .await?
            .ok_or_else(|| anyhow!("objective not found"))?;
        if current.revision != expected_revision {
            bail!(
                "objective revision conflict: expected {expected_revision}, actual {}",
                current.revision
            );
        }
        decision.validate(&current)?;
        let now = Utc::now().timestamp_millis();
        let process_instance = current_process_instance();
        let completed_at = decision.status.is_terminal().then_some(now);
        let evidence_ref = decision
            .evidence
            .as_ref()
            .map(|evidence| evidence.evidence_ref.clone());
        let mut delivery_run_to_complete: Option<DeliveryCompletionCandidate> = None;
        let mut tx = self.pool.begin().await?;
        if let Some(permit) = permit {
            if permit.objective_id != decision.objective_id {
                bail!("mutation permit objective does not match decision");
            }
            let remediation_claim = sqlx::query(
                "UPDATE objective_remediations SET updated_at=updated_at
                 WHERE id=? AND objective_id=? AND status='claimed'
                   AND lease_owner=? AND attempt_index=? AND lease_expires_at>?
                   AND COALESCE(binding_id, '')=COALESCE(?, '')",
            )
            .bind(&permit.remediation_id)
            .bind(&permit.objective_id)
            .bind(&permit.owner)
            .bind(permit.claim_epoch)
            .bind(now)
            .bind(&permit.binding_id)
            .execute(&mut *tx)
            .await?;
            if remediation_claim.rows_affected() != 1 {
                bail!("remediation claim expired or changed before settlement");
            }
            let objective_claim = sqlx::query(
                "UPDATE objectives SET updated_at=updated_at
                 WHERE id=? AND status='waiting_system' AND remediation_id=?
                   AND lease_owner=? AND lease_expires_at>?",
            )
            .bind(&permit.objective_id)
            .bind(&permit.remediation_id)
            .bind(&permit.owner)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            if objective_claim.rows_affected() != 1 {
                bail!("objective claim expired or changed before settlement");
            }
        }
        if decision.status == ObjectiveStatus::Completed || delivery_takeover.is_some() {
            let pointed_delivery_run_id: Option<String> = sqlx::query_scalar(
                "SELECT delivery_run_id FROM objectives WHERE id=? AND revision=?",
            )
            .bind(&decision.objective_id)
            .bind(expected_revision)
            .fetch_one(&mut *tx)
            .await?;
            if let Some(takeover) = delivery_takeover.as_ref() {
                if pointed_delivery_run_id.as_deref() != Some(takeover.run_id.as_str()) {
                    bail!("delivery takeover does not match the authoritative Objective pointer");
                }
                let pending_calls: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM tool_calls tool
                     JOIN objective_bindings binding
                       ON binding.id=tool.binding_id AND binding.objective_id=tool.objective_id
                     JOIN delivery_runs run
                       ON run.objective_id=tool.objective_id
                     WHERE run.id=? AND run.objective_id=?
                       AND run.claim_epoch=? AND run.reconciled_claim_epoch=run.claim_epoch
                       AND run.status='awaiting_completion_arbitration'
                       AND tool.tool_name='deliver_changes'
                       AND tool.status='pending'
                       AND tool.binding_id=?
                       AND tool.resource_generation=?
                       AND tool.action_signature=?
                       AND tool.resource_generation=binding.resource_generation
                       AND (
                         (binding.resource_kind='chat_root_turn'
                          AND binding.resource_id=run.root_turn_id)
                         OR
                         (binding.resource_kind='task_run'
                          AND binding.resource_id=run.task_id)
                       )",
                )
                .bind(&takeover.run_id)
                .bind(&decision.objective_id)
                .bind(takeover.claim_epoch)
                .bind(&takeover.binding_id)
                .bind(takeover.resource_generation)
                .bind(&takeover.action_signature)
                .fetch_one(&mut *tx)
                .await?;
                if pending_calls == 0 {
                    bail!("delivery takeover has no crash-left pending tool call to settle");
                }
                let result_json = serde_json::json!({
                    "status": "done",
                    "delivery_run_id": takeover.run_id,
                    "claim_epoch": takeover.claim_epoch,
                    "reconciled_after_takeover": true,
                })
                .to_string();
                let tool_rows = sqlx::query(
                    "SELECT tool.id, message.session_id
                     FROM tool_calls tool
                     JOIN messages message ON message.id=tool.message_id
                     JOIN objective_bindings binding
                       ON binding.id=tool.binding_id AND binding.objective_id=tool.objective_id
                     JOIN delivery_runs run ON run.objective_id=tool.objective_id
                     WHERE run.id=? AND run.objective_id=?
                       AND run.claim_epoch=? AND run.reconciled_claim_epoch=run.claim_epoch
                       AND tool.tool_name='deliver_changes' AND tool.status='pending'
                       AND tool.binding_id=?
                       AND tool.resource_generation=?
                       AND tool.action_signature=?
                       AND tool.resource_generation=binding.resource_generation
                       AND (
                         (binding.resource_kind='chat_root_turn'
                          AND binding.resource_id=run.root_turn_id)
                         OR
                         (binding.resource_kind='task_run'
                          AND binding.resource_id=run.task_id)
                       )",
                )
                .bind(&takeover.run_id)
                .bind(&decision.objective_id)
                .bind(takeover.claim_epoch)
                .bind(&takeover.binding_id)
                .bind(takeover.resource_generation)
                .bind(&takeover.action_signature)
                .fetch_all(&mut *tx)
                .await?;
                for row in tool_rows {
                    let trace_id: String = row.try_get("id")?;
                    let session_id: String = row.try_get("session_id")?;
                    let provider_tool_call_id = trace_id
                        .strip_prefix(&format!("{session_id}:"))
                        .ok_or_else(|| {
                            anyhow!("normalized delivery tool call identity is malformed")
                        })?;
                    let updated = sqlx::query(
                        "UPDATE tool_calls
                         SET status='done', result=?, error=NULL, duration_ms=COALESCE(duration_ms, 0)
                         WHERE id=? AND status='pending'",
                    )
                    .bind(&result_json)
                    .bind(&trace_id)
                    .execute(&mut *tx)
                    .await?;
                    if updated.rows_affected() != 1 {
                        bail!("delivery tool call changed during takeover settlement");
                    }
                    let replay_message_id = format!("{trace_id}:result");
                    let replay_content = serde_json::json!({
                        "tool_call_id": provider_tool_call_id,
                        "content": result_json.clone(),
                        "status": "done",
                    })
                    .to_string();
                    sqlx::query(
                        "INSERT INTO messages (id, session_id, role, content, created_at)
                         VALUES (?, ?, 'tool', ?, ?)
                         ON CONFLICT(id) DO UPDATE SET
                           session_id=excluded.session_id, role='tool', content=excluded.content",
                    )
                    .bind(replay_message_id)
                    .bind(session_id)
                    .bind(replay_content)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            let unresolved_receipts: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM side_effect_receipts
                 WHERE objective_id=? AND status IN ('not_started','started','unknown')",
            )
            .bind(&decision.objective_id)
            .fetch_one(&mut *tx)
            .await?;
            if unresolved_receipts > 0 {
                bail!(
                    "objective completion refused with {unresolved_receipts} unresolved side-effect receipt(s)"
                );
            }
            let tool_calls_exist: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='tool_calls'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if tool_calls_exist == 1 {
                let unresolved_tool_calls: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM tool_calls
                     WHERE objective_id=? AND status='pending'",
                )
                .bind(&decision.objective_id)
                .fetch_one(&mut *tx)
                .await?;
                if unresolved_tool_calls > 0 {
                    bail!(
                        "objective completion refused with {unresolved_tool_calls} unresolved tool call(s)"
                    );
                }
            }

            let objective_side_effect_started: i64 = sqlx::query_scalar(
                "SELECT side_effect_started FROM objectives
                 WHERE id=? AND revision=?",
            )
            .bind(&decision.objective_id)
            .bind(expected_revision)
            .fetch_one(&mut *tx)
            .await?;
            let current_binding: Option<(String, i64, i64)> =
                if let Some(binding_id) = permit.and_then(|permit| permit.binding_id.as_deref()) {
                    sqlx::query_as(
                        "SELECT id, resource_generation, side_effect_started
                         FROM objective_bindings
                         WHERE id=? AND objective_id=?",
                    )
                    .bind(binding_id)
                    .bind(&decision.objective_id)
                    .fetch_optional(&mut *tx)
                    .await?
                } else {
                    sqlx::query_as(
                        "SELECT id, resource_generation, side_effect_started
                         FROM objective_bindings
                         WHERE objective_id=?
                         ORDER BY CASE WHEN resource_id=? THEN 0 ELSE 1 END,
                                  resource_generation DESC, updated_at DESC, id DESC
                         LIMIT 1",
                    )
                    .bind(&decision.objective_id)
                    .bind(decision.resume_cursor.as_deref().unwrap_or(""))
                    .fetch_optional(&mut *tx)
                    .await?
                };
            let binding_side_effect_started = current_binding
                .as_ref()
                .is_some_and(|(_, _, started)| *started != 0);
            if pointed_delivery_run_id.is_some()
                || objective_side_effect_started != 0
                || binding_side_effect_started
            {
                let Some((binding_id, resource_generation, _)) = current_binding else {
                    bail!(
                        "objective completion refused because a side effect started without a current Objective binding"
                    );
                };
                if tool_calls_exist != 1 {
                    bail!(
                        "objective completion refused because a side effect started without normalized tool attribution"
                    );
                }
                let attributed_actions: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM tool_calls
                     WHERE objective_id=? AND binding_id=? AND resource_generation=?
                       AND NULLIF(TRIM(action_signature), '') IS NOT NULL",
                )
                .bind(&decision.objective_id)
                .bind(&binding_id)
                .bind(resource_generation)
                .fetch_one(&mut *tx)
                .await?;
                if attributed_actions == 0 {
                    bail!(
                        "objective completion refused because a side effect started without a trustworthy current attributed receipt"
                    );
                }
                let delivery_signatures: Vec<String> = sqlx::query_scalar(
                    "SELECT DISTINCT action_signature FROM tool_calls
                     WHERE objective_id=? AND binding_id=? AND resource_generation=?
                       AND tool_name='deliver_changes'
                       AND NULLIF(TRIM(action_signature), '') IS NOT NULL
                     ORDER BY action_signature",
                )
                .bind(&decision.objective_id)
                .bind(&binding_id)
                .bind(resource_generation)
                .fetch_all(&mut *tx)
                .await?;
                if pointed_delivery_run_id.is_some() && delivery_signatures.is_empty() {
                    bail!(
                        "objective completion refused because its pointed DeliveryRun lacks a completed current deliver_changes action"
                    );
                }
                if delivery_signatures.len() > 1 {
                    bail!(
                        "objective completion refused because current deliver_changes actions do not have one stable action signature"
                    );
                }
                if let Some(action_signature) = delivery_signatures.first() {
                    let completed_action_rows: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM tool_calls
                         WHERE objective_id=? AND binding_id=? AND resource_generation=?
                           AND tool_name='deliver_changes' AND status='done'
                           AND action_signature=?",
                    )
                    .bind(&decision.objective_id)
                    .bind(&binding_id)
                    .bind(resource_generation)
                    .bind(action_signature)
                    .fetch_one(&mut *tx)
                    .await?;
                    if completed_action_rows == 0 {
                        bail!(
                            "objective completion refused because deliver_changes has no completed current action"
                        );
                    }

                    let candidate_rows = sqlx::query(
                        "SELECT run.id, run.stage, run.claim_epoch,
                                run.repo_identity, run.worktree_identity,
                                run.expected_head_sha, run.change_set_digest,
                                run.requested_ceiling, run.reached_ceiling,
                                run.canonical_pr_number, run.canonical_pr_url
                         FROM delivery_runs AS run
                         JOIN objective_bindings AS binding
                           ON binding.id=? AND binding.objective_id=run.objective_id
                         JOIN objectives AS objective
                           ON objective.id=run.objective_id
                          AND objective.delivery_run_id=run.id
                         WHERE run.objective_id=?
                           AND run.run_kind='deliver_changes'
                           AND run.status IN ('awaiting_completion_arbitration','completed')
                           AND run.stage='complete'
                           AND run.claim_epoch>0
                           AND run.reconciled_claim_epoch=run.claim_epoch
                           AND (
                             (run.status='awaiting_completion_arbitration'
                              AND run.next_action_authorized=1)
                             OR
                             (run.status='completed'
                              AND run.next_action_authorized=0)
                           )
                           AND COALESCE(run.next_action, '')=''
                           AND COALESCE(run.failure_signature, '')=''
                           AND COALESCE(run.wait_class, 'none')='none'
                           AND NULLIF(TRIM(run.repo_identity), '') IS NOT NULL
                           AND NULLIF(TRIM(run.worktree_identity), '') IS NOT NULL
                           AND NULLIF(TRIM(run.expected_head_sha), '') IS NOT NULL
                           AND NULLIF(TRIM(run.change_set_digest), '') IS NOT NULL
                           AND (
                             (binding.resource_kind='chat_root_turn'
                              AND run.root_turn_id=binding.resource_id)
                             OR
                             (binding.resource_kind='task_run'
                              AND run.task_id=binding.resource_id)
                           )
                           AND CASE run.reached_ceiling
                             WHEN 'local' THEN 0
                             WHEN 'committed' THEN 1
                             WHEN 'pushed' THEN 2
                             WHEN 'pr_open' THEN 3
                             WHEN 'ci_green' THEN 4
                             WHEN 'merge_queued' THEN 5
                             WHEN 'merged' THEN 6
                             WHEN 'release_triggered' THEN 7
                             WHEN 'deployment_succeeded' THEN 8
                             WHEN 'live_verified' THEN 9
                             ELSE -1
                           END >= CASE run.requested_ceiling
                             WHEN 'pr_only' THEN 3
                             WHEN 'through_ci_green' THEN 4
                             WHEN 'through_merge' THEN 6
                             WHEN 'through_release' THEN 9
                             ELSE 100
                           END
                           AND run.canonical_pr_number IS NOT NULL
                           AND NULLIF(TRIM(run.canonical_pr_url), '') IS NOT NULL
                           AND run.canonical_head_sha=run.expected_head_sha
                           AND NOT EXISTS (
                             SELECT 1 FROM delivery_mutation_intents AS intent
                             WHERE intent.run_id=run.id
                               AND intent.status IN ('started','unknown')
                           )",
                    )
                    .bind(&binding_id)
                    .bind(&decision.objective_id)
                    .fetch_all(&mut *tx)
                    .await?;
                    if candidate_rows.len() != 1 {
                        bail!(
                            "objective completion refused because deliver_changes requires exactly one pointed receipt-backed DeliveryRun at its requested ceiling"
                        );
                    }
                    let candidate = &candidate_rows[0];
                    let run_id: String = candidate.try_get("id")?;
                    let stage: String = candidate.try_get("stage")?;
                    let claim_epoch: i64 = candidate.try_get("claim_epoch")?;
                    let repo_identity: String = candidate.try_get("repo_identity")?;
                    let worktree_identity: String = candidate.try_get("worktree_identity")?;
                    let expected_head_sha: String = candidate.try_get("expected_head_sha")?;
                    let change_set_digest: String = candidate.try_get("change_set_digest")?;
                    let requested_ceiling: String = candidate.try_get("requested_ceiling")?;
                    let reached_ceiling: String = candidate.try_get("reached_ceiling")?;
                    let canonical_pr_number: i64 = candidate.try_get("canonical_pr_number")?;
                    let canonical_pr_url: String = candidate.try_get("canonical_pr_url")?;
                    let already_terminal: bool = sqlx::query_scalar::<_, String>(
                        "SELECT status FROM delivery_runs WHERE id=?",
                    )
                    .bind(&run_id)
                    .fetch_one(&mut *tx)
                    .await?
                        == "completed";
                    let identity_material = serde_json::json!({
                        "objective_id": decision.objective_id,
                        "delivery_run_id": run_id,
                        "claim_epoch": claim_epoch,
                        "repo_identity": repo_identity,
                        "worktree_identity": worktree_identity,
                        "expected_head_sha": expected_head_sha,
                        "change_set_digest": change_set_digest,
                        "requested_ceiling": requested_ceiling,
                        "reached_ceiling": reached_ceiling,
                        "canonical_pr_number": canonical_pr_number,
                        "canonical_pr_url": canonical_pr_url,
                    })
                    .to_string();
                    let external_identity_digest =
                        format!("sha256:{:x}", Sha256::digest(identity_material.as_bytes()));
                    let idempotency_material = format!(
                        "delivery_completion\0{}\0{}\0{}\0{}",
                        decision.objective_id, binding_id, action_signature, run_id,
                    );
                    let idempotency_key = format!(
                        "sha256:{:x}",
                        Sha256::digest(idempotency_material.as_bytes())
                    );
                    let reconciliation_receipt_id = format!(
                        "sha256:{:x}",
                        Sha256::digest(
                            format!("side_effect_receipt\0{idempotency_key}").as_bytes()
                        )
                    );
                    let summary_json = serde_json::json!({
                        "delivery_run_id": run_id,
                        "claim_epoch": claim_epoch,
                        "requested_ceiling": requested_ceiling,
                        "reached_ceiling": reached_ceiling,
                    })
                    .to_string();
                    let mut exact_receipt: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM side_effect_receipts
                         WHERE id=? AND objective_id=? AND binding_id=?
                           AND action_fingerprint=? AND idempotency_key=?
                           AND status='reconciled' AND external_identity_digest=?",
                    )
                    .bind(&reconciliation_receipt_id)
                    .bind(&decision.objective_id)
                    .bind(&binding_id)
                    .bind(action_signature)
                    .bind(&idempotency_key)
                    .bind(&external_identity_digest)
                    .fetch_one(&mut *tx)
                    .await?;
                    if exact_receipt == 0 {
                        sqlx::query(
                            "INSERT INTO side_effect_receipts
                             (id, objective_id, binding_id, revision, action_fingerprint,
                              idempotency_key, status, external_identity_digest,
                              summary_json, created_at, observed_at)
                             VALUES (?, ?, ?, ?, ?, ?, 'reconciled', ?, ?, ?, ?)",
                        )
                        .bind(&reconciliation_receipt_id)
                        .bind(&decision.objective_id)
                        .bind(&binding_id)
                        .bind(expected_revision)
                        .bind(action_signature)
                        .bind(&idempotency_key)
                        .bind(&external_identity_digest)
                        .bind(summary_json)
                        .bind(now)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                        exact_receipt = 1;
                    }
                    if exact_receipt != 1 {
                        bail!(
                            "objective completion refused because the DeliveryRun reconciliation receipt conflicted"
                        );
                    }
                    delivery_run_to_complete = Some(DeliveryCompletionCandidate {
                        run_id,
                        binding_id: binding_id.clone(),
                        resource_generation,
                        stage,
                        claim_epoch,
                        repo_identity,
                        worktree_identity,
                        expected_head_sha,
                        change_set_digest,
                        requested_ceiling,
                        reached_ceiling,
                        canonical_pr_number,
                        canonical_pr_url,
                        action_signature: action_signature.clone(),
                        reconciliation_receipt_id,
                        already_terminal,
                        takeover_lease_owner: delivery_takeover
                            .as_ref()
                            .map(|takeover| takeover.lease_owner.clone()),
                    });
                }

                let unmatched_actions: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM tool_calls AS tool
                     WHERE tool.objective_id=? AND tool.binding_id=?
                       AND tool.resource_generation=?
                       AND NULLIF(TRIM(tool.action_signature), '') IS NOT NULL
                       AND NOT EXISTS (
                           SELECT 1 FROM side_effect_receipts AS receipt
                           WHERE receipt.objective_id=tool.objective_id
                             AND receipt.binding_id=?
                             AND receipt.action_fingerprint=tool.action_signature
                             AND receipt.status IN ('committed','reconciled')
                       )",
                )
                .bind(&decision.objective_id)
                .bind(&binding_id)
                .bind(resource_generation)
                .bind(&binding_id)
                .fetch_one(&mut *tx)
                .await?;
                if unmatched_actions > 0 {
                    bail!(
                        "objective completion refused with {unmatched_actions} current attributed side effect(s) lacking a matching committed receipt"
                    );
                }
            }
        }
        if permit.is_some_and(|permit| {
            permit.binding_id.is_some() != permit.resource_generation.is_some()
        }) {
            bail!("mutation permit binding and resource generation must be paired");
        }
        let authoritative_binding_id =
            if let Some(binding_id) = permit.and_then(|permit| permit.binding_id.clone()) {
                let generation = sqlx::query_scalar::<_, i64>(
                    "SELECT resource_generation FROM objective_bindings
                 WHERE id=? AND objective_id=?",
                )
                .bind(&binding_id)
                .bind(&decision.objective_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(generation) = generation else {
                    bail!("mutation permit binding does not belong to objective");
                };
                if permit.and_then(|permit| permit.resource_generation) != Some(generation) {
                    bail!("mutation permit resource generation changed before settlement");
                }
                Some(binding_id)
            } else {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM objective_bindings
                 WHERE objective_id=?
                 ORDER BY CASE WHEN resource_id=? THEN 0 ELSE 1 END,
                          updated_at DESC, resource_generation DESC
                 LIMIT 1",
                )
                .bind(&decision.objective_id)
                .bind(decision.resume_cursor.as_deref().unwrap_or(""))
                .fetch_optional(&mut *tx)
                .await?
            };
        let result = sqlx::query(
            "UPDATE objectives SET
               revision=?, status=?, decision_type=?, domain=?,
               reached_acceptance=?, requires_user_action=?, request_key=?,
               decision_key=?, action_signature=?, failure_code=?, failure_signature=?,
               recovery_owner=?, remediation_id=?, resume_cursor=?,
               output_started=MAX(output_started, ?),
               side_effect_started=MAX(side_effect_started, ?), next_observation_at=?,
               lease_owner=NULL, lease_expires_at=NULL, evidence_ref=?,
               cancellation_provenance=?, last_observed_process_instance=?,
               last_progress_at=?, updated_at=?, completed_at=?
             WHERE id=? AND revision=?",
        )
        .bind(decision.revision)
        .bind(decision.status.as_str())
        .bind(decision.decision_type.as_str())
        .bind(decision.domain.as_str())
        .bind(&decision.reached_acceptance)
        .bind(i64::from(decision.requires_user_action))
        .bind(&decision.request_key)
        .bind(&decision.decision_key)
        .bind(&decision.action_signature)
        .bind(&decision.failure_code)
        .bind(&decision.failure_signature)
        .bind(&decision.recovery_owner)
        .bind(&decision.remediation_id)
        .bind(&decision.resume_cursor)
        .bind(i64::from(decision.output_started))
        .bind(i64::from(decision.side_effect_started))
        .bind(decision.next_observation_at)
        .bind(&evidence_ref)
        .bind(&decision.cancellation_provenance)
        .bind(&process_instance)
        .bind(now)
        .bind(now)
        .bind(completed_at)
        .bind(&decision.objective_id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            bail!("objective revision changed while applying decision");
        }

        if decision.status == ObjectiveStatus::Cancelled {
            let cancelled_run = sqlx::query(
                "UPDATE delivery_runs
                 SET status='cancelled', next_action=NULL, next_action_authorized=0,
                     wait_class=NULL, failure_signature=NULL, failure_code=NULL,
                     failure_class=NULL, remediation_id=NULL,
                     business_decision_key=NULL, decision_options_json=NULL,
                     recommended_option=NULL, safe_default_action=NULL,
                     decision_reason=NULL, core_input_request_key=NULL,
                     core_inputs_json=NULL, core_input_attempts_json=NULL,
                     core_input_resume_stage=NULL, core_input_request_count=0,
                     lease_owner=NULL, lease_expires_at=NULL,
                     last_progress_at=?, progress_revision=progress_revision+1,
                     updated_at=?
                 WHERE objective_id=?
                   AND id=(SELECT delivery_run_id FROM objectives WHERE id=?)
                   AND status NOT IN ('completed','failed','cancelled','rejected')
                 RETURNING id, stage",
            )
            .bind(now)
            .bind(now)
            .bind(&decision.objective_id)
            .bind(&decision.objective_id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(run) = cancelled_run {
                let run_id: String = run.try_get("id")?;
                let stage: String = run.try_get("stage")?;
                sqlx::query(
                    "INSERT INTO delivery_run_events
                     (id, run_id, event_kind, stage, status, wait_class,
                      detail_json, process_instance, created_at)
                     VALUES (?, ?, 'objective_cancelled', ?, 'cancelled', NULL, ?, ?, ?)",
                )
                .bind(Uuid::new_v4().to_string())
                .bind(run_id)
                .bind(stage)
                .bind(
                    serde_json::json!({
                        "objective_id": decision.objective_id,
                        "objective_revision": decision.revision,
                        "cancellation_provenance": decision.cancellation_provenance,
                    })
                    .to_string(),
                )
                .bind(&process_instance)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
        }

        if let Some(candidate) = &delivery_run_to_complete {
            let completed_rows = if candidate.already_terminal {
                let exact_terminal: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM delivery_runs
                     WHERE id=? AND objective_id=? AND status='completed'
                       AND stage=? AND claim_epoch=?
                       AND reconciled_claim_epoch=claim_epoch
                       AND repo_identity=? AND worktree_identity=?
                       AND expected_head_sha=? AND change_set_digest=?
                       AND requested_ceiling=? AND reached_ceiling=?
                       AND canonical_pr_number=? AND canonical_pr_url=?
                       AND canonical_head_sha=expected_head_sha
                       AND next_action_authorized=0
                       AND lease_owner IS NULL AND lease_expires_at IS NULL",
                )
                .bind(&candidate.run_id)
                .bind(&decision.objective_id)
                .bind(&candidate.stage)
                .bind(candidate.claim_epoch)
                .bind(&candidate.repo_identity)
                .bind(&candidate.worktree_identity)
                .bind(&candidate.expected_head_sha)
                .bind(&candidate.change_set_digest)
                .bind(&candidate.requested_ceiling)
                .bind(&candidate.reached_ceiling)
                .bind(candidate.canonical_pr_number)
                .bind(&candidate.canonical_pr_url)
                .fetch_one(&mut *tx)
                .await?;
                exact_terminal
            } else {
                sqlx::query(
                    "UPDATE delivery_runs
                 SET status='completed', next_action=NULL, next_action_authorized=0,
                     wait_class=NULL, failure_signature=NULL, failure_code=NULL,
                     failure_class=NULL, remediation_id=NULL,
                     business_decision_key=NULL, decision_options_json=NULL,
                     recommended_option=NULL, safe_default_action=NULL,
                     decision_reason=NULL, core_input_request_key=NULL,
                     core_inputs_json=NULL, core_input_attempts_json=NULL,
                     core_input_resume_stage=NULL, core_input_request_count=0,
                     lease_owner=NULL, lease_expires_at=NULL,
                     last_progress_at=?, progress_revision=progress_revision+1,
                     updated_at=?
                 WHERE id=? AND objective_id=?
                   AND run_kind='deliver_changes'
                   AND status='awaiting_completion_arbitration'
                   AND stage=? AND claim_epoch=?
                   AND reconciled_claim_epoch=claim_epoch
                   AND repo_identity=? AND worktree_identity=?
                   AND expected_head_sha=? AND change_set_digest=?
                   AND requested_ceiling=? AND reached_ceiling=?
                   AND canonical_pr_number=? AND canonical_pr_url=?
                   AND canonical_head_sha=expected_head_sha
                   AND next_action_authorized=1
                   AND (? IS NULL OR (lease_owner=? AND lease_expires_at>?))
                   AND COALESCE(next_action, '')=''
                   AND COALESCE(failure_signature, '')=''
                   AND COALESCE(wait_class, 'none')='none'
                   AND EXISTS (
                     SELECT 1 FROM objectives AS objective
                     WHERE objective.id=delivery_runs.objective_id
                       AND objective.delivery_run_id=delivery_runs.id
                   )
                   AND EXISTS (
                     SELECT 1 FROM objective_bindings AS binding
                     WHERE binding.id=?
                       AND binding.objective_id=delivery_runs.objective_id
                       AND binding.resource_generation=?
                       AND (
                         (binding.resource_kind='chat_root_turn'
                          AND delivery_runs.root_turn_id=binding.resource_id)
                         OR
                         (binding.resource_kind='task_run'
                          AND delivery_runs.task_id=binding.resource_id)
                       )
                   )
                   AND NOT EXISTS (
                     SELECT 1 FROM delivery_mutation_intents AS intent
                     WHERE intent.run_id=delivery_runs.id
                       AND intent.status IN ('started','unknown')
                   )",
                )
                .bind(now)
                .bind(now)
                .bind(&candidate.run_id)
                .bind(&decision.objective_id)
                .bind(&candidate.stage)
                .bind(candidate.claim_epoch)
                .bind(&candidate.repo_identity)
                .bind(&candidate.worktree_identity)
                .bind(&candidate.expected_head_sha)
                .bind(&candidate.change_set_digest)
                .bind(&candidate.requested_ceiling)
                .bind(&candidate.reached_ceiling)
                .bind(candidate.canonical_pr_number)
                .bind(&candidate.canonical_pr_url)
                .bind(&candidate.takeover_lease_owner)
                .bind(&candidate.takeover_lease_owner)
                .bind(now)
                .bind(&candidate.binding_id)
                .bind(candidate.resource_generation)
                .execute(&mut *tx)
                .await?
                .rows_affected() as i64
            };
            if completed_rows != 1 {
                bail!("DeliveryRun completion proof changed during Objective arbitration");
            }
            if !candidate.already_terminal {
                sqlx::query(
                    "INSERT INTO delivery_run_events
                 (id, run_id, event_kind, stage, status, wait_class,
                  detail_json, process_instance, created_at)
                 VALUES (?, ?, 'objective_completed', ?, 'completed', ?, ?, ?, ?)",
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&candidate.run_id)
                .bind(&candidate.stage)
                .bind(Option::<String>::None)
                .bind(
                    serde_json::json!({
                        "objective_id": decision.objective_id,
                        "objective_revision": decision.revision,
                        "binding_id": authoritative_binding_id,
                        "action_signature": candidate.action_signature,
                        "reconciliation_receipt_id": candidate.reconciliation_receipt_id,
                    })
                    .to_string(),
                )
                .bind(&process_instance)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
        }

        if let Some(evidence) = &decision.evidence {
            sqlx::query(
                "INSERT INTO objective_evidence
                 (id, objective_id, revision, kind, scope, digest, evidence_ref,
                  observed_at, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&evidence.id)
            .bind(&decision.objective_id)
            .bind(decision.revision)
            .bind(evidence.kind.as_str())
            .bind(&evidence.scope)
            .bind(&evidence.digest)
            .bind(&evidence.evidence_ref)
            .bind(evidence.observed_at)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        let prior_remediation_status = if decision.status == ObjectiveStatus::Completed {
            "completed"
        } else if decision.status == ObjectiveStatus::Cancelled {
            "cancelled"
        } else {
            "superseded"
        };
        sqlx::query(
            "UPDATE objective_remediations SET status=?, lease_owner=NULL,
               lease_expires_at=NULL, last_progress_at=?, updated_at=?
             WHERE objective_id=?
               AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(prior_remediation_status)
        .bind(now)
        .bind(now)
        .bind(&decision.objective_id)
        .execute(&mut *tx)
        .await?;

        if decision.status == ObjectiveStatus::WaitingSystem {
            sqlx::query(
                "INSERT INTO objective_remediations
                 (id, objective_id, binding_id, domain, status, failure_code, failure_signature,
                  strategy, approach_index, attempt_index, resume_cursor,
                  next_observation_at, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'queued', ?, ?, 'reconcile_then_resume', 0, 0, ?, ?, ?, ?)",
            )
            .bind(
                decision
                    .remediation_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("waiting_system remediation id missing"))?,
            )
            .bind(&decision.objective_id)
            .bind(&authoritative_binding_id)
            .bind(decision.domain.as_str())
            .bind(
                decision
                    .failure_code
                    .as_deref()
                    .ok_or_else(|| anyhow!("waiting_system failure code missing"))?,
            )
            .bind(
                decision
                    .failure_signature
                    .as_deref()
                    .ok_or_else(|| anyhow!("waiting_system failure signature missing"))?,
            )
            .bind(&decision.resume_cursor)
            .bind(
                decision
                    .next_observation_at
                    .ok_or_else(|| anyhow!("waiting_system next observation missing"))?,
            )
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        let envelope_json = serde_json::to_string(&decision)?;
        sqlx::query(
            "INSERT INTO objective_decisions
             (id, objective_id, revision, domain, decision_type, failure_code,
              failure_signature, recovery_owner, remediation_id,
              requires_user_action, output_started, side_effect_started,
              envelope_json, evidence_ref, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&decision.objective_id)
        .bind(decision.revision)
        .bind(decision.domain.as_str())
        .bind(decision.decision_type.as_str())
        .bind(&decision.failure_code)
        .bind(&decision.failure_signature)
        .bind(&decision.recovery_owner)
        .bind(&decision.remediation_id)
        .bind(i64::from(decision.requires_user_action))
        .bind(i64::from(decision.output_started))
        .bind(i64::from(decision.side_effect_started))
        .bind(envelope_json)
        .bind(&evidence_ref)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO objective_events
             (id, objective_id, revision, event_type, status, decision_type,
              domain, failure_code, recovery_owner, created_at)
             VALUES (?, ?, ?, 'decision_applied', ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&decision.objective_id)
        .bind(decision.revision)
        .bind(decision.status.as_str())
        .bind(decision.decision_type.as_str())
        .bind(decision.domain.as_str())
        .bind(&decision.failure_code)
        .bind(&decision.recovery_owner)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        self.get(&decision.objective_id)
            .await?
            .ok_or_else(|| anyhow!("updated objective disappeared"))
    }
}

/// Install the unified Objective schema even when a historical database has a
/// conflicting sqlx migration version/checksum. The DDL is intentionally the
/// same file used by fresh installs and is safe to execute on every startup.
pub async fn ensure_schema(pool: &SqlitePool) -> crate::errors::Result<()> {
    sqlx::raw_sql(include_str!(
        "../../migrations/0007_unified_objective_control_plane.sql"
    ))
    .execute(pool)
    .await?;

    // Preserve historical duplicate rows as immutable audit evidence, while
    // ratcheting every upgraded database so no new cross-revision duplicate
    // can be admitted. A unique index is still installed for clean databases
    // below, but it cannot be created when legacy duplicates already exist.
    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS trg_side_effect_receipts_idempotency_ratchet
         BEFORE INSERT ON side_effect_receipts
         WHEN EXISTS (
             SELECT 1 FROM side_effect_receipts
             WHERE objective_id = NEW.objective_id
               AND idempotency_key = NEW.idempotency_key
         )
         BEGIN
             SELECT RAISE(ABORT, 'duplicate side-effect receipt idempotency key');
         END",
    )
    .execute(pool)
    .await?;

    let duplicate_idempotency_groups: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
             SELECT objective_id, idempotency_key
             FROM side_effect_receipts
             GROUP BY objective_id, idempotency_key
             HAVING COUNT(*) > 1
         )",
    )
    .fetch_one(pool)
    .await?;
    if duplicate_idempotency_groups == 0 {
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_side_effect_receipts_objective_idempotency
             ON side_effect_receipts(objective_id, idempotency_key)",
        )
        .execute(pool)
        .await?;
    } else {
        tracing::error!(
            duplicate_groups = duplicate_idempotency_groups,
            "historical side-effect receipts violate cross-revision idempotency; preserved rows, installed the insert ratchet, and skipped the unique index"
        );
    }

    // Compatibility projections are nullable and therefore safe for old
    // readers. Identity is linked only by ObjectiveStore after it has proved a
    // unique immutable chain; schema setup never guesses old ownership.
    for (table, column, column_type) in [
        ("chat_turn_state", "objective_id", "TEXT"),
        ("chat_turn_state", "turn_settled_at", "INTEGER"),
        ("chat_turn_state", "stream_closed_at", "INTEGER"),
        ("task_runs", "objective_id", "TEXT"),
        ("task_runs", "recovery_state", "TEXT"),
        ("task_runs", "next_observation_at", "INTEGER"),
        ("task_attempts", "objective_id", "TEXT"),
        ("tool_calls", "objective_id", "TEXT"),
        ("tool_calls", "binding_id", "TEXT"),
        ("tool_calls", "action_signature", "TEXT"),
        (
            "tool_calls",
            "resource_generation",
            "INTEGER NOT NULL DEFAULT 1",
        ),
        ("delivery_runs", "objective_id", "TEXT"),
    ] {
        ensure_column(pool, table, column, column_type).await?;
    }

    for (table, statement) in [
        (
            "chat_turn_state",
            "CREATE INDEX IF NOT EXISTS idx_chat_turn_state_objective ON chat_turn_state(objective_id)",
        ),
        (
            "task_runs",
            "CREATE INDEX IF NOT EXISTS idx_task_runs_objective ON task_runs(objective_id)",
        ),
        (
            "task_attempts",
            "CREATE INDEX IF NOT EXISTS idx_task_attempts_objective ON task_attempts(objective_id)",
        ),
        (
            "tool_calls",
            "CREATE INDEX IF NOT EXISTS idx_tool_calls_objective_binding
             ON tool_calls(objective_id, binding_id, resource_generation)",
        ),
    ] {
        if table_exists(pool, table).await? {
            sqlx::query(statement).execute(pool).await?;
        }
    }
    Ok(())
}

async fn table_exists(pool: &SqlitePool, table: &str) -> crate::errors::Result<bool> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
            .bind(table)
            .fetch_one(pool)
            .await?;
    Ok(count > 0)
}

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    column_type: &str,
) -> crate::errors::Result<()> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let exists = rows.iter().any(|row| {
        row.try_get::<String, _>("name")
            .map(|name| name == column)
            .unwrap_or(false)
    });
    if !exists {
        sqlx::query(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {column_type}"
        ))
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codefactory_agent_core::CompletionEvidence;
    use codefactory_agent_loop::run::{RunOutcome, StopReason};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn fresh_schema_enforces_cross_revision_side_effect_receipt_idempotency() {
        let pool = pool().await;
        let index_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='index'
               AND name='idx_side_effect_receipts_objective_idempotency'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(index_exists, 1);

        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-cross-revision-idempotency".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-cross-revision-idempotency".into()),
                root_turn_id: Some("turn-cross-revision-idempotency".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES (?, ?, 1, 'sha256:first-action', 'sha256:stable-key',
                     'committed', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let duplicate = sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES (?, ?, 2, 'sha256:second-action', 'sha256:stable-key',
                     'committed', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await;
        assert!(duplicate.is_err());
    }

    #[tokio::test]
    async fn historical_duplicate_receipts_are_preserved_but_new_duplicates_are_rejected() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/0007_unified_objective_control_plane.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-historical-duplicate-ratchet".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-historical-duplicate-ratchet".into()),
                root_turn_id: Some("turn-historical-duplicate-ratchet".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        for (id, revision, action) in [
            ("historical-receipt-a", 1_i64, "sha256:action-a"),
            ("historical-receipt-b", 2_i64, "sha256:action-b"),
        ] {
            sqlx::query(
                "INSERT INTO side_effect_receipts
                 (id, objective_id, revision, action_fingerprint, idempotency_key,
                  status, created_at, observed_at)
                 VALUES (?, ?, ?, ?, 'sha256:historical-stable-key',
                         'committed', ?, ?)",
            )
            .bind(id)
            .bind(&objective.id)
            .bind(revision)
            .bind(action)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        ensure_schema(&pool).await.unwrap();
        let preserved: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND idempotency_key='sha256:historical-stable-key'",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(preserved, 2, "schema repair must not delete audit evidence");

        let third = sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES ('historical-receipt-c', ?, 3, 'sha256:action-c',
                     'sha256:historical-stable-key', 'committed', ?, ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await;
        assert!(
            third.is_err(),
            "legacy duplicates may remain for audit, but the schema ratchet must reject new ones"
        );
    }

    const CURRENT_ACTION_SIGNATURE: &str = "sha256:current-action";

    struct GenericReceiptCompletionFixture {
        pool: SqlitePool,
        store: ObjectiveStore,
        objective: ObjectiveSnapshot,
        evidence: ObjectiveEvidence,
        old_binding_id: String,
        current_binding_id: String,
        now: i64,
    }

    async fn generic_receipt_completion_fixture(test_id: &str) -> GenericReceiptCompletionFixture {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE tool_calls (
               id TEXT PRIMARY KEY,
               objective_id TEXT,
               tool_name TEXT NOT NULL,
               status TEXT NOT NULL,
               binding_id TEXT,
               action_signature TEXT,
               resource_generation INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: format!("objective-{test_id}"),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(format!("session-{test_id}")),
                root_turn_id: Some(format!("turn-{test_id}")),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        let old_binding_id = format!("binding-{test_id}-generation-1");
        let current_binding_id = format!("binding-{test_id}-generation-2");
        for (binding_id, generation, side_effect_started) in [
            (old_binding_id.as_str(), 1_i64, 0_i64),
            (current_binding_id.as_str(), 2_i64, 1_i64),
        ] {
            sqlx::query(
                "INSERT INTO objective_bindings
                 (id, objective_id, domain, resource_kind, resource_id,
                  resource_generation, identity_digest, side_effect_started,
                  created_at, updated_at)
                 VALUES (?, ?, 'tool', 'chat_root_turn', ?, ?, ?, ?, ?, ?)",
            )
            .bind(binding_id)
            .bind(&objective.id)
            .bind(format!("turn-{test_id}"))
            .bind(generation)
            .bind(format!("sha256:binding-{test_id}-{generation}"))
            .bind(side_effect_started)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("UPDATE objectives SET side_effect_started=1 WHERE id=?")
            .bind(&objective.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             (id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES (?, ?, 'bash', 'done', ?, ?, 2)",
        )
        .bind(format!("tool-{test_id}"))
        .bind(&objective.id)
        .bind(&current_binding_id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .execute(&pool)
        .await
        .unwrap();
        let objective = store.get(&objective.id).await.unwrap().unwrap();
        assert!(objective.side_effect_started);
        let evidence = ObjectiveEvidence {
            id: format!("evidence-{test_id}"),
            kind: EvidenceKind::CurrentStateAcceptance,
            scope: format!("turn-{test_id}"),
            digest: "sha256:validated-current-state".into(),
            evidence_ref: format!("db:test-validation/{test_id}"),
            observed_at: now,
            reached_acceptance: "validated_change".into(),
        };

        GenericReceiptCompletionFixture {
            pool,
            store,
            objective,
            evidence,
            old_binding_id,
            current_binding_id,
            now,
        }
    }

    async fn insert_generic_receipt(
        fixture: &GenericReceiptCompletionFixture,
        receipt_id: &str,
        objective_id: &str,
        binding_id: &str,
        action_signature: &str,
        status: &str,
    ) {
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(receipt_id)
        .bind(objective_id)
        .bind(binding_id)
        .bind(fixture.objective.revision)
        .bind(action_signature)
        .bind(format!("sha256:key-{receipt_id}"))
        .bind(status)
        .bind(fixture.now)
        .bind(fixture.now)
        .execute(&fixture.pool)
        .await
        .unwrap();
    }

    fn completion_decision(fixture: &GenericReceiptCompletionFixture) -> DecisionEnvelope {
        CompletionArbiter::decide(&fixture.objective, &[fixture.evidence.clone()]).unwrap()
    }

    #[test]
    fn decision_router_never_hands_a_technical_failure_to_the_user() {
        let objective = ObjectiveSnapshot::new(
            "objective-1",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Tool,
            "local_validation",
        );
        let decision = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Tool,
                failure_code: "tool_timeout".into(),
                failure_signature: "bash:test:timeout".into(),
                next_observation_at: 1_723_000_030_000,
                resume_cursor: Some("tool-call-7".into()),
            },
        )
        .unwrap();

        assert_eq!(decision.decision_type, DecisionType::Waiting);
        assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
        assert!(!decision.requires_user_action);
        assert!(decision.recovery_owner.is_some());
        assert!(decision.remediation_id.is_some());
    }

    #[test]
    fn decision_router_requires_typed_fields_for_real_user_attention() {
        let objective = ObjectiveSnapshot::new(
            "objective-2",
            ObjectiveKind::Delivery,
            RecoveryDomain::Auth,
            "release",
        );
        let missing_request_key = DecisionRouter::route(
            &objective,
            RouteSignal::CoreInputRequired {
                domain: RecoveryDomain::Auth,
                request_key: "".into(),
                missing_inputs: vec!["oauth_login".into()],
                attempted_routes: vec!["refresh_token".into()],
                resume_cursor: Some("model-boundary-3".into()),
            },
        );
        assert!(missing_request_key.is_err());

        let decision = DecisionRouter::route(
            &objective,
            RouteSignal::AuthorizationRequired {
                domain: RecoveryDomain::Permission,
                request_key: "permission:publish:42".into(),
                action_signature: "publish:repo:head".into(),
                resume_cursor: Some("tool-call-42".into()),
            },
        )
        .unwrap();
        assert_eq!(decision.status, ObjectiveStatus::WaitingAuthorization);
        assert_eq!(decision.decision_type, DecisionType::AuthorizationRequired);
        assert!(decision.requires_user_action);
        assert_eq!(
            decision.action_signature.as_deref(),
            Some("publish:repo:head")
        );
    }

    #[tokio::test]
    async fn objective_store_uses_revision_cas_and_completion_arbiter_evidence() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-3".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-1".into()),
                root_turn_id: Some("turn-1".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "project_chat".into(),
            })
            .await
            .unwrap();

        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Provider,
                failure_code: "provider_503".into(),
                failure_signature: "endpoint-a:503".into(),
                next_observation_at: 1_723_000_030_000,
                resume_cursor: Some("model-boundary-1".into()),
            },
        )
        .unwrap();
        let revised = store.apply_decision(1, waiting).await.unwrap();
        assert_eq!(revised.revision, 2);
        assert_eq!(revised.status, ObjectiveStatus::WaitingSystem);
        assert!(store
            .apply_decision(1, revised.as_decision())
            .await
            .is_err());

        let waiting_again = DecisionRouter::route(
            &revised,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Provider,
                failure_code: "provider_503".into(),
                failure_signature: "endpoint-b:503".into(),
                next_observation_at: 1_723_000_060_000,
                resume_cursor: Some("model-boundary-2".into()),
            },
        )
        .unwrap();
        let revised = store.apply_decision(2, waiting_again).await.unwrap();
        assert_eq!(revised.revision, 3);
        let active_remediations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(&revised.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_remediations, 1);

        let no_evidence = CompletionArbiter::decide(&revised, &[]);
        assert!(no_evidence.is_err());
        let evidence = ObjectiveEvidence {
            id: "evidence-1".into(),
            kind: EvidenceKind::InformationalAnswer,
            scope: "turn-1".into(),
            digest: "sha256:answer".into(),
            evidence_ref: "db:messages/assistant-1".into(),
            observed_at: 1_723_000_040_000,
            reached_acceptance: "answer".into(),
        };
        let complete = CompletionArbiter::decide(&revised, &[evidence]).unwrap();
        let completed = store.apply_decision(3, complete).await.unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        assert!(completed.evidence_ref.is_some());
        let active_remediations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(&completed.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_remediations, 0);
    }

    #[tokio::test]
    async fn completion_fails_closed_while_receipt_or_tool_call_is_unresolved() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE tool_calls (
               id TEXT PRIMARY KEY, objective_id TEXT, status TEXT NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-unresolved-effect".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-unresolved-effect".into()),
                root_turn_id: Some("turn-unresolved-effect".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES ('receipt-unresolved', ?, 1, 'sha256:action', 'sha256:key',
                     'started', ?, ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls(id, objective_id, status)
             VALUES ('tool-unresolved', ?, 'pending')",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let evidence = ObjectiveEvidence {
            id: "evidence-unresolved-effect".into(),
            kind: EvidenceKind::CurrentStateAcceptance,
            scope: "turn-unresolved-effect".into(),
            digest: "sha256:validation".into(),
            evidence_ref: "db:test-validation".into(),
            observed_at: now,
            reached_acceptance: "validated_change".into(),
        };
        let complete = CompletionArbiter::decide(&objective, &[evidence]).unwrap();

        assert!(store
            .apply_decision(objective.revision, complete.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("unresolved side-effect receipt"));
        sqlx::query(
            "UPDATE side_effect_receipts SET status='committed' WHERE id='receipt-unresolved'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(store
            .apply_decision(objective.revision, complete.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("unresolved tool call"));
        sqlx::query("UPDATE tool_calls SET status='done' WHERE id='tool-unresolved'")
            .execute(&pool)
            .await
            .unwrap();
        let completed = store
            .apply_decision(objective.revision, complete)
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
    }

    #[tokio::test]
    async fn completion_rejects_side_effect_started_without_current_binding_receipt() {
        let fixture = generic_receipt_completion_fixture("zero-receipt").await;

        assert!(fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn completion_rejects_committed_receipt_owned_by_another_objective() {
        let fixture = generic_receipt_completion_fixture("wrong-objective").await;
        let other_objective = fixture
            .store
            .create(CreateObjective {
                id: "objective-receipt-owner".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-receipt-owner".into()),
                root_turn_id: Some("turn-receipt-owner".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let other_binding_id = "binding-receipt-owner";
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, side_effect_started,
              created_at, updated_at)
             VALUES (?, ?, 'tool', 'chat_root_turn', 'turn-receipt-owner', 1,
                     'sha256:binding-receipt-owner', 1, ?, ?)",
        )
        .bind(other_binding_id)
        .bind(&other_objective.id)
        .bind(fixture.now)
        .bind(fixture.now)
        .execute(&fixture.pool)
        .await
        .unwrap();
        insert_generic_receipt(
            &fixture,
            "receipt-wrong-objective",
            &other_objective.id,
            other_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "committed",
        )
        .await;

        assert!(fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn completion_rejects_committed_receipt_from_prior_binding_generation() {
        let fixture = generic_receipt_completion_fixture("old-binding-generation").await;
        insert_generic_receipt(
            &fixture,
            "receipt-old-binding-generation",
            &fixture.objective.id,
            &fixture.old_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "committed",
        )
        .await;

        assert!(fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn completion_rejects_committed_receipt_for_wrong_action_signature() {
        let fixture = generic_receipt_completion_fixture("wrong-action").await;
        insert_generic_receipt(
            &fixture,
            "receipt-wrong-action",
            &fixture.objective.id,
            &fixture.current_binding_id,
            "sha256:different-action",
            "committed",
        )
        .await;

        assert!(fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn completion_accepts_matching_current_binding_committed_or_reconciled_receipt() {
        for status in ["committed", "reconciled"] {
            let fixture =
                generic_receipt_completion_fixture(&format!("current-receipt-{status}")).await;
            insert_generic_receipt(
                &fixture,
                &format!("receipt-current-{status}"),
                &fixture.objective.id,
                &fixture.current_binding_id,
                CURRENT_ACTION_SIGNATURE,
                status,
            )
            .await;

            let completed = fixture
                .store
                .apply_decision(fixture.objective.revision, completion_decision(&fixture))
                .await
                .unwrap();
            assert_eq!(completed.status, ObjectiveStatus::Completed, "{status}");
        }
    }

    async fn persist_delivery_completion_candidate(
        fixture: &GenericReceiptCompletionFixture,
        run_id: &str,
        reached_ceiling: &str,
    ) {
        crate::agent::delivery_run::ensure_schema(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tool_calls SET tool_name='deliver_changes' WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let process = crate::agent::delivery_run::ProcessIdentity::new(
            format!("process-{run_id}"),
            "test",
            "test",
        );
        let run = crate::agent::delivery_run::NewDeliveryRun {
            id: run_id.into(),
            objective_id: fixture.objective.id.clone(),
            run_kind: "deliver_changes".into(),
            session_id: fixture.objective.session_id.clone(),
            root_turn_id: fixture.objective.root_turn_id.clone(),
            task_segment_id: None,
            task_id: None,
            workspace_path: format!("/tmp/{run_id}"),
            worktree_identity: format!("worktree-{run_id}"),
            repo_identity: format!("repo-{run_id}"),
            base_branch: "main".into(),
            head_branch: format!("codex/{run_id}"),
            change_set_digest: format!("sha256:change-set-{run_id}"),
            expected_head_sha: format!("head-{run_id}"),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "preflight".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("deliver".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let claim_epoch = crate::agent::delivery_run::create_delivery_run(
            &fixture.pool,
            &run,
            &process,
            fixture.now,
            90_000,
        )
        .await
        .unwrap();
        let pointer: String =
            sqlx::query_scalar("SELECT delivery_run_id FROM objectives WHERE id=?")
                .bind(&fixture.objective.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(pointer, run_id);
        crate::agent::delivery_run::record_delivery_observation(
            &fixture.pool,
            run_id,
            &process,
            claim_epoch,
            &crate::agent::delivery_run::DeliveryObservation {
                head_branch: run.head_branch,
                stage: "complete".into(),
                status: "awaiting_completion_arbitration".into(),
                wait_class: Some("none".into()),
                next_action: None,
                reached_ceiling: reached_ceiling.into(),
                expected_head_sha: run.expected_head_sha.clone(),
                canonical_pr_number: Some(42),
                canonical_pr_url: Some("https://example.invalid/pull/42".into()),
                canonical_head_sha: Some(run.expected_head_sha),
                failure_signature: None,
                core_input: None,
                identity_revision: None,
            },
            fixture.now + 1,
            90_000,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn receipt_backed_delivery_completion_atomically_completes_run_and_objective() {
        let fixture = generic_receipt_completion_fixture("delivery-ledger-bridge").await;
        persist_delivery_completion_candidate(
            &fixture,
            "delivery-ledger-bridge-run",
            "live_verified",
        )
        .await;

        let completed = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let run_status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_runs WHERE id='delivery-ledger-bridge-run'",
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(run_status, "completed");
        let terminal_projection: (
            i64,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT next_action_authorized, lease_owner, lease_expires_at,
                        wait_class, failure_signature
                 FROM delivery_runs WHERE id='delivery-ledger-bridge-run'",
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(terminal_projection, (0, None, None, None, None));
        let receipt: (String, String) = sqlx::query_as(
            "SELECT status, summary_json FROM side_effect_receipts
             WHERE objective_id=? AND binding_id=?
               AND action_fingerprint=?",
        )
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(receipt.0, "reconciled");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&receipt.1).unwrap()["delivery_run_id"],
            "delivery-ledger-bridge-run"
        );
        let completion_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events
             WHERE run_id='delivery-ledger-bridge-run'
               AND event_kind='objective_completed' AND status='completed'",
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(completion_events, 1);
    }

    async fn crash_left_delivery_fixture(
        test_id: &str,
        kind: ObjectiveKind,
    ) -> GenericReceiptCompletionFixture {
        let fixture = generic_receipt_completion_fixture(test_id).await;
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN message_id TEXT")
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN result TEXT")
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN error TEXT")
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN duration_ms INTEGER")
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               role TEXT NOT NULL,
               content TEXT NOT NULL,
               created_at INTEGER NOT NULL
             )",
        )
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objectives
             SET kind=?, domain='delivery', requested_acceptance=?
             WHERE id=?",
        )
        .bind(kind.as_str())
        .bind(if kind == ObjectiveKind::Live {
            "live_verification"
        } else {
            "delivery_receipt"
        })
        .bind(&fixture.objective.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
        persist_delivery_completion_candidate(&fixture, &format!("{test_id}-run"), "live_verified")
            .await;
        let assistant_message_id = format!("assistant-{test_id}");
        sqlx::query(
            "INSERT INTO messages(id, session_id, role, content, created_at)
             VALUES (?, ?, 'assistant', '', ?)",
        )
        .bind(&assistant_message_id)
        .bind(fixture.objective.session_id.as_deref().unwrap())
        .bind(fixture.now)
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE tool_calls
             SET id=?, message_id=?, status='pending'
             WHERE objective_id=?",
        )
        .bind(format!(
            "{}:provider-delivery-before-crash",
            fixture.objective.session_id.as_deref().unwrap()
        ))
        .bind(&assistant_message_id)
        .bind(&fixture.objective.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             (id, message_id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES (?, ?, ?, 'deliver_changes', 'pending', ?, ?, 2)",
        )
        .bind(format!(
            "{}:provider-delivery-after-reprompt",
            fixture.objective.session_id.as_deref().unwrap()
        ))
        .bind(&assistant_message_id)
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .execute(&fixture.pool)
        .await
        .unwrap();

        // The authoritative DeliveryRun pointer, not these legacy latches,
        // decides whether the completion bridge is mandatory.
        sqlx::query("UPDATE objectives SET side_effect_started=0 WHERE id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objective_bindings SET side_effect_started=0 WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        fixture
    }

    #[tokio::test]
    async fn takeover_terminalizes_crash_left_delivery_and_provider_replay_atomically() {
        let fixture =
            crash_left_delivery_fixture("delivery-crash-convergence", ObjectiveKind::Delivery)
                .await;
        let run_id = "delivery-crash-convergence-run";
        let claim_epoch: i64 =
            sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();

        let completed = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let projection: (String, String, i64, i64) = sqlx::query_as(
            "SELECT objective.status, run.status,
                    run.next_action_authorized, run.autonomous_completion
             FROM objectives objective
             JOIN delivery_runs run ON run.id=objective.delivery_run_id
             WHERE objective.id=?",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(projection, ("completed".into(), "completed".into(), 0, 1));
        let calls: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT status, result, error FROM tool_calls
             WHERE objective_id=? ORDER BY id",
        )
        .bind(&fixture.objective.id)
        .fetch_all(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(status, result, error)| {
            status == "done"
                && result
                    .as_deref()
                    .is_some_and(|value| value.contains(run_id))
                && error.is_none()
        }));
        let replay_messages: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages
             WHERE role='tool' AND content LIKE '%provider-delivery-%'
               AND content LIKE '%done%'",
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(replay_messages, 2);
        let receipt_evidence: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_evidence
             WHERE objective_id=? AND kind='delivery_receipt'",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(receipt_evidence, 1);

        let revision = completed.revision;
        let events_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM delivery_run_events WHERE run_id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        let replay_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE role='tool'")
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        let idempotent = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap();
        assert_eq!(idempotent.revision, revision);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM delivery_run_events WHERE run_id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            events_before
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE role='tool'")
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            replay_before
        );
    }

    #[tokio::test]
    async fn live_takeover_keeps_system_ownership_until_real_live_verification() {
        let fixture =
            crash_left_delivery_fixture("live-crash-convergence", ObjectiveKind::Live).await;
        let run_id = "live-crash-convergence-run";
        let claim_epoch: i64 =
            sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();

        let waiting = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap();
        assert_eq!(waiting.status, ObjectiveStatus::WaitingSystem);
        assert!(!waiting.requires_user_action);
        assert_eq!(
            waiting.recovery_owner.as_deref(),
            Some("objective-supervisor:chat")
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            "completed"
        );
        let evidence_kinds: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM objective_evidence WHERE objective_id=? ORDER BY kind",
        )
        .bind(&fixture.objective.id)
        .fetch_all(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(evidence_kinds, vec!["delivery_receipt"]);

        let live_evidence = ObjectiveEvidence {
            id: "live-observation-after-delivery".into(),
            kind: EvidenceKind::LiveVerification,
            scope: fixture.objective.root_turn_id.clone().unwrap(),
            digest: "sha256:real-live-observation".into(),
            evidence_ref: "live-observer:exact-public-artifact".into(),
            observed_at: fixture.now + 10,
            reached_acceptance: "live_verification".into(),
        };
        let decision = fixture
            .store
            .completion_decision_with_persisted_evidence(&waiting, vec![live_evidence])
            .await
            .unwrap();
        let completed = fixture
            .store
            .apply_decision(waiting.revision, decision)
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let delivery_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events
             WHERE run_id=? AND event_kind='objective_completed'",
        )
        .bind(run_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(delivery_events, 1, "later Live settlement is idempotent");
    }

    #[tokio::test]
    async fn takeover_delivery_completion_fails_closed_on_identity_or_authority_drift() {
        for (suffix, mutation, expected_error) in [
            ("wrong-binding", "wrong_binding", "current binding"),
            ("multi-action", "multi_action", "stable action signature"),
            ("stale-epoch", "stale_epoch", "claim epoch"),
            (
                "unresolved-intent",
                "unresolved_intent",
                "unresolved mutation intent",
            ),
            ("below-ceiling", "below_ceiling", "requested ceiling"),
            ("flags-zero", "flags_zero", "autonomous completion"),
        ] {
            let fixture = crash_left_delivery_fixture(
                &format!("delivery-crash-negative-{suffix}"),
                ObjectiveKind::Delivery,
            )
            .await;
            let run_id = format!("delivery-crash-negative-{suffix}-run");
            let mut claim_epoch: i64 =
                sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                    .bind(&run_id)
                    .fetch_one(&fixture.pool)
                    .await
                    .unwrap();
            match mutation {
                "wrong_binding" => {
                    sqlx::query("UPDATE tool_calls SET binding_id=? WHERE objective_id=?")
                        .bind(&fixture.old_binding_id)
                        .bind(&fixture.objective.id)
                        .execute(&fixture.pool)
                        .await
                        .unwrap();
                }
                "multi_action" => {
                    sqlx::query(
                        "UPDATE tool_calls SET action_signature='sha256:second-action'
                         WHERE id LIKE '%after-reprompt'",
                    )
                    .execute(&fixture.pool)
                    .await
                    .unwrap();
                }
                "stale_epoch" => claim_epoch += 1,
                "unresolved_intent" => {
                    sqlx::query(
                        "INSERT INTO delivery_mutation_intents
                         (intent_id, run_id, claim_epoch, rung, operation_key, status,
                          process_instance, started_at, updated_at)
                         VALUES (?, ?, ?, 'provider_release_trigger', 'release:key',
                                 'unknown', 'crashed-process', ?, ?)",
                    )
                    .bind(format!("intent-{suffix}"))
                    .bind(&run_id)
                    .bind(claim_epoch)
                    .bind(fixture.now)
                    .bind(fixture.now)
                    .execute(&fixture.pool)
                    .await
                    .unwrap();
                }
                "below_ceiling" => {
                    sqlx::query("UPDATE delivery_runs SET reached_ceiling='merged' WHERE id=?")
                        .bind(&run_id)
                        .execute(&fixture.pool)
                        .await
                        .unwrap();
                }
                "flags_zero" => {
                    sqlx::query(
                        "UPDATE delivery_runs
                         SET next_action_authorized=0, autonomous_completion=0 WHERE id=?",
                    )
                    .bind(&run_id)
                    .execute(&fixture.pool)
                    .await
                    .unwrap();
                }
                _ => unreachable!(),
            }

            let error = fixture
                .store
                .settle_reconciled_delivery_after_takeover(
                    &run_id,
                    claim_epoch,
                    &format!("process-{run_id}"),
                )
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected_error), "{suffix}: {error}");
            let state: (String, String) = sqlx::query_as(
                "SELECT objective.status, run.status
                 FROM objectives objective
                 JOIN delivery_runs run ON run.id=objective.delivery_run_id
                 WHERE objective.id=?",
            )
            .bind(&fixture.objective.id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
            assert_eq!(
                state,
                ("active".into(), "awaiting_completion_arbitration".into()),
                "{suffix}"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM side_effect_receipts WHERE objective_id=?"
                )
                .bind(&fixture.objective.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
                0,
                "{suffix}"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM tool_calls
                     WHERE objective_id=? AND status='pending'"
                )
                .bind(&fixture.objective.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
                2,
                "{suffix}"
            );
        }
    }

    #[tokio::test]
    async fn takeover_refuses_to_hide_an_unrelated_pending_tool_call_and_rolls_back() {
        let fixture = crash_left_delivery_fixture(
            "delivery-crash-unrelated-pending",
            ObjectiveKind::Delivery,
        )
        .await;
        let run_id = "delivery-crash-unrelated-pending-run";
        let claim_epoch: i64 =
            sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             (id, message_id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES (?, ?, ?, 'bash', 'pending', ?, 'sha256:unrelated-action', 2)",
        )
        .bind(format!(
            "{}:provider-unrelated-before-crash",
            fixture.objective.session_id.as_deref().unwrap()
        ))
        .bind("assistant-delivery-crash-unrelated-pending")
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .execute(&fixture.pool)
        .await
        .unwrap();

        let error = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unresolved tool call"), "{error}");
        let state: (String, String, i64, i64) = sqlx::query_as(
            "SELECT objective.status, run.status,
                    (SELECT COUNT(*) FROM tool_calls
                     WHERE objective_id=objective.id AND status='pending'),
                    (SELECT COUNT(*) FROM side_effect_receipts
                     WHERE objective_id=objective.id)
             FROM objectives objective
             JOIN delivery_runs run ON run.id=objective.delivery_run_id
             WHERE objective.id=?",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(
            state,
            (
                "active".into(),
                "awaiting_completion_arbitration".into(),
                3,
                0,
            )
        );
    }

    #[tokio::test]
    async fn takeover_requires_the_exact_live_delivery_lease_owner() {
        let fixture =
            crash_left_delivery_fixture("delivery-crash-lease-fence", ObjectiveKind::Delivery)
                .await;
        let run_id = "delivery-crash-lease-fence-run";
        let claim_epoch: i64 =
            sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        let wrong_owner = fixture
            .store
            .settle_reconciled_delivery_after_takeover(run_id, claim_epoch, "replacement-owner")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            wrong_owner.contains("lease expired or changed"),
            "{wrong_owner}"
        );
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=0 WHERE id=?")
            .bind(run_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let expired = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(expired.contains("lease expired or changed"), "{expired}");
        let state: (String, String, i64) = sqlx::query_as(
            "SELECT objective.status, run.status,
                    (SELECT COUNT(*) FROM side_effect_receipts
                     WHERE objective_id=objective.id)
             FROM objectives objective
             JOIN delivery_runs run ON run.id=objective.delivery_run_id
             WHERE objective.id=?",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(
            state,
            ("active".into(), "awaiting_completion_arbitration".into(), 0)
        );
    }

    async fn assert_delivery_completion_refused_and_rolled_back(
        fixture: &GenericReceiptCompletionFixture,
        run_id: &str,
        error_fragment: &str,
    ) {
        let error = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(fixture))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(error_fragment), "unexpected error: {error}");
        let objective_status: String =
            sqlx::query_scalar("SELECT status FROM objectives WHERE id=?")
                .bind(&fixture.objective.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(objective_status, "active");
        let run_status: String = sqlx::query_scalar("SELECT status FROM delivery_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(run_status, "awaiting_completion_arbitration");
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND action_fingerprint=?",
        )
        .bind(&fixture.objective.id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(receipts, 0, "failed arbitration must roll back its receipt");
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events
             WHERE run_id=? AND event_kind='objective_completed'",
        )
        .bind(run_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(events, 0, "failed arbitration must roll back its event");
    }

    #[tokio::test]
    async fn delivery_completion_rejects_missing_authoritative_run_pointer() {
        let fixture = generic_receipt_completion_fixture("delivery-pointer-missing").await;
        let run_id = "delivery-pointer-missing-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE objectives SET delivery_run_id=NULL WHERE id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();

        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;
    }

    #[tokio::test]
    async fn delivery_completion_rejects_non_done_or_multiple_current_actions() {
        let fixture = generic_receipt_completion_fixture("delivery-action-not-done").await;
        let run_id = "delivery-action-not-done-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE tool_calls SET status='waiting' WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "no completed current action",
        )
        .await;

        let fixture = generic_receipt_completion_fixture("delivery-action-ambiguous").await;
        let run_id = "delivery-action-ambiguous-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query(
            "INSERT INTO tool_calls
             (id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES ('tool-delivery-action-ambiguous-2', ?, 'deliver_changes',
                     'done', ?, 'sha256:second-delivery-action', 2)",
        )
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .execute(&fixture.pool)
        .await
        .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "one stable action signature",
        )
        .await;
    }

    #[tokio::test]
    async fn delivery_completion_rejects_stale_epoch_or_unresolved_mutation_intent() {
        let fixture = generic_receipt_completion_fixture("delivery-stale-epoch").await;
        let run_id = "delivery-stale-epoch-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE delivery_runs SET reconciled_claim_epoch=0 WHERE id=?")
            .bind(run_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;

        let fixture = generic_receipt_completion_fixture("delivery-intent-unknown").await;
        let run_id = "delivery-intent-unknown-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query(
            "INSERT INTO delivery_mutation_intents
             (intent_id, run_id, claim_epoch, rung, operation_key, status,
              process_instance, started_at, updated_at)
             VALUES ('intent-delivery-unknown', ?, 1, 'release', 'release:1',
                     'unknown', 'process-test', ?, ?)",
        )
        .bind(run_id)
        .bind(fixture.now)
        .bind(fixture.now)
        .execute(&fixture.pool)
        .await
        .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;
    }

    #[tokio::test]
    async fn delivery_completion_rejects_below_ceiling_or_wrong_canonical_head() {
        let fixture = generic_receipt_completion_fixture("delivery-below-ceiling").await;
        let run_id = "delivery-below-ceiling-run";
        persist_delivery_completion_candidate(&fixture, run_id, "merged").await;
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;

        let fixture = generic_receipt_completion_fixture("delivery-wrong-head").await;
        let run_id = "delivery-wrong-head-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE delivery_runs SET canonical_head_sha='other-head' WHERE id=?")
            .bind(run_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;
    }

    #[tokio::test]
    async fn valid_delivery_cannot_hide_an_unreceipted_generic_side_effect() {
        let fixture = generic_receipt_completion_fixture("delivery-plus-generic").await;
        let run_id = "delivery-plus-generic-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query(
            "INSERT INTO tool_calls
             (id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES ('tool-delivery-plus-generic-2', ?, 'bash', 'done',
                     ?, 'sha256:unreceipted-generic-action', 2)",
        )
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .execute(&fixture.pool)
        .await
        .unwrap();

        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "lacking a matching committed receipt",
        )
        .await;
    }

    #[tokio::test]
    async fn pointed_delivery_run_cannot_be_bypassed_by_only_generic_receipts() {
        let fixture = generic_receipt_completion_fixture("pointed-delivery-bypass").await;
        let run_id = "pointed-delivery-bypass-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE tool_calls SET tool_name='bash' WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        insert_generic_receipt(
            &fixture,
            "receipt-pointed-delivery-bypass",
            &fixture.objective.id,
            &fixture.current_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "committed",
        )
        .await;

        let error = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("pointed DeliveryRun"), "{error}");
        let state: (String, String) = sqlx::query_as(
            "SELECT objective.status, run.status
             FROM objectives objective
             JOIN delivery_runs run ON run.id=objective.delivery_run_id
             WHERE objective.id=?",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(
            state,
            ("active".into(), "awaiting_completion_arbitration".into())
        );
    }

    #[tokio::test]
    async fn explicit_cancel_atomically_fences_pointed_delivery_from_recovery() {
        let fixture = generic_receipt_completion_fixture("cancel-pointed-delivery").await;
        let run_id = "cancel-pointed-delivery-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(fixture.now - 1)
            .bind(run_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let cancelled = DecisionRouter::route(
            &fixture.objective,
            RouteSignal::Cancelled {
                domain: RecoveryDomain::Delivery,
                provenance: "explicit_cancel".into(),
            },
        )
        .unwrap();
        let objective = fixture
            .store
            .apply_decision(fixture.objective.revision, cancelled)
            .await
            .unwrap();
        assert_eq!(objective.status, ObjectiveStatus::Cancelled);
        let run_projection: (String, i64, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT status, next_action_authorized, lease_owner, lease_expires_at
             FROM delivery_runs WHERE id=?",
        )
        .bind(run_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(run_projection, ("cancelled".into(), 0, None, None));
        let recovery = crate::agent::delivery_run::plan_startup_recovery(
            &fixture.pool,
            &crate::agent::delivery_run::ProcessIdentity::new(
                "process-after-explicit-cancel",
                "test",
                "test",
            ),
            fixture.now + 2,
            90_000,
        )
        .await
        .unwrap();
        assert!(recovery.claimed.is_empty());
    }

    #[tokio::test]
    async fn completion_attributes_actions_to_exact_binding_not_same_numbered_generation() {
        let fixture = generic_receipt_completion_fixture("exact-binding-attribution").await;
        let old_binding_id = "binding-exact-old-root";
        let current_binding_id = "binding-exact-current-root";
        sqlx::query("DELETE FROM tool_calls WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM objective_bindings WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, side_effect_started,
              created_at, updated_at)
             VALUES
               (?, ?, 'tool', 'chat_root_turn', 'turn-exact-old-root',
                1, 'sha256:binding-exact-old-root', 1, ?, ?),
               (?, ?, 'tool', 'chat_root_turn', 'turn-exact-current-root',
                1, 'sha256:binding-exact-current-root', 1, ?, ?)",
        )
        .bind(old_binding_id)
        .bind(&fixture.objective.id)
        .bind(fixture.now)
        .bind(fixture.now)
        .bind(current_binding_id)
        .bind(&fixture.objective.id)
        .bind(fixture.now + 100)
        .bind(fixture.now + 100)
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             (id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES
               ('tool-exact-old-root', ?, 'bash', 'done', ?,
                'sha256:old-root-action', 1),
               ('tool-exact-current-root', ?, 'bash', 'done', ?, ?, 1)",
        )
        .bind(&fixture.objective.id)
        .bind(old_binding_id)
        .bind(&fixture.objective.id)
        .bind(current_binding_id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .execute(&fixture.pool)
        .await
        .unwrap();
        insert_generic_receipt(
            &fixture,
            "receipt-exact-current-root",
            &fixture.objective.id,
            current_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "committed",
        )
        .await;

        let completed = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let old_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE binding_id=? AND action_fingerprint='sha256:old-root-action'",
        )
        .bind(old_binding_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(old_receipts, 0);
    }

    #[tokio::test]
    async fn prior_delivery_wait_row_does_not_block_later_receipt_backed_completion() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE tool_calls (
               id TEXT PRIMARY KEY, objective_id TEXT, status TEXT NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-delivery-wait-history".into(),
                kind: ObjectiveKind::Delivery,
                session_id: Some("session-delivery-wait-history".into()),
                root_turn_id: Some("turn-delivery-wait-history".into()),
                domain: RecoveryDomain::Delivery,
                requested_acceptance: "delivery_receipt".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls(id, objective_id, status)
             VALUES ('delivery-wait-history', ?, 'waiting')",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let now = Utc::now().timestamp_millis();
        let evidence = ObjectiveEvidence {
            id: "delivery-receipt-history".into(),
            kind: EvidenceKind::DeliveryReceipt,
            scope: "turn-delivery-wait-history".into(),
            digest: "sha256:delivery-receipt".into(),
            evidence_ref: "delivery-run:completed".into(),
            observed_at: now,
            reached_acceptance: "delivery_receipt".into(),
        };
        let complete = CompletionArbiter::decide(&objective, &[evidence]).unwrap();
        let completed = store
            .apply_decision(objective.revision, complete)
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
    }

    #[tokio::test]
    async fn chat_objective_is_created_once_and_linked_to_the_transport_projection() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status)
             VALUES ('turn-linked', 'session-linked', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let first = store
            .ensure_chat_objective(
                "session-linked",
                "turn-linked",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();
        let second = store
            .ensure_chat_objective(
                "session-linked",
                "turn-linked",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();
        assert_eq!(first.id, second.id);
        let linked: String = sqlx::query_scalar(
            "SELECT objective_id FROM chat_turn_state WHERE root_turn_id='turn-linked'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, first.id);
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE root_turn_id='turn-linked'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        let binding: (String, String, i64, String) = sqlx::query_as(
            "SELECT id, objective_id, resource_generation, identity_digest
             FROM objective_bindings
             WHERE domain='chat' AND resource_kind='chat_root_turn'
               AND resource_id='turn-linked'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(binding.1, first.id);
        assert_eq!(binding.2, 1);
        assert!(binding.3.starts_with("sha256:"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM objective_bindings
                 WHERE objective_id=? AND domain='chat' AND resource_kind='chat_root_turn'",
            )
            .bind(&first.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1,
            "idempotent ensure must preserve one authoritative binding"
        );

        let waiting = DecisionRouter::route(
            &second,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_timeout".into(),
                failure_signature: "sha256:linked-timeout".into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-linked".into()),
            },
        )
        .unwrap();
        store
            .apply_decision(second.revision, waiting)
            .await
            .unwrap();
        let claim = store
            .claim_due_remediations("binding-supervisor", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claim.binding_id.as_deref(), Some(binding.0.as_str()));
        assert_eq!(claim.resource_generation, Some(1));
        let permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id.clone(),
            owner: "binding-supervisor".into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id.clone(),
            resource_generation: claim.resource_generation,
        };
        let retry = DecisionRouter::route(
            &claim.objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_still_unavailable".into(),
                failure_signature: "sha256:linked-timeout-retry".into(),
                next_observation_at: Utc::now().timestamp_millis() + 1_000,
                resume_cursor: Some("turn-linked".into()),
            },
        )
        .unwrap();
        sqlx::query("UPDATE objective_bindings SET resource_generation=2 WHERE id=?")
            .bind(&binding.0)
            .execute(&pool)
            .await
            .unwrap();
        assert!(store
            .apply_claimed_decision(claim.objective.revision, retry.clone(), &permit)
            .await
            .unwrap_err()
            .to_string()
            .contains("resource generation"));
        sqlx::query("UPDATE objective_bindings SET resource_generation=1 WHERE id=?")
            .bind(&binding.0)
            .execute(&pool)
            .await
            .unwrap();
        let retried = store
            .apply_claimed_decision(claim.objective.revision, retry, &permit)
            .await
            .unwrap();
        assert_eq!(retried.status, ObjectiveStatus::WaitingSystem);
        assert_ne!(retried.remediation_id, Some(claim.remediation_id));
    }

    #[tokio::test]
    async fn contextual_root_continues_the_single_open_objective_and_elevates_its_ceiling() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status) VALUES
             ('turn-original', 'session-continuation', 'active'),
             ('turn-approval', 'session-continuation', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let original = store
            .ensure_chat_objective(
                "session-continuation",
                "turn-original",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();

        let continued = store
            .ensure_or_continue_chat_objective(
                "session-continuation",
                "turn-approval",
                Some("turn-original"),
                ObjectiveKind::Delivery,
                "delivery_receipt",
            )
            .await
            .unwrap();

        assert_eq!(continued.id, original.id);
        assert_eq!(continued.kind, ObjectiveKind::Delivery);
        assert!(continued.revision > original.revision);
        let bound: String = sqlx::query_scalar(
            "SELECT objective_id FROM chat_turn_state WHERE root_turn_id='turn-approval'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(bound, original.id);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn contextual_root_fails_closed_when_two_open_objectives_are_present() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status) VALUES
             ('turn-prior', 'session-ambiguous', 'active'),
             ('turn-current', 'session-ambiguous', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool);
        store
            .ensure_chat_objective(
                "session-ambiguous",
                "turn-prior",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();
        store
            .ensure_chat_objective(
                "session-ambiguous",
                "turn-current",
                ObjectiveKind::Informational,
                "informational_answer",
            )
            .await
            .unwrap();

        let error = store
            .ensure_or_continue_chat_objective(
                "session-ambiguous",
                "turn-current",
                Some("turn-prior"),
                ObjectiveKind::Delivery,
                "delivery_receipt",
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("multiple open objectives"));
    }

    #[tokio::test]
    async fn legacy_contextual_root_is_reconciled_to_one_new_opaque_objective() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status) VALUES
             ('turn-legacy', 'session-legacy', 'interrupted'),
             ('turn-legacy-continue', 'session-legacy', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());

        let reconciled = store
            .ensure_or_continue_chat_objective(
                "session-legacy",
                "turn-legacy-continue",
                Some("turn-legacy"),
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();

        assert!(!reconciled.id.starts_with("chat:"));
        assert_eq!(reconciled.kind, ObjectiveKind::LocalMutation);
        let bindings: Vec<String> = sqlx::query_scalar(
            "SELECT objective_id FROM chat_turn_state
             WHERE root_turn_id IN ('turn-legacy','turn-legacy-continue')
             ORDER BY root_turn_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(bindings, vec![reconciled.id.clone(), reconciled.id.clone()]);
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_events
             WHERE objective_id=? AND event_type='legacy_root_reconciled'",
        )
        .bind(&reconciled.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_count, 1);
    }

    #[tokio::test]
    async fn remediation_claim_is_leased_once_and_can_be_deferred_without_user_handoff() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-lease".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-lease".into()),
                root_turn_id: Some("turn-lease".into()),
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
                failure_code: "panic".into(),
                failure_signature: "panic:test".into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-lease".into()),
            },
        )
        .unwrap();
        store.apply_decision(1, waiting).await.unwrap();

        let first = store
            .claim_due_remediations("supervisor-a", 4, 30_000)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        let second = store
            .claim_due_remediations("supervisor-b", 4, 30_000)
            .await
            .unwrap();
        assert!(second.is_empty());

        store
            .defer_claimed_remediation(
                &first[0].objective.id,
                &first[0].remediation_id,
                "supervisor-a",
                first[0].claim_epoch,
                1_000,
            )
            .await
            .unwrap();
        let immediate = store
            .claim_due_remediations("supervisor-b", 4, 30_000)
            .await
            .unwrap();
        assert!(immediate.is_empty());
        sqlx::query("UPDATE objective_remediations SET next_observation_at=? WHERE id=?")
            .bind(Utc::now().timestamp_millis() - 1)
            .bind(&first[0].remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET next_observation_at=? WHERE id=?")
            .bind(Utc::now().timestamp_millis() - 1)
            .bind(&first[0].objective.id)
            .execute(&pool)
            .await
            .unwrap();
        let reclaimed = store
            .claim_due_remediations("supervisor-a", 4, 30_000)
            .await
            .unwrap();
        assert_eq!(reclaimed.len(), 1);
        assert!(reclaimed[0].claim_epoch > first[0].claim_epoch);
        assert!(!store
            .renew_claimed_remediation(
                &first[0].objective.id,
                &first[0].remediation_id,
                "supervisor-a",
                first[0].claim_epoch,
                30_000,
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn claimed_remediation_lease_is_renewed_only_by_its_current_owner() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-lease-renewal".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-lease-renewal".into()),
                root_turn_id: Some("turn-lease-renewal".into()),
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
                failure_signature: "provider_timeout:test".into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-lease-renewal".into()),
            },
        )
        .unwrap();
        store.apply_decision(1, waiting).await.unwrap();
        let claim = store
            .claim_due_remediations("supervisor-renewal", 1, 1_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let before: i64 =
            sqlx::query_scalar("SELECT lease_expires_at FROM objective_remediations WHERE id=?")
                .bind(&claim.remediation_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert!(!store
            .renew_claimed_remediation(
                &claim.objective.id,
                &claim.remediation_id,
                "another-supervisor",
                claim.claim_epoch,
                60_000,
            )
            .await
            .unwrap());
        assert!(store
            .renew_claimed_remediation(
                &claim.objective.id,
                &claim.remediation_id,
                "supervisor-renewal",
                claim.claim_epoch,
                60_000,
            )
            .await
            .unwrap());

        let (renewed_remediation_lease, renewed_objective_lease): (i64, i64) = sqlx::query_as(
            "SELECT remediation.lease_expires_at, objective.lease_expires_at
             FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE remediation.id=?",
        )
        .bind(&claim.remediation_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(renewed_remediation_lease > before);
        assert_eq!(renewed_remediation_lease, renewed_objective_lease);

        let expired = Utc::now().timestamp_millis() - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.objective.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!store
            .renew_claimed_remediation(
                &claim.objective.id,
                &claim.remediation_id,
                "supervisor-renewal",
                claim.claim_epoch,
                60_000,
            )
            .await
            .unwrap());

        let (remediation_lease, objective_lease): (i64, i64) = sqlx::query_as(
            "SELECT remediation.lease_expires_at, objective.lease_expires_at
             FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE remediation.id=?",
        )
        .bind(&claim.remediation_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remediation_lease, expired);
        assert_eq!(remediation_lease, objective_lease);
    }

    #[tokio::test]
    async fn expired_claim_is_reclaimed_after_process_loss() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-expired-claim".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-expired-claim".into()),
                root_turn_id: Some("turn-expired-claim".into()),
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
                failure_code: "process_lost".into(),
                failure_signature: "process_lost:test".into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-expired-claim".into()),
            },
        )
        .unwrap();
        store.apply_decision(1, waiting).await.unwrap();
        let claim = store
            .claim_due_remediations("dead-supervisor", 1, 1_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let expired = Utc::now().timestamp_millis() - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.objective.id)
            .execute(&pool)
            .await
            .unwrap();

        let reclaimed = store
            .claim_due_remediations("replacement-supervisor", 1, 30_000)
            .await
            .unwrap();
        assert_eq!(reclaimed.len(), 1);
        let owner: String =
            sqlx::query_scalar("SELECT lease_owner FROM objective_remediations WHERE id=?")
                .bind(&claim.remediation_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owner, "replacement-supervisor");
    }

    #[tokio::test]
    async fn startup_reconciles_only_active_objectives_owned_by_an_old_process() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let stale = store
            .create(CreateObjective {
                id: "objective-stale-process".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-stale-process".into()),
                root_turn_id: Some("turn-stale-process".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let current = store
            .create(CreateObjective {
                id: "objective-current-process".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-current-process".into()),
                root_turn_id: Some("turn-current-process".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "UPDATE objectives SET created_process_instance='old-process',
             last_observed_process_instance='old-process' WHERE id=?",
        )
        .bind(&stale.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objectives SET created_process_instance='current-process',
             last_observed_process_instance='current-process' WHERE id=?",
        )
        .bind(&current.id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            store
                .reconcile_stale_active_objectives("current-process")
                .await
                .unwrap(),
            1
        );
        let stale = store.get(&stale.id).await.unwrap().unwrap();
        assert_eq!(stale.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(stale.failure_code.as_deref(), Some("process_restarted"));
        assert!(!stale.requires_user_action);
        let current = store.get(&current.id).await.unwrap().unwrap();
        assert_eq!(current.status, ObjectiveStatus::Active);
    }

    #[tokio::test]
    async fn restored_authorization_resumes_the_same_objective_without_a_user_message() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool);
        let objective = store
            .create(CreateObjective {
                id: "objective-auth".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-auth".into()),
                root_turn_id: Some("turn-auth".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let authorization = DecisionRouter::route(
            &objective,
            RouteSignal::AuthorizationRequired {
                domain: RecoveryDomain::Auth,
                request_key: "chatgpt-auth:objective-auth".into(),
                action_signature: "oauth:chatgpt:resume:objective-auth".into(),
                resume_cursor: Some("turn-auth".into()),
            },
        )
        .unwrap();
        let waiting = store.apply_decision(1, authorization).await.unwrap();
        assert_eq!(waiting.status, ObjectiveStatus::WaitingAuthorization);

        assert_eq!(
            store
                .resume_waiting_authorizations(RecoveryDomain::Auth, "chatgpt-auth:")
                .await
                .unwrap(),
            1
        );
        let resumed = store.get("objective-auth").await.unwrap().unwrap();
        assert_eq!(resumed.status, ObjectiveStatus::WaitingSystem);
        assert!(!resumed.requires_user_action);
        assert_eq!(resumed.resume_cursor.as_deref(), Some("turn-auth"));
        assert!(resumed.next_observation_at.is_some());
    }

    #[test]
    fn transport_outcome_is_arbitrated_against_the_business_objective() {
        let informational = ObjectiveSnapshot::new(
            "objective-info",
            ObjectiveKind::Informational,
            RecoveryDomain::Chat,
            "answer",
        );
        let answer = RunOutcome {
            final_text: "verified answer".into(),
            completion_evidence: CompletionEvidence {
                completed: true,
                ..Default::default()
            },
            input_tokens: 10,
            output_tokens: 4,
            stop_reason: StopReason::Finished,
        };
        let complete = decision_for_run_outcome(&informational, &answer).unwrap();
        assert_eq!(complete.status, ObjectiveStatus::Completed);
        assert_eq!(complete.decision_type, DecisionType::Complete);

        let local = ObjectiveSnapshot::new(
            "objective-local",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        let prose_only = decision_for_run_outcome(&local, &answer).unwrap();
        assert_eq!(prose_only.status, ObjectiveStatus::WaitingSystem);
        assert!(!prose_only.requires_user_action);
    }

    #[test]
    fn platform_and_ceiling_outcomes_remain_system_owned() {
        let objective = ObjectiveSnapshot::new(
            "objective-recovery",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        for (stop_reason, expected_type) in [
            (StopReason::PlatformIncident, DecisionType::PlatformIncident),
            (StopReason::FailedInternal, DecisionType::FailedInternal),
            (StopReason::IterationCeiling, DecisionType::Waiting),
        ] {
            let outcome = RunOutcome {
                final_text: String::new(),
                completion_evidence: CompletionEvidence::default(),
                input_tokens: 0,
                output_tokens: 0,
                stop_reason,
            };
            let decision = decision_for_run_outcome(&objective, &outcome).unwrap();
            assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
            assert_eq!(decision.decision_type, expected_type);
            assert!(!decision.requires_user_action);
        }
    }
}
