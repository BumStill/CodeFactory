// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crate::agent::failover::{RouteCandidate, RouteCandidatePlan};
use crate::agent::AgentLoop;
use crate::config::settings::Settings;
use crate::errors::AppError;
use crate::mcp::McpManager;
use crate::openrouter::types::StreamEvent;
use crate::AppState;

fn endpoint_requires_api_key(api_style: &crate::config::settings::ApiStyle) -> bool {
    !matches!(api_style, crate::config::settings::ApiStyle::Chatgpt)
}

#[derive(Debug)]
struct RouteCandidateResolution {
    candidates: Vec<RouteCandidate>,
    excluded: Vec<String>,
}

/// Resolve a stable per-turn route snapshot without probing or mutating the
/// user's preferred endpoint. The preferred endpoint is always considered
/// first; configured alternatives follow in deterministic name order.
///
/// Credential values are carried only into the in-memory route plan. Exclusion
/// diagnostics intentionally mention the endpoint and remediation class, never
/// the secret value or keychain error text.
fn resolve_route_candidates_with<F>(
    settings: &Settings,
    requested_model: &str,
    mut load_secret: F,
    chatgpt_authenticated: bool,
) -> RouteCandidateResolution
where
    F: FnMut(&str) -> std::result::Result<Option<String>, String>,
{
    let mut endpoint_names: Vec<String> = settings.endpoints.keys().cloned().collect();
    endpoint_names.sort();
    if let Some(primary_index) = endpoint_names
        .iter()
        .position(|name| name == &settings.default_endpoint)
    {
        let primary = endpoint_names.remove(primary_index);
        endpoint_names.insert(0, primary);
    }

    let mut candidates = Vec::new();
    let mut excluded = Vec::new();
    for endpoint_name in endpoint_names {
        let Some(endpoint) = settings.endpoints.get(&endpoint_name) else {
            continue;
        };
        let Some(model_id) = settings.resolve_model_for_endpoint(&endpoint_name, requested_model)
        else {
            excluded.push(format!("{endpoint_name}：没有可用模型"));
            continue;
        };

        let api_key = if !endpoint_requires_api_key(&endpoint.api_style) {
            if !chatgpt_authenticated {
                excluded.push(format!("{endpoint_name}：缺少 ChatGPT 登录凭据"));
                continue;
            }
            String::new()
        } else {
            let key_ref = endpoint
                .key_ref
                .clone()
                .unwrap_or_else(|| format!("codefactory.endpoint.{endpoint_name}"));
            match load_secret(&key_ref) {
                Ok(Some(secret)) if !secret.trim().is_empty() => secret,
                Ok(_) => {
                    excluded.push(format!("{endpoint_name}：缺少凭据"));
                    continue;
                }
                Err(_) => {
                    excluded.push(format!("{endpoint_name}：凭据读取失败"));
                    continue;
                }
            }
        };

        candidates.push(RouteCandidate {
            endpoint_name,
            model_id,
            base_url: endpoint.base_url.clone(),
            api_key,
            api_style: endpoint.api_style.clone(),
        });
    }

    RouteCandidateResolution {
        candidates,
        excluded,
    }
}

const CREDENTIAL_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

async fn bounded_blocking_lookup<T, F>(lookup: F) -> std::result::Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> std::result::Result<T, String> + Send + 'static,
{
    bounded_blocking_lookup_with_timeout(CREDENTIAL_LOOKUP_TIMEOUT, lookup).await
}

async fn bounded_blocking_lookup_with_timeout<T, F>(
    timeout: Duration,
    lookup: F,
) -> std::result::Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> std::result::Result<T, String> + Send + 'static,
{
    match tokio::time::timeout(timeout, tokio::task::spawn_blocking(lookup)).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("凭据读取任务异常".into()),
        Err(_) => Err("凭据读取超时".into()),
    }
}

pub(crate) async fn resolve_route_plan(
    settings: &Settings,
    requested_model: &str,
) -> Result<(RouteCandidatePlan, Vec<String>), AppError> {
    // macOS Security.framework can block indefinitely when a missing or locked
    // keychain item is queried. Resolve every configured credential off the
    // async runtime and cap each lookup, otherwise one unusable fallback (for
    // example OpenRouter without a key) freezes the whole chat before the agent
    // loop can emit any progress.
    let chatgpt_lookup = bounded_blocking_lookup(|| {
        crate::codex_auth::load_tokens()
            .map(|tokens| tokens.is_some())
            .map_err(|_| "ChatGPT 登录凭据读取失败".to_string())
    });

    let mut secret_refs: Vec<String> = settings
        .endpoints
        .iter()
        .filter(|(_, endpoint)| endpoint_requires_api_key(&endpoint.api_style))
        .map(|(endpoint_name, endpoint)| {
            endpoint
                .key_ref
                .clone()
                .unwrap_or_else(|| format!("codefactory.endpoint.{endpoint_name}"))
        })
        .collect();
    secret_refs.sort();
    secret_refs.dedup();

    let secret_lookups = async move {
        let mut lookups = tokio::task::JoinSet::new();
        for key_ref in secret_refs {
            lookups.spawn(async move {
                let lookup_ref = key_ref.clone();
                let result = bounded_blocking_lookup(move || {
                    crate::secrets::get_key(&lookup_ref).map_err(|_| "端点凭据读取失败".to_string())
                })
                .await;
                (key_ref, result)
            });
        }
        let mut snapshot = HashMap::new();
        while let Some(joined) = lookups.join_next().await {
            if let Ok((key_ref, result)) = joined {
                snapshot.insert(key_ref, result);
            }
        }
        snapshot
    };

    let (chatgpt_authenticated, mut secret_snapshot) = tokio::join!(chatgpt_lookup, secret_lookups);
    let resolution = resolve_route_candidates_with(
        settings,
        requested_model,
        |key_ref| {
            secret_snapshot
                .remove(key_ref)
                .unwrap_or_else(|| Err("凭据未解析".into()))
        },
        chatgpt_authenticated.unwrap_or(false),
    );
    let mut routes = resolution.candidates.into_iter();
    let Some(primary) = routes.next() else {
        let detail = if resolution.excluded.is_empty() {
            "没有配置任何模型端点".to_string()
        } else {
            resolution.excluded.join("；")
        };
        return Err(AppError::Other(format!(
            "所有可用模型端点均不可用：{detail}。请在模型设置中登录或配置凭据后重试。"
        )));
    };
    let mut plan = RouteCandidatePlan::new(primary);
    for fallback in routes {
        plan.push_fallback(fallback);
    }
    Ok((plan, resolution.excluded))
}

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

/// Request cancellation of the in-flight chat turn for `session_id`. Flips the
/// per-session cooperative flag that the agent loop polls between rounds, so the
/// turn stops cleanly — it does NOT interrupt an in-flight tool call. No-ops if
/// nothing is running for that session. Scoped to chat only: this never touches
/// the task scheduler (that has its own `cancel_implementation`).
/// Readiness of the delivery channel for the onboarding wizard: a logged-in
/// gh CLI (preferred, zero app-side config) or a configured REST token.
#[derive(serde::Serialize)]
pub struct DeliveryChannelStatus {
    pub gh_cli: bool,
    pub rest_token: bool,
}

#[tauri::command]
pub async fn delivery_channel_status(
    state: State<'_, AppState>,
) -> Result<DeliveryChannelStatus, AppError> {
    let settings = state.settings.read().await;
    let rest_token = settings
        .git_remotes
        .iter()
        .any(|r| matches!(r.provider, crate::config::settings::GitProvider::Github));
    drop(settings);
    let gh_cli = crate::agent::delivery::gh_cli_available();
    Ok(DeliveryChannelStatus { gh_cli, rest_token })
}

#[tauri::command]
pub async fn cancel_chat(session_id: String, state: State<'_, AppState>) -> Result<(), AppError> {
    if let Some(flag) = state.chat_cancels.lock().await.get(&session_id) {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("cancel_chat: requested stop for session {session_id}");
    }
    Ok(())
}

#[tauri::command]
pub async fn is_chat_running(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<bool, AppError> {
    Ok(state.chat_cancels.lock().await.contains_key(&session_id))
}

async fn clear_chat_running_if_current(
    chat_cancels: &crate::ChatCancelMap,
    session_id: &str,
    completed_flag: &Arc<AtomicBool>,
) {
    let mut flags = chat_cancels.lock().await;
    if flags
        .get(session_id)
        .is_some_and(|current| Arc::ptr_eq(current, completed_flag))
    {
        flags.remove(session_id);
    }
}

/// Await the actual agent future through a JoinHandle so panics and runtime
/// cancellation cannot disappear when the outer fire-and-forget task is
/// detached from the command response.
async fn supervise_chat_task<F, T>(future: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, AppError>> + Send + 'static,
    T: Send + 'static,
{
    match tokio::spawn(future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(join_error) => Err(format!("后台执行异常:{join_error}")),
    }
}

struct ChatRunningSetupGuard {
    chat_cancels: crate::ChatCancelMap,
    session_id: String,
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl ChatRunningSetupGuard {
    fn new(chat_cancels: crate::ChatCancelMap, session_id: String, flag: Arc<AtomicBool>) -> Self {
        Self {
            chat_cancels,
            session_id,
            flag,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ChatRunningSetupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let chat_cancels = self.chat_cancels.clone();
        let session_id = self.session_id.clone();
        let flag = self.flag.clone();
        tokio::spawn(async move {
            clear_chat_running_if_current(&chat_cancels, &session_id, &flag).await;
        });
    }
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    session_id: String,
    content: String,
    user_message_persisted: Option<bool>,
    state: State<'_, AppState>,
    mcp: State<'_, Arc<McpManager>>,
) -> Result<(), AppError> {
    // Register cancellation before any database, model, or credential work so
    // a stop click cannot race the command's setup phase.
    let cancel_flag = Arc::new(AtomicBool::new(false));
    state
        .chat_cancels
        .lock()
        .await
        .insert(session_id.clone(), cancel_flag.clone());
    let mut running_setup_guard = ChatRunningSetupGuard::new(
        state.chat_cancels.clone(),
        session_id.clone(),
        cancel_flag.clone(),
    );

    let settings = state.settings.read().await.clone();

    // Persist user message unless draft materialization already wrote it in
    // the same transaction as the session row. The count still determines
    // first-turn title behavior for legacy create-then-send callers.
    let is_first_message = {
        let pool = state.db.read().await;
        if !user_message_persisted.unwrap_or(false) {
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

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages WHERE session_id = ?")
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
            if let Ok(()) =
                sqlx::query("UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?")
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

    // Freeze all locally usable routes for this turn. This is a runtime
    // availability plan only: it never overwrites the user's preferred
    // endpoint/model in Settings.
    let (route_plan, excluded_routes) = resolve_route_plan(&settings, &session.model_id).await?;
    let primary_route = route_plan
        .candidates()
        .first()
        .expect("route plan always has a primary")
        .clone();
    let endpoint_name = primary_route.endpoint_name.clone();
    let resolved_model = primary_route.model_id.clone();
    let base_url = primary_route.base_url.clone();
    let api_key = primary_route.api_key.clone();
    let api_style = primary_route.api_style.clone();

    if endpoint_name == settings.default_endpoint && resolved_model != session.model_id {
        tracing::warn!(
            "send_message: repaired session model '{}' to endpoint '{}' active model '{}'",
            session.model_id,
            settings.default_endpoint,
            resolved_model
        );
        let pool = state.db.read().await;
        let now = Utc::now().timestamp_millis();
        sqlx::query("UPDATE sessions SET model_id = ?, updated_at = ? WHERE id = ?")
            .bind(&resolved_model)
            .bind(now)
            .bind(&session_id)
            .execute(&*pool)
            .await?;
        if let Ok(updated_session) =
            sqlx::query_as::<_, crate::storage::Session>("SELECT * FROM sessions WHERE id = ?")
                .bind(&session_id)
                .fetch_one(&*pool)
                .await
        {
            let event_name = format!("session_updated:{}", session_id);
            app.emit(&event_name, &updated_session).ok();
        }
    }

    tracing::info!(
        "send_message: endpoint={} model={} candidate_count={} excluded_count={}",
        endpoint_name,
        resolved_model,
        route_plan.candidates().len(),
        excluded_routes.len(),
    );

    // Fetch history
    let history = {
        let pool = state.db.read().await;
        sqlx::query_as::<_, crate::storage::Message>(
            "SELECT * FROM messages WHERE session_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(&session_id)
        .fetch_all(&*pool)
        .await?
    };

    // Framework-side plan/act dispatch (no user-facing mode toggle): if the
    // previous assistant turn ended on a pending proposal and this message
    // approves it, run THIS turn under the execute contract instead of
    // plan-first — so the agent doesn't re-ask "Ready to proceed?" for work
    // the user already greenlit. `history` already includes the just-inserted
    // user message as its last element, so the most recent assistant message
    // is the proposal we're checking.
    let prev_assistant = history
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && m.completion_state.is_none())
        .map(|m| m.content.clone());
    // Full access is a permission policy only. It may reduce approval prompts
    // after a tool is selected, but it must never turn a diagnostic question
    // into an execute-contract turn.
    let mode = select_chat_mode(
        settings.permissions.full_access,
        prev_assistant.as_deref(),
        &content,
    );
    tracing::info!("send_message: dispatch mode = {:?}", mode);

    let db = state.db.read().await.clone();
    let settings_state = state.settings.clone();
    let settings_for_notify = state.settings.clone();
    let pending_permissions = state.pending_permissions.clone();
    let mcp_manager: Arc<McpManager> = Arc::clone(&mcp);

    // Spawn agent loop (non-blocking); emit Error event to frontend if it fails
    let app_clone = app.clone();
    let event_name = format!("stream:{}", session_id);
    let session_id_clone = session_id.clone();
    let chat_cancels = state.chat_cancels.clone();
    let tracked_cancel_flag = cancel_flag.clone();
    tokio::spawn(async move {
        let db_for_error = db.clone();
        let session_for_error = session_id_clone.clone();
        let loop_result = supervise_chat_task(async move {
            let mut agent = AgentLoop::new_with_mode(
                app,
                db,
                session_id_clone,
                endpoint_name,
                resolved_model,
                base_url,
                api_key,
                api_style,
                std::path::PathBuf::from(session.cwd),
                settings_state,
                pending_permissions,
                mcp_manager,
                None,
                mode,
            )
            .with_failover_plan(route_plan)
            .with_cancel(cancel_flag);
            agent.run(history).await
        })
        .await;
        if let Err(error_text) = loop_result {
            tracing::error!("Agent loop error: {error_text}");
            // Persist the failure so it survives reloads: the 2026-07-21
            // field report had four interruptions with zero forensic trace
            // because the error only ever existed as this transient stream
            // event. Tagged turn_error → rendered as an error notice, and
            // excluded from provider history replay.
            let persisted_error_text = format!("回合中断:{error_text}");
            if let Err(persist_err) = sqlx::query(
                "INSERT INTO messages (id, session_id, role, content, completion_state, created_at) \
                 VALUES (?,?,?,?,?,?)",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&session_for_error)
            .bind("user")
            .bind(&persisted_error_text)
            .bind("turn_error")
            .bind(chrono::Utc::now().timestamp_millis())
            .execute(&db_for_error)
            .await
            {
                tracing::warn!("failed to persist turn error: {persist_err}");
            }
            {
                let settings = settings_for_notify.read().await;
                crate::notify::send(
                    &settings,
                    crate::notify::NotifyEvent::TurnError,
                    error_text.chars().take(200).collect(),
                );
            }
            app_clone
                .emit(
                    &event_name,
                    StreamEvent::Error {
                        message: error_text,
                    },
                )
                .ok();
        }
        clear_chat_running_if_current(&chat_cancels, &session_for_error, &tracked_cancel_flag)
            .await;
    });
    running_setup_guard.disarm();

    Ok(())
}

/// One turn of an anonymous conversation, supplied by the frontend — which
/// holds the ONLY copy of the history (nothing is persisted server-side).
#[derive(serde::Deserialize)]
pub struct AnonTurn {
    pub role: String,
    pub content: String,
}

fn anon_message(session_id: &str, role: String, content: String) -> crate::storage::Message {
    // Dummy id / timestamps: these Messages only feed the model this run and
    // are never written to the DB.
    crate::storage::Message {
        id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        role,
        content,
        model_id: None,
        input_tokens: None,
        output_tokens: None,
        tool_calls: None,
        reasoning_content: None,
        completion_state: None,
        created_at: 0,
    }
}

/// Send a message in an ANONYMOUS / ephemeral session.
///
/// Nothing is persisted: no user/assistant/tool messages, no cost entries, no
/// checkpoints, and no `sessions` row. The frontend owns the entire history
/// (`history`), and the conversation exists only in memory + this run's model
/// context — a private/sensitive chat leaves no trace on disk.
///
/// `session_id` is a frontend-generated id used purely to route stream events
/// (`stream:<id>`); it never touches the DB. `cwd` + `model_id` are passed
/// explicitly since there is no session row to read them from.
#[tauri::command]
pub async fn send_message_anonymous(
    app: AppHandle,
    session_id: String,
    content: String,
    history: Vec<AnonTurn>,
    cwd: String,
    model_id: String,
    state: State<'_, AppState>,
    mcp: State<'_, Arc<McpManager>>,
) -> Result<(), AppError> {
    // Match persisted chats: expose the cancellation flag before setup work.
    let cancel_flag = Arc::new(AtomicBool::new(false));
    state
        .chat_cancels
        .lock()
        .await
        .insert(session_id.clone(), cancel_flag.clone());
    let mut running_setup_guard = ChatRunningSetupGuard::new(
        state.chat_cancels.clone(),
        session_id.clone(),
        cancel_flag.clone(),
    );

    let settings = state.settings.read().await.clone();

    // Resolve the same stable failover plan as persisted chats. Anonymous mode
    // changes persistence only; it must not be less resilient.
    let (route_plan, _excluded_routes) = resolve_route_plan(&settings, &model_id).await?;
    let primary_route = route_plan
        .candidates()
        .first()
        .expect("route plan always has a primary")
        .clone();
    let endpoint_name = primary_route.endpoint_name.clone();
    let resolved_model = primary_route.model_id.clone();
    let base_url = primary_route.base_url.clone();
    let api_key = primary_route.api_key.clone();
    let api_style = primary_route.api_style.clone();

    // Anonymous sessions have no project dir; resolve an empty cwd to the shared
    // scratch dir so tools + the system prompt get a valid working directory.
    let cwd = if cwd.trim().is_empty() {
        let home =
            dirs::home_dir().ok_or_else(|| AppError::Other("home dir not resolvable".into()))?;
        let dir = home.join(".codefactory").join("quick");
        std::fs::create_dir_all(&dir).ok();
        dir.to_string_lossy().to_string()
    } else {
        cwd
    };

    // Build in-memory history: prior turns from the frontend + this new message.
    let mut full_history: Vec<crate::storage::Message> = history
        .into_iter()
        .map(|t| anon_message(&session_id, t.role, t.content))
        .collect();
    full_history.push(anon_message(&session_id, "user".into(), content));

    let db = state.db.read().await.clone();
    let settings_state = state.settings.clone();
    let pending_permissions = state.pending_permissions.clone();
    let mcp_manager: Arc<McpManager> = Arc::clone(&mcp);

    let app_clone = app.clone();
    let event_name = format!("stream:{}", session_id);
    let session_id_clone = session_id.clone();
    let chat_cancels = state.chat_cancels.clone();
    let tracked_cancel_flag = cancel_flag.clone();
    tokio::spawn(async move {
        let completed_session_id = session_id_clone.clone();
        let loop_result = supervise_chat_task(async move {
            // `.anonymous()` disables every DB write + cost record in the loop.
            let mut agent = AgentLoop::new(
                app,
                db,
                session_id_clone,
                endpoint_name,
                resolved_model,
                base_url,
                api_key,
                api_style,
                std::path::PathBuf::from(cwd),
                settings_state,
                pending_permissions,
                mcp_manager,
                None,
            )
            .anonymous()
            .with_failover_plan(route_plan)
            .with_cancel(cancel_flag);
            agent.run(full_history).await
        })
        .await;
        if let Err(error_text) = loop_result {
            tracing::error!("Anonymous agent loop error: {error_text}");
            app_clone
                .emit(
                    &event_name,
                    StreamEvent::Error {
                        message: error_text,
                    },
                )
                .ok();
        }
        clear_chat_running_if_current(&chat_cancels, &completed_session_id, &tracked_cancel_flag)
            .await;
    });
    running_setup_guard.disarm();

    Ok(())
}

fn select_chat_mode(
    _full_access: bool,
    prev_assistant: Option<&str>,
    content: &str,
) -> crate::agent::AgentMode {
    crate::agent::decide_chat_mode(prev_assistant, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::{ApiStyle, Endpoint};

    #[tokio::test]
    async fn completed_chat_only_clears_its_own_running_flag() {
        let flags: crate::ChatCancelMap =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let completed = Arc::new(AtomicBool::new(false));
        let replacement = Arc::new(AtomicBool::new(false));
        flags
            .lock()
            .await
            .insert("session".into(), replacement.clone());

        clear_chat_running_if_current(&flags, "session", &completed).await;
        assert!(Arc::ptr_eq(
            flags.lock().await.get("session").unwrap(),
            &replacement,
        ));

        clear_chat_running_if_current(&flags, "session", &replacement).await;
        assert!(!flags.lock().await.contains_key("session"));
    }

    #[tokio::test]
    async fn failed_setup_guard_clears_only_the_flag_it_registered() {
        let flags: crate::ChatCancelMap =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let failed_setup = Arc::new(AtomicBool::new(false));
        flags
            .lock()
            .await
            .insert("failed".into(), failed_setup.clone());

        {
            let _guard = ChatRunningSetupGuard::new(flags.clone(), "failed".into(), failed_setup);
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while flags.lock().await.contains_key("failed") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("setup cleanup should remove the failed run");

        let stale_setup = Arc::new(AtomicBool::new(false));
        let replacement = Arc::new(AtomicBool::new(false));
        flags
            .lock()
            .await
            .insert("replaced".into(), stale_setup.clone());
        {
            let _guard = ChatRunningSetupGuard::new(flags.clone(), "replaced".into(), stale_setup);
            flags
                .lock()
                .await
                .insert("replaced".into(), replacement.clone());
        }
        tokio::task::yield_now().await;
        assert!(Arc::ptr_eq(
            flags.lock().await.get("replaced").unwrap(),
            &replacement,
        ));
    }

    #[test]
    fn chatgpt_endpoint_never_looks_up_an_endpoint_api_key() {
        assert!(!endpoint_requires_api_key(&ApiStyle::Chatgpt));
        assert!(endpoint_requires_api_key(&ApiStyle::Openai));
        assert!(endpoint_requires_api_key(&ApiStyle::Anthropic));
    }

    #[test]
    fn route_candidates_keep_primary_then_only_locally_usable_fallbacks() {
        let mut settings = crate::config::settings::Settings::default();
        settings.default_endpoint = "chatgpt".into();
        settings.default_model = "gpt-5.5".into();
        settings.endpoints.clear();
        settings.endpoints.insert(
            "chatgpt".into(),
            Endpoint {
                base_url: crate::codex_auth::CHATGPT_BASE_URL.into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                custom_models: vec![],
                active_model: Some("gpt-5.5".into()),
            },
        );
        settings.endpoints.insert(
            "deepseek".into(),
            Endpoint {
                base_url: "https://api.deepseek.example/v1".into(),
                key_ref: Some("deepseek-secret".into()),
                api_style: ApiStyle::Openai,
                custom_models: vec![],
                active_model: Some("deepseek-v4-pro".into()),
            },
        );
        settings.endpoints.insert(
            "openrouter".into(),
            Endpoint {
                base_url: "https://openrouter.example/v1".into(),
                key_ref: Some("openrouter-secret".into()),
                api_style: ApiStyle::Openai,
                custom_models: vec![],
                active_model: Some("anthropic/claude-opus-4-7".into()),
            },
        );

        let resolution = resolve_route_candidates_with(
            &settings,
            "gpt-5.5",
            |key_ref| {
                Ok(if key_ref == "deepseek-secret" {
                    Some("configured-deepseek-key".into())
                } else {
                    None
                })
            },
            true,
        );

        assert_eq!(
            resolution
                .candidates
                .iter()
                .map(|route| (route.endpoint_name.as_str(), route.model_id.as_str()))
                .collect::<Vec<_>>(),
            vec![("chatgpt", "gpt-5.5"), ("deepseek", "deepseek-v4-pro")]
        );
        assert!(resolution
            .excluded
            .iter()
            .any(|reason| reason.contains("openrouter") && reason.contains("缺少凭据")));
    }

    #[tokio::test]
    async fn credential_lookup_timeout_cannot_freeze_chat_setup() {
        let started = std::time::Instant::now();
        let result = bounded_blocking_lookup_with_timeout(
            Duration::from_millis(10),
            || -> std::result::Result<(), String> {
                std::thread::sleep(Duration::from_millis(200));
                Ok(())
            },
        )
        .await;

        assert_eq!(result.unwrap_err(), "凭据读取超时");
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn full_access_is_permission_only_and_does_not_override_chat_mode() {
        assert_eq!(
            select_chat_mode(true, None, "这是怎么了？"),
            crate::agent::AgentMode::Interactive
        );
        assert_eq!(
            select_chat_mode(false, None, "这是怎么了？"),
            crate::agent::AgentMode::Interactive
        );
        assert_eq!(
            select_chat_mode(true, Some("方案已经准备好。是否开始实施？"), "做吧"),
            crate::agent::AgentMode::Execute
        );
    }

    #[tokio::test]
    async fn supervised_chat_task_converts_a_panic_into_a_visible_failure() {
        let result = supervise_chat_task(async move {
            panic!("synthetic agent panic");
            #[allow(unreachable_code)]
            Ok::<(), AppError>(())
        })
        .await;

        let failure = result.expect_err("panic must not disappear with the JoinHandle");
        assert!(failure.contains("后台执行异常"));
        assert!(failure.contains("synthetic agent panic"));
    }
}
