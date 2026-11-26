-- M287: one active liveness owner per unfinished session, plus structured
-- violation/recovery events used by the supervisor and read-only metrics.
CREATE TABLE IF NOT EXISTS session_liveness_leases (
    session_id TEXT PRIMARY KEY NOT NULL,
    objective_id TEXT,
    root_turn_id TEXT,
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    process_instance TEXT,
    lease_generation INTEGER NOT NULL DEFAULT 1,
    started_at INTEGER NOT NULL,
    heartbeat_at INTEGER NOT NULL,
    deadline_at INTEGER NOT NULL,
    not_before_at INTEGER,
    channel_alive INTEGER,
    display_kind TEXT NOT NULL,
    display_summary TEXT NOT NULL,
    recovery_action TEXT,
    recovery_attempt INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'settled')),
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_session_liveness_leases_deadline
    ON session_liveness_leases(status, deadline_at);

CREATE TABLE IF NOT EXISTS session_liveness_events (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL,
    objective_id TEXT,
    owner_kind TEXT,
    violation_kind TEXT NOT NULL,
    idle_ms INTEGER NOT NULL,
    deadline_at INTEGER,
    detected_at INTEGER NOT NULL,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL,
    resolved_at INTEGER,
    detail_code TEXT
);

CREATE INDEX IF NOT EXISTS idx_session_liveness_events_detected
    ON session_liveness_events(detected_at, violation_kind);
