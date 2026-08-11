-- Stable objective/worktree identity and recovery observability.
-- Existing rows remain nullable and are failed closed as legacy_orphan by the
-- recovery planner; migration must never guess authority for old side effects.
ALTER TABLE delivery_runs ADD COLUMN objective_id TEXT;
ALTER TABLE delivery_runs ADD COLUMN worktree_identity TEXT;
ALTER TABLE delivery_runs ADD COLUMN created_app_version TEXT;
ALTER TABLE delivery_runs ADD COLUMN created_app_build TEXT;
ALTER TABLE delivery_runs ADD COLUMN created_process_instance TEXT;
ALTER TABLE delivery_runs ADD COLUMN last_observed_app_version TEXT;
ALTER TABLE delivery_runs ADD COLUMN last_observed_app_build TEXT;
ALTER TABLE delivery_runs ADD COLUMN last_observed_process_instance TEXT;
ALTER TABLE delivery_runs ADD COLUMN recovery_attempt INTEGER NOT NULL DEFAULT 0 CHECK(recovery_attempt >= 0);
ALTER TABLE delivery_runs ADD COLUMN failure_code TEXT;
ALTER TABLE delivery_runs ADD COLUMN failure_class TEXT;
ALTER TABLE delivery_runs ADD COLUMN queue_wait_ms INTEGER;
ALTER TABLE delivery_runs ADD COLUMN runtime_ms INTEGER;
ALTER TABLE delivery_runs ADD COLUMN remediation_id TEXT;
ALTER TABLE chat_turn_state ADD COLUMN user_reprompt_driver TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_delivery_runs_active_objective_repo
    ON delivery_runs(objective_id, repo_identity)
    WHERE objective_id IS NOT NULL
      AND objective_id <> ''
      AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected');

CREATE TABLE IF NOT EXISTS objective_recovery_attempts (
    id                    TEXT PRIMARY KEY,
    objective_id          TEXT,
    root_turn_id          TEXT,
    delivery_run_id       TEXT,
    domain                TEXT NOT NULL,
    attempt_index         INTEGER NOT NULL CHECK(attempt_index >= 1),
    failure_code          TEXT NOT NULL,
    failure_class         TEXT NOT NULL,
    output_started        INTEGER NOT NULL DEFAULT 0 CHECK(output_started IN (0, 1)),
    side_effect_started   INTEGER NOT NULL DEFAULT 0 CHECK(side_effect_started IN (0, 1)),
    queue_wait_ms         INTEGER,
    runtime_ms            INTEGER,
    process_instance      TEXT NOT NULL,
    resume_owner          TEXT NOT NULL,
    terminal_decision     TEXT NOT NULL,
    created_at            INTEGER NOT NULL,
    FOREIGN KEY(delivery_run_id) REFERENCES delivery_runs(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_objective_recovery_attempts_objective
    ON objective_recovery_attempts(objective_id, created_at);
CREATE INDEX IF NOT EXISTS idx_objective_recovery_attempts_turn
    ON objective_recovery_attempts(root_turn_id, created_at);
