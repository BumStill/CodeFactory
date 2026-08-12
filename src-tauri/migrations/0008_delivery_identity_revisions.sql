-- Auditable, receipt-backed revisions of the expected delivery head.
-- A lease grants reconciliation ownership, but cannot by itself rewrite the
-- objective/repo/worktree/change-set identity selected before side effects.

-- Epoch zero denotes legacy rows that have not yet been claimed under the
-- fenced DeliveryRun protocol. A takeover increments claim_epoch but leaves
-- reconciled_claim_epoch behind until read-only reconciliation succeeds.
ALTER TABLE delivery_runs
    ADD COLUMN claim_epoch INTEGER NOT NULL DEFAULT 0
        CHECK(claim_epoch >= 0);

ALTER TABLE delivery_runs
    ADD COLUMN reconciled_claim_epoch INTEGER NOT NULL DEFAULT 0
        CHECK(reconciled_claim_epoch >= 0 AND reconciled_claim_epoch <= claim_epoch);

CREATE TABLE IF NOT EXISTS delivery_identity_revisions (
    receipt_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    repo_identity TEXT NOT NULL,
    worktree_identity TEXT NOT NULL,
    previous_expected_head_sha TEXT NOT NULL,
    previous_change_set_digest TEXT NOT NULL,
    next_expected_head_sha TEXT NOT NULL,
    next_change_set_digest TEXT NOT NULL,
    process_instance TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(run_id) REFERENCES delivery_runs(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_delivery_identity_revisions_run
    ON delivery_identity_revisions(run_id, created_at);

-- Every external delivery write is preceded by a committed intent. A
-- started/unknown row survives timeout, lease loss, and process restart so a
-- takeover can only observe and reconcile the exact operation before replay.
CREATE TABLE IF NOT EXISTS delivery_mutation_intents (
    intent_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    claim_epoch INTEGER NOT NULL CHECK(claim_epoch > 0),
    rung TEXT NOT NULL CHECK(rung <> ''),
    operation_key TEXT NOT NULL CHECK(operation_key <> ''),
    status TEXT NOT NULL CHECK(status IN (
        'started', 'committed', 'unknown', 'reconciled_committed'
    )),
    process_instance TEXT NOT NULL CHECK(process_instance <> ''),
    evidence_json TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(run_id) REFERENCES delivery_runs(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_delivery_mutation_intents_run
    ON delivery_mutation_intents(run_id, started_at);

CREATE UNIQUE INDEX IF NOT EXISTS idx_delivery_mutation_intents_one_unresolved
    ON delivery_mutation_intents(run_id)
    WHERE status IN ('started', 'unknown');

-- The same externally-visible effect may not be dispatched twice merely
-- because the process crashed after settling the intent but before projecting
-- business progress.
CREATE UNIQUE INDEX IF NOT EXISTS idx_delivery_mutation_intents_operation
    ON delivery_mutation_intents(run_id, rung, operation_key);
