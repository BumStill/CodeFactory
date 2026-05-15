// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use tauri::State;
use uuid::Uuid;

use crate::errors::AppError;
use crate::storage::{Message, Session};
use crate::AppState;

#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> Result<Vec<Session>, AppError> {
    let pool = state.db.read().await;
    // Filter out subagent-spawned child sessions; only show top-level chats here.
    let sessions = sqlx::query_as::<_, Session>(
        "SELECT * FROM sessions WHERE parent_session_id IS NULL ORDER BY updated_at DESC LIMIT 100",
    )
    .fetch_all(&*pool)
    .await?;
    Ok(sessions)
}

#[tauri::command]
pub async fn create_session(
    title: String,
    cwd: String,
    model_id: String,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    let pool = state.db.read().await;
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp_millis();

    sqlx::query(
        "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at) VALUES (?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&title)
    .bind(&cwd)
    .bind(&model_id)
    .bind(now)
    .bind(now)
    .execute(&*pool)
    .await?;

    let session = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ?")
        .bind(&id)
        .fetch_one(&*pool)
        .await?;
    Ok(session)
}

#[tauri::command]
pub async fn get_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    let pool = state.db.read().await;
    let session = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ?")
        .bind(&session_id)
        .fetch_one(&*pool)
        .await?;
    Ok(session)
}

#[tauri::command]
pub async fn delete_session(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await;
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(&session_id)
        .execute(&*pool)
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn update_session_model(
    session_id: String,
    model_id: String,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    let pool = state.db.read().await;
    let now = Utc::now().timestamp_millis();
    sqlx::query("UPDATE sessions SET model_id = ?, updated_at = ? WHERE id = ?")
        .bind(&model_id)
        .bind(now)
        .bind(&session_id)
        .execute(&*pool)
        .await?;

    let session = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ?")
        .bind(&session_id)
        .fetch_one(&*pool)
        .await?;
    Ok(session)
}

#[tauri::command]
pub async fn update_session_title(
    session_id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    let pool = state.db.read().await;
    let now = Utc::now().timestamp_millis();
    sqlx::query("UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?")
        .bind(&title)
        .bind(now)
        .bind(&session_id)
        .execute(&*pool)
        .await?;

    let session = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ?")
        .bind(&session_id)
        .fetch_one(&*pool)
        .await?;
    Ok(session)
}

#[tauri::command]
pub async fn get_messages(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Message>, AppError> {
    let pool = state.db.read().await;
    let messages = sqlx::query_as::<_, Message>(
        "SELECT * FROM messages WHERE session_id = ? ORDER BY created_at ASC",
    )
    .bind(&session_id)
    .fetch_all(&*pool)
    .await?;
    Ok(messages)
}
