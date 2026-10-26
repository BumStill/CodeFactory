-- M185 synthetic v1.91.44 message/session shape, frozen before MEM-A.
-- Historical ledger intentionally differs from today's migration 1 checksum.
CREATE TABLE _sqlx_migrations (
 version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
 success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL
);
INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time)
 VALUES(1,'historical init',1,X'00',0);
CREATE TABLE sessions (
 id TEXT PRIMARY KEY, title TEXT NOT NULL, cwd TEXT NOT NULL, model_id TEXT NOT NULL,
 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
 total_input_tokens INTEGER NOT NULL DEFAULT 0, total_output_tokens INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE messages (
 id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 role TEXT NOT NULL, content TEXT NOT NULL, endpoint_id TEXT, model_id TEXT,
 input_tokens INTEGER, output_tokens INTEGER, created_at INTEGER NOT NULL,
 tool_calls TEXT, reasoning_content TEXT, completion_state TEXT, usage_request_id TEXT
);
CREATE INDEX idx_messages_session ON messages(session_id,created_at);
