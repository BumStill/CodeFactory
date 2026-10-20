-- Phase 2 R2: one tool-level monitor hint per root turn; no transcript notice.
CREATE TABLE IF NOT EXISTS thin_wait_notes (
    session_id TEXT PRIMARY KEY,
    root_turn_id TEXT,
    waiting_for TEXT NOT NULL,
    hinted INTEGER NOT NULL DEFAULT 0
);
