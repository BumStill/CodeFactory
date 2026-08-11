// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::agent::failover::{RouteCandidate, RouteCandidatePlan};
use crate::agent::AgentLoop;
use crate::config::settings::Settings;
use crate::errors::AppError;
use crate::mcp::McpManager;
use crate::openrouter::types::StreamEvent;
use crate::AppState;

fn is_chatgpt_auth_expired(endpoint_name: &str, message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("auth_expired")
        || (endpoint_name == crate::codex_auth::CHATGPT_ENDPOINT_KEY
            && (lower.contains("http 401")
                || lower.contains("401 unauthorized")
                || lower.contains("invalid_grant")
                || lower.contains("refresh_token")))
}

fn endpoint_requires_api_key(api_style: &crate::config::settings::ApiStyle) -> bool {
    !matches!(api_style, crate::config::settings::ApiStyle::Chatgpt)
}

#[derive(Debug)]
struct RouteCandidateResolution {
    candidates: Vec<RouteCandidate>,
    excluded: Vec<String>,
}

fn settings_for_session_route(
    settings: &Settings,
    endpoint_id: Option<&str>,
    model_id: &str,
    policy: &str,
) -> Result<Settings, AppError> {
    let mut turn = settings.clone();
    let endpoint = if let Some(endpoint) =
        endpoint_id.filter(|endpoint| settings.endpoints.contains_key(*endpoint))
    {
        endpoint
    } else {
        let matching = settings
            .endpoints
            .iter()
            .filter(|(_, endpoint)| {
                endpoint.active_model.as_deref() == Some(model_id)
                    || endpoint
                        .custom_models
                        .iter()
                        .any(|model| model.id == model_id)
            })
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        match matching.as_slice() {
            [only] => *only,
            many if many.contains(&settings.default_endpoint.as_str()) => {
                settings.default_endpoint.as_str()
            }
            _ => {
                return Err(AppError::Other(format!(
                    "MODEL_ROUTE_UNRESOLVED: 无法确定模型 '{model_id}' 属于哪个端点，请在本会话模型菜单中重新选择"
                )))
            }
        }
    };
    turn.default_endpoint = endpoint.to_string();
    if policy == "fixed" || policy.is_empty() {
        turn.endpoints.retain(|candidate, _| candidate == endpoint);
    }
    Ok(turn)
}

/// Resolve a stable per-turn route snapshot without probing or mutating the
/// user's preferred endpoint. The preferred endpoint is always considered
/// first; configured alternatives follow in deterministic name order.
///
/// Credential values are carried only into the in-memory route plan. Exclusion
/// diagnostics intentionally mention the endpoint and remediation class, never
/// the secret value or keychain error text.
fn resolve_route_candidates(
    settings: &Settings,
    requested_model: &str,
) -> RouteCandidateResolution {
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

        let credential_ref = if endpoint_requires_api_key(&endpoint.api_style) {
            Some(
                endpoint
                    .key_ref
                    .clone()
                    .unwrap_or_else(|| format!("codefactory.endpoint.{endpoint_name}")),
            )
        } else {
            None
        };

        candidates.push(RouteCandidate {
            supports_vision: crate::agent::context::model_supports_vision(
                settings,
                &endpoint_name,
                &model_id,
            ),
            endpoint_name,
            model_id,
            base_url: endpoint.base_url.clone(),
            credential_ref,
            legacy_inline_api_key: None,
            api_style: endpoint.api_style.clone(),
        });
    }

    RouteCandidateResolution {
        candidates,
        excluded,
    }
}

pub(crate) async fn resolve_route_plan(
    settings: &Settings,
    requested_model: &str,
    policy: &str,
    requires_vision: bool,
) -> Result<(RouteCandidatePlan, Vec<String>), AppError> {
    // Planning is deliberately credential-blind. Touching every configured
    // Keychain item here caused unrelated DeepSeek authorization prompts even
    // when a ChatGPT route was selected. The transport resolves only the route
    // it is about to invoke.
    let mut resolution = resolve_route_candidates(settings, requested_model);
    if requires_vision && policy != "fixed" {
        resolution
            .candidates
            .retain(|candidate| candidate.supports_vision);
        if resolution.candidates.is_empty() {
            return Err(AppError::Other(
                "IMAGE_INPUT_UNSUPPORTED: 当前没有可用的图片模型；图片已保留。请选择支持图片的模型，系统将自动续接当前目标"
                    .into(),
            ));
        }
    }
    let mut routes = resolution.candidates.into_iter();
    let Some(primary) = routes.next() else {
        let detail = if resolution.excluded.is_empty() {
            "没有配置任何模型端点".to_string()
        } else {
            resolution.excluded.join("；")
        };
        return Err(AppError::Other(format!(
            "所有可用模型端点均不可用：{detail}。请在模型设置中完成登录或凭据配置；能力恢复后系统将自动续接当前目标。"
        )));
    };
    let mut plan = if policy == "auto" {
        RouteCandidatePlan::new_automatic(primary)
    } else {
        RouteCandidatePlan::new(primary)
    };
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

fn chat_objective_kind(
    capability: crate::agent::TurnCapability,
    content: &str,
) -> crate::agent::objective::ObjectiveKind {
    use crate::agent::objective::ObjectiveKind;
    match capability {
        crate::agent::TurnCapability::ReviewOnly => ObjectiveKind::Informational,
        crate::agent::TurnCapability::Implement => ObjectiveKind::LocalMutation,
        crate::agent::TurnCapability::Deliver => {
            let normalized = content.to_ascii_lowercase();
            if ["上线", "生产", "部署", "live", "production", "deploy"]
                .iter()
                .any(|cue| normalized.contains(cue))
            {
                ObjectiveKind::Live
            } else {
                ObjectiveKind::Delivery
            }
        }
    }
}

fn requested_acceptance(kind: crate::agent::objective::ObjectiveKind) -> &'static str {
    use crate::agent::objective::ObjectiveKind;
    match kind {
        ObjectiveKind::Informational => "informational_answer",
        ObjectiveKind::LocalMutation => "validated_change",
        ObjectiveKind::Delivery => "delivery_receipt",
        ObjectiveKind::Live => "live_verification",
        ObjectiveKind::LegacyOrphan => "legacy_reconciliation",
    }
}

async fn project_chat_objective(
    db: &sqlx::SqlitePool,
    app: &AppHandle,
    event_name: &str,
    root_turn_id: &str,
    objective: &crate::agent::objective::ObjectiveSnapshot,
) -> Result<(), AppError> {
    use crate::agent::objective::ObjectiveStatus;
    let now = Utc::now().timestamp_millis();
    let (phase, activity_kind, activity_label) = match objective.status {
        ObjectiveStatus::Completed => ("finalizing", "objective_completed", "目标证据已满足"),
        ObjectiveStatus::Cancelled => ("finalizing", "objective_cancelled", "已按用户要求停止"),
        ObjectiveStatus::WaitingSystem => ("recovering", "system_recovery", "系统正在恢复并续接"),
        ObjectiveStatus::WaitingCoreInput => ("waiting", "core_input_required", "需要补充核心输入"),
        ObjectiveStatus::WaitingAuthorization => ("waiting", "authorization_required", "等待必要授权"),
        ObjectiveStatus::WaitingBusinessDecision => ("waiting", "business_decision_required", "等待业务决策"),
        ObjectiveStatus::Active => ("working", "objective_active", "系统正在继续处理"),
        ObjectiveStatus::LegacyOrphan => ("recovering", "legacy_reconciliation", "系统正在核对历史工作"),
    };
    let waiting_reason = objective
        .failure_code
        .as_deref()
        .or(objective.request_key.as_deref())
        .or(objective.decision_key.as_deref());
    let completed_at = objective.status.is_terminal().then_some(now);
    let revision: i64 = sqlx::query_scalar(
        "UPDATE chat_turn_state SET revision=revision+1, phase=?, status=?,
           recent_activity_kind=?, recent_activity_label=?, waiting_reason=?,
           updated_at=?, completed_at=?, terminal_reason=?
         WHERE root_turn_id=? RETURNING revision",
    )
    .bind(phase)
    .bind(objective.status.as_str())
    .bind(activity_kind)
    .bind(activity_label)
    .bind(waiting_reason)
    .bind(now)
    .bind(completed_at)
    .bind(if objective.status.is_terminal() {
        Some(objective.decision_type.as_str())
    } else {
        None
    })
    .bind(root_turn_id)
    .fetch_one(db)
    .await?;
    app.emit(
        event_name,
        StreamEvent::TurnActivityUpdated {
            root_turn_id: root_turn_id.to_string(),
            revision,
            phase: phase.into(),
            status: objective.status.as_str().into(),
            recent_activity_kind: activity_kind.into(),
            recent_activity_label: activity_label.into(),
            waiting_reason: waiting_reason.map(str::to_string),
            updated_at: now,
            terminal_reason: if objective.status.is_terminal() {
                Some(objective.decision_type.as_str().into())
            } else {
                None
            },
            objective_id: Some(objective.id.clone()),
            objective_status: Some(objective.status.as_str().into()),
            recovery_owner: objective.recovery_owner.clone(),
            next_observation_at: objective.next_observation_at,
            last_progress_at: objective.last_progress_at,
        },
    )
    .ok();
    Ok(())
}

async fn settle_chat_objective_from_outcome(
    db: &sqlx::SqlitePool,
    app: &AppHandle,
    event_name: &str,
    objective_id: &str,
    root_turn_id: &str,
    outcome: &codefactory_agent_loop::run::RunOutcome,
) -> Result<crate::agent::objective::ObjectiveSnapshot, AppError> {
    let store = crate::agent::objective::ObjectiveStore::new(db.clone());
    let current = store
        .get(objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other(format!("objective {objective_id} missing")))?;
    let terminal_reason = sqlx::query_scalar::<_, String>(
        "SELECT terminal_reason FROM chat_turn_state WHERE root_turn_id=?",
    )
    .bind(root_turn_id)
    .fetch_optional(db)
    .await?;
    let decision = crate::agent::objective::decision_for_run_outcome_with_reason(
        &current,
        outcome,
        terminal_reason.as_deref(),
    )
    .map_err(|error| AppError::Other(error.to_string()))?;
    let revised = store
        .apply_decision(current.revision, decision)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    project_chat_objective(db, app, event_name, root_turn_id, &revised).await?;
    Ok(revised)
}

async fn settle_chat_objective_from_error(
    db: &sqlx::SqlitePool,
    app: &AppHandle,
    event_name: &str,
    objective_id: &str,
    root_turn_id: &str,
    auth_expired: bool,
    error_text: &str,
) -> Result<crate::agent::objective::ObjectiveSnapshot, AppError> {
    use crate::agent::objective::{DecisionRouter, RecoveryDomain, RouteSignal};
    let store = crate::agent::objective::ObjectiveStore::new(db.clone());
    let current = store
        .get(objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other(format!("objective {objective_id} missing")))?;
    let signal = if auth_expired {
        RouteSignal::AuthorizationRequired {
            domain: RecoveryDomain::Auth,
            request_key: format!("chatgpt-auth:{objective_id}"),
            action_signature: format!("oauth:chatgpt:resume:{objective_id}"),
            resume_cursor: Some(root_turn_id.to_string()),
        }
    } else {
        RouteSignal::TechnicalFailure {
            domain: RecoveryDomain::Chat,
            failure_code: "agent_loop_error".into(),
            failure_signature: format!("sha256:{:x}", Sha256::digest(error_text.as_bytes())),
            next_observation_at: Utc::now().timestamp_millis() + 5_000,
            resume_cursor: Some(root_turn_id.to_string()),
        }
    };
    let decision = DecisionRouter::route(&current, signal)
        .map_err(|error| AppError::Other(error.to_string()))?;
    let revised = store
        .apply_decision(current.revision, decision)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    project_chat_objective(db, app, event_name, root_turn_id, &revised).await?;
    Ok(revised)
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
    {
        let mut active = state.chat_cancels.lock().await;
        if state
            .update_restart_reserved
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AppError::Other(
                "应用更新已进入安全重启阶段，请等待自动恢复工作区".into(),
            ));
        }
        active.insert(session_id.clone(), cancel_flag.clone());
    }
    let mut running_setup_guard = ChatRunningSetupGuard::new(
        state.chat_cancels.clone(),
        session_id.clone(),
        cancel_flag.clone(),
    );

    let settings = state.settings.read().await.clone();

    // Persist user message unless draft materialization already wrote it in
    // the same transaction as the session row. The count still determines
    // first-turn title behavior for legacy create-then-send callers.
    let (is_first_message, root_turn_id) = {
        let pool = state.db.read().await;
        let inserted_id = if !user_message_persisted.unwrap_or(false) {
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
            Some(msg_id)
        } else {
            None
        };

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages WHERE session_id = ?")
            .bind(&session_id)
            .fetch_one(&*pool)
            .await?;
        let root_turn_id = match inserted_id {
            Some(id) => id,
            None => {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM messages
                 WHERE session_id=? AND role='user' AND completion_state IS NULL
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                )
                .bind(&session_id)
                .fetch_one(&*pool)
                .await?
            }
        };
        (count.0 == 1, root_turn_id)
    };

    let continuation_root_turn_id = {
        let pool = state.db.read().await;
        let now = Utc::now().timestamp_millis();
        let ordinal: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM chat_task_segments WHERE session_id=?",
        )
        .bind(&session_id)
        .fetch_one(&*pool)
        .await?;
        let previous_segment_id = if crate::agent::is_contextual_approval(&content) {
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM chat_task_segments WHERE session_id=? ORDER BY ordinal DESC LIMIT 1",
            )
            .bind(&session_id)
            .fetch_optional(&*pool)
            .await?
        } else {
            None
        };
        let segment_id = Uuid::new_v4().to_string();
        let title: String = content.chars().take(60).collect();
        sqlx::query(
            "INSERT OR IGNORE INTO chat_task_segments
             (id, session_id, ordinal, title, status, goal_root_turn_id, previous_segment_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?)",
        )
        .bind(&segment_id)
        .bind(&session_id)
        .bind(ordinal)
        .bind(if title.trim().is_empty() {
            "新任务"
        } else {
            &title
        })
        .bind(&root_turn_id)
        .bind(&previous_segment_id)
        .bind(now)
        .bind(now)
        .execute(&*pool)
        .await?;
        let segment_id: String = sqlx::query_scalar(
            "SELECT id FROM chat_task_segments WHERE session_id=? AND goal_root_turn_id=?",
        )
        .bind(&session_id)
        .bind(&root_turn_id)
        .fetch_one(&*pool)
        .await?;
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, task_segment_id, revision, phase, status,
              started_at, updated_at, recent_activity_kind, recent_activity_label)
             VALUES (?, ?, ?, 1, 'planning', 'active', ?, ?, 'turn_started', '正在理解任务')
             ON CONFLICT(root_turn_id) DO UPDATE SET
               revision=chat_turn_state.revision+1, phase='planning', status='active',
               updated_at=excluded.updated_at, completed_at=NULL, terminal_reason=NULL",
        )
        .bind(&root_turn_id)
        .bind(&session_id)
        .bind(&segment_id)
        .bind(now)
        .bind(now)
        .execute(&*pool)
        .await?;
        let continuation_root_turn_id = if let Some(previous_segment_id) = previous_segment_id.as_deref() {
            sqlx::query_scalar::<_, String>(
                "SELECT goal_root_turn_id FROM chat_task_segments WHERE id=? AND session_id=?",
            )
            .bind(previous_segment_id)
            .bind(&session_id)
            .fetch_optional(&*pool)
            .await?
        } else {
            None
        };
        if let Some(continuation_root_turn_id) = continuation_root_turn_id.as_deref() {
            let objective_id = sqlx::query_scalar::<_, Option<String>>(
                "SELECT objective_id FROM chat_turn_state
                 WHERE root_turn_id=? AND session_id=?",
            )
            .bind(continuation_root_turn_id)
            .bind(&session_id)
            .fetch_optional(&*pool)
            .await?
            .flatten()
            .filter(|value| !value.is_empty());
            if let Some(objective_id) = objective_id {
                let open_state = sqlx::query_as::<_, (String, Option<String>)>(
                    "SELECT status, wait_class FROM delivery_runs
                     WHERE objective_id=?
                       AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
                     ORDER BY updated_at DESC LIMIT 1",
                )
                .bind(objective_id)
                .fetch_optional(&*pool)
                .await?;
                if let Some((status, _wait_class)) = open_state {
                    let driver = match status.as_str() {
                        "waiting" => "recoverable_waiting_open",
                        "platform_incident" | "agent_action_required" | "failed_internal" => {
                            "system_owned_remediation_open"
                        }
                        "awaiting_completion_arbitration" => "completion_arbitration_open",
                        _ => "authorized_objective_still_open",
                    };
                    sqlx::query(
                        "UPDATE chat_turn_state SET user_reprompt_driver=? WHERE root_turn_id=?",
                    )
                    .bind(driver)
                    .bind(&root_turn_id)
                    .execute(&*pool)
                    .await?;
                }
            }
        }
        continuation_root_turn_id
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
    let turn_settings = settings_for_session_route(
        &settings,
        session.endpoint_id.as_deref(),
        &session.model_id,
        &session.model_policy,
    )?;
    // Fetch history as the agent should see it — excludes gate-rejected drafts.
    // It is also the capability source for the frozen turn plan.
    let history = {
        let pool = state.db.read().await;
        crate::storage::load_agent_history(&pool, &session_id).await?
    };
    let requires_vision = history.iter().any(|message| {
        !crate::agent::attachments::extract_openai_parts(&message.content).is_empty()
    });
    let (route_plan, excluded_routes) = resolve_route_plan(
        &turn_settings,
        &session.model_id,
        &session.model_policy,
        requires_vision,
    )
    .await?;
    let primary_route = route_plan
        .candidates()
        .first()
        .expect("route plan always has a primary")
        .clone();
    let endpoint_name = primary_route.endpoint_name.clone();
    let endpoint_for_error = endpoint_name.clone();
    let resolved_model = primary_route.model_id.clone();
    let base_url = primary_route.base_url.clone();
    let api_key = String::new();
    let api_style = primary_route.api_style.clone();

    if endpoint_name == turn_settings.default_endpoint && resolved_model != session.model_id {
        tracing::warn!(
            "send_message: repaired session model '{}' to endpoint '{}' active model '{}'",
            session.model_id,
            turn_settings.default_endpoint,
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
        .find(|m| {
            m.role == "assistant"
                && m.completion_state.is_none()
                && (!crate::agent::is_contextual_approval(&content)
                    || crate::agent::proposal_capability(&m.content).is_some())
        })
        .map(|m| m.content.clone());
    // Full access is a permission policy only. It may reduce approval prompts
    // after a tool is selected, but it must never turn a diagnostic question
    // into an execute-contract turn.
    let mut contract = crate::agent::decide_chat_contract(prev_assistant.as_deref(), &content);
    // Session-persisted delivery authorization (DB-backed so it survives app
    // restarts): once this session asked to deliver, later non-planning turns
    // keep Deliver capability so follow-up work can ship without a repeat
    // confirmation. A new explicit delivery request (re)grants it; a
    // revocation clears it.
    {
        let db = state.db.read().await.clone();
        let authorized = crate::agent::fetch_session_delivery_authorized(&db, &session_id).await;
        if crate::agent::is_delivery_revocation(&content) {
            crate::agent::set_session_delivery_authorized(&db, &session_id, false).await;
        } else if contract.capability == crate::agent::TurnCapability::Deliver {
            crate::agent::set_session_delivery_authorized(&db, &session_id, true).await;
        } else if authorized {
            contract = crate::agent::with_persisted_delivery_authorization(contract, true);
        }
    }
    let mode = select_chat_mode(
        settings.permissions.full_access,
        prev_assistant.as_deref(),
        &content,
    );
    debug_assert_eq!(mode, contract.mode);
    tracing::info!(
        "send_message: dispatch mode = {:?}, capability = {:?}",
        contract.mode,
        contract.capability
    );

    let db = state.db.read().await.clone();
    let objective_kind = chat_objective_kind(contract.capability, &content);
    let objective = crate::agent::objective::ObjectiveStore::new(db.clone())
        .ensure_or_continue_chat_objective(
            &session_id,
            &root_turn_id,
            continuation_root_turn_id.as_deref(),
            objective_kind,
            requested_acceptance(objective_kind),
        )
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    let objective_id = objective.id.clone();
    let settings_state = state.settings.clone();
    let settings_for_notify = state.settings.clone();
    let pending_permissions = state.pending_permissions.clone();
    let mcp_manager: Arc<McpManager> = Arc::clone(&mcp);

    // Spawn agent loop (non-blocking); emit Error event to frontend if it fails
    let app_clone = app.clone();
    let event_name = format!("stream:{}", session_id);
    let session_id_clone = session_id.clone();
    let root_turn_for_error = root_turn_id.clone();
    let objective_for_settlement = objective_id.clone();
    let chat_cancels = state.chat_cancels.clone();
    let tracked_cancel_flag = cancel_flag.clone();
    let interjections = state.interjections.clone();
    let interjections_cleanup = state.interjections.clone();
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
                contract.mode,
            )
            .with_turn_capability(contract.capability)
            .with_turn_grants(contract.grants)
            .with_failover_plan(route_plan)
            .with_cancel(cancel_flag)
            .with_steer(interjections);
            agent.run(history).await
        })
        .await;
        match loop_result {
            Ok(outcome) => {
                if let Err(error) = settle_chat_objective_from_outcome(
                    &db_for_error,
                    &app_clone,
                    &event_name,
                    &objective_for_settlement,
                    &root_turn_for_error,
                    &outcome,
                )
                .await
                {
                    tracing::error!("failed to settle chat objective: {error}");
                }
            }
            Err(error_text) => {
                tracing::error!("Agent loop error: {error_text}");
                let auth_expired = is_chatgpt_auth_expired(&endpoint_for_error, &error_text);
                if let Err(error) = settle_chat_objective_from_error(
                    &db_for_error,
                    &app_clone,
                    &event_name,
                    &objective_for_settlement,
                    &root_turn_for_error,
                    auth_expired,
                    &error_text,
                )
                .await
                {
                    tracing::error!("failed to persist chat objective recovery: {error}");
                }
                {
                    let settings = settings_for_notify.read().await;
                    crate::notify::send(
                        &settings,
                        crate::notify::NotifyEvent::TurnError,
                        error_text.chars().take(200).collect(),
                    );
                }
                if auth_expired {
                    app_clone
                        .emit(
                            &event_name,
                            StreamEvent::RuntimeError {
                                code: "AUTH_EXPIRED".into(),
                                message: "ChatGPT 授权已过期。重新验证成功后，系统会从安全检查点自动续接当前目标。".into(),
                                endpoint_id: Some(crate::codex_auth::CHATGPT_ENDPOINT_KEY.into()),
                                recoverable: true,
                            },
                        )
                        .ok();
                }
            }
        }
        clear_chat_running_if_current(&chat_cancels, &session_for_error, &tracked_cancel_flag)
            .await;
        // A steer typed just as the turn ended was never drained. Drop it here
        // so a later, unrelated turn cannot pick it up at its first round
        // boundary; the frontend re-sends anything it never saw applied.
        crate::commands::interjections::drain_for_session(
            &interjections_cleanup,
            &session_for_error,
        )
        .await;
        let reclaimed = crate::tools::browser_session::close_for_session(&session_for_error).await;
        if reclaimed > 0 {
            tracing::info!(
                "send_message: reclaimed {reclaimed} browser session(s) for {session_for_error}"
            );
        }
    });
    running_setup_guard.disarm();

    Ok(())
}

/// Resume an already-authorized persisted chat objective without inserting a
/// synthetic user message or creating a second root turn. Called only by the
/// durable remediation supervisor after it owns the objective lease.
pub(crate) async fn resume_chat_objective(
    app: AppHandle,
    objective: crate::agent::objective::ObjectiveSnapshot,
) -> Result<(), AppError> {
    use crate::agent::objective::{ObjectiveKind, ObjectiveStatus};

    if objective.status != ObjectiveStatus::WaitingSystem {
        return Ok(());
    }
    let session_id = objective
        .session_id
        .clone()
        .ok_or_else(|| AppError::Other("chat objective has no session identity".into()))?;
    let root_turn_id = objective
        .root_turn_id
        .clone()
        .ok_or_else(|| AppError::Other("chat objective has no root-turn identity".into()))?;

    let state = app.state::<AppState>();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let mut active = state.chat_cancels.lock().await;
        if active.contains_key(&session_id) {
            return Err(AppError::Other("chat session already has an active runner".into()));
        }
        if state
            .update_restart_reserved
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AppError::Other("update restart reservation is active".into()));
        }
        active.insert(session_id.clone(), cancel_flag.clone());
    }
    let mut running_guard = ChatRunningSetupGuard::new(
        state.chat_cancels.clone(),
        session_id.clone(),
        cancel_flag.clone(),
    );
    let db = state.db.read().await.clone();
    let settings_snapshot = state.settings.read().await.clone();
    let settings_state = state.settings.clone();
    let pending_permissions = state.pending_permissions.clone();
    let chat_cancels = state.chat_cancels.clone();
    let interjections = state.interjections.clone();
    let tracked_cancel = cancel_flag.clone();
    drop(state);
    let mcp_manager = Arc::clone(&*app.state::<Arc<McpManager>>());

    let session = sqlx::query_as::<_, crate::storage::Session>(
        "SELECT * FROM sessions WHERE id=?",
    )
    .bind(&session_id)
    .fetch_one(&db)
    .await?;
    let original_content: String =
        sqlx::query_scalar("SELECT content FROM messages WHERE id=? AND session_id=? AND role='user'")
            .bind(&root_turn_id)
            .bind(&session_id)
            .fetch_one(&db)
            .await?;
    let history = crate::storage::load_agent_history(&db, &session_id).await?;
    let turn_settings = settings_for_session_route(
        &settings_snapshot,
        session.endpoint_id.as_deref(),
        &session.model_id,
        &session.model_policy,
    )?;
    let requires_vision = history.iter().any(|message| {
        !crate::agent::attachments::extract_openai_parts(&message.content).is_empty()
    });
    let (route_plan, _) = resolve_route_plan(
        &turn_settings,
        &session.model_id,
        &session.model_policy,
        requires_vision,
    )
    .await?;
    let primary_route = route_plan
        .candidates()
        .first()
        .ok_or_else(|| AppError::Other("objective resume has no model route".into()))?
        .clone();
    let endpoint_for_error = primary_route.endpoint_name.clone();
    let inferred = crate::agent::decide_chat_contract(None, &original_content);
    let (mode, capability) = match objective.kind {
        ObjectiveKind::Informational => (
            crate::agent::AgentMode::Interactive,
            crate::agent::TurnCapability::ReviewOnly,
        ),
        ObjectiveKind::LocalMutation => (
            crate::agent::AgentMode::Execute,
            crate::agent::TurnCapability::Implement,
        ),
        ObjectiveKind::Delivery | ObjectiveKind::Live => (
            crate::agent::AgentMode::Execute,
            crate::agent::TurnCapability::Deliver,
        ),
        ObjectiveKind::LegacyOrphan => {
            return Err(AppError::Other(
                "legacy objective requires identity reconciliation before resume".into(),
            ));
        }
    };

    let event_name = format!("stream:{session_id}");
    let app_for_run = app.clone();
    let db_for_run = db.clone();
    let session_for_run = session_id.clone();
    let loop_result = supervise_chat_task(async move {
        let mut agent = AgentLoop::new_with_mode(
            app_for_run,
            db_for_run,
            session_for_run,
            primary_route.endpoint_name,
            primary_route.model_id,
            primary_route.base_url,
            String::new(),
            primary_route.api_style,
            std::path::PathBuf::from(session.cwd),
            settings_state,
            pending_permissions,
            mcp_manager,
            None,
            mode,
        )
        .with_turn_capability(capability)
        .with_turn_grants(inferred.grants)
        .with_failover_plan(route_plan)
        .with_cancel(cancel_flag)
        .with_steer(interjections);
        agent.run(history).await
    })
    .await;

    match loop_result {
        Ok(outcome) => {
            settle_chat_objective_from_outcome(
                &db,
                &app,
                &event_name,
                &objective.id,
                &root_turn_id,
                &outcome,
            )
            .await?;
        }
        Err(error_text) => {
            let auth_expired = is_chatgpt_auth_expired(&endpoint_for_error, &error_text);
            settle_chat_objective_from_error(
                &db,
                &app,
                &event_name,
                &objective.id,
                &root_turn_id,
                auth_expired,
                &error_text,
            )
            .await?;
            if auth_expired {
                app.emit(
                    &event_name,
                    StreamEvent::RuntimeError {
                        code: "AUTH_EXPIRED".into(),
                        message: "ChatGPT 授权已过期。重新验证成功后，系统会从安全检查点自动续接当前目标。".into(),
                        endpoint_id: Some(crate::codex_auth::CHATGPT_ENDPOINT_KEY.into()),
                        recoverable: true,
                    },
                )
                .ok();
            }
        }
    }

    clear_chat_running_if_current(&chat_cancels, &session_id, &tracked_cancel).await;
    let reclaimed = crate::tools::browser_session::close_for_session(&session_id).await;
    if reclaimed > 0 {
        tracing::info!("objective resume reclaimed {reclaimed} browser session(s)");
    }
    running_guard.disarm();
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
        endpoint_id: None,
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
    endpoint_id: Option<String>,
    model_policy: Option<String>,
    state: State<'_, AppState>,
    mcp: State<'_, Arc<McpManager>>,
) -> Result<(), AppError> {
    // Match persisted chats: expose the cancellation flag before setup work.
    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let mut active = state.chat_cancels.lock().await;
        if state
            .update_restart_reserved
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AppError::Other(
                "应用更新已进入安全重启阶段，请等待自动恢复工作区".into(),
            ));
        }
        active.insert(session_id.clone(), cancel_flag.clone());
    }
    let mut running_setup_guard = ChatRunningSetupGuard::new(
        state.chat_cancels.clone(),
        session_id.clone(),
        cancel_flag.clone(),
    );

    let settings = state.settings.read().await.clone();

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

    let prev_assistant = history
        .iter()
        .rev()
        .find(|turn| {
            turn.role == "assistant"
                && (!crate::agent::is_contextual_approval(&content)
                    || crate::agent::proposal_capability(&turn.content).is_some())
        })
        .map(|turn| turn.content.as_str());
    let mut contract = crate::agent::decide_chat_contract(prev_assistant, &content);
    // Session-persisted delivery authorization (DB-backed, same semantics as
    // send_message): once this session asked to deliver, later non-planning
    // turns inherit Deliver capability; explicit delivery re-grants,
    // revocation clears.
    {
        let db = state.db.read().await.clone();
        let authorized = crate::agent::fetch_session_delivery_authorized(&db, &session_id).await;
        if crate::agent::is_delivery_revocation(&content) {
            crate::agent::set_session_delivery_authorized(&db, &session_id, false).await;
        } else if contract.capability == crate::agent::TurnCapability::Deliver {
            crate::agent::set_session_delivery_authorized(&db, &session_id, true).await;
        } else if authorized {
            contract = crate::agent::with_persisted_delivery_authorization(contract, true);
        }
    }

    // Build in-memory history: prior turns from the frontend + this new message.
    let mut full_history: Vec<crate::storage::Message> = history
        .into_iter()
        .map(|t| anon_message(&session_id, t.role, t.content))
        .collect();
    full_history.push(anon_message(&session_id, "user".into(), content));
    let active_policy = model_policy.as_deref().unwrap_or("prefer");
    let turn_settings =
        settings_for_session_route(&settings, endpoint_id.as_deref(), &model_id, active_policy)?;
    let requires_vision = full_history.iter().any(|message| {
        !crate::agent::attachments::extract_openai_parts(&message.content).is_empty()
    });
    let (route_plan, _excluded_routes) =
        resolve_route_plan(&turn_settings, &model_id, active_policy, requires_vision).await?;
    let primary_route = route_plan
        .candidates()
        .first()
        .expect("route plan always has a primary")
        .clone();
    let endpoint_name = primary_route.endpoint_name.clone();
    let resolved_model = primary_route.model_id.clone();
    let base_url = primary_route.base_url.clone();
    let api_key = String::new();
    let api_style = primary_route.api_style.clone();

    let db = state.db.read().await.clone();
    let settings_state = state.settings.clone();
    let pending_permissions = state.pending_permissions.clone();
    let mcp_manager: Arc<McpManager> = Arc::clone(&mcp);

    let app_clone = app.clone();
    let event_name = format!("stream:{}", session_id);
    let session_id_clone = session_id.clone();
    let chat_cancels = state.chat_cancels.clone();
    let tracked_cancel_flag = cancel_flag.clone();
    let interjections = state.interjections.clone();
    let interjections_cleanup = state.interjections.clone();
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
            .with_turn_capability(contract.capability)
            .with_turn_grants(contract.grants)
            .with_failover_plan(route_plan)
            .with_cancel(cancel_flag)
            .with_steer(interjections);
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
        // Same race as the persisted path: a steer that arrived after the last
        // round boundary is dropped here rather than leaking into a later turn.
        crate::commands::interjections::drain_for_session(
            &interjections_cleanup,
            &completed_session_id,
        )
        .await;
        let reclaimed =
            crate::tools::browser_session::close_for_session(&completed_session_id).await;
        if reclaimed > 0 {
            tracing::info!(
                "anonymous chat: reclaimed {reclaimed} browser session(s) for {completed_session_id}"
            );
        }
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
    use crate::agent::failover::{ActiveRouteState, EndpointHealthRegistry};
    use crate::config::settings::{ApiStyle, Endpoint};
    use std::time::Duration;

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
    fn route_candidates_keep_primary_and_defer_credentials_to_transport() {
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

        let resolution = resolve_route_candidates(&settings, "gpt-5.5");

        assert_eq!(
            resolution
                .candidates
                .iter()
                .map(|route| (route.endpoint_name.as_str(), route.model_id.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("chatgpt", "gpt-5.5"),
                ("deepseek", "deepseek-v4-pro"),
                ("openrouter", "anthropic/claude-opus-4-7")
            ]
        );
        assert_eq!(
            resolution.candidates[1].credential_ref.as_deref(),
            Some("deepseek-secret")
        );
        assert_eq!(
            resolution.candidates[2].credential_ref.as_deref(),
            Some("openrouter-secret")
        );
        assert!(resolution.excluded.is_empty());
    }

    #[test]
    fn route_planning_does_not_read_any_endpoint_credential() {
        let mut settings = crate::config::settings::Settings::default();
        settings.default_endpoint = "chatgpt".into();
        settings.default_model = "gpt-5.5".into();
        settings.endpoints.insert(
            "deepseek".into(),
            Endpoint {
                base_url: "https://api.deepseek.example/v1".into(),
                key_ref: Some("must-not-read".into()),
                api_style: ApiStyle::Openai,
                custom_models: vec![],
                active_model: Some("deepseek-v4-pro".into()),
            },
        );

        let resolution = resolve_route_candidates(&settings, "gpt-5.5");

        assert!(!resolution.candidates.is_empty());
    }

    #[tokio::test]
    async fn automatic_image_route_excludes_text_only_candidates_before_transport() {
        let mut settings = crate::config::settings::Settings::default();
        settings.default_endpoint = "deepseek".into();
        settings.default_model = "deepseek-v4-pro".into();
        settings.endpoints.clear();
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
            "chatgpt".into(),
            Endpoint {
                base_url: crate::codex_auth::CHATGPT_BASE_URL.into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                custom_models: vec![],
                active_model: Some("gpt-5.5".into()),
            },
        );

        let (plan, _) = resolve_route_plan(&settings, "deepseek-v4-pro", "auto", true)
            .await
            .expect("vision route exists");

        assert_eq!(plan.candidates().len(), 1);
        assert_eq!(plan.candidates()[0].endpoint_name, "chatgpt");
        assert_eq!(plan.candidates()[0].model_id, "gpt-5.5");
    }

    #[tokio::test]
    async fn prefer_retries_the_selected_primary_while_auto_respects_recent_health() {
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

        let (prefer_plan, _) = resolve_route_plan(&settings, "gpt-5.5", "prefer", false)
            .await
            .expect("prefer route");
        let prefer_health = EndpointHealthRegistry::new(Duration::from_secs(120));
        prefer_health.mark_unavailable("chatgpt");
        let prefer_state = ActiveRouteState::from_plan_with_health(prefer_plan, prefer_health);
        assert_eq!(prefer_state.current().endpoint_name, "chatgpt");

        let (auto_plan, _) = resolve_route_plan(&settings, "gpt-5.5", "auto", false)
            .await
            .expect("auto route");
        let auto_health = EndpointHealthRegistry::new(Duration::from_secs(120));
        auto_health.mark_unavailable("chatgpt");
        let auto_state = ActiveRouteState::from_plan_with_health(auto_plan, auto_health);
        assert_eq!(auto_state.current().endpoint_name, "deepseek");
        assert!(auto_state.take_initial_route_change().is_some());
    }

    #[test]
    fn structured_auth_expired_is_detected_even_when_chatgpt_was_a_fallback() {
        assert!(is_chatgpt_auth_expired(
            "deepseek",
            "AUTH_EXPIRED: ChatGPT 授权已过期"
        ));
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
