-- M235: cross-session long-term memory. repo_identity is '' for user scope, so
-- the partial unique index below can merge duplicates in both scopes.
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    content_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('preference','project_fact','decision')),
    scope TEXT NOT NULL CHECK (scope IN ('user','project')),
    repo_identity TEXT NOT NULL DEFAULT '',
    repo_label TEXT NOT NULL DEFAULT '',
    source_session_id TEXT NOT NULL,
    source_message_id TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','superseded')),
    superseded_by TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_used_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS memories_active_content
    ON memories(scope, repo_identity, content_key) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS memories_active_scope
    ON memories(status, scope, repo_identity);
CREATE TABLE IF NOT EXISTS memory_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL
);
