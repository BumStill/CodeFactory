// SPDX-License-Identifier: Apache-2.0
//! Privacy-safe raw observations emitted by candidate scenario binaries.
//!
//! These fields are evidence inputs, not a gate verdict. The default-branch
//! trusted builder decides stage policy and constructs the final case receipt.

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub(crate) struct ProcessObservation {
    pub supervisor_hard_kill_issued: bool,
    pub worker_reaped: bool,
    #[serde(skip)]
    pub phase_one_exit_was_failure: bool,
    pub replacement_process_distinct: bool,
    pub descendant_process_count: u32,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub(crate) struct CleanupObservation {
    pub cleanup_attempted: bool,
    pub orphan_sweep_performed: bool,
    pub leaked_resource_count: u32,
}

pub(crate) fn e2e001_failure_legacy_receipt() -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "scenario_id": "HLT-001",
        "build_git_sha": option_env!("CODEFACTORY_BUILD_GIT_SHA").unwrap_or("unknown"),
        "error": "unattended_smoke_failed"
    })
}

pub(crate) fn attach_e2e001_case_observation(
    mut legacy: serde_json::Value,
    process: ProcessObservation,
    cleanup: CleanupObservation,
) -> serde_json::Value {
    let process_restart_observed = process.worker_reaped && process.replacement_process_distinct;
    let phase_one_was_hard_killed = process.supervisor_hard_kill_issued
        && process.worker_reaped
        && process.phase_one_exit_was_failure;
    let cleanup_ok = cleanup.cleanup_attempted
        && cleanup.orphan_sweep_performed
        && cleanup.leaked_resource_count == 0;

    let object = legacy
        .as_object_mut()
        .expect("unattended smoke legacy receipt must be an object");
    object.insert("observation_schema_version".into(), 1.into());
    object.insert("case_id".into(), "E2E-001".into());
    object.insert(
        "scenario_ids".into(),
        serde_json::json!(["CXD-002", "HLT-001", "HLT-002"]),
    );
    object.insert(
        "process_restart_observed".into(),
        process_restart_observed.into(),
    );
    object.insert(
        "phase_one_was_hard_killed".into(),
        phase_one_was_hard_killed.into(),
    );
    for (key, value) in serde_json::to_value(process)
        .expect("serialize process observation")
        .as_object()
        .expect("process observation must serialize as an object")
    {
        object.insert(key.clone(), value.clone());
    }
    for (key, value) in serde_json::to_value(cleanup)
        .expect("serialize cleanup observation")
        .as_object()
        .expect("cleanup observation must serialize as an object")
    {
        object.insert(key.clone(), value.clone());
    }
    object.insert("cleanup_ok".into(), cleanup_ok.into());
    legacy
}
