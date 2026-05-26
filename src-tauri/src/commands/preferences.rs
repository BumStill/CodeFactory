// SPDX-License-Identifier: Apache-2.0
//! Structured user preferences — key→value scoped per cwd.
//!
//! Why a separate table rather than just a section of memory.md:
//! preferences are **typed** (we want to enumerate, sort, render as form
//! fields), **per-key updatable** (vs. blob editing), and **source-tagged**
//! so the UI can show what the AI proposed vs. what the user set manually.
//!
//! The AI reads these via `user_context::build()` and injects them into
//! every decomposition / execution prompt. The Profile page renders them
//! as editable rows so the user always knows what the AI "knows".

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{command, State};

use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreference {
    pub cwd: String,
    pub key: String,
    pub value: String,
    /// 'user' | 'ai' | 'default'
    pub source: String,
    pub updated_at: String,
}

/// The default set surfaced on first project open. These mirror the
/// placeholders the Profile page used to hard-code so existing users
/// don't see an empty preferences pane after upgrade.
pub const DEFAULT_PREFERENCES: &[(&str, &str)] = &[
    ("autonomy_level",    "medium"),
    ("communication_style", "concise"),
    ("testing_habit",     "tdd"),
    ("code_style",        ""),
];

/// Sentinel cwd value for the global scope. Preferences stored under this
/// cwd are merged with per-project preferences at build time (project wins
/// on conflict). Treated as a normal cwd row everywhere else — same table,
/// same upsert/delete commands — which keeps the surface tiny.
pub const GLOBAL_CWD: &str = "_global_";

/// Ensure default rows exist for a cwd. Idempotent — runs on every
/// list_user_preferences call so first-time projects auto-populate.
async fn seed_defaults_if_empty(
    pool: &sqlx::SqlitePool,
    cwd: &str,
) -> Result<(), AppError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_preferences WHERE cwd = ?",
    )
    .bind(cwd)
    .fetch_one(pool)
    .await?;
    if count > 0 {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    for (k, v) in DEFAULT_PREFERENCES {
        sqlx::query(
            "INSERT OR IGNORE INTO user_preferences \
             (cwd, key, value, source, updated_at) VALUES (?,?,?,'default',?)",
        )
        .bind(cwd)
        .bind(*k)
        .bind(*v)
        .bind(&now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[command]
pub async fn list_user_preferences(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<UserPreference>, AppError> {
    let pool = state.db.read().await;
    seed_defaults_if_empty(&pool, &cwd).await?;
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT cwd, key, value, source, updated_at \
         FROM user_preferences WHERE cwd = ? ORDER BY key",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(cwd, key, value, source, updated_at)| UserPreference {
            cwd, key, value, source, updated_at,
        })
        .collect())
}

#[command]
pub async fn upsert_user_preference(
    cwd: String,
    key: String,
    value: String,
    source: Option<String>,
    state: State<'_, AppState>,
) -> Result<UserPreference, AppError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AppError::Other("preference key cannot be empty".into()));
    }
    let source = source.unwrap_or_else(|| "user".into());
    let now = Utc::now().to_rfc3339();
    let pool = state.db.read().await;
    sqlx::query(
        "INSERT INTO user_preferences (cwd, key, value, source, updated_at) \
         VALUES (?,?,?,?,?) \
         ON CONFLICT(cwd, key) DO UPDATE SET \
           value = excluded.value, \
           source = excluded.source, \
           updated_at = excluded.updated_at",
    )
    .bind(&cwd)
    .bind(key)
    .bind(&value)
    .bind(&source)
    .bind(&now)
    .execute(&*pool)
    .await?;
    Ok(UserPreference {
        cwd,
        key: key.into(),
        value,
        source,
        updated_at: now,
    })
}

#[command]
pub async fn delete_user_preference(
    cwd: String,
    key: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await;
    sqlx::query("DELETE FROM user_preferences WHERE cwd = ? AND key = ?")
        .bind(&cwd)
        .bind(&key)
        .execute(&*pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE user_preferences (
                cwd TEXT, key TEXT, value TEXT, source TEXT, updated_at TEXT,
                PRIMARY KEY (cwd, key)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn seed_runs_once() {
        let pool = fresh_pool().await;
        seed_defaults_if_empty(&pool, "/proj").await.unwrap();
        seed_defaults_if_empty(&pool, "/proj").await.unwrap(); // second call no-op
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_preferences WHERE cwd='/proj'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n as usize, DEFAULT_PREFERENCES.len());
    }

    #[tokio::test]
    async fn upsert_updates_existing_key() {
        let pool = fresh_pool().await;
        seed_defaults_if_empty(&pool, "/proj").await.unwrap();
        // Hand-roll an upsert against the test pool (the command needs AppState)
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO user_preferences (cwd, key, value, source, updated_at) \
             VALUES (?,?,?,?,?) ON CONFLICT(cwd, key) DO UPDATE SET value=excluded.value, source=excluded.source, updated_at=excluded.updated_at",
        )
        .bind("/proj").bind("autonomy_level").bind("high").bind("user").bind(&now)
        .execute(&pool).await.unwrap();
        let row: (String, String) = sqlx::query_as(
            "SELECT value, source FROM user_preferences WHERE cwd='/proj' AND key='autonomy_level'",
        )
        .fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, "high");
        assert_eq!(row.1, "user");
    }
}
