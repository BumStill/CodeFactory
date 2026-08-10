CREATE TABLE IF NOT EXISTS delivery_runs (
    id                     TEXT PRIMARY KEY,
    run_kind               TEXT NOT NULL,
    session_id             TEXT,
    root_turn_id           TEXT,
    task_segment_id        TEXT,
    task_id                TEXT,
    workspace_path         TEXT,
    repo_identity          TEXT,
    base_branch            TEXT,
    head_branch            TEXT,
    change_set_digest      TEXT,
    expected_head_sha      TEXT,
    canonical_pr_number    INTEGER,
    canonical_pr_url       TEXT,
    canonical_head_sha     TEXT,
    requested_ceiling      TEXT NOT NULL,
    reached_ceiling        TEXT NOT NULL,
    stage                  TEXT NOT NULL,
    status                 TEXT NOT NULL,
    wait_class             TEXT,
    next_action            TEXT,
    next_action_authorized INTEGER NOT NULL DEFAULT 0 CHECK(next_action_authorized IN (0, 1)),
    autonomous_completion  INTEGER NOT NULL DEFAULT 0 CHECK(autonomous_completion IN (0, 1)),
    decision_policy        TEXT NOT NULL DEFAULT 'apply_recommended' CHECK(decision_policy IN ('apply_recommended', 'require_irreversible_decision')),
    failure_signature      TEXT,
    stage_attempt          INTEGER NOT NULL DEFAULT 0 CHECK(stage_attempt >= 0),
    lease_owner            TEXT,
    lease_expires_at       INTEGER,
    last_observed_at       INTEGER NOT NULL,
    last_progress_at       INTEGER NOT NULL,
    progress_revision      INTEGER NOT NULL DEFAULT 0 CHECK(progress_revision >= 0),
    app_version            TEXT NOT NULL,
    app_build              TEXT NOT NULL,
    process_instance       TEXT NOT NULL,
    business_decision_key  TEXT,
    decision_options_json  TEXT,
    recommended_option     TEXT,
    safe_default_action    TEXT,
    decision_reason        TEXT,
    core_input_request_key TEXT,
    core_inputs_json       TEXT,
    core_input_attempts_json TEXT,
    core_input_resume_stage TEXT,
    core_input_request_count INTEGER NOT NULL DEFAULT 0 CHECK(core_input_request_count BETWEEN 0 AND 1),
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL,
    CHECK(status <> 'needs_user'),
    CHECK(
      status <> 'needs_business_decision'
      OR (
        decision_policy = 'require_irreversible_decision'
        AND business_decision_key IS NOT NULL AND business_decision_key <> ''
        AND decision_options_json IS NOT NULL AND decision_options_json <> ''
        AND recommended_option IS NOT NULL AND recommended_option <> ''
        AND safe_default_action IS NOT NULL AND safe_default_action <> ''
        AND decision_reason IS NOT NULL AND decision_reason <> ''
      )
    ),
    CHECK(
      status <> 'core_input_required'
      OR (
        core_input_request_key IS NOT NULL AND core_input_request_key <> ''
        AND core_inputs_json IS NOT NULL AND core_inputs_json <> ''
        AND core_input_attempts_json IS NOT NULL AND core_input_attempts_json <> ''
        AND core_input_resume_stage IS NOT NULL AND core_input_resume_stage <> ''
        AND core_input_request_count = 1
      )
    )
);

CREATE TABLE IF NOT EXISTS delivery_run_events (
    id                 TEXT PRIMARY KEY,
    run_id             TEXT NOT NULL,
    event_kind         TEXT NOT NULL,
    stage              TEXT NOT NULL,
    status             TEXT NOT NULL,
    wait_class         TEXT,
    detail_json        TEXT,
    process_instance   TEXT NOT NULL,
    created_at         INTEGER NOT NULL,
    FOREIGN KEY(run_id) REFERENCES delivery_runs(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_delivery_runs_recovery
    ON delivery_runs(status, lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_delivery_runs_session
    ON delivery_runs(session_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_delivery_runs_pr
    ON delivery_runs(canonical_pr_number, canonical_head_sha);
CREATE INDEX IF NOT EXISTS idx_delivery_run_events_run
    ON delivery_run_events(run_id, created_at);

CREATE TRIGGER IF NOT EXISTS trg_delivery_runs_requested_ceiling_immutable
BEFORE UPDATE OF requested_ceiling ON delivery_runs
WHEN NEW.requested_ceiling <> OLD.requested_ceiling
BEGIN
    SELECT RAISE(ABORT, 'delivery requested ceiling is immutable');
END;
