// SPDX-License-Identifier: Apache-2.0

#[path = "../src/agent/scenario_case_observation.rs"]
mod observation;

use observation::{
    attach_e2e001_case_observation, e2e001_failure_legacy_receipt, CleanupObservation,
    ProcessObservation,
};

fn legacy_success_receipt() -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "scenario_id": "HLT-001",
        "build_git_sha": "unknown",
        "process_restart_observed": true,
        "phase_one_was_hard_killed": true,
        "same_objective": true,
        "user_message_count": 1,
        "human_prompt_count": 0,
        "side_effect_receipt_count": 1,
        "replay_call_link_count": 2,
        "objective_status": "completed",
        "live_owner_count": 0,
        "claimable_remediation_count": 0,
        "artifact_verified": true,
        "cleanup_ok": true
    })
}

#[test]
fn e2e001_observation_preserves_legacy_fields_and_adds_exact_case_identity() {
    let receipt = attach_e2e001_case_observation(
        legacy_success_receipt(),
        ProcessObservation {
            supervisor_hard_kill_issued: true,
            worker_reaped: true,
            phase_one_exit_was_failure: true,
            replacement_process_distinct: true,
            descendant_process_count: 0,
        },
        CleanupObservation {
            cleanup_attempted: true,
            orphan_sweep_performed: true,
            leaked_resource_count: 0,
        },
    );

    assert_eq!(receipt["ok"], true);
    assert_eq!(receipt["scenario_id"], "HLT-001");
    assert_eq!(receipt["user_message_count"], 1);
    assert_eq!(receipt["observation_schema_version"], 1);
    assert_eq!(receipt["case_id"], "E2E-001");
    assert_eq!(
        receipt["scenario_ids"],
        serde_json::json!(["CXD-002", "HLT-001", "HLT-002"])
    );
    assert_eq!(receipt["supervisor_hard_kill_issued"], true);
    assert_eq!(receipt["worker_reaped"], true);
    assert_eq!(receipt["replacement_process_distinct"], true);
    assert_eq!(receipt["descendant_process_count"], 0);
    assert_eq!(receipt["cleanup_attempted"], true);
    assert_eq!(receipt["orphan_sweep_performed"], true);
    assert_eq!(receipt["leaked_resource_count"], 0);
}

#[test]
fn failed_observation_keeps_cleanup_evidence_without_raw_error_text() {
    let receipt = attach_e2e001_case_observation(
        e2e001_failure_legacy_receipt(),
        ProcessObservation::default(),
        CleanupObservation {
            cleanup_attempted: true,
            orphan_sweep_performed: true,
            leaked_resource_count: 0,
        },
    );

    assert_eq!(receipt["ok"], false);
    assert_eq!(receipt["error"], "unattended_smoke_failed");
    assert_eq!(receipt["observation_schema_version"], 1);
    assert_eq!(receipt["case_id"], "E2E-001");
    assert_eq!(receipt["cleanup_attempted"], true);
    assert_eq!(receipt["orphan_sweep_performed"], true);
    assert_eq!(receipt["leaked_resource_count"], 0);
    assert!(!receipt.to_string().contains("/Users/"));
    assert!(!receipt.to_string().contains("token="));
}

#[test]
fn process_and_cleanup_legacy_outcomes_are_derived_from_the_new_evidence() {
    let mut legacy = legacy_success_receipt();
    legacy["process_restart_observed"] = serde_json::Value::Bool(true);
    legacy["phase_one_was_hard_killed"] = serde_json::Value::Bool(true);
    legacy["cleanup_ok"] = serde_json::Value::Bool(true);

    let receipt = attach_e2e001_case_observation(
        legacy,
        ProcessObservation::default(),
        CleanupObservation::default(),
    );

    assert_eq!(receipt["process_restart_observed"], false);
    assert_eq!(receipt["phase_one_was_hard_killed"], false);
    assert_eq!(receipt["cleanup_ok"], false);
}
