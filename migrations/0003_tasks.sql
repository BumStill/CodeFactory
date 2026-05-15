-- SPDX-License-Identifier: Apache-2.0
-- Phase 2: Task tree + subagent pool persistence.
-- Each task_run is one unit of work executed by a subagent.
-- task_dependencies is an adjacency list: (task_id, depends_on_task_id).
-- A child session may be spawned per subagent; we add parent_session_id to sessions
-- so we can filter sub-sessions out of the main session list.

ALTER TABLE sessions ADD COLUMN parent_session_id TEXT;

CREATE TABLE IF NOT EXISTS task_runs (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',  -- pending | running | completed | failed | cancelled
    cwd TEXT NOT NULL,
    parent_task_id TEXT,
    sub_session_id TEXT,                     -- session created for the subagent run, if any
    created_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    result TEXT,                              -- JSON: { summary, files_changed, tool_calls_count, ... }
    error TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (parent_task_id) REFERENCES task_runs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS task_dependencies (
    task_id TEXT NOT NULL,
    depends_on_task_id TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on_task_id),
    FOREIGN KEY (task_id) REFERENCES task_runs(id) ON DELETE CASCADE,
    FOREIGN KEY (depends_on_task_id) REFERENCES task_runs(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_task_runs_session ON task_runs(session_id);
CREATE INDEX IF NOT EXISTS idx_task_runs_status ON task_runs(status);
CREATE INDEX IF NOT EXISTS idx_task_runs_parent ON task_runs(parent_task_id);
CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);
