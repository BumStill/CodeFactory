// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use std::sync::Arc;

use crate::agent::AgentLoop;
use crate::errors::AppError;
use crate::mcp::McpManager;
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
    mcp: State<'_, Arc<McpManager>>,
) -> Result<(), AppError> {
    let settings = state.settings.read().await.clone();

    // Persist user message
    let is_first_message = {
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

        // Check if this is the first message in the session
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?",
        )
        .bind(&session_id)
        .fetch_one(&*pool)
        .await?;
        count.0 == 1
    };

    // Fetch session for cwd + model
    let session = {
        let pool = state.db.read().await;
        sqlx::query_as::<_, crate::storage::Session>("SELECT * FROM sessions WHERE id = ?")
            .bind(&session_id)
            .fetch_one(&*pool)
            .await?
    };

    // Auto-checkpoint: capture the working-tree snapshot before the agent
    // starts so the user can revert with one click if anything goes wrong.
    // Best-effort: failures (non-git cwd, missing git binary, locked refs)
    // log and continue — we don't want to block the chat over a missing
    // safety net.
    {
        use std::path::Path;
        let label: String = content.chars().take(80).collect();
        match crate::agent::checkpoint::create(Path::new(&session.cwd), &label) {
            Ok(Some(sha)) => {
                let cp_id = Uuid::new_v4().to_string();
                let now = Utc::now().to_rfc3339();
                let pool = state.db.read().await;
                if let Err(e) = sqlx::query(
                    "INSERT INTO checkpoints (id, session_id, message_id, cwd, git_sha, label, created_at, reverted)
                     VALUES (?, ?, NULL, ?, ?, ?, ?, 0)",
                )
                .bind(&cp_id)
                .bind(&session_id)
                .bind(&session.cwd)
                .bind(&sha)
                .bind(&label)
                .bind(&now)
                .execute(&*pool)
                .await
                {
                    tracing::warn!("checkpoint INSERT failed: {e}");
                } else {
                    app.emit("checkpoint-created", &session_id).ok();
                }
            }
            Ok(None) => {} // cwd not a git repo — silently skip
            Err(e) => tracing::warn!("checkpoint create failed: {e}"),
        }
    }

    // Auto-update title from first message content
    if is_first_message {
        let new_title: String = content
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(40)
            .collect::<String>()
            .trim()
            .to_string();

        if !new_title.is_empty() {
            let pool = state.db.read().await;
            let now = Utc::now().timestamp_millis();
            if let Ok(()) = sqlx::query(
                "UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&new_title)
            .bind(now)
            .bind(&session_id)
            .execute(&*pool)
            .await
            .map(|_| ())
            {
                if let Ok(updated_session) = sqlx::query_as::<_, crate::storage::Session>(
                    "SELECT * FROM sessions WHERE id = ?",
                )
                .bind(&session_id)
                .fetch_one(&*pool)
                .await
                {
                    let event_name = format!("session_updated:{}", session_id);
                    app.emit(&event_name, &updated_session).ok();
                }
            }
        }
    }

    // Resolve endpoint + key
    let endpoint = settings
        .endpoints
        .get(&settings.default_endpoint)
        .ok_or_else(|| AppError::Other("No default endpoint configured".into()))?
        .clone();

    let api_style = endpoint.api_style.clone();

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
    let mcp_manager: Arc<McpManager> = Arc::clone(&mcp);

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
            api_style,
            std::path::PathBuf::from(session.cwd),
            settings_state,
            pending_permissions,
            mcp_manager,
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
