-- SPDX-License-Identifier: Apache-2.0
-- Token cost tracking per AI response
CREATE TABLE IF NOT EXISTS cost_entries (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL,
    model         TEXT NOT NULL,
    endpoint      TEXT NOT NULL DEFAULT '',
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd      REAL    NOT NULL DEFAULT 0.0,
    created_at    TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS cost_entries_session ON cost_entries (session_id);
CREATE INDEX IF NOT EXISTS cost_entries_date    ON cost_entries (created_at);
