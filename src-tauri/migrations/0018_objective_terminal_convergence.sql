-- SPDX-License-Identifier: Apache-2.0
-- P0 terminal convergence and system-owned incident parking.

ALTER TABLE chat_turn_state ADD COLUMN terminal_revision INTEGER;
ALTER TABLE chat_turn_state ADD COLUMN visible_final_message_id TEXT;
ALTER TABLE chat_turn_state ADD COLUMN visible_final_kind TEXT;
ALTER TABLE chat_turn_state ADD COLUMN next_action TEXT;
ALTER TABLE chat_turn_state ADD COLUMN objective_revision INTEGER;
ALTER TABLE objectives ADD COLUMN attention_request_json TEXT;

CREATE TABLE IF NOT EXISTS objective_incidents (
    id                 TEXT PRIMARY KEY,
    objective_id       TEXT NOT NULL UNIQUE REFERENCES objectives(id) ON DELETE CASCADE,
    status             TEXT NOT NULL CHECK(status IN ('open', 'resolved')),
    failure_code       TEXT NOT NULL,
    failure_signature  TEXT,
    owner              TEXT NOT NULL,
    resume_cursor      TEXT,
    opened_at          INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    resolved_at        INTEGER
);
CREATE INDEX IF NOT EXISTS idx_objective_incidents_status
    ON objective_incidents(status, updated_at);
