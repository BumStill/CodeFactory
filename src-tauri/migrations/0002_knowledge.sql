CREATE TABLE IF NOT EXISTS knowledge_libraries (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    root_path     TEXT NOT NULL UNIQUE,
    enabled       INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL,
    last_scan_at  TEXT,
    scan_status   TEXT NOT NULL DEFAULT 'idle',
    include_globs TEXT,
    exclude_globs TEXT
);

CREATE TABLE IF NOT EXISTS knowledge_documents (
    id         TEXT PRIMARY KEY,
    library_id TEXT NOT NULL,
    path       TEXT NOT NULL,
    kind       TEXT NOT NULL,
    hash       TEXT NOT NULL,
    mtime      INTEGER NOT NULL,
    size       INTEGER NOT NULL,
    title      TEXT,
    status     TEXT NOT NULL,
    error      TEXT,
    updated_at TEXT NOT NULL,
    UNIQUE(library_id, path),
    FOREIGN KEY(library_id) REFERENCES knowledge_libraries(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS knowledge_chunks (
    id             TEXT PRIMARY KEY,
    document_id    TEXT NOT NULL,
    chunk_index    INTEGER NOT NULL,
    content_type   TEXT NOT NULL,
    text           TEXT NOT NULL,
    page           INTEGER,
    slide          INTEGER,
    heading        TEXT,
    token_estimate INTEGER NOT NULL,
    metadata_json  TEXT NOT NULL DEFAULT '{}',
    FOREIGN KEY(document_id) REFERENCES knowledge_documents(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS retrieval_events (
    id               TEXT PRIMARY KEY,
    session_id       TEXT,
    task_id          TEXT,
    query            TEXT NOT NULL,
    filters_json     TEXT NOT NULL DEFAULT '{}',
    result_refs_json TEXT NOT NULL DEFAULT '[]',
    created_at       TEXT NOT NULL,
    latency_ms       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_knowledge_documents_library ON knowledge_documents(library_id);
CREATE INDEX IF NOT EXISTS idx_knowledge_documents_status ON knowledge_documents(status);
CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_document ON knowledge_chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_retrieval_events_created ON retrieval_events(created_at);
