-- A system-owned recovery ceiling is scoped to one user-authorized recovery
-- generation. Historical attempts remain immutable audit evidence, but an
-- explicit user reprompt after technical exhaustion starts a fresh bounded
-- generation on the same opaque business Objective.

ALTER TABLE objectives
    ADD COLUMN recovery_generation INTEGER NOT NULL DEFAULT 0
    CHECK(recovery_generation >= 0);

ALTER TABLE objective_remediations
    ADD COLUMN recovery_generation INTEGER NOT NULL DEFAULT 0
    CHECK(recovery_generation >= 0);

CREATE INDEX IF NOT EXISTS idx_objective_remediations_generation
    ON objective_remediations(objective_id, recovery_generation, failure_signature);
