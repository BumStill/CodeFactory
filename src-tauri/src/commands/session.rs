// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use tauri::State;
use uuid::Uuid;

use crate::errors::AppError;
use crate::storage::{Message, Session};
use crate::AppState;

fn resolve_new_session_model(
    settings: &crate::config::Settings,
    requested_model: &str,
) -> Option<String> {
    settings.resolve_model_for_endpoint(&settings.default_endpoint, requested_model)
}

#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> Result<Vec<Session>, AppError> {
    let pool = state.db.read().await;
    // Filter out subagent-spawned child sessions AND ephemeral quick-task
    // sessions; the home page Recent Projects list should only show
    // "real" project sessions the user explicitly created.
    let sessions = sqlx::query_as::<_, Session>(
        "SELECT * FROM sessions \
         WHERE parent_session_id IS NULL AND kind != 'quick' \
         ORDER BY updated_at DESC LIMIT 100",
    )
    .fetch_all(&*pool)
    .await?;
    Ok(sessions)
}

/// Return the single persistent Quick Task session, creating it on first
/// use. The cwd lives under the user's home so the AI has a safe scratch
/// area (created on demand). Reused across visits — the user gets a
/// continuous "scratch chat" history that doesn't pollute Recent Projects.
#[tauri::command]
pub async fn get_or_create_quick_session(
    model_id: String,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    // Try to find an existing quick session first.
    {
        let pool = state.db.read().await;
        if let Ok(existing) = sqlx::query_as::<_, Session>(
            "SELECT * FROM sessions WHERE kind = 'quick' ORDER BY updated_at DESC LIMIT 1",
        )
        .fetch_one(&*pool)
        .await
        {
            return Ok(existing);
        }
    }

    // Create one. cwd = ~/.codefactory/quick — auto-mkdir so the agent's
    // working directory is valid and write tools have a safe home.
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::Other("home dir not resolvable".into()))?;
    let quick_dir = home.join(".codefactory").join("quick");
    std::fs::create_dir_all(&quick_dir).map_err(|e| {
        AppError::Other(format!("create quick-task dir failed: {e}"))
    })?;

    let settings = state.settings.read().await.clone();
    let model_id = resolve_new_session_model(&settings, &model_id).ok_or_else(|| {
        AppError::Other(format!(
            "No model configured for endpoint '{}'. Please choose a model in the picker.",
            settings.default_endpoint
        ))
    })?;

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp_millis();
    let pool = state.db.read().await;
    sqlx::query(
        "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at, kind) \
         VALUES (?,?,?,?,?,?,'quick')",
    )
    .bind(&id)
    .bind("快速任务")
    .bind(quick_dir.to_string_lossy().to_string())
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

/// Create a *fresh* Quick Task session. Multi-session: each gets its own
/// scratch dir `~/.codefactory/quick/<id>`. Unlike get_or_create_quick_session
/// this never reuses an existing one — it's the "new quick task" action.
#[tauri::command]
pub async fn create_quick_session(
    model_id: String,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    let settings = state.settings.read().await.clone();
    let resolved_model = resolve_new_session_model(&settings, &model_id).ok_or_else(|| {
        AppError::Other(format!(
            "No model configured for endpoint '{}'. Please choose a model in the picker.",
            settings.default_endpoint
        ))
    })?;
    let id = Uuid::new_v4().to_string();
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::Other("home dir not resolvable".into()))?;
    let quick_dir = home.join(".codefactory").join("quick").join(&id);
    std::fs::create_dir_all(&quick_dir)
        .map_err(|e| AppError::Other(format!("create quick-task dir failed: {e}")))?;
    let now = Utc::now().timestamp_millis();
    let pool = state.db.read().await;
    sqlx::query(
        "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at, kind) \
         VALUES (?,?,?,?,?,?,'quick')",
    )
    .bind(&id)
    .bind("快速任务")
    .bind(quick_dir.to_string_lossy().to_string())
    .bind(&resolved_model)
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

/// List Quick Task sessions (most-recent first) for the quick-session switcher.
#[tauri::command]
pub async fn list_quick_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<Session>, AppError> {
    let pool = state.db.read().await;
    let sessions = sqlx::query_as::<_, Session>(
        "SELECT * FROM sessions WHERE kind = 'quick' ORDER BY updated_at DESC LIMIT 50",
    )
    .fetch_all(&*pool)
    .await?;
    Ok(sessions)
}

/// Set (or clear, with `None`) a session's per-session reasoning-effort
/// override. The agent reads this and falls back to the global default.
/// Intentionally does NOT bump `updated_at`: changing the effort is a settings
/// tweak, not activity, so it shouldn't resurface the session to the top of the
/// quick-session switcher (which orders by updated_at).
#[tauri::command]
pub async fn update_session_reasoning_effort(
    session_id: String,
    effort: Option<String>,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    let pool = state.db.read().await;
    sqlx::query("UPDATE sessions SET reasoning_effort = ? WHERE id = ?")
        .bind(&effort)
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
pub async fn create_session(
    title: String,
    cwd: String,
    model_id: String,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    let settings = state.settings.read().await.clone();
    let resolved_model = resolve_new_session_model(&settings, &model_id).ok_or_else(|| {
        AppError::Other(format!(
            "No model configured for endpoint '{}'. Please choose a model in the picker.",
            settings.default_endpoint
        ))
    })?;
    if resolved_model != model_id {
        tracing::warn!(
            "create_session: repaired requested model '{}' to endpoint '{}' active model '{}'",
            model_id,
            settings.default_endpoint,
            resolved_model
        );
    }

    let pool = state.db.read().await;
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp_millis();

    sqlx::query(
        "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at) VALUES (?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(&title)
    .bind(&cwd)
    .bind(&resolved_model)
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

    // Mirror the choice to the endpoint's active_model so it persists across
    // app restarts AND survives the user switching to a different endpoint
    // and back — the bug we're fixing in this release.
    {
        let mut settings = state.settings.write().await;
        let endpoint_name = settings.default_endpoint.clone();
        if settings.set_active_model(&endpoint_name, &model_id) {
            // Best-effort persist — failure here doesn't undo the DB write.
            if let Err(e) = crate::config::settings::save(&settings) {
                tracing::warn!("Failed to persist active_model: {e}");
            }
        }
    }

    Ok(session)
}

/// Explicit endpoint-scoped model setter. Used by the ModelPicker when the
/// user picks a model without an active session, and by Settings when the
/// user changes their default per-endpoint.
#[tauri::command]
pub async fn set_endpoint_active_model(
    endpoint_name: String,
    model_id: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let mut settings = state.settings.write().await;
    if settings.set_active_model(&endpoint_name, &model_id) {
        crate::config::settings::save(&settings)?;
    }
    Ok(())
}

/// Read the active model for a given endpoint. Used by the frontend to
/// auto-update the picker when the user switches endpoints.
#[tauri::command]
pub async fn get_endpoint_active_model(
    endpoint_name: String,
    state: State<'_, AppState>,
) -> Result<String, AppError> {
    let settings = state.settings.read().await;
    Ok(settings.active_model_for(&endpoint_name))
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
        "SELECT * FROM messages WHERE session_id = ? ORDER BY created_at ASC, rowid ASC",
    )
    .bind(&session_id)
    .fetch_all(&*pool)
    .await?;
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_model_falls_back_to_the_active_model_for_the_current_endpoint() {
        let mut settings = crate::config::Settings::default();
        settings.default_endpoint = "deepseek".into();
        settings.endpoints.insert(
            "deepseek".into(),
            crate::config::settings::Endpoint {
                base_url: "https://api.deepseek.com".into(),
                key_ref: Some("codefactory.endpoint.deepseek".into()),
                api_style: crate::config::settings::ApiStyle::Openai,
                custom_models: vec![],
                active_model: Some("deepseek-v4-pro".into()),
            },
        );

        let resolved = resolve_new_session_model(
            &settings,
            "anthropic/claude-opus-4-7",
        );

        assert_eq!(resolved.as_deref(), Some("deepseek-v4-pro"));
    }
}
