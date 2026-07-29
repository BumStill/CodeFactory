CREATE TABLE IF NOT EXISTS chat_task_segments (
    id                  TEXT PRIMARY KEY,
    session_id          TEXT NOT NULL,
    ordinal             INTEGER NOT NULL,
    title               TEXT NOT NULL,
    status              TEXT NOT NULL,
    goal_root_turn_id   TEXT NOT NULL,
    previous_segment_id TEXT,
    checkpoint_json     TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE(session_id, ordinal),
    UNIQUE(session_id, goal_root_turn_id),
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS chat_turn_state (
    root_turn_id         TEXT PRIMARY KEY,
    session_id           TEXT NOT NULL,
    task_segment_id      TEXT,
    revision             INTEGER NOT NULL DEFAULT 1,
    phase                TEXT NOT NULL,
    status               TEXT NOT NULL,
    recovery_attempt     INTEGER NOT NULL DEFAULT 0,
    current_step_id      TEXT,
    next_step            TEXT,
    recent_activity_kind TEXT,
    recent_activity_label TEXT,
    waiting_reason       TEXT,
    started_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL,
    completed_at         INTEGER,
    terminal_reason      TEXT,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY(task_segment_id) REFERENCES chat_task_segments(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS task_attempts (
    id                   TEXT PRIMARY KEY,
    task_id              TEXT NOT NULL,
    attempt_index        INTEGER NOT NULL,
    sub_session_id       TEXT,
    status               TEXT NOT NULL,
    failure_code         TEXT,
    started_at           TEXT NOT NULL,
    completed_at         TEXT,
    error                TEXT,
    result               TEXT,
    verification_results TEXT,
    UNIQUE(task_id, attempt_index),
    UNIQUE(sub_session_id),
    FOREIGN KEY(task_id) REFERENCES task_runs(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_chat_task_segments_session
    ON chat_task_segments(session_id, ordinal);
CREATE INDEX IF NOT EXISTS idx_chat_turn_state_session
    ON chat_turn_state(session_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_task_attempts_task
    ON task_attempts(task_id, attempt_index);
