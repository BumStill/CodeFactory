// SPDX-License-Identifier: Apache-2.0
//! M287 session-level liveness invariant.

use serde::{Deserialize, Serialize};

pub const LIVENESS_SUPERVISOR_INTERVAL_MS: i64 = 5_000;
pub const LIVENESS_SUPERVISOR_MAX_INTERVAL_MS: i64 = 60_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessOwnerKind {
    Model,
    Tool,
    BackgroundTask,
    Permission,
    UserInput,
    Wake,
    Delivery,
    StartupRecovery,
    Supervisor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LivenessOwner {
    pub kind: LivenessOwnerKind,
    pub owner_id: String,
    pub heartbeat_at: i64,
    pub deadline_at: i64,
    pub not_before_at: Option<i64>,
    pub channel_alive: Option<bool>,
    pub display_summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionLivenessSnapshot {
    pub session_id: String,
    pub objective_id: Option<String>,
    pub owners: Vec<LivenessOwner>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessViolation {
    MissingOwner,
    MultipleOwners,
    DeadlineExceeded,
    DeadChannel,
    WakeBeforeNotBefore,
    WakeTooLate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrWaitObservation {
    pub is_draft: bool,
    pub hold: bool,
    pub state: String,
}

pub fn pr_wait_violation(observation: &PrWaitObservation) -> Option<&'static str> {
    if observation.is_draft {
        Some("draft_pr_cannot_progress")
    } else if observation.hold {
        Some("held_pr_cannot_progress")
    } else {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LivenessEvaluation {
    Healthy(LivenessOwner),
    Violation(LivenessViolation),
}

pub fn evaluate_session_liveness(
    snapshot: &SessionLivenessSnapshot,
    now_ms: i64,
) -> LivenessEvaluation {
    let owner = match snapshot.owners.as_slice() {
        [] => return LivenessEvaluation::Violation(LivenessViolation::MissingOwner),
        [owner] => owner,
        _ => return LivenessEvaluation::Violation(LivenessViolation::MultipleOwners),
    };

    if owner.kind == LivenessOwnerKind::Permission && owner.channel_alive == Some(false) {
        return LivenessEvaluation::Violation(LivenessViolation::DeadChannel);
    }
    if owner.kind == LivenessOwnerKind::Wake {
        if owner
            .not_before_at
            .is_some_and(|not_before| owner.deadline_at < not_before)
        {
            return LivenessEvaluation::Violation(LivenessViolation::WakeBeforeNotBefore);
        }
        if owner.not_before_at.is_some_and(|not_before| {
            owner.deadline_at
                > not_before.saturating_add(LIVENESS_SUPERVISOR_MAX_INTERVAL_MS)
        }) {
            return LivenessEvaluation::Violation(LivenessViolation::WakeTooLate);
        }
    }
    if owner.deadline_at < now_ms {
        return LivenessEvaluation::Violation(LivenessViolation::DeadlineExceeded);
    }

    LivenessEvaluation::Healthy(owner.clone())
}

/// Keep a scheduled wake at or after the external not-before boundary. An early
/// firing is a clock/boundary correction, not a recovery attempt, so this pure
/// function carries no attempt counter and cannot consume backoff.
pub fn normalize_wake_at(_now_ms: i64, not_before_ms: i64, proposed_ms: i64) -> i64 {
    proposed_ms.max(not_before_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(kind: LivenessOwnerKind, deadline_at: i64) -> LivenessOwner {
        LivenessOwner {
            kind,
            owner_id: "owner-1".into(),
            heartbeat_at: 9_000,
            deadline_at,
            not_before_at: None,
            channel_alive: None,
            display_summary: "正在推进".into(),
        }
    }

    fn snapshot(owners: Vec<LivenessOwner>) -> SessionLivenessSnapshot {
        SessionLivenessSnapshot {
            session_id: "session-1".into(),
            objective_id: Some("objective-1".into()),
            owners,
        }
    }

    #[test]
    fn r1_exactly_one_live_owner_is_healthy() {
        let expected = owner(LivenessOwnerKind::Model, 20_000);
        assert_eq!(
            evaluate_session_liveness(&snapshot(vec![expected.clone()]), 10_000),
            LivenessEvaluation::Healthy(expected)
        );
    }

    #[test]
    fn r1_missing_and_multiple_owners_are_violations() {
        assert_eq!(
            evaluate_session_liveness(&snapshot(vec![]), 10_000),
            LivenessEvaluation::Violation(LivenessViolation::MissingOwner)
        );
        assert_eq!(
            evaluate_session_liveness(
                &snapshot(vec![
                    owner(LivenessOwnerKind::Model, 20_000),
                    owner(LivenessOwnerKind::Tool, 20_000),
                ]),
                10_000,
            ),
            LivenessEvaluation::Violation(LivenessViolation::MultipleOwners)
        );
    }

    #[test]
    fn r1_expired_owner_and_dead_permission_channel_are_violations() {
        assert_eq!(
            evaluate_session_liveness(
                &snapshot(vec![owner(LivenessOwnerKind::Tool, 9_999)]),
                10_000,
            ),
            LivenessEvaluation::Violation(LivenessViolation::DeadlineExceeded)
        );
        let mut permission = owner(LivenessOwnerKind::Permission, 20_000);
        permission.channel_alive = Some(false);
        assert_eq!(
            evaluate_session_liveness(&snapshot(vec![permission]), 10_000),
            LivenessEvaluation::Violation(LivenessViolation::DeadChannel)
        );
    }

    #[test]
    fn r1_wake_must_not_precede_not_before_or_lag_a_supervisor_period() {
        let mut early = owner(LivenessOwnerKind::Wake, 9_000);
        early.not_before_at = Some(12_000);
        assert_eq!(
            evaluate_session_liveness(&snapshot(vec![early]), 10_000),
            LivenessEvaluation::Violation(LivenessViolation::WakeBeforeNotBefore)
        );

        let mut late = owner(LivenessOwnerKind::Wake, 80_001);
        late.not_before_at = Some(20_000);
        assert_eq!(
            evaluate_session_liveness(&snapshot(vec![late]), 10_000),
            LivenessEvaluation::Violation(LivenessViolation::WakeTooLate)
        );
    }

    #[test]
    fn r5_early_wake_is_normalized_without_backoff() {
        assert_eq!(normalize_wake_at(10_000, 12_000, 9_998), 12_000);
        assert_eq!(normalize_wake_at(10_000, 12_000, 70_000), 70_000);
    }

    #[test]
    fn counterexamples_are_healthy_before_their_deadlines() {
        for kind in [
            LivenessOwnerKind::Model,
            LivenessOwnerKind::BackgroundTask,
            LivenessOwnerKind::UserInput,
        ] {
            assert!(matches!(
                evaluate_session_liveness(&snapshot(vec![owner(kind, 60_000)]), 10_000),
                LivenessEvaluation::Healthy(_)
            ));
        }
    }

    #[test]
    fn draft_or_hold_pr_is_a_wait_violation_but_ready_open_pr_is_healthy() {
        assert_eq!(pr_wait_violation(&PrWaitObservation {
            is_draft: true, hold: false, state: "OPEN".into(),
        }), Some("draft_pr_cannot_progress"));
        assert_eq!(pr_wait_violation(&PrWaitObservation {
            is_draft: false, hold: true, state: "OPEN".into(),
        }), Some("held_pr_cannot_progress"));
        assert_eq!(pr_wait_violation(&PrWaitObservation {
            is_draft: false, hold: false, state: "OPEN".into(),
        }), None);
    }
}
