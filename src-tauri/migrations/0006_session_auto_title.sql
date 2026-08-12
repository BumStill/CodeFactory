-- Existing sessions predate automatic semantic titles. Mark them as legacy so
-- an upgrade never treats a user-visible title as an overwriteable placeholder.
ALTER TABLE sessions ADD COLUMN title_source TEXT NOT NULL DEFAULT 'legacy';

-- Cross-process single-flight lease. A crashed lease is recoverable after the
-- bounded title deadline; the session row remains the title truth source.
CREATE TABLE IF NOT EXISTS session_title_jobs (
    session_id TEXT PRIMARY KEY,
    lease_id TEXT NOT NULL,
    started_at INTEGER NOT NULL
);

-- Operational telemetry stores only stable metadata. Prompt text and generated
-- titles are deliberately absent, including on Provider failures.
CREATE TABLE IF NOT EXISTS session_title_attempts (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL,
    failure_code TEXT,
    duration_ms INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_session_title_attempts_session_created
    ON session_title_attempts(session_id, created_at DESC);
