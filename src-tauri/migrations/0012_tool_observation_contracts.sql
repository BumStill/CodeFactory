-- Durable, privacy-preserving observation contracts for generic tool side effects.
--
-- The linked receipt remains the mutation fence.  This table records only the
-- minimum information needed to observe a post-crash local resource: a
-- workspace-relative locator plus pre/post SHA-256 digests.  Raw tool
-- arguments, file content, secrets and external identifiers are forbidden.
CREATE TABLE IF NOT EXISTS side_effect_observation_contracts (
    receipt_id                       TEXT PRIMARY KEY
                                             REFERENCES side_effect_receipts(id)
                                             ON DELETE CASCADE,
    objective_id                     TEXT NOT NULL
                                             REFERENCES objectives(id)
                                             ON DELETE CASCADE,
    binding_id                       TEXT
                                             REFERENCES objective_bindings(id)
                                             ON DELETE SET NULL,
    action_fingerprint               TEXT NOT NULL,
    operation_domain                 TEXT NOT NULL CHECK(operation_domain IN ('tool_file')),
    observer_kind                    TEXT NOT NULL CHECK(observer_kind IN ('file_content_sha256_v1')),
    safe_locator_json                TEXT NOT NULL,
    precondition_digest              TEXT NOT NULL,
    expected_postcondition_digest    TEXT NOT NULL,
    state                            TEXT NOT NULL CHECK(state IN (
                                             'applied', 'definitely_not_applied',
                                             'still_unknown', 'conflict')),
    observed_digest                  TEXT,
    last_dispatch_epoch              INTEGER NOT NULL DEFAULT 0 CHECK(last_dispatch_epoch >= 0),
    observation_count                INTEGER NOT NULL DEFAULT 0 CHECK(observation_count >= 0),
    created_at                       INTEGER NOT NULL,
    observed_at                      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_side_effect_observation_objective
    ON side_effect_observation_contracts(objective_id, state, observed_at);

CREATE INDEX IF NOT EXISTS idx_side_effect_observation_binding
    ON side_effect_observation_contracts(binding_id, state, observed_at);
