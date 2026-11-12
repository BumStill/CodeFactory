-- SPDX-License-Identifier: Apache-2.0
-- Content observations only: no execution grant, recovery state, or scheduler.
CREATE TABLE IF NOT EXISTS session_content_epochs (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL CHECK(epoch >= 0)
);
CREATE TRIGGER IF NOT EXISTS trg_session_content_permission_epoch
AFTER UPDATE OF permission_mode ON sessions
WHEN OLD.permission_mode IS NOT NEW.permission_mode
BEGIN
    INSERT INTO session_content_epochs(session_id,epoch) VALUES(NEW.id,1)
    ON CONFLICT(session_id) DO UPDATE SET epoch=epoch+1;
END;
CREATE TABLE IF NOT EXISTS workspace_content_intents (
    dispatch_id TEXT PRIMARY KEY,
    objective_id TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES execution_workspaces(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    tool_call_id TEXT NOT NULL REFERENCES tool_calls(id) ON DELETE CASCADE,
    run_instance_id TEXT NOT NULL REFERENCES chat_run_controls(run_instance_id),
    scope_json TEXT NOT NULL,
    action_fingerprint TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    precondition_digest TEXT NOT NULL,
    expected_digest TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_workspace_content_objective
ON workspace_content_intents(objective_id,workspace_id,created_at);
CREATE TABLE IF NOT EXISTS workspace_content_results (
    dispatch_id TEXT PRIMARY KEY REFERENCES workspace_content_intents(dispatch_id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK(state IN ('applied','conflict','unknown')),
    actual_success INTEGER NOT NULL CHECK(actual_success IN (0,1)),
    observed_digest TEXT,
    finished_at INTEGER NOT NULL,
    CHECK(state != 'applied' OR (actual_success=1 AND observed_digest IS NOT NULL))
);
CREATE TRIGGER IF NOT EXISTS trg_workspace_content_intents_immutable
BEFORE UPDATE ON workspace_content_intents BEGIN
    SELECT RAISE(ABORT,'content dispatch intents are immutable');
END;
CREATE TRIGGER IF NOT EXISTS trg_workspace_content_results_immutable
BEFORE UPDATE ON workspace_content_results BEGIN
    SELECT RAISE(ABORT,'content dispatch observations are immutable');
END;
