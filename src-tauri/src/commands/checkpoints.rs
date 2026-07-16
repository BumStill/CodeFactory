// SPDX-License-Identifier: Apache-2.0
//! Per-message git checkpoint commands — give the user a one-click rollback
//! when the agent takes a wrong turn. See agent/checkpoint.rs for the
//! snapshot mechanics.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::State;
use uuid::Uuid;

use crate::agent::checkpoint::{self, CheckpointFileChange, CheckpointInfo};
use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Serialize, Deserialize)]
struct CheckpointRow {
    id: String,
    session_id: String,
    message_id: Option<String>,
    cwd: String,
    git_sha: String,
    label: String,
    created_at: String,
    reverted: i64,
}

impl From<CheckpointRow> for CheckpointInfo {
    fn from(r: CheckpointRow) -> Self {
        CheckpointInfo {
            id: r.id,
            session_id: r.session_id,
            message_id: r.message_id,
            cwd: r.cwd,
            git_sha: r.git_sha,
            label: r.label,
            created_at: r.created_at,
            reverted: r.reverted != 0,
        }
    }
}

/// Create a checkpoint right before sending a user message off to the
/// agent. Returns `None` when the cwd isn't a git repo (silent no-op).
///
/// Called by `commands::chat::send_message`. Also exposed as a Tauri
/// command so the UI can request a manual checkpoint when convenient.
#[tauri::command]
pub async fn create_checkpoint(
    session_id: String,
    message_id: Option<String>,
    cwd: String,
    label: String,
    state: State<'_, AppState>,
) -> Result<Option<CheckpointInfo>, AppError> {
    let sha = match checkpoint::create(Path::new(&cwd), &label) {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(None),
        Err(e) => {
            // Don't fail the whole chat just because git misbehaved —
            // surface it as a log line and continue without a checkpoint.
            tracing::warn!("checkpoint creation failed: {e}");
            return Ok(None);
        }
    };

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    let pool = state.db.read().await;
    sqlx::query(
        "INSERT INTO checkpoints (id, session_id, message_id, cwd, git_sha, label, created_at, reverted)
         VALUES (?, ?, ?, ?, ?, ?, ?, 0)",
    )
    .bind(&id)
    .bind(&session_id)
    .bind(&message_id)
    .bind(&cwd)
    .bind(&sha)
    .bind(&label)
    .bind(&now)
    .execute(&*pool)
    .await?;

    Ok(Some(CheckpointInfo {
        id,
        session_id,
        message_id,
        cwd,
        git_sha: sha,
        label,
        created_at: now,
        reverted: false,
    }))
}

#[tauri::command]
pub async fn list_checkpoints(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<CheckpointInfo>, AppError> {
    let pool = state.db.read().await;
    let rows: Vec<CheckpointRow> = sqlx::query_as::<_, CheckpointRow>(
        "SELECT id, session_id, message_id, cwd, git_sha, label, created_at, reverted
         FROM checkpoints WHERE session_id = ?
         ORDER BY created_at DESC",
    )
    .bind(&session_id)
    .fetch_all(&*pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn checkpoint_changeset(
    checkpoint_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<CheckpointFileChange>, AppError> {
    let pool = state.db.read().await;
    let row: CheckpointRow = sqlx::query_as::<_, CheckpointRow>(
        "SELECT id, session_id, message_id, cwd, git_sha, label, created_at, reverted
         FROM checkpoints WHERE id = ?",
    )
    .bind(&checkpoint_id)
    .fetch_one(&*pool)
    .await?;
    checkpoint::changeset(Path::new(&row.cwd), &row.git_sha)
        .map_err(|e| AppError::Other(e))
}

#[tauri::command]
pub async fn revert_checkpoint(
    checkpoint_id: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await;
    let row: CheckpointRow = sqlx::query_as::<_, CheckpointRow>(
        "SELECT id, session_id, message_id, cwd, git_sha, label, created_at, reverted
         FROM checkpoints WHERE id = ?",
    )
    .bind(&checkpoint_id)
    .fetch_one(&*pool)
    .await?;

    checkpoint::revert(Path::new(&row.cwd), &row.git_sha).map_err(|e| AppError::Other(e))?;

    sqlx::query("UPDATE checkpoints SET reverted = 1 WHERE id = ?")
        .bind(&checkpoint_id)
        .execute(&*pool)
        .await?;

    // Point-in-time resume-journal invalidation: the whole-tree restore just
    // wiped every edit made after this checkpoint, so every task completed
    // at/after it must re-run instead of replaying from cache. Fires once,
    // here — the downstream cascade falls out of the next resume pass.
    match crate::agent::journal::invalidate_on_revert(&pool, &row.session_id, &row.created_at).await
    {
        Ok(n) if n > 0 => {
            tracing::info!(
                "checkpoint revert invalidated {n} completed task(s) in session {}",
                row.session_id
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("journal invalidation on revert failed (non-fatal): {e}"),
    }
    Ok(())
}

// sqlx::FromRow derive lives in storage layer pattern; do it explicitly here
// to avoid a wide derive macro for a single internal row.
impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for CheckpointRow {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        use sqlx::Row;
        Ok(Self {
            id: row.try_get("id")?,
            session_id: row.try_get("session_id")?,
            message_id: row.try_get("message_id")?,
            cwd: row.try_get("cwd")?,
            git_sha: row.try_get("git_sha")?,
            label: row.try_get("label")?,
            created_at: row.try_get("created_at")?,
            reverted: row.try_get("reverted")?,
        })
    }
}
