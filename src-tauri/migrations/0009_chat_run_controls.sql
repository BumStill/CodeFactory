-- A stop click must survive a crash even when it races chat setup before the
-- opaque Objective binding exists. The process-local AtomicBool remains a
-- fast cooperative signal; this row is the durable, exact run-instance truth.

CREATE TABLE IF NOT EXISTS chat_run_controls (
    run_instance_id          TEXT PRIMARY KEY,
    session_id               TEXT NOT NULL,
    root_turn_id             TEXT,
    objective_id             TEXT REFERENCES objectives(id) ON DELETE SET NULL,
    objective_revision       INTEGER CHECK(objective_revision IS NULL OR objective_revision >= 1),
    status                   TEXT NOT NULL CHECK(status IN (
                                 'active', 'cancel_requested',
                                 'cancelled', 'completed')),
    created_process_instance TEXT NOT NULL,
    cancel_requested_at      INTEGER,
    settled_at               INTEGER,
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL,
    CHECK(
      status <> 'cancel_requested' OR cancel_requested_at IS NOT NULL
    ),
    CHECK(
      status NOT IN ('cancelled', 'completed') OR settled_at IS NOT NULL
    ),
    CHECK(
      objective_id IS NULL OR (
        root_turn_id IS NOT NULL AND root_turn_id <> ''
        AND objective_revision IS NOT NULL
      )
    )
);

CREATE INDEX IF NOT EXISTS idx_chat_run_controls_session
    ON chat_run_controls(session_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_chat_run_controls_cancel_requested
    ON chat_run_controls(status, cancel_requested_at)
    WHERE status = 'cancel_requested';
CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_run_controls_live_session
    ON chat_run_controls(session_id)
    WHERE status IN ('active', 'cancel_requested');
