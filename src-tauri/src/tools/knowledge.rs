// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use serde_json::{json, Value};

use super::{ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::knowledge::KnowledgeSearchQuery;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    library_id: Option<String>,
    #[serde(default)]
    library_ids: Option<Vec<String>>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    top_k: Option<usize>,
}

#[derive(Deserialize)]
struct GetChunkArgs {
    chunk_id: String,
}

pub fn search_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "kb_search".into(),
            description: "Search indexed personal knowledge libraries. Returns source-grounded chunks with document path and page/slide metadata.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "library_id": { "type": "string", "description": "Optional knowledge library id" },
                    "library_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional knowledge library ids; task scope is enforced by CodeFactory"
                    },
                    "kind": { "type": "string", "enum": ["docx", "pptx", "pdf"] },
                    "top_k": { "type": "integer", "default": 8, "minimum": 1, "maximum": 50 }
                },
                "required": ["query"]
            }),
        },
    }
}

pub fn get_chunk_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "kb_get_chunk".into(),
            description:
                "Read a full indexed knowledge chunk by chunk id, including source metadata.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chunk_id": { "type": "string" }
                },
                "required": ["chunk_id"]
            }),
        },
    }
}

pub async fn execute_search(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Ok(args) = serde_json::from_value::<SearchArgs>(args) else {
        return Ok(ToolOutput::err("Invalid arguments"));
    };
    let Some(pool) = &ctx.db else {
        return Ok(ToolOutput::err(
            "Knowledge search is unavailable: database is not attached",
        ));
    };
    let (library_id, library_ids) = match scoped_library_filter(
        args.library_id,
        args.library_ids.unwrap_or_default(),
        ctx.knowledge_library_ids.as_deref(),
    ) {
        Ok(filter) => filter,
        Err(e) => return Ok(ToolOutput::err(e.to_string())),
    };
    let results = crate::knowledge::search(
        pool,
        KnowledgeSearchQuery {
            query: args.query,
            library_id,
            library_ids,
            kind: args.kind,
            top_k: args.top_k,
            session_id: ctx.session_id.clone(),
            task_id: ctx.task_id.clone(),
        },
    )
    .await?;
    Ok(ToolOutput::ok(serde_json::to_string_pretty(&results)?))
}

pub async fn execute_get_chunk(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Ok(args) = serde_json::from_value::<GetChunkArgs>(args) else {
        return Ok(ToolOutput::err("Invalid arguments"));
    };
    let Some(pool) = &ctx.db else {
        return Ok(ToolOutput::err(
            "Knowledge chunk read is unavailable: database is not attached",
        ));
    };
    if let Some(scope) = ctx.knowledge_library_ids.as_deref() {
        if scope.is_empty() {
            return Ok(ToolOutput::err("Knowledge chunk read is unavailable: no enabled knowledge libraries are scoped to this task"));
        }
        let library_id = crate::knowledge::chunk_library_id(pool, &args.chunk_id).await?;
        if !scope.iter().any(|allowed| allowed == &library_id) {
            return Ok(ToolOutput::err(format!(
                "Knowledge chunk '{}' is outside the enabled task knowledge scope",
                args.chunk_id
            )));
        }
    }
    let chunk = crate::knowledge::get_chunk(pool, &args.chunk_id).await?;
    Ok(ToolOutput::ok(serde_json::to_string_pretty(&chunk)?))
}

fn scoped_library_filter(
    library_id: Option<String>,
    library_ids: Vec<String>,
    scope: Option<&[String]>,
) -> Result<(Option<String>, Option<Vec<String>>)> {
    let Some(scope) = scope else {
        let ids = if library_ids.is_empty() {
            None
        } else {
            Some(library_ids)
        };
        return Ok((library_id, ids));
    };
    if scope.is_empty() {
        return Err(crate::errors::AppError::Other(
            "Knowledge search is unavailable: no enabled knowledge libraries are scoped to this task"
                .into(),
        ));
    }
    if let Some(requested) = &library_id {
        if !scope.iter().any(|allowed| allowed == requested) {
            return Err(crate::errors::AppError::Other(format!(
                "Knowledge library '{}' is outside the enabled task knowledge scope",
                requested
            )));
        }
        return Ok((library_id, None));
    }
    if !library_ids.is_empty() {
        for requested in &library_ids {
            if !scope.iter().any(|allowed| allowed == requested) {
                return Err(crate::errors::AppError::Other(format!(
                    "Knowledge library '{}' is outside the enabled task knowledge scope",
                    requested
                )));
            }
        }
        return Ok((None, Some(library_ids)));
    }
    Ok((None, Some(scope.to_vec())))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn kb_search_uses_attached_database_and_returns_source_json() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        crate::knowledge::ensure_schema(&pool)
            .await
            .expect("knowledge schema");

        let library_id = Uuid::new_v4().to_string();
        let document_id = Uuid::new_v4().to_string();
        let chunk_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO knowledge_libraries
             (id, name, root_path, enabled, created_at, scan_status)
             VALUES (?, 'fixture', '/tmp/kb', 1, '2026-01-01', 'completed')",
        )
        .bind(&library_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO knowledge_documents
             (id, library_id, path, kind, hash, mtime, size, title, status, updated_at)
             VALUES (?, ?, '/tmp/kb/deck.pptx', 'pptx', 'hash', 1, 10, 'Deck', 'indexed', '2026-01-01')",
        )
        .bind(&document_id)
        .bind(&library_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO knowledge_chunks
             (id, document_id, chunk_index, content_type, text, slide, token_estimate, metadata_json)
             VALUES (?, ?, 0, 'slide', 'Atlas launch plan source', 3, 5, '{}')",
        )
        .bind(&chunk_id)
        .bind(&document_id)
        .execute(&pool)
        .await
        .unwrap();

        let cwd = std::env::temp_dir().join(format!("codefactory-kb-tool-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let output = execute_search(
            json!({ "query": "Atlas", "top_k": 5 }),
            &crate::tools::ExecCtx {
                cwd: cwd.clone(),
                db: Some(pool.clone()),
                session_id: Some("parent-session".into()),
                task_id: Some("task-1".into()),
                knowledge_library_ids: Some(vec![library_id.clone()]),
                settings: None,
                app: None,
                pending_secrets: None,
                settings_state: None,
            },
        )
        .await
        .expect("tool output");
        let _ = std::fs::remove_dir_all(cwd);

        assert!(!output.is_error);
        assert!(output.content.contains(&chunk_id));
        assert!(output.content.contains("deck.pptx"));
        assert!(output.content.contains("\"slide\": 3"));

        let event: (Option<String>, Option<String>, String) =
            sqlx::query_as("SELECT session_id, task_id, filters_json FROM retrieval_events")
                .fetch_one(&pool)
                .await
                .expect("retrieval event");
        assert_eq!(event.0.as_deref(), Some("parent-session"));
        assert_eq!(event.1.as_deref(), Some("task-1"));
        assert!(
            event.2.contains(&library_id),
            "audit filters should record scoped library ids: {}",
            event.2
        );
    }
}
