// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeLibrary {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub enabled: bool,
    pub created_at: String,
    pub last_scan_at: Option<String>,
    pub scan_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeChunkDetail {
    pub id: String,
    pub document_id: String,
    pub chunk_index: i64,
    pub content_type: String,
    pub text: String,
    pub page: Option<i64>,
    pub slide: Option<i64>,
    pub heading: Option<String>,
    pub token_estimate: i64,
    pub metadata_json: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeScanSummary {
    pub library_id: String,
    /// Files that matched a supported kind and were visited in this pass.
    pub scanned_files: usize,
    /// Documents whose chunks were (re)written in this pass.
    pub indexed_documents: usize,
    pub failed_documents: usize,
    pub chunks_indexed: usize,
    /// Documents that did not exist in the index before this pass.
    pub added_documents: usize,
    /// Documents whose content hash changed and were re-chunked.
    pub updated_documents: usize,
    /// Documents skipped because their bytes are already indexed.
    pub unchanged_documents: usize,
    /// Index rows dropped because the file no longer exists on disk.
    pub removed_documents: usize,
    /// True when this call merged into a scan that was already running.
    pub coalesced: bool,
}

impl KnowledgeScanSummary {
    fn new(library_id: &str) -> Self {
        Self {
            library_id: library_id.to_string(),
            ..Default::default()
        }
    }
}

/// One document that could not be indexed, surfaced so the knowledge UI can
/// show *which* file failed and why instead of only a count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDocumentFailure {
    pub path: String,
    pub kind: String,
    pub error: String,
    pub updated_at: String,
}

/// How indexing references a file on disk: cheap metadata first, content hash
/// only when the metadata moved. `skipped` means "already indexed, cost zero".
const DOCUMENT_STATE_SELECT: &str =
    "SELECT id, hash, mtime, size, status, error FROM knowledge_documents
     WHERE library_id = ? AND path = ?";

#[derive(Debug, Clone)]
struct DocumentState {
    id: String,
    hash: String,
    mtime: i64,
    size: i64,
    status: String,
}

enum IndexOutcome {
    /// Chunks were written; `added` distinguishes a new file from a re-index.
    Parsed { chunks: usize, added: bool },
    /// Bytes are identical to the indexed copy, only bookkeeping was refreshed.
    Unchanged,
    /// Metadata was identical: extraction never ran.
    Skipped,
}

/// Counts real extraction passes per library. Incremental behaviour is only
/// trustworthy if we can prove a rescan did *not* parse an unchanged file
/// (M191 R3); per-library keys keep that proof immune to parallel tests.
static PARSE_COUNTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, usize>>> =
    std::sync::OnceLock::new();

fn record_parse(library_id: &str) {
    let counts = PARSE_COUNTS.get_or_init(Default::default);
    let mut counts = match counts.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *counts.entry(library_id.to_string()).or_insert(0) += 1;
}

/// Document extraction passes for one library. Test-only: it is how "a rescan
/// really skipped extraction" becomes machine-checkable.
#[cfg(test)]
pub fn parse_count_for(library_id: &str) -> usize {
    let counts = PARSE_COUNTS.get_or_init(Default::default);
    let counts = match counts.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    counts.get(library_id).copied().unwrap_or(0)
}

#[derive(Default)]
struct ScanSlot {
    /// A scan was requested while one was already running: run one more pass
    /// after the current one so no change is silently dropped.
    rerun: bool,
}

fn scan_slots() -> &'static std::sync::Mutex<std::collections::HashMap<String, ScanSlot>> {
    static SLOTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, ScanSlot>>,
    > = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn lock_scan_slots() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, ScanSlot>>
{
    match scan_slots().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Exclusive per-library scan ownership. Dropping releases the slot, so an
/// aborted or panicking scan can never wedge auto-indexing (M191 R5).
struct ScanLease {
    library_id: String,
}

impl ScanLease {
    fn acquire(library_id: &str) -> Option<Self> {
        let mut slots = lock_scan_slots();
        if let Some(slot) = slots.get_mut(library_id) {
            slot.rerun = true;
            return None;
        }
        slots.insert(library_id.to_string(), ScanSlot::default());
        Some(Self {
            library_id: library_id.to_string(),
        })
    }

    fn take_rerun(&self) -> bool {
        let mut slots = lock_scan_slots();
        slots
            .get_mut(&self.library_id)
            .map(|slot| std::mem::replace(&mut slot.rerun, false))
            .unwrap_or(false)
    }
}

impl Drop for ScanLease {
    fn drop(&mut self) {
        lock_scan_slots().remove(&self.library_id);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSearchQuery {
    pub query: String,
    #[serde(default)]
    pub library_id: Option<String>,
    #[serde(default)]
    pub library_ids: Option<Vec<String>>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub top_k: Option<usize>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub library_id: String,
    pub path: String,
    pub kind: String,
    pub title: Option<String>,
    pub chunk_index: i64,
    pub page: Option<i64>,
    pub slide: Option<i64>,
    pub heading: Option<String>,
    pub snippet: String,
    pub score: i64,
}

#[derive(Debug, Clone)]
struct ExtractedChunk {
    text: String,
    content_type: String,
    page: Option<i64>,
    slide: Option<i64>,
    heading: Option<String>,
    metadata_json: String,
}

pub async fn ensure_schema(pool: &SqlitePool) -> crate::errors::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS knowledge_libraries (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            root_path     TEXT NOT NULL UNIQUE,
            enabled       INTEGER NOT NULL DEFAULT 1,
            created_at    TEXT NOT NULL,
            last_scan_at  TEXT,
            scan_status   TEXT NOT NULL DEFAULT 'idle',
            include_globs TEXT,
            exclude_globs TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS knowledge_documents (
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
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS knowledge_chunks (
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
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS retrieval_events (
            id               TEXT PRIMARY KEY,
            session_id       TEXT,
            task_id          TEXT,
            query            TEXT NOT NULL,
            filters_json     TEXT NOT NULL DEFAULT '{}',
            result_refs_json TEXT NOT NULL DEFAULT '[]',
            created_at       TEXT NOT NULL,
            latency_ms       INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    for index in [
        "CREATE INDEX IF NOT EXISTS idx_knowledge_documents_library ON knowledge_documents(library_id)",
        "CREATE INDEX IF NOT EXISTS idx_knowledge_documents_status ON knowledge_documents(status)",
        "CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_document ON knowledge_chunks(document_id)",
        "CREATE INDEX IF NOT EXISTS idx_retrieval_events_created ON retrieval_events(created_at)",
    ] {
        sqlx::query(index).execute(pool).await?;
    }
    Ok(())
}

pub async fn add_library(
    pool: &SqlitePool,
    name: String,
    root_path: String,
) -> crate::errors::Result<KnowledgeLibrary> {
    let root = canonical_root(&root_path)?;
    let root_path = root.to_string_lossy().to_string();
    if let Some(existing) = library_by_root(pool, &root_path).await? {
        return Ok(existing);
    }

    let now = Utc::now().to_rfc3339();
    let library = KnowledgeLibrary {
        id: Uuid::new_v4().to_string(),
        name,
        root_path,
        enabled: true,
        created_at: now,
        last_scan_at: None,
        scan_status: "idle".into(),
    };
    sqlx::query(
        "INSERT INTO knowledge_libraries
         (id, name, root_path, enabled, created_at, scan_status)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&library.id)
    .bind(&library.name)
    .bind(&library.root_path)
    .bind(1_i64)
    .bind(&library.created_at)
    .bind(&library.scan_status)
    .execute(pool)
    .await?;
    Ok(library)
}

pub async fn list_libraries(pool: &SqlitePool) -> crate::errors::Result<Vec<KnowledgeLibrary>> {
    let rows = sqlx::query(
        "SELECT id, name, root_path, enabled, created_at, last_scan_at, scan_status
         FROM knowledge_libraries ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_library).collect())
}

pub async fn set_library_enabled(
    pool: &SqlitePool,
    library_id: &str,
    enabled: bool,
) -> crate::errors::Result<()> {
    let result = sqlx::query("UPDATE knowledge_libraries SET enabled = ? WHERE id = ?")
        .bind(if enabled { 1_i64 } else { 0_i64 })
        .bind(library_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(crate::errors::AppError::Other(format!(
            "Knowledge library '{library_id}' was not found"
        )));
    }
    Ok(())
}

pub async fn delete_library(pool: &SqlitePool, library_id: &str) -> crate::errors::Result<()> {
    let result = sqlx::query("DELETE FROM knowledge_libraries WHERE id = ?")
        .bind(library_id)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(crate::errors::AppError::Other(format!(
            "Knowledge library '{library_id}' was not found"
        )));
    }
    Ok(())
}

pub async fn enabled_library_context(
    pool: &SqlitePool,
) -> crate::errors::Result<crate::storage::tasks::TaskConnectorContext> {
    let libraries = list_libraries(pool)
        .await?
        .into_iter()
        .filter(|library| library.enabled)
        .map(
            |library| crate::storage::tasks::TaskKnowledgeLibraryContext {
                id: library.id,
                name: library.name,
                root_path: library.root_path,
                scan_status: library.scan_status,
                last_scan_at: library.last_scan_at,
            },
        )
        .collect();
    Ok(crate::storage::tasks::TaskConnectorContext {
        knowledge_libraries: libraries,
    })
}

/// Bring one library index up to date with the folder on disk.
///
/// One scan per library at a time: a concurrent caller never runs a second
/// pass over the same folder, it records that another pass is needed and
/// returns immediately (M191 R5). Changes that arrive mid-scan are then picked
/// up by exactly one follow-up pass before the slot is released.
pub async fn scan_library(
    pool: &SqlitePool,
    library_id: &str,
) -> crate::errors::Result<KnowledgeScanSummary> {
    let Some(lease) = ScanLease::acquire(library_id) else {
        let mut summary = KnowledgeScanSummary::new(library_id);
        summary.coalesced = true;
        return Ok(summary);
    };

    let mut summary = scan_library_once(pool, library_id).await?;
    while lease.take_rerun() {
        summary = scan_library_once(pool, library_id).await?;
    }
    Ok(summary)
}

async fn scan_library_once(
    pool: &SqlitePool,
    library_id: &str,
) -> crate::errors::Result<KnowledgeScanSummary> {
    let library = library_by_id(pool, library_id).await?;
    let scan_started = Utc::now().to_rfc3339();
    sqlx::query("UPDATE knowledge_libraries SET scan_status = 'scanning' WHERE id = ?")
        .bind(library_id)
        .execute(pool)
        .await?;

    let mut summary = KnowledgeScanSummary::new(library_id);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in WalkDir::new(&library.root_path)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some(kind) = supported_kind(path) else {
            continue;
        };
        summary.scanned_files += 1;
        seen.insert(path.to_string_lossy().to_string());
        match index_document(pool, &library, path, kind).await {
            Ok(IndexOutcome::Parsed { chunks, added }) => {
                summary.indexed_documents += 1;
                summary.chunks_indexed += chunks;
                if added {
                    summary.added_documents += 1;
                } else {
                    summary.updated_documents += 1;
                }
            }
            Ok(IndexOutcome::Unchanged) | Ok(IndexOutcome::Skipped) => {
                summary.unchanged_documents += 1;
            }
            Err(e) => {
                summary.failed_documents += 1;
                record_failed_document(pool, &library, path, kind, &e.to_string()).await?;
            }
        }
    }

    // Files removed or renamed since the last pass must stop answering
    // retrieval: drop the document row and every chunk behind it.
    let indexed_paths: Vec<String> =
        sqlx::query_scalar("SELECT path FROM knowledge_documents WHERE library_id = ?")
            .bind(library_id)
            .fetch_all(pool)
            .await?;
    for path in indexed_paths {
        if seen.contains(&path) {
            continue;
        }
        remove_document(pool, library_id, &path).await?;
        summary.removed_documents += 1;
    }

    let status = if summary.failed_documents > 0 {
        "completed_with_errors"
    } else {
        "completed"
    };
    sqlx::query(
        "UPDATE knowledge_libraries
         SET scan_status = ?, last_scan_at = ?
         WHERE id = ?",
    )
    .bind(status)
    .bind(scan_started)
    .bind(library_id)
    .execute(pool)
    .await?;

    Ok(summary)
}

/// Drop one document and its chunks in a single transaction.
async fn remove_document(
    pool: &SqlitePool,
    library_id: &str,
    path: &str,
) -> crate::errors::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM knowledge_chunks WHERE document_id IN
         (SELECT id FROM knowledge_documents WHERE library_id = ? AND path = ?)",
    )
    .bind(library_id)
    .bind(path)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM knowledge_documents WHERE library_id = ? AND path = ?")
        .bind(library_id)
        .bind(path)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Startup repairs: a scan that was running when the process died must not
/// leave `scanning` forever, and must not leave childless/partial rows behind.
pub async fn recover_interrupted_scans(pool: &SqlitePool) -> crate::errors::Result<usize> {
    sqlx::query(
        "DELETE FROM knowledge_chunks WHERE document_id IN
         (SELECT id FROM knowledge_documents WHERE status = 'scanning')",
    )
    .execute(pool)
    .await?;
    let requeued = sqlx::query(
        "UPDATE knowledge_documents
         SET status = 'pending', error = 'Interrupted by restart; queued for re-index'
         WHERE status = 'scanning'",
    )
    .execute(pool)
    .await?;
    sqlx::query("UPDATE knowledge_libraries SET scan_status = 'idle' WHERE scan_status = 'scanning'")
        .execute(pool)
        .await?;
    Ok(requeued.rows_affected() as usize)
}

/// Startup / periodic reconciliation: index every enabled library so files
/// added while the app was closed (or the watcher missed) are picked up.
pub async fn reconcile_all_libraries(pool: &SqlitePool) -> crate::errors::Result<Vec<String>> {
    recover_interrupted_scans(pool).await?;
    let mut scanned = Vec::new();
    for library in list_libraries(pool).await?.into_iter().filter(|l| l.enabled) {
        match scan_library(pool, &library.id).await {
            Ok(_) => scanned.push(library.id),
            Err(e) => tracing::warn!("knowledge reconcile failed for {}: {e}", library.id),
        }
    }
    Ok(scanned)
}

/// Documents currently failing to index, for the knowledge page status line.
pub async fn library_failures(
    pool: &SqlitePool,
    library_id: &str,
) -> crate::errors::Result<Vec<KnowledgeDocumentFailure>> {
    let rows = sqlx::query(
        "SELECT path, kind, COALESCE(error, ''), updated_at FROM knowledge_documents
         WHERE library_id = ? AND status = 'error' ORDER BY path",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;
    let mut failures = Vec::with_capacity(rows.len());
    for row in rows {
        failures.push(KnowledgeDocumentFailure {
            path: row.try_get("path")?,
            kind: row.try_get("kind")?,
            error: row.try_get(2)?,
            updated_at: row.try_get("updated_at")?,
        });
    }
    Ok(failures)
}

pub async fn search(
    pool: &SqlitePool,
    query: KnowledgeSearchQuery,
) -> crate::errors::Result<Vec<KnowledgeSearchResult>> {
    let started = std::time::Instant::now();
    let terms = query_terms(&query.query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    let mut rows_query = String::from(
        "SELECT
            c.id AS chunk_id, c.document_id, c.chunk_index, c.text, c.page, c.slide, c.heading,
            d.library_id, d.path, d.kind, d.title
         FROM knowledge_chunks c
         JOIN knowledge_documents d ON d.id = c.document_id
         JOIN knowledge_libraries l ON l.id = d.library_id
         WHERE d.status = 'indexed' AND l.enabled = 1",
    );
    let library_ids = effective_library_ids(&query);
    if !library_ids.is_empty() {
        rows_query.push_str(" AND d.library_id IN (");
        rows_query.push_str(&vec!["?"; library_ids.len()].join(","));
        rows_query.push(')');
    }
    if query.kind.is_some() {
        rows_query.push_str(" AND d.kind = ?");
    }

    let mut q = sqlx::query(&rows_query);
    for library_id in &library_ids {
        q = q.bind(library_id);
    }
    if let Some(kind) = &query.kind {
        q = q.bind(kind);
    }

    let rows = q.fetch_all(pool).await?;
    let mut results = Vec::new();
    for row in rows {
        let text: String = row.try_get("text")?;
        let path: String = row.try_get("path")?;
        let haystack = format!("{} {}", text.to_lowercase(), path.to_lowercase());
        let score = terms
            .iter()
            .map(|term| haystack.matches(term).count() as i64)
            .sum::<i64>();
        if score == 0 {
            continue;
        }
        results.push(KnowledgeSearchResult {
            chunk_id: row.try_get("chunk_id")?,
            document_id: row.try_get("document_id")?,
            library_id: row.try_get("library_id")?,
            path,
            kind: row.try_get("kind")?,
            title: row.try_get("title")?,
            chunk_index: row.try_get("chunk_index")?,
            page: row.try_get("page")?,
            slide: row.try_get("slide")?,
            heading: row.try_get("heading")?,
            snippet: make_snippet(&text, &terms),
            score,
        });
    }
    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.chunk_index.cmp(&b.chunk_index))
    });
    let top_k = query.top_k.unwrap_or(8).clamp(1, 50);
    results.truncate(top_k);
    record_retrieval_event(pool, &query, &results, started.elapsed().as_millis() as i64).await?;
    Ok(results)
}

pub async fn get_chunk(
    pool: &SqlitePool,
    chunk_id: &str,
) -> crate::errors::Result<KnowledgeChunkDetail> {
    let row = sqlx::query(
        "SELECT id, document_id, chunk_index, content_type, text, page, slide, heading,
                token_estimate, metadata_json
         FROM knowledge_chunks WHERE id = ?",
    )
    .bind(chunk_id)
    .fetch_one(pool)
    .await?;
    Ok(KnowledgeChunkDetail {
        id: row.try_get("id")?,
        document_id: row.try_get("document_id")?,
        chunk_index: row.try_get("chunk_index")?,
        content_type: row.try_get("content_type")?,
        text: row.try_get("text")?,
        page: row.try_get("page")?,
        slide: row.try_get("slide")?,
        heading: row.try_get("heading")?,
        token_estimate: row.try_get("token_estimate")?,
        metadata_json: row.try_get("metadata_json")?,
    })
}

pub async fn chunk_library_id(pool: &SqlitePool, chunk_id: &str) -> crate::errors::Result<String> {
    let row: (String,) = sqlx::query_as(
        "SELECT d.library_id
         FROM knowledge_chunks c
         JOIN knowledge_documents d ON d.id = c.document_id
         WHERE c.id = ?",
    )
    .bind(chunk_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

pub async fn library_is_enabled(
    pool: &SqlitePool,
    library_id: &str,
) -> crate::errors::Result<bool> {
    let enabled: Option<i64> =
        sqlx::query_scalar("SELECT enabled FROM knowledge_libraries WHERE id = ?")
            .bind(library_id)
            .fetch_optional(pool)
            .await?;
    Ok(enabled.unwrap_or(0) != 0)
}

async fn library_by_root(
    pool: &SqlitePool,
    root_path: &str,
) -> crate::errors::Result<Option<KnowledgeLibrary>> {
    let row = sqlx::query(
        "SELECT id, name, root_path, enabled, created_at, last_scan_at, scan_status
         FROM knowledge_libraries WHERE root_path = ?",
    )
    .bind(root_path)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_library))
}

/// Public view of one library, used by the registration command so the UI
/// reflects `scanning` immediately after adding a folder.
pub async fn library_by_id_public(
    pool: &SqlitePool,
    id: &str,
) -> crate::errors::Result<KnowledgeLibrary> {
    library_by_id(pool, id).await
}

async fn library_by_id(pool: &SqlitePool, id: &str) -> crate::errors::Result<KnowledgeLibrary> {
    let row = sqlx::query(
        "SELECT id, name, root_path, enabled, created_at, last_scan_at, scan_status
         FROM knowledge_libraries WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row_to_library(&row))
}

fn row_to_library(row: &sqlx::sqlite::SqliteRow) -> KnowledgeLibrary {
    KnowledgeLibrary {
        id: row.try_get("id").unwrap_or_default(),
        name: row.try_get("name").unwrap_or_default(),
        root_path: row.try_get("root_path").unwrap_or_default(),
        enabled: row.try_get::<i64, _>("enabled").unwrap_or(1) != 0,
        created_at: row.try_get("created_at").unwrap_or_default(),
        last_scan_at: row
            .try_get::<Option<String>, _>("last_scan_at")
            .unwrap_or(None),
        scan_status: row.try_get("scan_status").unwrap_or_else(|_| "idle".into()),
    }
}

async fn index_document(
    pool: &SqlitePool,
    library: &KnowledgeLibrary,
    path: &Path,
    kind: &str,
) -> crate::errors::Result<IndexOutcome> {
    let metadata = std::fs::metadata(path)?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = metadata.len() as i64;
    let path_str = path.to_string_lossy().to_string();
    let existing = load_document_state(pool, &library.id, &path_str).await?;

    // Cheap path: same size and same second-resolution mtime as the indexed
    // copy means nothing was touched — no read, no extraction, no chunk churn.
    if let Some(previous) = &existing {
        if previous.status == "indexed"
            && !previous.hash.is_empty()
            && previous.mtime == mtime
            && previous.size == size
        {
            return Ok(IndexOutcome::Skipped);
        }
    }

    let bytes = std::fs::read(path)?;
    let hash = stable_hash_hex(&bytes);
    let now = Utc::now().to_rfc3339();

    // Metadata moved but bytes did not (Office re-save, copy, touch): refresh
    // bookkeeping only, so re-indexing never re-embeds identical content.
    if let Some(previous) = &existing {
        if previous.status == "indexed" && !previous.hash.is_empty() && previous.hash == hash {
            sqlx::query("UPDATE knowledge_documents SET mtime = ?, size = ?, updated_at = ? WHERE id = ?")
                .bind(mtime)
                .bind(size)
                .bind(&now)
                .bind(&previous.id)
                .execute(pool)
                .await?;
            return Ok(IndexOutcome::Unchanged);
        }
    }

    record_parse(&library.id);
    let chunks = extract_document(path, kind)?;
    if chunks.is_empty() {
        return Err(crate::errors::AppError::Other(format!(
            "No extractable text found in {}",
            path.display()
        )));
    }
    let added = existing.is_none();
    let document_id = existing
        .as_ref()
        .map(|previous| previous.id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let title = chunks
        .first()
        .map(|chunk| first_words(&chunk.text, 12))
        .filter(|s| !s.is_empty());

    // One transaction per document: a crash or write error leaves either the
    // previous complete chunk set or the new one, never half of each (M191 R4).
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO knowledge_documents
         (id, library_id, path, kind, hash, mtime, size, title, status, error, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'indexed', NULL, ?)
         ON CONFLICT(library_id, path) DO UPDATE SET
           kind = excluded.kind,
           hash = excluded.hash,
           mtime = excluded.mtime,
           size = excluded.size,
           title = excluded.title,
           status = 'indexed',
           error = NULL,
           updated_at = excluded.updated_at",
    )
    .bind(&document_id)
    .bind(&library.id)
    .bind(&path_str)
    .bind(kind)
    .bind(&hash)
    .bind(mtime)
    .bind(size)
    .bind(&title)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM knowledge_chunks WHERE document_id = ?")
        .bind(&document_id)
        .execute(&mut *tx)
        .await?;
    for (idx, chunk) in chunks.iter().enumerate() {
        sqlx::query(
            "INSERT INTO knowledge_chunks
             (id, document_id, chunk_index, content_type, text, page, slide, heading,
              token_estimate, metadata_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&document_id)
        .bind(idx as i64)
        .bind(&chunk.content_type)
        .bind(&chunk.text)
        .bind(chunk.page)
        .bind(chunk.slide)
        .bind(&chunk.heading)
        .bind(estimate_tokens(&chunk.text))
        .bind(&chunk.metadata_json)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(IndexOutcome::Parsed {
        chunks: chunks.len(),
        added,
    })
}

async fn load_document_state(
    pool: &SqlitePool,
    library_id: &str,
    path: &str,
) -> crate::errors::Result<Option<DocumentState>> {
    let row = sqlx::query(DOCUMENT_STATE_SELECT)
        .bind(library_id)
        .bind(path)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| DocumentState {
        id: row.get::<String, _>("id"),
        hash: row.get::<String, _>("hash"),
        mtime: row.get::<i64, _>("mtime"),
        size: row.get::<i64, _>("size"),
        status: row.get::<String, _>("status"),
    }))
}

async fn record_failed_document(
    pool: &SqlitePool,
    library: &KnowledgeLibrary,
    path: &Path,
    kind: &str,
    error: &str,
) -> crate::errors::Result<()> {
    let metadata = std::fs::metadata(path).ok();
    let mtime = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = metadata.map(|m| m.len() as i64).unwrap_or(0);
    let path_str = path.to_string_lossy().to_string();
    let document_id = existing_document_id(pool, &library.id, &path_str)
        .await?
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO knowledge_documents
         (id, library_id, path, kind, hash, mtime, size, title, status, error, updated_at)
         VALUES (?, ?, ?, ?, '', ?, ?, NULL, 'error', ?, ?)
         ON CONFLICT(library_id, path) DO UPDATE SET
           kind = excluded.kind,
           mtime = excluded.mtime,
           size = excluded.size,
           status = 'error',
           error = excluded.error,
           updated_at = excluded.updated_at",
    )
    .bind(&document_id)
    .bind(&library.id)
    .bind(&path_str)
    .bind(kind)
    .bind(mtime)
    .bind(size)
    .bind(error.chars().take(500).collect::<String>())
    .bind(now)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM knowledge_chunks WHERE document_id = ?")
        .bind(document_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn existing_document_id(
    pool: &SqlitePool,
    library_id: &str,
    path: &str,
) -> crate::errors::Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM knowledge_documents WHERE library_id = ? AND path = ?")
            .bind(library_id)
            .bind(path)
            .fetch_optional(pool)
            .await?,
    )
}

fn extract_document(path: &Path, kind: &str) -> crate::errors::Result<Vec<ExtractedChunk>> {
    match kind {
        "docx" => extract_docx(path),
        "pptx" => extract_pptx(path),
        "pdf" => extract_pdf(path),
        _ => Ok(Vec::new()),
    }
}

/// Extract readable plain text from a supported Office/PDF file
/// (`.docx` / `.pptx` / `.pdf`). Returns `Ok(None)` when the extension
/// isn't a supported document type so callers can fall back to byte
/// reads. Slide/page markers are inlined so the model gets structure
/// cues even though this is lossy (no per-shape addressing — use the
/// `read_pptx` tool when you need to edit).
pub fn extract_plain_text(path: &Path) -> crate::errors::Result<Option<String>> {
    let Some(kind) = supported_kind(path) else {
        return Ok(None);
    };
    let chunks = extract_document(path, kind)?;
    let mut out = String::new();
    let mut last_marker: Option<String> = None;
    for chunk in &chunks {
        let marker = match (chunk.slide, chunk.page) {
            (Some(slide), _) => Some(format!("[Slide {slide}]")),
            (_, Some(page)) => Some(format!("[Page {page}]")),
            _ => None,
        };
        if marker != last_marker {
            if let Some(m) = &marker {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(m);
                out.push('\n');
            }
            last_marker = marker;
        }
        out.push_str(&chunk.text);
        out.push('\n');
    }
    Ok(Some(out.trim().to_string()))
}

fn extract_docx(path: &Path) -> crate::errors::Result<Vec<ExtractedChunk>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| crate::errors::AppError::Other(format!("Invalid docx zip: {e}")))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| crate::errors::AppError::Other(format!("Missing word/document.xml: {e}")))?
        .read_to_string(&mut xml)?;
    let text = normalize_text(&visible_xml_text(&xml));
    Ok(split_text_chunks(&text, None, None, "body"))
}

fn extract_pptx(path: &Path) -> crate::errors::Result<Vec<ExtractedChunk>> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| crate::errors::AppError::Other(format!("Invalid pptx zip: {e}")))?;
    let mut slides = Vec::new();
    for idx in 0..archive.len() {
        let file = archive.by_index(idx).map_err(|e| {
            crate::errors::AppError::Other(format!("Could not read pptx entry: {e}"))
        })?;
        let name = file.name().to_string();
        if let Some(slide_no) = slide_number_from_name(&name) {
            slides.push((slide_no, name));
        }
    }
    slides.sort_by_key(|(slide_no, _)| *slide_no);

    let mut chunks = Vec::new();
    for (slide_no, name) in slides {
        let mut xml = String::new();
        archive
            .by_name(&name)
            .map_err(|e| {
                crate::errors::AppError::Other(format!("Could not read pptx slide {name}: {e}"))
            })?
            .read_to_string(&mut xml)?;
        let text = normalize_text(&visible_xml_text(&xml));
        if text.is_empty() {
            continue;
        }
        chunks.extend(split_text_chunks(&text, None, Some(slide_no), "slide"));
    }
    Ok(chunks)
}

fn extract_pdf(path: &Path) -> crate::errors::Result<Vec<ExtractedChunk>> {
    let bytes = std::fs::read(path)?;
    let text = pdf_text_from_bytes(&bytes);
    Ok(split_text_chunks(&text, Some(1), None, "page"))
}

/// Best-effort PDF text for bytes that never touch the disk — the same
/// literal-string pass `extract_pdf` uses, shared with `web_fetch` so a URL
/// that serves a PDF reads the same way a local one does.
pub(crate) fn pdf_text_from_bytes(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    normalize_text(&extract_pdf_literal_strings(&raw))
}

fn split_text_chunks(
    text: &str,
    page: Option<i64>,
    slide: Option<i64>,
    content_type: &str,
) -> Vec<ExtractedChunk> {
    const MAX_CHARS: usize = 1400;
    let mut chunks = Vec::new();
    let text = text.trim();
    if text.is_empty() {
        return chunks;
    }
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + MAX_CHARS).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let chunk_text = text[start..end].trim();
        if !chunk_text.is_empty() {
            chunks.push(ExtractedChunk {
                text: chunk_text.to_string(),
                content_type: content_type.into(),
                page,
                slide,
                heading: None,
                metadata_json: "{}".into(),
            });
        }
        start = end;
    }
    chunks
}

fn visible_xml_text(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut in_tag = false;
    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_xml_entities(&out)
}

fn decode_xml_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn extract_pdf_literal_strings(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '(' {
            continue;
        }
        let mut literal = String::new();
        let mut escaped = false;
        for next in chars.by_ref() {
            if escaped {
                literal.push(match next {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
                escaped = false;
                continue;
            }
            match next {
                '\\' => escaped = true,
                ')' => break,
                other => literal.push(other),
            }
        }
        if literal.chars().any(|c| c.is_alphabetic()) {
            out.push_str(&literal);
            out.push(' ');
        }
    }
    out
}

fn normalize_text(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn slide_number_from_name(name: &str) -> Option<i64> {
    let file = name.strip_prefix("ppt/slides/slide")?;
    let number = file.strip_suffix(".xml")?;
    number.parse().ok()
}

fn supported_kind(path: &Path) -> Option<&'static str> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "docx" => Some("docx"),
        "pptx" => Some("pptx"),
        "pdf" => Some("pdf"),
        _ => None,
    }
}

fn canonical_root(root_path: &str) -> crate::errors::Result<PathBuf> {
    let path = PathBuf::from(root_path);
    if !path.is_dir() {
        return Err(crate::errors::AppError::Other(format!(
            "Knowledge library root is not a directory: {}",
            path.display()
        )));
    }
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn estimate_tokens(text: &str) -> i64 {
    ((text.chars().count() as f64) / 4.0).ceil().max(1.0) as i64
}

fn first_words(text: &str, max_words: usize) -> String {
    text.split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join(" ")
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| {
            term.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|term| !term.is_empty())
        .collect()
}

fn effective_library_ids(query: &KnowledgeSearchQuery) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(library_id) = &query.library_id {
        if !library_id.trim().is_empty() {
            ids.push(library_id.clone());
        }
    }
    if let Some(library_ids) = &query.library_ids {
        for library_id in library_ids {
            if !library_id.trim().is_empty() && !ids.contains(library_id) {
                ids.push(library_id.clone());
            }
        }
    }
    ids
}

fn make_snippet(text: &str, terms: &[String]) -> String {
    let lower = text.to_lowercase();
    let first_match = terms
        .iter()
        .filter_map(|term| lower.find(term))
        .min()
        .unwrap_or(0);
    let start = first_match.saturating_sub(80);
    let end = (first_match + 240).min(text.len());
    let mut safe_start = start;
    while !text.is_char_boundary(safe_start) {
        safe_start += 1;
    }
    let mut safe_end = end;
    while !text.is_char_boundary(safe_end) {
        safe_end -= 1;
    }
    text[safe_start..safe_end].trim().to_string()
}

async fn record_retrieval_event(
    pool: &SqlitePool,
    query: &KnowledgeSearchQuery,
    results: &[KnowledgeSearchResult],
    latency_ms: i64,
) -> crate::errors::Result<()> {
    let refs = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "chunk_id": r.chunk_id,
                "document_id": r.document_id,
                "path": r.path,
                "page": r.page,
                "slide": r.slide,
            })
        })
        .collect::<Vec<_>>();
    let filters = serde_json::json!({
        "library_id": query.library_id.as_ref(),
        "library_ids": effective_library_ids(query),
        "kind": query.kind.as_ref(),
        "top_k": query.top_k,
    });
    sqlx::query(
        "INSERT INTO retrieval_events
         (id, session_id, task_id, query, filters_json, result_refs_json, created_at, latency_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&query.session_id)
    .bind(&query.task_id)
    .bind(&query.query)
    .bind(serde_json::to_string(&filters)?)
    .bind(serde_json::to_string(&refs)?)
    .bind(Utc::now().to_rfc3339())
    .bind(latency_ms)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    #[tokio::test]
    async fn enabled_library_context_snapshots_only_enabled_libraries() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        super::ensure_schema(&pool).await.expect("knowledge schema");

        for (id, enabled) in [("enabled-kb", 1_i64), ("disabled-kb", 0_i64)] {
            sqlx::query(
                "INSERT INTO knowledge_libraries
                 (id, name, root_path, enabled, created_at, scan_status)
                 VALUES (?, ?, ?, ?, '2026-01-01', 'completed')",
            )
            .bind(id)
            .bind(format!("name-{id}"))
            .bind(format!("/tmp/{id}"))
            .bind(enabled)
            .execute(&pool)
            .await
            .expect("insert library");
        }

        let context = super::enabled_library_context(&pool)
            .await
            .expect("enabled context");

        assert_eq!(context.knowledge_library_ids(), vec!["enabled-kb"]);
        assert_eq!(context.knowledge_libraries[0].root_path, "/tmp/enabled-kb");
    }

    #[tokio::test]
    async fn unscoped_search_excludes_disabled_libraries() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        super::ensure_schema(&pool).await.expect("knowledge schema");

        for (library_id, enabled) in [("enabled-kb", 1_i64), ("disabled-kb", 0_i64)] {
            let document_id = format!("{library_id}-document");
            sqlx::query(
                "INSERT INTO knowledge_libraries
                 (id, name, root_path, enabled, created_at, scan_status)
                 VALUES (?, ?, ?, ?, '2026-01-01', 'completed')",
            )
            .bind(library_id)
            .bind(library_id)
            .bind(format!("/tmp/{library_id}"))
            .bind(enabled)
            .execute(&pool)
            .await
            .expect("insert library");
            sqlx::query(
                "INSERT INTO knowledge_documents
                 (id, library_id, path, kind, hash, mtime, size, title, status, updated_at)
                 VALUES (?, ?, ?, 'pdf', 'hash', 1, 10, 'Atlas', 'indexed', '2026-01-01')",
            )
            .bind(&document_id)
            .bind(library_id)
            .bind(format!("/tmp/{library_id}/atlas.pdf"))
            .execute(&pool)
            .await
            .expect("insert document");
            sqlx::query(
                "INSERT INTO knowledge_chunks
                 (id, document_id, chunk_index, content_type, text, token_estimate, metadata_json)
                 VALUES (?, ?, 0, 'page', 'Atlas reference', 3, '{}')",
            )
            .bind(format!("{library_id}-chunk"))
            .bind(&document_id)
            .execute(&pool)
            .await
            .expect("insert chunk");
        }

        let results = super::search(
            &pool,
            super::KnowledgeSearchQuery {
                query: "Atlas".into(),
                library_id: None,
                library_ids: None,
                kind: None,
                top_k: Some(10),
                session_id: None,
                task_id: None,
            },
        )
        .await
        .expect("search enabled libraries");

        assert_eq!(
            results.len(),
            1,
            "disabled libraries must never leak into unscoped chat search"
        );
        assert_eq!(results[0].library_id, "enabled-kb");
    }

    #[tokio::test]
    async fn library_management_persists_enabled_state_and_deletes_only_the_index() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");
        super::ensure_schema(&pool).await.expect("knowledge schema");

        let root = std::env::temp_dir().join(format!("codefactory-kb-manage-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create fixture root");
        let source = root.join("source.docx");
        write_docx(source.clone(), "Atlas source remains on disk");

        let library = super::add_library(&pool, "fixture".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        super::scan_library(&pool, &library.id)
            .await
            .expect("scan library");

        super::set_library_enabled(&pool, &library.id, false)
            .await
            .expect("disable library");
        let libraries = super::list_libraries(&pool).await.expect("list libraries");
        assert_eq!(libraries.len(), 1);
        assert!(!libraries[0].enabled);

        super::delete_library(&pool, &library.id)
            .await
            .expect("delete library index");
        assert!(super::list_libraries(&pool)
            .await
            .expect("list after delete")
            .is_empty());
        let document_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_documents")
            .fetch_one(&pool)
            .await
            .expect("document count");
        let chunk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_chunks")
            .fetch_one(&pool)
            .await
            .expect("chunk count");
        assert_eq!(document_count, 0);
        assert_eq!(chunk_count, 0);
        assert!(
            source.exists(),
            "removing a library must not delete user source files"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn scan_library_indexes_docx_pptx_pdf_and_searches_with_source_refs() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        super::ensure_schema(&pool).await.expect("knowledge schema");

        let root = std::env::temp_dir().join(format!("codefactory-kb-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create fixture root");
        write_docx(
            root.join("proposal.docx"),
            "Reusable launch narrative for Atlas",
        );
        write_pptx(root.join("strategy.pptx"), "Atlas launch slide example");
        std::fs::write(
            root.join("brief.pdf"),
            b"%PDF-1.4\n1 0 obj <<>> stream\nBT (Atlas PDF appendix) Tj ET\nendstream\nendobj\n%%EOF",
        )
        .expect("write pdf fixture");

        let library = super::add_library(&pool, "fixture".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        let summary = super::scan_library(&pool, &library.id)
            .await
            .expect("scan library");

        let results = super::search(
            &pool,
            super::KnowledgeSearchQuery {
                query: "Atlas launch".into(),
                library_id: Some(library.id.clone()),
                library_ids: None,
                kind: None,
                top_k: Some(10),
                session_id: None,
                task_id: None,
            },
        )
        .await
        .expect("search knowledge");

        let _ = std::fs::remove_dir_all(root);

        assert_eq!(summary.indexed_documents, 3);
        assert_eq!(summary.failed_documents, 0);
        assert!(
            results
                .iter()
                .any(|r| r.kind == "docx" && r.path.ends_with("proposal.docx")),
            "docx result should include source path, got: {results:?}"
        );
        assert!(
            results
                .iter()
                .any(|r| r.kind == "pptx" && r.slide == Some(1)),
            "pptx result should include slide metadata, got: {results:?}"
        );
        assert!(
            results.iter().any(|r| r.kind == "pdf" && r.page == Some(1)),
            "pdf result should include page metadata, got: {results:?}"
        );
        assert!(
            results.iter().all(|r| r.snippet.contains("Atlas")),
            "all results should be query-relevant snippets: {results:?}"
        );

        let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM retrieval_events")
            .fetch_one(&pool)
            .await
            .expect("retrieval event count");
        assert_eq!(event_count, 1, "search should write an audit event");
    }

    #[tokio::test]
    async fn scan_library_records_corrupt_file_without_aborting() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        super::ensure_schema(&pool).await.expect("knowledge schema");

        let root = std::env::temp_dir().join(format!("codefactory-kb-corrupt-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create fixture root");
        write_docx(root.join("valid.docx"), "Valid Atlas reference");
        std::fs::write(root.join("broken.docx"), b"not-a-zip").expect("write broken docx");

        let library = super::add_library(&pool, "fixture".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        let summary = super::scan_library(&pool, &library.id)
            .await
            .expect("scan library");

        let rows: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT path, status, error FROM knowledge_documents ORDER BY path")
                .fetch_all(&pool)
                .await
                .expect("documents");

        let _ = std::fs::remove_dir_all(root);

        assert_eq!(summary.scanned_files, 2);
        assert_eq!(summary.indexed_documents, 1);
        assert_eq!(summary.failed_documents, 1);
        assert!(
            rows.iter()
                .any(|(path, status, _)| path.ends_with("valid.docx") && status == "indexed"),
            "valid document should be indexed: {rows:?}"
        );
        assert!(
            rows.iter().any(|(path, status, error)| {
                path.ends_with("broken.docx")
                    && status == "error"
                    && error.as_deref().unwrap_or("").contains("Invalid docx zip")
            }),
            "broken document should be recorded as error without aborting: {rows:?}"
        );
    }

    // ---------- M191: personal knowledge base auto-update ----------

    use sqlx::SqlitePool;

    async fn memory_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        super::ensure_schema(&pool).await.expect("knowledge schema");
        pool
    }

    fn fixture_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "codefactory-kb-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create fixture root");
        // Library roots are canonicalized on registration; fixtures must use the
        // same spelling so stored document paths match (macOS /var -> /private/var).
        std::fs::canonicalize(&root).expect("canonical fixture root")
    }

    async fn search_hits(pool: &SqlitePool, library_id: &str, query: &str) -> Vec<super::KnowledgeSearchResult> {
        super::search(
            pool,
            super::KnowledgeSearchQuery {
                query: query.into(),
                library_id: Some(library_id.to_string()),
                library_ids: None,
                kind: None,
                top_k: Some(10),
                session_id: None,
                task_id: None,
            },
        )
        .await
        .expect("search knowledge")
    }

    async fn indexed_documents(pool: &SqlitePool, library_id: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM knowledge_documents WHERE library_id = ? AND status = 'indexed'",
        )
        .bind(library_id)
        .fetch_one(pool)
        .await
        .expect("count indexed documents")
    }

    async fn await_indexed(pool: &SqlitePool, library_id: &str, expected: i64) {
        for _ in 0..200 {
            if indexed_documents(pool, library_id).await >= expected {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("library {library_id} did not index {expected} document(s) in time");
    }

    /// R1 + R2: registering a folder indexes it with no manual scan, and an
    /// edited file is re-indexed through the same background entry point the
    /// watcher uses — old chunks must not survive the edit.
    #[tokio::test]
    async fn background_index_builds_on_registration_and_follows_edits() {
        let pool = memory_pool().await;
        let root = fixture_root("auto-edit");
        let file = root.join("note.docx");
        write_docx(file.clone(), "Atlas launch plan");
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");

        // Same entry point the register command / watcher uses.
        crate::knowledge_watch::spawn_scan(pool.clone(), library.id.clone());
        await_indexed(&pool, &library.id, 1).await;
        assert!(
            !search_hits(&pool, &library.id, "Atlas").await.is_empty(),
            "registration alone must make the document retrievable"
        );

        write_docx(file, "Beacon delivery memo");
        crate::knowledge_watch::spawn_scan(pool.clone(), library.id.clone());
        for _ in 0..200 {
            if !search_hits(&pool, &library.id, "Beacon").await.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let hits = search_hits(&pool, &library.id, "Beacon").await;
        let stale = search_hits(&pool, &library.id, "Atlas").await;
        let _ = std::fs::remove_dir_all(&root);

        assert!(!hits.is_empty(), "edited content must become retrievable");
        assert!(
            stale.is_empty(),
            "chunks of the superseded revision must be replaced, got: {stale:?}"
        );
        assert_eq!(indexed_documents(&pool, &library.id).await, 1);
    }

    /// R2: files deleted on disk must stop answering retrieval.
    #[tokio::test]
    async fn deleted_file_leaves_no_documents_or_chunks() {
        let pool = memory_pool().await;
        let root = fixture_root("delete");
        write_docx(root.join("keep.docx"), "Atlas keeps its place");
        write_docx(root.join("gone.docx"), "Beacon retires");
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        super::scan_library(&pool, &library.id).await.expect("first scan");

        std::fs::remove_file(root.join("gone.docx")).expect("delete source file");
        let summary = super::scan_library(&pool, &library.id)
            .await
            .expect("second scan");
        let beacon = search_hits(&pool, &library.id, "Beacon").await;
        let atlas = search_hits(&pool, &library.id, "Atlas").await;
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(summary.removed_documents, 1, "summary: {summary:?}");
        assert_eq!(indexed_documents(&pool, &library.id).await, 1);
        assert!(beacon.is_empty(), "deleted document still retrievable: {beacon:?}");
        assert!(!atlas.is_empty(), "surviving document must stay indexed");
    }

    /// R2: a rename is a removal plus an addition, not a duplicated document.
    #[tokio::test]
    async fn renamed_file_moves_its_index_entry() {
        let pool = memory_pool().await;
        let root = fixture_root("rename");
        write_docx(root.join("draft.docx"), "Atlas anniversary draft");
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        super::scan_library(&pool, &library.id).await.expect("first scan");

        std::fs::rename(root.join("draft.docx"), root.join("final.docx")).expect("rename");
        let summary = super::scan_library(&pool, &library.id)
            .await
            .expect("second scan");
        let hits = search_hits(&pool, &library.id, "Atlas").await;
        let paths: Vec<String> =
            sqlx::query_scalar("SELECT path FROM knowledge_documents WHERE library_id = ?")
                .bind(&library.id)
                .fetch_all(&pool)
                .await
                .expect("document paths");
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(summary.removed_documents, 1, "summary: {summary:?}");
        assert_eq!(summary.added_documents, 1, "summary: {summary:?}");
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("final.docx"), "paths: {paths:?}");
        assert!(hits.iter().all(|hit| hit.path.ends_with("final.docx")));
    }

    /// R3: a rescan of an untouched folder must not extract or rewrite anything.
    #[tokio::test]
    async fn rescan_of_unchanged_folder_does_no_extraction() {
        let pool = memory_pool().await;
        let root = fixture_root("unchanged");
        write_docx(root.join("a.docx"), "Atlas unchanged one");
        write_docx(root.join("b.docx"), "Atlas unchanged two");
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        super::scan_library(&pool, &library.id).await.expect("first scan");
        let chunk_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM knowledge_chunks ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("chunk ids");
        let parses_before = super::parse_count_for(&library.id);
        let updated_before: String =
            sqlx::query_scalar("SELECT updated_at FROM knowledge_documents ORDER BY path LIMIT 1")
                .fetch_one(&pool)
                .await
                .expect("updated_at");

        let summary = super::scan_library(&pool, &library.id)
            .await
            .expect("second scan");
        let chunk_ids_after: Vec<String> =
            sqlx::query_scalar("SELECT id FROM knowledge_chunks ORDER BY id")
                .fetch_all(&pool)
                .await
                .expect("chunk ids after");
        let updated_after: String =
            sqlx::query_scalar("SELECT updated_at FROM knowledge_documents ORDER BY path LIMIT 1")
                .fetch_one(&pool)
                .await
                .expect("updated_at after");
        let parses_after = super::parse_count_for(&library.id);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(parses_after, parses_before, "rescan re-extracted documents");
        assert_eq!(summary.unchanged_documents, 2, "summary: {summary:?}");
        assert_eq!(summary.indexed_documents, 0);
        assert_eq!(summary.chunks_indexed, 0);
        assert_eq!(chunk_ids_after, chunk_ids, "chunks must not be rewritten");
        assert_eq!(updated_after, updated_before, "untouched rows must not churn");
    }

    /// R3: a touched-but-identical file (Office re-save, copy) refreshes
    /// metadata only — the content hash decides, not the mtime.
    #[tokio::test]
    async fn touched_but_identical_file_is_not_reindexed() {
        let pool = memory_pool().await;
        let root = fixture_root("touched");
        let file = root.join("a.docx");
        write_docx(file.clone(), "Atlas identical content");
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        super::scan_library(&pool, &library.id).await.expect("first scan");
        let bytes = std::fs::read(&file).expect("read fixture");
        let chunk_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM knowledge_chunks ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("chunk ids");
        let parses_before = super::parse_count_for(&library.id);
        let mtime_before: i64 = sqlx::query_scalar("SELECT mtime FROM knowledge_documents")
            .fetch_one(&pool)
            .await
            .expect("mtime");

        // Same bytes, new mtime: wait past the second-resolution mtime clock.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        std::fs::write(&file, &bytes).expect("rewrite identical bytes");

        let summary = super::scan_library(&pool, &library.id)
            .await
            .expect("second scan");
        let chunk_ids_after: Vec<String> =
            sqlx::query_scalar("SELECT id FROM knowledge_chunks ORDER BY id")
                .fetch_all(&pool)
                .await
                .expect("chunk ids after");
        let mtime_after: i64 = sqlx::query_scalar("SELECT mtime FROM knowledge_documents")
            .fetch_one(&pool)
            .await
            .expect("mtime after");
        let parses_after = super::parse_count_for(&library.id);
        let _ = std::fs::remove_dir_all(&root);

        assert!(mtime_after > mtime_before, "fixture must move the mtime");
        assert_eq!(
            parses_after, parses_before,
            "identical bytes must not be re-extracted just because mtime moved"
        );
        assert_eq!(summary.unchanged_documents, 1, "summary: {summary:?}");
        assert_eq!(summary.indexed_documents, 0);
        assert_eq!(chunk_ids_after, chunk_ids, "chunks must keep their identity");
    }

    /// R1: files added while the app was closed are picked up by the startup
    /// reconcile pass (no user action, no watcher event to rely on).
    #[tokio::test]
    async fn startup_reconcile_indexes_files_added_while_app_was_closed() {
        let pool = memory_pool().await;
        let root = fixture_root("reconcile");
        write_docx(root.join("first.docx"), "Atlas first memo");
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        super::scan_library(&pool, &library.id).await.expect("first scan");

        write_docx(root.join("second.docx"), "Beacon added while closed");
        let scanned = super::reconcile_all_libraries(&pool)
            .await
            .expect("startup reconcile");
        let hits = search_hits(&pool, &library.id, "Beacon").await;
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(scanned, vec![library.id.clone()]);
        assert_eq!(indexed_documents(&pool, &library.id).await, 2);
        assert!(!hits.is_empty(), "offline addition must be indexed on start");
    }

    /// R4: a scan killed by an app crash must not leave `scanning` forever or
    /// half-written chunks behind; the next pass completes the document.
    #[tokio::test]
    async fn interrupted_scan_state_is_repaired_on_restart() {
        let pool = memory_pool().await;
        let root = fixture_root("crash");
        write_docx(root.join("a.docx"), "Atlas crash recovery");
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        // Simulate the observable state of a scan killed mid-way.
        sqlx::query("UPDATE knowledge_libraries SET scan_status = 'scanning' WHERE id = ?")
            .bind(&library.id)
            .execute(&pool)
            .await
            .expect("mark library scanning");
        sqlx::query(
            "INSERT INTO knowledge_documents
             (id, library_id, path, kind, hash, mtime, size, title, status, error, updated_at)
             VALUES ('half-doc', ?, ?, 'docx', '', 0, 0, NULL, 'scanning', NULL, '2026-01-01')",
        )
        .bind(&library.id)
        .bind(root.join("a.docx").to_string_lossy().to_string())
        .execute(&pool)
        .await
        .expect("insert partial document");
        sqlx::query(
            "INSERT INTO knowledge_chunks
             (id, document_id, chunk_index, content_type, text, token_estimate, metadata_json)
             VALUES ('half-chunk', 'half-doc', 0, 'page', 'partial Atlas', 2, '{}')",
        )
        .execute(&pool)
        .await
        .expect("insert partial chunk");

        let requeued = super::recover_interrupted_scans(&pool)
            .await
            .expect("recover interrupted scans");
        let status: String = sqlx::query_scalar("SELECT scan_status FROM knowledge_libraries WHERE id = ?")
            .bind(&library.id)
            .fetch_one(&pool)
            .await
            .expect("library status");
        let chunks_after_recovery: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_chunks")
            .fetch_one(&pool)
            .await
            .expect("chunk count");
        let summary = super::scan_library(&pool, &library.id)
            .await
            .expect("scan after recovery");
        let doc_status: String = sqlx::query_scalar(
            "SELECT status FROM knowledge_documents WHERE library_id = ? AND path = ?",
        )
        .bind(&library.id)
        .bind(root.join("a.docx").to_string_lossy().to_string())
        .fetch_one(&pool)
        .await
        .expect("document status");
        let final_chunks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_chunks")
            .fetch_one(&pool)
            .await
            .expect("final chunk count");
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(requeued, 1, "the interrupted document must be requeued");
        assert_ne!(status, "scanning", "library must not stay stuck scanning");
        assert_eq!(chunks_after_recovery, 0, "partial chunks must be dropped");
        assert_eq!(summary.added_documents + summary.updated_documents, 1);
        assert_eq!(doc_status, "indexed");
        assert!(final_chunks >= 1, "document must be fully indexed again");
    }

    /// R4 + R5: an aborted scan releases its library and leaves no orphan or
    /// half-written document behind.
    #[tokio::test]
    async fn aborted_scan_releases_the_library_and_leaves_no_partial_rows() {
        let pool = memory_pool().await;
        let root = fixture_root("abort");
        for index in 0..5 {
            write_docx(
                root.join(format!("doc-{index}.docx")),
                &format!("Atlas document {index}"),
            );
        }
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");

        let aborted = tokio::time::timeout(
            std::time::Duration::from_micros(1),
            super::scan_library(&pool, &library.id),
        )
        .await;
        assert!(aborted.is_err(), "fixture scan should have been cut short");

        let summary = super::scan_library(&pool, &library.id)
            .await
            .expect("scan must not be wedged by the aborted pass");
        let orphans: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM knowledge_chunks c
             LEFT JOIN knowledge_documents d ON d.id = c.document_id
             WHERE d.id IS NULL",
        )
        .fetch_one(&pool)
        .await
        .expect("orphan chunks");
        let empty_documents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM knowledge_documents d
             WHERE d.status = 'indexed'
               AND NOT EXISTS (SELECT 1 FROM knowledge_chunks c WHERE c.document_id = d.id)",
        )
        .fetch_one(&pool)
        .await
        .expect("documents without chunks");
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(summary.indexed_documents, 5, "summary: {summary:?}");
        assert_eq!(orphans, 0, "chunk rows must never outlive their document");
        assert_eq!(empty_documents, 0, "indexed documents must own chunks");
    }

    /// R5: exactly one pass per library; concurrent requests coalesce into a
    /// single follow-up instead of racing over the same folder.
    #[test]
    fn scan_lease_is_exclusive_and_coalesces_followups() {
        let lease = super::ScanLease::acquire("lease-kb").expect("first acquire wins");
        assert!(
            super::ScanLease::acquire("lease-kb").is_none(),
            "a second pass over the same library must not start"
        );
        assert!(lease.take_rerun(), "the late request must schedule one rerun");
        assert!(!lease.take_rerun(), "the rerun flag is consumed once");
        assert!(
            super::ScanLease::acquire("lease-other-kb").is_some(),
            "different libraries stay independent"
        );
        drop(lease);
        assert!(
            super::ScanLease::acquire("lease-kb").is_some(),
            "releasing must let the next pass run"
        );
    }

    /// R6: the UI needs the failing file and the reason, not just a count.
    #[tokio::test]
    async fn library_failures_report_paths_and_reasons() {
        let pool = memory_pool().await;
        let root = fixture_root("failures");
        write_docx(root.join("ok.docx"), "Atlas fine");
        std::fs::write(root.join("broken.docx"), b"not-a-zip").expect("write broken docx");
        let library = super::add_library(&pool, "kb".into(), root.to_string_lossy().into())
            .await
            .expect("add library");
        super::scan_library(&pool, &library.id).await.expect("scan");

        let failures = super::library_failures(&pool, &library.id)
            .await
            .expect("failures");
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(failures.len(), 1, "failures: {failures:?}");
        assert!(failures[0].path.ends_with("broken.docx"));
        assert_eq!(failures[0].kind, "docx");
        assert!(
            failures[0].error.contains("Invalid docx zip"),
            "failure must explain the cause: {}",
            failures[0].error
        );
    }

    fn write_docx(path: std::path::PathBuf, text: &str) {
        let file = std::fs::File::create(path).expect("create docx");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("word/document.xml", options)
            .expect("docx xml");
        write!(
            zip,
            r#"<w:document><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body></w:document>"#,
            text
        )
        .expect("write docx xml");
        zip.finish().expect("finish docx");
    }

    fn write_pptx(path: std::path::PathBuf, text: &str) {
        let file = std::fs::File::create(path).expect("create pptx");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("ppt/slides/slide1.xml", options)
            .expect("slide xml");
        write!(
            zip,
            r#"<p:sld><p:cSld><p:spTree><a:t>{}</a:t></p:spTree></p:cSld></p:sld>"#,
            text
        )
        .expect("write pptx xml");
        zip.finish().expect("finish pptx");
    }
}
