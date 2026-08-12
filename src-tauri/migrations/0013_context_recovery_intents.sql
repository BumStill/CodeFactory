-- SPDX-License-Identifier: Apache-2.0
-- One-shot authorization for a Context recovery episode. The source terminal
-- attempt may be consumed only once across process/lease takeovers. No prompt,
-- provider payload, external identity, or secret is stored here.

CREATE TABLE IF NOT EXISTS context_recovery_intents (
    id                  TEXT PRIMARY KEY,
    objective_id        TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    objective_revision  INTEGER NOT NULL CHECK(objective_revision >= 1),
    -- objective_recovery_attempts is a compatibility table created by runtime
    -- bootstrap on older databases, so this identity is transaction-verified
    -- by the writer instead of using a cross-optional-table foreign key.
    source_attempt_id   TEXT NOT NULL,
    remediation_id      TEXT NOT NULL REFERENCES objective_remediations(id) ON DELETE CASCADE,
    binding_id          TEXT NOT NULL REFERENCES objective_bindings(id) ON DELETE CASCADE,
    resource_generation INTEGER NOT NULL CHECK(resource_generation >= 1),
    resume_cursor       TEXT NOT NULL CHECK(resume_cursor <> ''),
    lease_owner         TEXT NOT NULL CHECK(lease_owner <> ''),
    claim_epoch         INTEGER NOT NULL CHECK(claim_epoch >= 1),
    status              TEXT NOT NULL CHECK(status IN ('started', 'settled', 'unknown')),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE(source_attempt_id),
    UNIQUE(objective_id, objective_revision, binding_id, resource_generation)
);

CREATE INDEX IF NOT EXISTS idx_context_recovery_intents_objective
    ON context_recovery_intents(objective_id, created_at DESC);

CREATE TRIGGER IF NOT EXISTS trg_context_recovery_intent_identity_immutable
BEFORE UPDATE OF objective_id, objective_revision, source_attempt_id,
                 remediation_id, binding_id, resource_generation,
                 resume_cursor, lease_owner, claim_epoch
ON context_recovery_intents
BEGIN
    SELECT RAISE(ABORT, 'context recovery intent identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_context_recovery_intent_status_forward
BEFORE UPDATE OF status ON context_recovery_intents
WHEN NOT (
    OLD.status = NEW.status OR
    (OLD.status = 'started' AND NEW.status IN ('settled', 'unknown'))
)
BEGIN
    SELECT RAISE(ABORT, 'context recovery intent status cannot move backward');
END;
