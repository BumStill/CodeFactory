-- R1/R7: one small authority table for thin sessions. Legacy objective rows
-- and their remediation/decision/binding tables are never projected as thin
-- state, and no legacy failure_code reaches a thin session.
CREATE TABLE IF NOT EXISTS thin_session_state (
    session_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('running', 'waiting_wake', 'waiting_user', 'ended')),
    waiting_for TEXT,
    wake_source TEXT,
    question TEXT,
    reason TEXT,
    outcome_location TEXT,
    failure_signature TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);
