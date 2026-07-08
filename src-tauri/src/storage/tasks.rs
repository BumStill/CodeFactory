// SPDX-License-Identifier: Apache-2.0
//! Persistence layer for `task_runs` and their dependencies.
//!
//! Timestamp columns (`created_at`, `started_at`, `completed_at`) are stored as
//! ISO-8601 strings to keep them human-readable in the DB and easy to render
//! on the frontend. Existing message timestamps use ms-since-epoch integers,
//! but tasks are a brand-new table and ISO-8601 is friendlier for the small
//! audit-log style queries we expect here.
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::errors::Result;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TaskKnowledgeLibraryContext {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub scan_status: String,
    pub last_scan_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TaskConnectorContext {
    #[serde(default)]
    pub knowledge_libraries: Vec<TaskKnowledgeLibraryContext>,
}

impl TaskConnectorContext {
    pub fn is_empty(&self) -> bool {
        self.knowledge_libraries.is_empty()
    }

    pub fn from_json(raw: Option<&str>) -> Option<Self> {
        let raw = raw?;
        serde_json::from_str(raw).ok()
    }

    pub fn knowledge_library_ids(&self) -> Vec<String> {
        self.knowledge_libraries
            .iter()
            .filter(|library| !library.id.trim().is_empty())
            .map(|library| library.id.clone())
            .collect()
    }

    pub fn render_markdown(&self) -> String {
        if self.knowledge_libraries.is_empty() {
            return String::new();
        }
        let mut out = String::from(
            "## Enabled connectors\n\nPersonal knowledge libraries are enabled for this task. \
             Use `kb_search` to retrieve source-grounded snippets and `kb_get_chunk` only for \
             chunks returned by search. Cite source paths plus page or slide numbers in your summary.\n\n",
        );
        for library in &self.knowledge_libraries {
            out.push_str(&format!(
                "- {} (`{}`): status `{}`, root `{}`{}\n",
                library.name,
                library.id,
                library.scan_status,
                library.root_path,
                library
                    .last_scan_at
                    .as_ref()
                    .map(|scan| format!(", last scan `{scan}`"))
                    .unwrap_or_default()
            ));
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskRun {
    pub id: String,
    pub session_id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub cwd: String,
    pub parent_task_id: Option<String>,
    /// Session row created for the subagent that ran this task (if any).
    /// Lets the dashboard deep-link into the subagent's transcript.
    pub sub_session_id: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub attempt_count: i32,
    /// JSON array of VerificationResult (Phase 3). NULL when not yet run.
    pub verification_results: Option<String>,
    /// JSON TaskConnectorContext persisted at task creation time.
    pub task_context_json: Option<String>,
    /// JSON Vec<String> of user-visible acceptance criteria the agent
    /// must verify before declaring done. Drives autonomous-mode
    /// completion check + scheduler-side respawn-on-incomplete loop.
    pub acceptance_criteria_json: Option<String>,
    /// The spec this task was decomposed from (set when the tree is created
    /// from a spec's "开始实现"). None for ad-hoc Workspace tasks. Lets the task
    /// tree show "来自规范《X》" and close the spec→task→execution loop.
    #[serde(default)]
    pub spec_req_id: Option<String>,
    #[serde(default)]
    pub spec_title: Option<String>,
}

pub async fn insert_task(pool: &SqlitePool, task: &TaskRun) -> Result<()> {
    sqlx::query(
        "INSERT INTO task_runs (id, session_id, title, description, status, cwd, parent_task_id, \
         sub_session_id, created_at, started_at, completed_at, result, error, attempt_count, \
         verification_results, task_context_json, acceptance_criteria_json, spec_req_id, spec_title) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&task.id)
    .bind(&task.session_id)
    .bind(&task.title)
    .bind(&task.description)
    .bind(&task.status)
    .bind(&task.cwd)
    .bind(&task.parent_task_id)
    .bind(&task.sub_session_id)
    .bind(&task.created_at)
    .bind(&task.started_at)
    .bind(&task.completed_at)
    .bind(&task.result)
    .bind(&task.error)
    .bind(task.attempt_count)
    .bind(&task.verification_results)
    .bind(&task.task_context_json)
    .bind(&task.acceptance_criteria_json)
    .bind(&task.spec_req_id)
    .bind(&task.spec_title)
    .execute(pool)
    .await?;
    Ok(())
}

/// Persist verification results (JSON) for a task.
pub async fn save_verification_results(
    pool: &SqlitePool,
    task_id: &str,
    results_json: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE task_runs SET verification_results = ? WHERE id = ?",
    )
    .bind(results_json)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Read verification results JSON for a task (returns None if not yet run).
pub async fn get_verification_results(
    pool: &SqlitePool,
    task_id: &str,
) -> Result<Option<String>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT verification_results FROM task_runs WHERE id = ?")
            .bind(task_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(v,)| v))
}

// Scaffolding: generic status setter. The live scheduler uses the specific
// `mark_task_*` helpers below; this stays for arbitrary status transitions.
#[allow(dead_code)]
pub async fn update_task_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE task_runs SET status = ?, error = ? WHERE id = ?")
        .bind(status)
        .bind(error)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// Scaffolding: generic result setter (mark_task_completed sets result inline).
#[allow(dead_code)]
pub async fn update_task_result(pool: &SqlitePool, id: &str, result: &str) -> Result<()> {
    sqlx::query("UPDATE task_runs SET result = ? WHERE id = ?")
        .bind(result)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_task_started(pool: &SqlitePool, id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE task_runs SET status = 'running', started_at = COALESCE(started_at, ?), \
         attempt_count = attempt_count + 1 WHERE id = ?",
    )
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_task_completed(pool: &SqlitePool, id: &str, result: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE task_runs SET status = 'completed', completed_at = ?, result = ?, error = NULL \
         WHERE id = ?",
    )
    .bind(&now)
    .bind(result)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_task_failed(pool: &SqlitePool, id: &str, error: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE task_runs SET status = 'failed', completed_at = ?, error = ? WHERE id = ?",
    )
    .bind(&now)
    .bind(error)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_task_cancelled(pool: &SqlitePool, id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE task_runs SET status = 'cancelled', completed_at = ? \
         WHERE id = ? AND status IN ('pending', 'running')",
    )
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn retry_failed_tasks(pool: &SqlitePool, session_id: &str) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE task_runs \
         SET status = 'pending', started_at = NULL, completed_at = NULL, error = NULL, \
             result = NULL, verification_results = NULL \
         WHERE session_id = ? AND status IN ('failed', 'cancelled')",
    )
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

// Scaffolding: standalone attempt bump for retry flows; mark_task_started also
// increments on (re)start, so this stays until retries call it directly.
pub async fn increment_attempt(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE task_runs SET attempt_count = attempt_count + 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_sub_session(pool: &SqlitePool, id: &str, sub_session_id: &str) -> Result<()> {
    sqlx::query("UPDATE task_runs SET sub_session_id = ? WHERE id = ?")
        .bind(sub_session_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_session_tasks(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<TaskRun>> {
    let rows = sqlx::query_as::<_, TaskRun>(
        "SELECT * FROM task_runs WHERE session_id = ? ORDER BY created_at ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_all_tasks_for_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<TaskRun>> {
    let rows = sqlx::query_as::<_, TaskRun>(
        "SELECT * FROM task_runs WHERE session_id = ? ORDER BY created_at ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_task(pool: &SqlitePool, id: &str) -> Result<Option<TaskRun>> {
    let row = sqlx::query_as::<_, TaskRun>("SELECT * FROM task_runs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn list_pending_tasks_for_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<TaskRun>> {
    let rows = sqlx::query_as::<_, TaskRun>(
        "SELECT * FROM task_runs WHERE session_id = ? AND status = 'pending' \
         ORDER BY created_at ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn add_dependency(
    pool: &SqlitePool,
    task_id: &str,
    depends_on_task_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO task_dependencies (task_id, depends_on_task_id) VALUES (?, ?)",
    )
    .bind(task_id)
    .bind(depends_on_task_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_dependencies(pool: &SqlitePool, task_id: &str) -> Result<Vec<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT depends_on_task_id FROM task_dependencies WHERE task_id = ?")
            .bind(task_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(s,)| s).collect())
}

/// A task is "ready" iff every dependency it has is in the `completed` state.
/// Tasks with zero dependencies are always ready.
pub async fn is_task_ready(pool: &SqlitePool, task_id: &str) -> Result<bool> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM task_dependencies d \
         LEFT JOIN task_runs t ON t.id = d.depends_on_task_id \
         WHERE d.task_id = ? AND (t.status IS NULL OR t.status != 'completed')",
    )
    .bind(task_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::query(
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                cwd TEXT NOT NULL,
                parent_task_id TEXT,
                sub_session_id TEXT,
                created_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT,
                result TEXT,
                error TEXT,
                attempt_count INTEGER NOT NULL DEFAULT 0,
                verification_results TEXT,
                task_context_json TEXT,
                acceptance_criteria_json TEXT,
                spec_req_id TEXT,
                spec_title TEXT
            )",
        )
        .execute(&pool)
        .await
        .expect("create schema");
        pool
    }

    fn task(id: &str, status: &str) -> TaskRun {
        TaskRun {
            id: id.into(),
            session_id: "session-1".into(),
            title: format!("task {id}"),
            description: "test task".into(),
            status: status.into(),
            cwd: "/tmp/project".into(),
            parent_task_id: None,
            sub_session_id: None,
            created_at: "2026-07-08T00:00:00Z".into(),
            started_at: Some("2026-07-08T00:01:00Z".into()),
            completed_at: Some("2026-07-08T00:02:00Z".into()),
            result: Some("stale result".into()),
            error: Some("stale error".into()),
            attempt_count: 3,
            verification_results: Some(r#"[{"check":"test","passed":false}]"#.into()),
            task_context_json: None,
            acceptance_criteria_json: None,
            spec_req_id: None,
            spec_title: None,
        }
    }

    #[tokio::test]
    async fn retry_failed_tasks_resets_repairable_rows_to_pending() {
        let pool = test_pool().await;
        insert_task(&pool, &task("failed-1", "failed")).await.unwrap();
        insert_task(&pool, &task("cancelled-1", "cancelled")).await.unwrap();
        insert_task(&pool, &task("completed-1", "completed")).await.unwrap();

        let changed = retry_failed_tasks(&pool, "session-1").await.unwrap();
        assert_eq!(changed, 2);

        for id in ["failed-1", "cancelled-1"] {
            let repaired = get_task(&pool, id).await.unwrap().unwrap();
            assert_eq!(repaired.status, "pending");
            assert_eq!(repaired.started_at, None);
            assert_eq!(repaired.completed_at, None);
            assert_eq!(repaired.result, None);
            assert_eq!(repaired.error, None);
            assert_eq!(repaired.verification_results, None);
            assert_eq!(repaired.attempt_count, 3);
        }

        let untouched = get_task(&pool, "completed-1").await.unwrap().unwrap();
        assert_eq!(untouched.status, "completed");
        assert_eq!(untouched.result.as_deref(), Some("stale result"));
        assert_eq!(untouched.error.as_deref(), Some("stale error"));
    }
}
