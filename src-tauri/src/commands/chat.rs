// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::agent::AgentLoop;
use crate::errors::AppError;
use crate::openrouter::types::StreamEvent;
use crate::AppState;

#[tauri::command]
pub async fn respond_to_permission(
    tool_call_id: String,
    allow: bool,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let Some(sender) = state.pending_permissions.lock().await.remove(&tool_call_id) else {
        return Err(AppError::Other(format!(
            "Permission request '{tool_call_id}' is no longer active"
        )));
    };

    sender
        .send(allow)
        .map_err(|_| AppError::Other("Permission request receiver closed".into()))
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    session_id: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let settings = state.settings.read().await.clone();

    // Persist user message
    {
        let pool = state.db.read().await;
        let msg_id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,?,?,?,?)",
        )
        .bind(&msg_id)
        .bind(&session_id)
        .bind("user")
        .bind(&content)
        .bind(now)
        .execute(&*pool)
        .await?;
    }

    // Fetch session for cwd + model
    let session = {
        let pool = state.db.read().await;
        sqlx::query_as::<_, crate::storage::Session>("SELECT * FROM sessions WHERE id = ?")
            .bind(&session_id)
            .fetch_one(&*pool)
            .await?
    };

    // Resolve endpoint + key
    let endpoint = settings
        .endpoints
        .get(&settings.default_endpoint)
        .ok_or_else(|| AppError::Other("No default endpoint configured".into()))?
        .clone();

    let key_ref = endpoint
        .key_ref
        .clone()
        .unwrap_or_else(|| format!("codefactory.endpoint.{}", settings.default_endpoint));
    let api_key = crate::secrets::get_key(&key_ref)?.unwrap_or_default();

    tracing::info!(
        "send_message: endpoint={} model={} key_ref={} key_len={}",
        endpoint.base_url,
        session.model_id,
        key_ref,
        api_key.len(),
    );

    if api_key.is_empty() {
        return Err(AppError::Other(format!(
            "API key not found for key_ref '{}'. Please configure it in Settings.",
            key_ref
        )));
    }

    // Fetch history
    let history = {
        let pool = state.db.read().await;
        sqlx::query_as::<_, crate::storage::Message>(
            "SELECT * FROM messages WHERE session_id = ? ORDER BY created_at ASC",
        )
        .bind(&session_id)
        .fetch_all(&*pool)
        .await?
    };

    let db = state.db.read().await.clone();
    let settings_state = state.settings.clone();
    let pending_permissions = state.pending_permissions.clone();

    // Spawn agent loop (non-blocking); emit Error event to frontend if it fails
    let app_clone = app.clone();
    let event_name = format!("stream:{}", session_id);
    let session_id_clone = session_id.clone();
    tokio::spawn(async move {
        let mut agent = AgentLoop::new(
            app,
            db,
            session_id_clone,
            session.model_id,
            endpoint.base_url,
            api_key,
            std::path::PathBuf::from(session.cwd),
            settings_state,
            pending_permissions,
        );
        if let Err(e) = agent.run(history).await {
            tracing::error!("Agent loop error: {e:#}");
            app_clone
                .emit(
                    &event_name,
                    StreamEvent::Error {
                        message: e.to_string(),
                    },
                )
                .ok();
        }
    });

    Ok(())
}
