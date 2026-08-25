-- SPDX-License-Identifier: Apache-2.0
-- Capability-versioned reactivation for parked system-owned incidents.

CREATE TABLE IF NOT EXISTS recovery_capabilities (
    domain          TEXT PRIMARY KEY,
    revision        INTEGER NOT NULL CHECK(revision >= 1),
    contract_digest TEXT NOT NULL,
    executable      INTEGER NOT NULL CHECK(executable IN (0, 1)),
    updated_at      INTEGER NOT NULL
);

ALTER TABLE objective_incidents ADD COLUMN domain TEXT NOT NULL DEFAULT 'chat';
ALTER TABLE objective_incidents ADD COLUMN blocked_capability_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE objective_incidents ADD COLUMN reactivation_status TEXT NOT NULL DEFAULT 'waiting_capability';
ALTER TABLE objective_incidents ADD COLUMN reactivation_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE objective_incidents ADD COLUMN last_reactivated_revision INTEGER;

ALTER TABLE objective_remediations ADD COLUMN execution_attempt_index INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_objective_incidents_reactivation
    ON objective_incidents(status, reactivation_status, domain,
                           blocked_capability_revision, updated_at);
