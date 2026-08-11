-- SPDX-License-Identifier: Apache-2.0
-- Unified business-objective truth. Existing turn/task/delivery tables remain
-- readable projections; only identity-complete rows are linked by application
-- code. The same DDL is mirrored by agent::objective::ensure_schema because
-- historical installations may already own migration version 6.

CREATE TABLE IF NOT EXISTS objectives (
    id                       TEXT PRIMARY KEY,
    revision                 INTEGER NOT NULL DEFAULT 1 CHECK(revision >= 1),
    kind                     TEXT NOT NULL CHECK(kind IN (
                                 'informational', 'local_mutation', 'delivery',
                                 'live', 'legacy_orphan')),
    session_id               TEXT,
    root_turn_id             TEXT,
    task_id                  TEXT,
    delivery_run_id          TEXT,
    scope_identity           TEXT,
    repo_identity            TEXT,
    worktree_identity        TEXT,
    base_sha                 TEXT,
    head_sha                 TEXT,
    change_set_identity      TEXT,
    status                   TEXT NOT NULL CHECK(status IN (
                                 'active', 'waiting_system', 'waiting_core_input',
                                 'waiting_authorization', 'waiting_business_decision',
                                 'completed', 'cancelled', 'legacy_orphan')),
    decision_type            TEXT NOT NULL CHECK(decision_type IN (
                                 'continue', 'waiting', 'apply_recommended',
                                 'platform_incident', 'failed_internal',
                                 'core_input_required', 'authorization_required',
                                 'needs_business_decision', 'complete', 'cancelled')),
    domain                   TEXT NOT NULL,
    autonomous_completion    INTEGER NOT NULL DEFAULT 1 CHECK(autonomous_completion IN (0, 1)),
    requested_acceptance     TEXT NOT NULL,
    reached_acceptance       TEXT,
    requires_user_action     INTEGER NOT NULL DEFAULT 0 CHECK(requires_user_action IN (0, 1)),
    request_key              TEXT,
    decision_key             TEXT,
    action_signature         TEXT,
    failure_code             TEXT,
    failure_signature        TEXT,
    recovery_owner           TEXT,
    remediation_id           TEXT,
    resume_cursor            TEXT,
    output_started           INTEGER NOT NULL DEFAULT 0 CHECK(output_started IN (0, 1)),
    side_effect_started      INTEGER NOT NULL DEFAULT 0 CHECK(side_effect_started IN (0, 1)),
    next_observation_at      INTEGER,
    lease_owner              TEXT,
    lease_expires_at         INTEGER,
    evidence_ref             TEXT,
    cancellation_provenance  TEXT,
    created_surface          TEXT NOT NULL DEFAULT 'unknown',
    created_app_version      TEXT,
    created_app_build        TEXT,
    created_process_instance TEXT,
    last_observed_app_version TEXT,
    last_observed_app_build  TEXT,
    last_observed_process_instance TEXT,
    last_progress_at         INTEGER,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL,
    completed_at             INTEGER,
    CHECK(
      requires_user_action = 0 OR (
        status IN ('waiting_core_input', 'waiting_authorization', 'waiting_business_decision')
        AND decision_type IN ('core_input_required', 'authorization_required', 'needs_business_decision')
      )
    ),
    CHECK(
      status <> 'completed' OR (
        decision_type = 'complete' AND evidence_ref IS NOT NULL AND evidence_ref <> ''
        AND reached_acceptance IS NOT NULL AND completed_at IS NOT NULL
        AND recovery_owner IS NULL AND remediation_id IS NULL
        AND lease_owner IS NULL AND lease_expires_at IS NULL
      )
    ),
    CHECK(decision_type <> 'complete' OR status = 'completed'),
    CHECK(
      status <> 'cancelled' OR (
        decision_type = 'cancelled'
        AND cancellation_provenance IN ('explicit_cancel', 'explicit_deny')
        AND completed_at IS NOT NULL
      )
    ),
    CHECK(status <> 'legacy_orphan' OR requires_user_action = 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_objectives_active_root_turn
    ON objectives(root_turn_id)
    WHERE root_turn_id IS NOT NULL
      AND status NOT IN ('completed', 'cancelled', 'legacy_orphan');
CREATE UNIQUE INDEX IF NOT EXISTS idx_objectives_active_task
    ON objectives(task_id)
    WHERE task_id IS NOT NULL
      AND status NOT IN ('completed', 'cancelled', 'legacy_orphan');
CREATE INDEX IF NOT EXISTS idx_objectives_due
    ON objectives(status, next_observation_at, lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_objectives_session
    ON objectives(session_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS objective_bindings (
    id                  TEXT PRIMARY KEY,
    objective_id        TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    domain              TEXT NOT NULL,
    resource_kind       TEXT NOT NULL,
    resource_id         TEXT NOT NULL,
    resource_generation INTEGER NOT NULL DEFAULT 1 CHECK(resource_generation >= 1),
    identity_digest     TEXT NOT NULL,
    resume_cursor       TEXT,
    output_started      INTEGER NOT NULL DEFAULT 0 CHECK(output_started IN (0, 1)),
    side_effect_started INTEGER NOT NULL DEFAULT 0 CHECK(side_effect_started IN (0, 1)),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE(domain, resource_kind, resource_id, resource_generation)
);
CREATE INDEX IF NOT EXISTS idx_objective_bindings_objective
    ON objective_bindings(objective_id, domain);

CREATE TABLE IF NOT EXISTS objective_events (
    id              TEXT PRIMARY KEY,
    objective_id    TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    revision        INTEGER NOT NULL CHECK(revision >= 1),
    event_type      TEXT NOT NULL,
    status          TEXT,
    decision_type   TEXT,
    domain          TEXT NOT NULL,
    failure_code    TEXT,
    recovery_owner  TEXT,
    detail_json     TEXT,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_objective_events_objective
    ON objective_events(objective_id, created_at);

CREATE TABLE IF NOT EXISTS objective_evidence (
    id              TEXT PRIMARY KEY,
    objective_id    TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    revision        INTEGER NOT NULL CHECK(revision >= 1),
    kind            TEXT NOT NULL,
    scope           TEXT NOT NULL,
    digest          TEXT NOT NULL,
    evidence_ref    TEXT NOT NULL,
    observed_at     INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    UNIQUE(objective_id, revision, kind, digest)
);

CREATE TABLE IF NOT EXISTS objective_decisions (
    id                  TEXT PRIMARY KEY,
    objective_id        TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    revision            INTEGER NOT NULL CHECK(revision >= 1),
    domain              TEXT NOT NULL,
    decision_type       TEXT NOT NULL,
    failure_code        TEXT,
    failure_signature   TEXT,
    recovery_owner      TEXT,
    remediation_id      TEXT,
    requires_user_action INTEGER NOT NULL DEFAULT 0 CHECK(requires_user_action IN (0, 1)),
    output_started      INTEGER NOT NULL DEFAULT 0 CHECK(output_started IN (0, 1)),
    side_effect_started INTEGER NOT NULL DEFAULT 0 CHECK(side_effect_started IN (0, 1)),
    envelope_json       TEXT NOT NULL,
    evidence_ref        TEXT,
    created_at          INTEGER NOT NULL,
    UNIQUE(objective_id, revision)
);

CREATE TABLE IF NOT EXISTS objective_remediations (
    id                  TEXT PRIMARY KEY,
    objective_id        TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    binding_id          TEXT REFERENCES objective_bindings(id) ON DELETE SET NULL,
    domain              TEXT NOT NULL,
    status              TEXT NOT NULL CHECK(status IN (
                            'queued', 'claimed', 'observing', 'repairing',
                            'verifying', 'waiting', 'completed', 'cancelled', 'superseded')),
    failure_code        TEXT NOT NULL,
    failure_signature   TEXT NOT NULL,
    strategy            TEXT NOT NULL,
    approach_index      INTEGER NOT NULL DEFAULT 0 CHECK(approach_index >= 0),
    attempt_index       INTEGER NOT NULL DEFAULT 0 CHECK(attempt_index >= 0),
    action_fingerprint  TEXT,
    resume_cursor       TEXT,
    receipt_id          TEXT,
    next_observation_at INTEGER NOT NULL,
    lease_owner         TEXT,
    lease_expires_at    INTEGER,
    last_progress_at    INTEGER,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_objective_remediations_active_resource
    ON objective_remediations(objective_id, domain, COALESCE(binding_id, ''))
    WHERE status NOT IN ('completed', 'cancelled', 'superseded');
CREATE INDEX IF NOT EXISTS idx_objective_remediations_due
    ON objective_remediations(status, next_observation_at, lease_expires_at);

CREATE TABLE IF NOT EXISTS side_effect_receipts (
    id                       TEXT PRIMARY KEY,
    objective_id             TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    binding_id               TEXT REFERENCES objective_bindings(id) ON DELETE SET NULL,
    revision                 INTEGER NOT NULL CHECK(revision >= 1),
    action_fingerprint       TEXT NOT NULL,
    idempotency_key          TEXT NOT NULL,
    status                   TEXT NOT NULL CHECK(status IN (
                                 'not_started', 'started', 'committed',
                                 'unknown', 'reconciled', 'cancelled')),
    external_identity_digest TEXT,
    summary_json             TEXT,
    created_at               INTEGER NOT NULL,
    observed_at              INTEGER NOT NULL,
    UNIQUE(objective_id, revision, action_fingerprint, idempotency_key)
);
CREATE INDEX IF NOT EXISTS idx_side_effect_receipts_objective
    ON side_effect_receipts(objective_id, action_fingerprint);
