-- A user stop is session-scoped, not process-scoped. Keep the intent durable
-- until every Objective owned by the session is terminal so a restart or a
-- stale supervisor cannot resurrect work that the user explicitly stopped.

CREATE TABLE IF NOT EXISTS chat_session_cancel_intents (
    session_id   TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    status       TEXT NOT NULL CHECK(status IN ('requested', 'settled')),
    requested_at INTEGER NOT NULL,
    settled_at   INTEGER,
    updated_at   INTEGER NOT NULL,
    CHECK(status <> 'settled' OR settled_at IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_chat_session_cancel_intents_requested
    ON chat_session_cancel_intents(status, requested_at)
    WHERE status='requested';
