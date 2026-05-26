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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeScanSummary {
    pub library_id: String,
    pub scanned_files: usize,
    pub indexed_documents: usize,
    pub failed_documents: usize,
    pub chunks_indexed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSearchQuery {
    pub query: String,
    #[serde(default)]
    pub library_id: Option<String>,
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

pub async fn scan_library(
    pool: &SqlitePool,
    library_id: &str,
) -> crate::errors::Result<KnowledgeScanSummary> {
    let library = library_by_id(pool, library_id).await?;
    let scan_started = Utc::now().to_rfc3339();
    sqlx::query("UPDATE knowledge_libraries SET scan_status = 'scanning' WHERE id = ?")
        .bind(library_id)
        .execute(pool)
        .await?;

    let mut summary = KnowledgeScanSummary {
        library_id: library_id.to_string(),
        scanned_files: 0,
        indexed_documents: 0,
        failed_documents: 0,
        chunks_indexed: 0,
    };

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
        match index_document(pool, &library, path, kind).await {
            Ok(chunks) => {
                summary.indexed_documents += 1;
                summary.chunks_indexed += chunks;
            }
            Err(e) => {
                summary.failed_documents += 1;
                record_failed_document(pool, &library, path, kind, &e.to_string()).await?;
            }
        }
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
         WHERE d.status = 'indexed'",
    );
    if query.library_id.is_some() {
        rows_query.push_str(" AND d.library_id = ?");
    }
    if query.kind.is_some() {
        rows_query.push_str(" AND d.kind = ?");
    }

    let mut q = sqlx::query(&rows_query);
    if let Some(library_id) = &query.library_id {
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
        last_scan_at: row.try_get::<Option<String>, _>("last_scan_at").unwrap_or(None),
        scan_status: row.try_get("scan_status").unwrap_or_else(|_| "idle".into()),
    }
}

async fn index_document(
    pool: &SqlitePool,
    library: &KnowledgeLibrary,
    path: &Path,
    kind: &str,
) -> crate::errors::Result<usize> {
    let bytes = std::fs::read(path)?;
    let hash = stable_hash_hex(&bytes);
    let metadata = std::fs::metadata(path)?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = metadata.len() as i64;
    let path_str = path.to_string_lossy().to_string();
    let document_id = existing_document_id(pool, &library.id, &path_str)
        .await?
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let chunks = extract_document(path, kind)?;
    if chunks.is_empty() {
        return Err(crate::errors::AppError::Other(format!(
            "No extractable text found in {}",
            path.display()
        )));
    }
    let title = chunks
        .first()
        .map(|chunk| first_words(&chunk.text, 12))
        .filter(|s| !s.is_empty());
    let now = Utc::now().to_rfc3339();

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
    .execute(pool)
    .await?;

    sqlx::query("DELETE FROM knowledge_chunks WHERE document_id = ?")
        .bind(&document_id)
        .execute(pool)
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
        .execute(pool)
        .await?;
    }

    Ok(chunks.len())
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
    Ok(sqlx::query_scalar(
        "SELECT id FROM knowledge_documents WHERE library_id = ? AND path = ?",
    )
    .bind(library_id)
    .bind(path)
    .fetch_optional(pool)
    .await?)
}

fn extract_document(path: &Path, kind: &str) -> crate::errors::Result<Vec<ExtractedChunk>> {
    match kind {
        "docx" => extract_docx(path),
        "pptx" => extract_pptx(path),
        "pdf" => extract_pdf(path),
        _ => Ok(Vec::new()),
    }
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
    let raw = String::from_utf8_lossy(&bytes);
    let text = normalize_text(&extract_pdf_literal_strings(&raw));
    Ok(split_text_chunks(&text, Some(1), None, "page"))
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
    match path.extension()?.to_string_lossy().to_ascii_lowercase().as_str() {
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
    async fn scan_library_indexes_docx_pptx_pdf_and_searches_with_source_refs() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        super::ensure_schema(&pool).await.expect("knowledge schema");

        let root = std::env::temp_dir().join(format!("codefactory-kb-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create fixture root");
        write_docx(root.join("proposal.docx"), "Reusable launch narrative for Atlas");
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
            results.iter().any(|r| r.kind == "docx" && r.path.ends_with("proposal.docx")),
            "docx result should include source path, got: {results:?}"
        );
        assert!(
            results.iter().any(|r| r.kind == "pptx" && r.slide == Some(1)),
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

        let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT path, status, error FROM knowledge_documents ORDER BY path",
        )
        .fetch_all(&pool)
        .await
        .expect("documents");

        let _ = std::fs::remove_dir_all(root);

        assert_eq!(summary.scanned_files, 2);
        assert_eq!(summary.indexed_documents, 1);
        assert_eq!(summary.failed_documents, 1);
        assert!(
            rows.iter().any(|(path, status, _)| path.ends_with("valid.docx") && status == "indexed"),
            "valid document should be indexed: {rows:?}"
        );
        assert!(
            rows.iter().any(|(path, status, error)| {
                path.ends_with("broken.docx") && status == "error" && error.as_deref().unwrap_or("").contains("Invalid docx zip")
            }),
            "broken document should be recorded as error without aborting: {rows:?}"
        );
    }

    fn write_docx(path: std::path::PathBuf, text: &str) {
        let file = std::fs::File::create(path).expect("create docx");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("word/document.xml", options).expect("docx xml");
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
        zip.start_file("ppt/slides/slide1.xml", options).expect("slide xml");
        write!(
            zip,
            r#"<p:sld><p:cSld><p:spTree><a:t>{}</a:t></p:spTree></p:cSld></p:sld>"#,
            text
        )
        .expect("write pptx xml");
        zip.finish().expect("finish pptx");
    }
}
