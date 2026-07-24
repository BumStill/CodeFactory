// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use serde::Serialize;
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

fn draft_title(first_message: &str) -> String {
    let title: String = first_message
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(40)
        .collect::<String>()
        .trim()
        .to_string();
    if title.is_empty() {
        "新会话".into()
    } else {
        title
    }
}

async fn materialize_session_and_first_message(
    pool: &sqlx::SqlitePool,
    draft_id: &str,
    mode: &str,
    cwd: &str,
    model_id: &str,
    first_message: &str,
    now: i64,
) -> Result<Session, AppError> {
    if let Some(existing) = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ?")
        .bind(draft_id)
        .fetch_optional(pool)
        .await?
    {
        return Ok(existing);
    }

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO sessions
         (id, title, cwd, model_id, created_at, updated_at, kind)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(draft_id)
    .bind(draft_title(first_message))
    .bind(cwd)
    .bind(model_id)
    .bind(now)
    .bind(now)
    .bind(mode)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at)
         VALUES (?, ?, 'user', ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(draft_id)
    .bind(first_message)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let session = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ?")
        .bind(draft_id)
        .fetch_one(pool)
        .await?;
    Ok(session)
}

#[tauri::command]
pub async fn materialize_draft_session(
    draft_id: String,
    mode: String,
    cwd: Option<String>,
    model_id: String,
    first_message: String,
    state: State<'_, AppState>,
) -> Result<Session, AppError> {
    if mode != "quick" && mode != "project" {
        return Err(AppError::Other(format!("Unsupported draft mode '{mode}'")));
    }
    if first_message.trim().is_empty() {
        return Err(AppError::Other("First message cannot be empty".into()));
    }

    let settings = state.settings.read().await.clone();
    let resolved_model = resolve_new_session_model(&settings, &model_id).ok_or_else(|| {
        AppError::Other(format!(
            "No model configured for endpoint '{}'. Please choose a model in the picker.",
            settings.default_endpoint
        ))
    })?;

    let resolved_cwd = if mode == "project" {
        let path = cwd.filter(|value| !value.trim().is_empty()).ok_or_else(|| {
            AppError::Other("Project draft requires a working directory".into())
        })?;
        let path_buf = std::path::PathBuf::from(&path);
        if !path_buf.is_dir() {
            return Err(AppError::Other(format!("Project directory does not exist: {path}")));
        }
        path
    } else {
        let home = dirs::home_dir()
            .ok_or_else(|| AppError::Other("home dir not resolvable".into()))?;
        let quick_dir = home
            .join(".codefactory")
            .join("quick")
            .join(&draft_id);
        std::fs::create_dir_all(&quick_dir)
            .map_err(|error| AppError::Other(format!("create quick-task dir failed: {error}")))?;
        quick_dir.to_string_lossy().to_string()
    };

    let pool = state.db.read().await;
    materialize_session_and_first_message(
        &pool,
        &draft_id,
        &mode,
        &resolved_cwd,
        &resolved_model,
        &first_message,
        Utc::now().timestamp_millis(),
    )
    .await
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

const DEFAULT_MESSAGE_PAGE_USER_TURNS: i64 = 8;
const MAX_MESSAGE_PAGE_USER_TURNS: i64 = 32;
const MAX_MESSAGE_PAGE_ROWS: i64 = 400;
const MAX_MESSAGE_PAGE_SERIALIZED_BYTES: usize = 2 * 1024 * 1024;
const MAX_MESSAGE_FIELD_BYTES: usize = 128 * 1024;
const PAYLOAD_OMITTED: &str = "[older tool output omitted from this history page]";

#[derive(Debug, Clone, Serialize)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub has_more: bool,
    pub next_before_rowid: Option<i64>,
    pub truncated: bool,
}

#[derive(sqlx::FromRow)]
struct MessagePageRow {
    page_rowid: i64,
    id: String,
    session_id: String,
    role: String,
    content: String,
    model_id: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    tool_calls: Option<String>,
    reasoning_content: Option<String>,
    completion_state: Option<String>,
    created_at: i64,
}

impl MessagePageRow {
    fn into_parts(self) -> (i64, Message) {
        (
            self.page_rowid,
            Message {
                id: self.id,
                session_id: self.session_id,
                role: self.role,
                content: self.content,
                model_id: self.model_id,
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                tool_calls: self.tool_calls,
                reasoning_content: self.reasoning_content,
                completion_state: self.completion_state,
                created_at: self.created_at,
            },
        )
    }
}

async fn load_message_page(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    before_rowid: Option<i64>,
    user_turn_limit: i64,
) -> Result<MessagePage, sqlx::Error> {
    // Freeze the newest-page upper bound before any later await. Other
    // CodeFactory processes may share this SQLite file; using i64::MAX would
    // allow rows appended between the boundary query and final SELECT to leak
    // into this page and exceed the 400-row contract.
    let upper_rowid = match before_rowid {
        Some(cursor) => cursor,
        None => sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MAX(rowid) FROM messages WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(pool)
        .await?
        .and_then(|rowid| rowid.checked_add(1))
        .unwrap_or(i64::MAX),
    };
    load_message_page_below(
        pool,
        session_id,
        upper_rowid,
        user_turn_limit,
    )
    .await
}

async fn load_message_page_below(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    upper_rowid: i64,
    user_turn_limit: i64,
) -> Result<MessagePage, sqlx::Error> {
    let turn_limit = user_turn_limit.clamp(1, MAX_MESSAGE_PAGE_USER_TURNS);

    // A persisted completion-gate instruction may use role=user, but it is not
    // a user-authored turn. Page only on real user messages so tool replay and
    // completion recovery rows stay attached to their owning turn.
    let turn_boundary = sqlx::query_scalar::<_, i64>(
        "SELECT rowid
         FROM messages
         WHERE session_id = ?
           AND rowid < ?
           AND role = 'user'
           AND (completion_state IS NULL OR completion_state = '')
         ORDER BY rowid DESC
         LIMIT 1 OFFSET ?",
    )
    .bind(session_id)
    .bind(upper_rowid)
    .bind(turn_limit - 1)
    .fetch_optional(pool)
    .await?;
    let earliest_rowid = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MIN(rowid)
         FROM messages
         WHERE session_id = ? AND rowid < ?",
    )
    .bind(session_id)
    .bind(upper_rowid)
    .fetch_one(pool)
    .await?;
    let page_start = turn_boundary.or(earliest_rowid);

    let Some(page_start) = page_start else {
        return Ok(MessagePage {
            messages: Vec::new(),
            has_more: false,
            next_before_rowid: None,
            truncated: false,
        });
    };

    // A single tool-heavy turn can itself contain thousands of rows. The
    // user-turn boundary preserves normal ownership semantics, while this
    // second hard budget guarantees that no legacy or pathological session
    // can cross the Tauri bridge unbounded. Loading the next page reconnects
    // any declaration/replay pair split by this emergency row boundary.
    let bounded_start = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MIN(rowid) FROM (
            SELECT rowid
            FROM messages
            WHERE session_id = ? AND rowid >= ? AND rowid < ?
            ORDER BY rowid DESC
            LIMIT ?
         )",
    )
    .bind(session_id)
    .bind(page_start)
    .bind(upper_rowid)
    .bind(MAX_MESSAGE_PAGE_ROWS)
    .fetch_one(pool)
    .await?
    .unwrap_or(page_start);
    let truncated = bounded_start > page_start;

    // Fetch cursor metadata and DTO fields in one SQLite statement. Separate
    // SELECTs can observe different snapshots when another CodeFactory
    // process writes the shared database between awaits.
    let rows = sqlx::query_as::<_, MessagePageRow>(
        "SELECT rowid AS page_rowid,
                id, session_id, role, content, model_id, input_tokens,
                output_tokens, tool_calls, reasoning_content,
                completion_state, created_at
         FROM messages
         WHERE session_id = ? AND rowid >= ? AND rowid < ?
         ORDER BY rowid ASC",
    )
    .bind(session_id)
    .bind(bounded_start)
    .bind(upper_rowid)
    .fetch_all(pool)
    .await?;
    let (mut rowids, mut messages): (Vec<_>, Vec<_>) =
        rows.into_iter().map(MessagePageRow::into_parts).unzip();
    let field_truncated = bound_message_page_fields(&mut messages);
    let mut effective_start = bounded_start;
    let has_more = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(
            SELECT 1 FROM messages
            WHERE session_id = ? AND rowid < ?
         )",
    )
    .bind(session_id)
    .bind(bounded_start)
    .fetch_one(pool)
    .await?
        != 0;

    let mut page = MessagePage {
        messages,
        has_more,
        next_before_rowid: has_more.then_some(effective_start),
        truncated: truncated || field_truncated,
    };

    // Enforce the bridge contract against the exact serialized DTO, not a
    // content-only estimate. If several individually valid rows exceed the
    // page budget, move the oldest rows behind the next cursor instead of
    // destroying their contents; the next "load older" request can retrieve
    // them intact.
    while page.messages.len() > 1
        && serde_json::to_vec(&page)
            .map(|bytes| bytes.len() > MAX_MESSAGE_PAGE_SERIALIZED_BYTES)
            .unwrap_or(true)
    {
        page.messages.remove(0);
        rowids.remove(0);
        effective_start = rowids[0];
        page.has_more = true;
        page.next_before_rowid = Some(effective_start);
        page.truncated = true;
    }

    // A single row is field-bounded above, so normal app-authored metadata
    // leaves ample room. Keep a debug assertion and a release-safe fallback
    // for a corrupt database containing megabyte-sized metadata fields.
    if serde_json::to_vec(&page)
        .map(|bytes| bytes.len() > MAX_MESSAGE_PAGE_SERIALIZED_BYTES)
        .unwrap_or(true)
    {
        if let Some(message) = page.messages.first_mut() {
            message.id = truncate_utf8(&message.id, 4 * 1024);
            message.session_id = truncate_utf8(&message.session_id, 4 * 1024);
            message.role = truncate_utf8(&message.role, 128);
            message.model_id = message
                .model_id
                .take()
                .map(|value| truncate_utf8(&value, 4 * 1024));
            message.completion_state = message
                .completion_state
                .take()
                .map(|value| truncate_utf8(&value, 4 * 1024));
            page.truncated = true;
        }
    }
    debug_assert!(
        serde_json::to_vec(&page)
            .map(|bytes| bytes.len() <= MAX_MESSAGE_PAGE_SERIALIZED_BYTES)
            .unwrap_or(false),
        "MessagePage must stay within the Tauri bridge payload budget"
    );
    Ok(page)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    if max_bytes <= PAYLOAD_OMITTED.len() {
        return PAYLOAD_OMITTED[..max_bytes.min(PAYLOAD_OMITTED.len())].to_string();
    }
    let mut end = max_bytes - PAYLOAD_OMITTED.len();
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], PAYLOAD_OMITTED)
}

fn bound_replay_content(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    if let Ok(serde_json::Value::Object(mut replay)) =
        serde_json::from_str::<serde_json::Value>(value)
    {
        replay.insert(
            "content".into(),
            serde_json::Value::String(PAYLOAD_OMITTED.into()),
        );
        if let Some(tool_call_id) = replay
            .get("tool_call_id")
            .and_then(|field| field.as_str())
            .map(|value| truncate_utf8(value, 4 * 1024))
        {
            replay.insert(
                "tool_call_id".into(),
                serde_json::Value::String(tool_call_id),
            );
        }
        let bounded = serde_json::Value::Object(replay);
        if let Ok(serialized) = serde_json::to_string(&bounded) {
            if serialized.len() <= max_bytes {
                return serialized;
            }
        }
    }
    serde_json::json!({
        "content": PAYLOAD_OMITTED,
        "status": "error"
    })
    .to_string()
}

fn bound_message_page_fields(messages: &mut [Message]) -> bool {
    let mut truncated = false;

    // reasoning_content is provider replay state and has never been consumed
    // by the chat UI, so it must not cross the bridge at all.
    for message in messages.iter_mut() {
        message.reasoning_content = None;

        if let Some(tool_calls) = message.tool_calls.as_mut() {
            if tool_calls.len() > MAX_MESSAGE_FIELD_BYTES {
                *tool_calls = "[]".into();
                truncated = true;
            }
        }

        if message.content.len() > MAX_MESSAGE_FIELD_BYTES {
            message.content = if message.role == "tool" {
                bound_replay_content(&message.content, MAX_MESSAGE_FIELD_BYTES)
            } else {
                truncate_utf8(&message.content, MAX_MESSAGE_FIELD_BYTES)
            };
            truncated = true;
        }
    }

    truncated
}

#[tauri::command]
pub async fn get_message_page(
    session_id: String,
    before_rowid: Option<i64>,
    user_turn_limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<MessagePage, AppError> {
    let pool = state.db.read().await;
    Ok(load_message_page(
        &pool,
        &session_id,
        before_rowid,
        user_turn_limit.unwrap_or(DEFAULT_MESSAGE_PAGE_USER_TURNS),
    )
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn materialization_atomically_persists_one_session_and_one_first_message() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open db");
        create_materialization_schema(&pool).await;

        let session = materialize_session_and_first_message(
            &pool,
            "draft-1",
            "quick",
            "/tmp/quick/draft-1",
            "deepseek-v4",
            "第一条真实消息",
            123,
        )
        .await
        .expect("materialize");

        assert_eq!(session.id, "draft-1");
        assert_eq!(session.kind, "quick");
        let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        let message_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(session_count, 1);
        assert_eq!(message_count, 1);
    }

    #[tokio::test]
    async fn materialization_rolls_back_session_when_first_message_insert_fails() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open db");
        create_materialization_schema(&pool).await;
        sqlx::query(
            "CREATE TRIGGER reject_first_message
             BEFORE INSERT ON messages
             WHEN NEW.content = 'reject'
             BEGIN SELECT RAISE(ABORT, 'message rejected'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let result = materialize_session_and_first_message(
            &pool,
            "draft-rollback",
            "quick",
            "/tmp/quick/draft-rollback",
            "model",
            "reject",
            123,
        )
        .await;

        assert!(result.is_err());
        let session_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE id = 'draft-rollback'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(session_count, 0, "failed first-message write must roll back the session row");
    }

    #[tokio::test]
    async fn materialization_is_idempotent_for_the_same_draft_id() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open db");
        create_materialization_schema(&pool).await;

        for _ in 0..2 {
            materialize_session_and_first_message(
                &pool,
                "draft-1",
                "project",
                "/tmp/project",
                "model",
                "只保存一次",
                123,
            )
            .await
            .expect("idempotent materialize");
        }

        let message_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages WHERE session_id = 'draft-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(message_count, 1);
    }

    async fn create_materialization_schema(pool: &sqlx::SqlitePool) {
        sqlx::query(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                cwd TEXT NOT NULL,
                model_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                parent_session_id TEXT,
                kind TEXT NOT NULL DEFAULT 'project',
                reasoning_effort TEXT
            )",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                model_id TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                tool_calls TEXT,
                reasoning_content TEXT,
                completion_state TEXT,
                created_at INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn message_history_is_bounded_by_real_user_turn_and_cursor_stable() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open db");
        create_materialization_schema(&pool).await;
        sqlx::query(
            "INSERT INTO sessions (
                id, title, cwd, model_id, created_at, updated_at,
                total_input_tokens, total_output_tokens, kind
             ) VALUES ('long', 'long', '/tmp', 'model', 1, 1, 0, 0, 'project')",
        )
        .execute(&pool)
        .await
        .unwrap();

        for turn in 0..12 {
            for (suffix, role, completion_state) in [
                ("user", "user", None),
                ("tool-declaration", "assistant", None),
                ("tool-replay", "tool", None),
                ("final", "assistant", None),
                ("gate", "user", Some("gate_warning")),
            ] {
                sqlx::query(
                    "INSERT INTO messages (
                        id, session_id, role, content, model_id, input_tokens,
                        output_tokens, tool_calls, reasoning_content,
                        completion_state, created_at
                     ) VALUES (?, 'long', ?, ?, NULL, NULL, NULL, NULL, NULL, ?, 1000)",
                )
                .bind(format!("{turn}-{suffix}"))
                .bind(role)
                .bind(format!("turn-{turn}-{suffix}"))
                .bind(completion_state)
                .execute(&pool)
                .await
                .unwrap();
            }
        }

        let newest = load_message_page(&pool, "long", None, 3)
            .await
            .expect("newest page");
        assert!(newest.has_more);
        assert!(newest.next_before_rowid.is_some());
        assert_eq!(newest.messages.first().unwrap().id, "9-user");
        assert_eq!(newest.messages.last().unwrap().id, "11-gate");
        assert_eq!(newest.messages.len(), 15);

        let older = load_message_page(
            &pool,
            "long",
            newest.next_before_rowid,
            3,
        )
        .await
        .expect("older page");
        assert_eq!(older.messages.first().unwrap().id, "6-user");
        assert_eq!(older.messages.last().unwrap().id, "8-gate");
        assert_eq!(older.messages.len(), 15);

        let newest_ids = newest
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(
            older
                .messages
                .iter()
                .all(|message| !newest_ids.contains(message.id.as_str())),
            "cursor pages must not overlap even when every created_at is identical",
        );
    }

    #[tokio::test]
    async fn a_single_pathological_turn_is_hard_bounded_by_raw_rows() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open db");
        create_materialization_schema(&pool).await;
        sqlx::query(
            "INSERT INTO sessions (
                id, title, cwd, model_id, created_at, updated_at,
                total_input_tokens, total_output_tokens, kind
             ) VALUES ('huge', 'huge', '/tmp', 'model', 1, 1, 0, 0, 'project')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ('huge-user', 'huge', 'user', 'start', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for index in 0..1_000 {
            sqlx::query(
                "INSERT INTO messages (id, session_id, role, content, created_at)
                 VALUES (?, 'huge', 'tool', ?, 1)",
            )
            .bind(format!("tool-{index}"))
            .bind(
                serde_json::json!({
                    "tool_call_id": format!("call-{index}"),
                    "content": format!("result-{index}"),
                    "status": "done"
                })
                .to_string(),
            )
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ('huge-final', 'huge', 'assistant', ?, 1)",
        )
        .bind("x".repeat(3 * 1024 * 1024))
        .execute(&pool)
        .await
        .unwrap();

        let mut cursor = None;
        let mut previous_cursor = i64::MAX;
        let mut all_ids = std::collections::HashSet::new();
        let mut page_count = 0;
        loop {
            let page = load_message_page(&pool, "huge", cursor, 8)
                .await
                .expect("bounded giant-turn page");
            page_count += 1;
            assert!(page.messages.len() <= MAX_MESSAGE_PAGE_ROWS as usize);
            assert!(
                serde_json::to_vec(&page).unwrap().len()
                    <= MAX_MESSAGE_PAGE_SERIALIZED_BYTES
            );
            for message in &page.messages {
                assert!(
                    all_ids.insert(message.id.clone()),
                    "cursor pages must never overlap",
                );
                if message.role == "tool" {
                    serde_json::from_str::<serde_json::Value>(&message.content)
                        .expect("tool replay remains valid JSON");
                }
            }
            if page_count == 1 {
                assert!(page.truncated);
                assert_eq!(page.messages.last().unwrap().id, "huge-final");
                assert!(page.messages.last().unwrap().content.len() <= MAX_MESSAGE_FIELD_BYTES);
            }
            if !page.has_more {
                assert!(page.next_before_rowid.is_none());
                break;
            }
            let next = page.next_before_rowid.expect("has_more cursor");
            assert!(next < previous_cursor, "cursor must strictly decrease");
            previous_cursor = next;
            cursor = Some(next);
        }
        assert_eq!(page_count, 3);
        assert_eq!(all_ids.len(), 1_002);
        assert!(all_ids.contains("huge-user"));
        assert!(all_ids.contains("huge-final"));
    }

    #[tokio::test]
    async fn newest_page_freezes_its_upper_rowid_before_concurrent_appends() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open db");
        create_materialization_schema(&pool).await;
        sqlx::query(
            "INSERT INTO sessions (
                id, title, cwd, model_id, created_at, updated_at,
                total_input_tokens, total_output_tokens, kind
             ) VALUES ('concurrent', 'concurrent', '/tmp', 'model', 1, 1, 0, 0, 'project')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ('concurrent-user', 'concurrent', 'user', 'start', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for index in 0..398 {
            sqlx::query(
                "INSERT INTO messages (id, session_id, role, content, created_at)
                 VALUES (?, 'concurrent', 'tool', '{}', 1)",
            )
            .bind(format!("before-{index}"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let fixed_upper = sqlx::query_scalar::<_, i64>(
            "SELECT MAX(rowid) + 1 FROM messages WHERE session_id = 'concurrent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        for index in 0..20 {
            sqlx::query(
                "INSERT INTO messages (id, session_id, role, content, created_at)
                 VALUES (?, 'concurrent', 'tool', '{}', 1)",
            )
            .bind(format!("late-{index}"))
            .execute(&pool)
            .await
            .unwrap();
        }

        let frozen = load_message_page_below(&pool, "concurrent", fixed_upper, 8)
            .await
            .expect("frozen snapshot page");
        assert_eq!(frozen.messages.len(), 399);
        assert!(frozen
            .messages
            .iter()
            .all(|message| !message.id.starts_with("late-")));

        let refreshed = load_message_page(&pool, "concurrent", None, 8)
            .await
            .expect("new request sees appended rows");
        assert_eq!(refreshed.messages.len(), MAX_MESSAGE_PAGE_ROWS as usize);
        assert!(refreshed
            .messages
            .iter()
            .any(|message| message.id == "late-19"));
    }

    #[tokio::test]
    async fn oversized_tool_fields_stay_valid_and_the_full_page_is_under_two_mib() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open db");
        create_materialization_schema(&pool).await;
        sqlx::query(
            "INSERT INTO sessions (
                id, title, cwd, model_id, created_at, updated_at,
                total_input_tokens, total_output_tokens, kind
             ) VALUES ('payload', 'payload', '/tmp', 'model', 1, 1, 0, 0, 'project')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ('payload-user', 'payload', 'user', 'start', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let huge_args = "参".repeat(MAX_MESSAGE_FIELD_BYTES);
        let tool_calls = serde_json::json!([{
            "id": "call-huge",
            "type": "function",
            "function": { "name": "bash", "arguments": huge_args }
        }])
        .to_string();
        sqlx::query(
            "INSERT INTO messages (
                id, session_id, role, content, tool_calls, reasoning_content, created_at
             ) VALUES ('payload-declaration', 'payload', 'assistant', '', ?, ?, 2)",
        )
        .bind(tool_calls)
        .bind("private reasoning".repeat(100_000))
        .execute(&pool)
        .await
        .unwrap();
        let replay = serde_json::json!({
            "tool_call_id": "call-huge",
            "content": "x".repeat(MAX_MESSAGE_FIELD_BYTES * 2),
            "status": "done"
        })
        .to_string();
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ('payload-replay', 'payload', 'tool', ?, 3)",
        )
        .bind(replay)
        .execute(&pool)
        .await
        .unwrap();

        let page = load_message_page(&pool, "payload", None, 8)
            .await
            .expect("bounded payload page");
        assert!(page.truncated);
        assert!(page
            .messages
            .iter()
            .all(|message| message.reasoning_content.is_none()));
        assert_eq!(page.messages[1].tool_calls.as_deref(), Some("[]"));
        let replay: serde_json::Value =
            serde_json::from_str(&page.messages[2].content).expect("valid replay JSON");
        assert_eq!(replay["tool_call_id"], "call-huge");
        assert_eq!(replay["content"], PAYLOAD_OMITTED);
        assert!(
            serde_json::to_vec(&page).unwrap().len()
                <= MAX_MESSAGE_PAGE_SERIALIZED_BYTES
        );
        let multilingual = "你".repeat(MAX_MESSAGE_FIELD_BYTES);
        let preview = truncate_utf8(&multilingual, MAX_MESSAGE_FIELD_BYTES);
        assert!(preview.len() <= MAX_MESSAGE_FIELD_BYTES);
        assert!(preview.is_char_boundary(preview.len()));
        for non_object in [
            serde_json::to_string(&vec!["x".repeat(MAX_MESSAGE_FIELD_BYTES)]).unwrap(),
            serde_json::to_string(&"x".repeat(MAX_MESSAGE_FIELD_BYTES)).unwrap(),
        ] {
            let bounded = bound_replay_content(&non_object, MAX_MESSAGE_FIELD_BYTES);
            let parsed: serde_json::Value =
                serde_json::from_str(&bounded).expect("fallback remains valid JSON");
            assert!(parsed.is_object());
            assert_eq!(parsed["content"], PAYLOAD_OMITTED);
        }
    }

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
