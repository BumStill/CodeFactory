-- SPDX-License-Identifier: Apache-2.0
CREATE TABLE IF NOT EXISTS sessions (
    id                  TEXT    PRIMARY KEY,
    title               TEXT    NOT NULL,
    cwd                 TEXT    NOT NULL,
    model_id            TEXT    NOT NULL,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    total_input_tokens  INTEGER NOT NULL DEFAULT 0,
    total_output_tokens INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS messages (
    id          TEXT    PRIMARY KEY,
    session_id  TEXT    NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT    NOT NULL,   -- user | assistant | tool | system
    content     TEXT    NOT NULL,   -- plain text or JSON for tool messages
    model_id    TEXT,
    input_tokens  INTEGER,
    output_tokens INTEGER,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tool_calls (
    id          TEXT    PRIMARY KEY,
    message_id  TEXT    NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    tool_name   TEXT    NOT NULL,
    arguments   TEXT    NOT NULL,   -- JSON
    result      TEXT,               -- JSON
    status      TEXT    NOT NULL,   -- pending | approved | denied | done | error
    error       TEXT,
    duration_ms INTEGER,
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_tool_calls_message ON tool_calls(message_id);
