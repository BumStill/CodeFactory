-- SPDX-License-Identifier: Apache-2.0
-- Durable provider/auth recovery truth.  These tables deliberately record
-- observations and write-ahead intent only; wiring them to the runtime is a
-- separate adapter step.

CREATE TABLE IF NOT EXISTS provider_route_episodes (
    id                        TEXT PRIMARY KEY,
    objective_id              TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    admission_revision        INTEGER NOT NULL CHECK(admission_revision >= 1),
    last_objective_revision   INTEGER NOT NULL CHECK(last_objective_revision >= admission_revision),
    binding_id                TEXT NOT NULL REFERENCES objective_bindings(id) ON DELETE CASCADE,
    resource_generation       INTEGER NOT NULL CHECK(resource_generation >= 1),
    session_id                TEXT NOT NULL,
    root_turn_id              TEXT NOT NULL,
    policy                    TEXT NOT NULL CHECK(policy IN ('fixed', 'prefer', 'auto')),
    candidate_snapshot_digest TEXT NOT NULL,
    candidate_snapshot_json   TEXT NOT NULL,
    status                    TEXT NOT NULL CHECK(status IN (
                                  'active', 'waiting', 'completed', 'unknown', 'cancelled')),
    resume_cursor             TEXT NOT NULL,
    output_started            INTEGER NOT NULL DEFAULT 0 CHECK(output_started IN (0, 1)),
    side_effect_started       INTEGER NOT NULL DEFAULT 0 CHECK(side_effect_started IN (0, 1)),
    owner_kind                TEXT NOT NULL CHECK(owner_kind IN ('remediation', 'chat_run')),
    owner_id                  TEXT NOT NULL,
    owner_epoch               INTEGER NOT NULL CHECK(owner_epoch >= 1),
    next_observation_at       INTEGER,
    created_at                INTEGER NOT NULL,
    updated_at                INTEGER NOT NULL,
    completed_at              INTEGER,
    UNIQUE(objective_id, admission_revision, binding_id, resource_generation, id)
);
CREATE INDEX IF NOT EXISTS idx_provider_route_episodes_objective
    ON provider_route_episodes(objective_id, updated_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_provider_route_episodes_live_binding
    ON provider_route_episodes(objective_id, binding_id, resource_generation)
    WHERE status IN ('active', 'waiting', 'unknown');

CREATE TABLE IF NOT EXISTS provider_route_attempts (
    id                  TEXT PRIMARY KEY,
    episode_id          TEXT NOT NULL REFERENCES provider_route_episodes(id) ON DELETE CASCADE,
    objective_id        TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    objective_revision  INTEGER NOT NULL CHECK(objective_revision >= 1),
    binding_id          TEXT NOT NULL REFERENCES objective_bindings(id) ON DELETE CASCADE,
    resource_generation INTEGER NOT NULL CHECK(resource_generation >= 1),
    attempt_order       INTEGER NOT NULL CHECK(attempt_order >= 1),
    endpoint            TEXT NOT NULL,
    model               TEXT NOT NULL,
    request_digest      TEXT NOT NULL,
    resume_cursor       TEXT NOT NULL,
    status              TEXT NOT NULL CHECK(status IN (
                            'prepared', 'in_flight', 'streaming', 'response_committed',
                            'failed_replayable', 'failed_fatal', 'unknown', 'cancelled')),
    failure_class       TEXT,
    failure_code        TEXT,
    output_started      INTEGER NOT NULL DEFAULT 0 CHECK(output_started IN (0, 1)),
    side_effect_started INTEGER NOT NULL DEFAULT 0 CHECK(side_effect_started IN (0, 1)),
    owner_kind          TEXT NOT NULL CHECK(owner_kind IN ('remediation', 'chat_run')),
    owner_id            TEXT NOT NULL,
    owner_epoch         INTEGER NOT NULL CHECK(owner_epoch >= 1),
    response_digest     TEXT,
    canonical_message_id TEXT,
    side_effect_receipt_id TEXT REFERENCES side_effect_receipts(id) ON DELETE SET NULL,
    created_at          INTEGER NOT NULL,
    started_at          INTEGER,
    observed_at         INTEGER,
    completed_at        INTEGER,
    UNIQUE(episode_id, attempt_order)
);
CREATE INDEX IF NOT EXISTS idx_provider_route_attempts_episode
    ON provider_route_attempts(episode_id, attempt_order DESC);
CREATE INDEX IF NOT EXISTS idx_provider_route_attempts_objective
    ON provider_route_attempts(objective_id, objective_revision, created_at DESC);

CREATE TABLE IF NOT EXISTS provider_output_checkpoints (
    attempt_id         TEXT PRIMARY KEY REFERENCES provider_route_attempts(id) ON DELETE CASCADE,
    objective_id       TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    objective_revision INTEGER NOT NULL CHECK(objective_revision >= 1),
    state              TEXT NOT NULL CHECK(state IN ('partial', 'committed', 'unknown')),
    content            TEXT NOT NULL,
    content_digest     TEXT NOT NULL,
    chunk_count        INTEGER NOT NULL CHECK(chunk_count >= 0),
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS auth_capability_receipts (
    id                TEXT PRIMARY KEY,
    objective_id      TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    objective_revision INTEGER NOT NULL CHECK(objective_revision >= 1),
    request_key       TEXT NOT NULL,
    provider          TEXT NOT NULL,
    credential_ref    TEXT NOT NULL,
    capability_digest TEXT NOT NULL,
    status            TEXT NOT NULL CHECK(status IN ('ready', 'missing', 'expired', 'unknown')),
    source            TEXT NOT NULL CHECK(source IN ('callback', 'startup', 'adapter')),
    observed_at       INTEGER NOT NULL,
    created_at        INTEGER NOT NULL,
    UNIQUE(objective_id, objective_revision, request_key, status, capability_digest)
);
CREATE INDEX IF NOT EXISTS idx_auth_capability_receipts_objective
    ON auth_capability_receipts(objective_id, objective_revision, observed_at DESC);
