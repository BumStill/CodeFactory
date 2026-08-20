// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::agent::failover::{RouteCandidate, RouteCandidatePlan};
use crate::agent::AgentLoop;
use crate::config::settings::Settings;
use crate::errors::AppError;
use crate::mcp::McpManager;
use crate::openrouter::types::StreamEvent;
use crate::session_title::{
    apply_local_title_fallback, is_low_information, spawn_title_generation,
    TITLE_SOURCE_PLACEHOLDER,
};
use crate::AppState;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChatRunIdentity {
    root_turn_id: String,
    objective_id: String,
    objective_revision: i64,
}

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
    app: AppHandle,
    intent_id: String,
    allow: bool,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await.clone();
    let store = crate::agent::permission_intent::PermissionIntentStore::new(pool.clone());
    let intent = store
        .get(&intent_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other("unknown durable permission intent".into()))?;

    let sender = state.pending_permissions.lock().await.remove(&intent_id);
    if let Some(sender) = sender {
        store
            .record_user_response(
                &intent.prompt_key(),
                if allow {
                    crate::agent::permission_intent::PermissionPromptResponse::Allow
                } else {
                    crate::agent::permission_intent::PermissionPromptResponse::Deny
                },
                Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|error| AppError::Other(error.to_string()))?;
        // The durable decision is authoritative. If the process-local waiter
        // disappeared after removal from the registry, immediately adopt the
        // exact response into the same Objective instead of inviting a retry
        // or waiting for another application restart.
        if sender.send(allow).is_err() {
            let settlement = store
                .reconcile_orphaned_response(&intent_id, Utc::now().timestamp_millis())
                .await
                .map_err(|error| AppError::Other(error.to_string()))?;
            let objective = crate::agent::objective::ObjectiveStore::new(pool.clone())
                .get(&settlement.objective_id)
                .await
                .map_err(|error| AppError::Other(error.to_string()))?
                .ok_or_else(|| AppError::Other("permission Objective disappeared".into()))?;
            if let Some(root_turn_id) = objective.root_turn_id.as_deref() {
                project_chat_objective(
                    &pool,
                    &app,
                    &format!("stream:{}", intent.session_id),
                    root_turn_id,
                    &objective,
                )
                .await?;
            }
        }
        return Ok(());
    }

    // No process-local receiver means this is a supervisor-projected prompt.
    // Settle the exact durable wait and schedule (or terminally deny) the same
    // Objective in one transaction; never manufacture a new user turn.
    let settlement = store
        .settle_projected_response(
            &intent_id,
            if allow {
                crate::agent::permission_intent::PermissionPromptResponse::Allow
            } else {
                crate::agent::permission_intent::PermissionPromptResponse::Deny
            },
            Utc::now().timestamp_millis(),
        )
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    let objective = crate::agent::objective::ObjectiveStore::new(pool.clone())
        .get(&settlement.objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other("permission Objective disappeared".into()))?;
    if objective.revision != settlement.objective_revision {
        return Err(AppError::Other(
            "permission Objective advanced before response projection".into(),
        ));
    }
    if let Some(root_turn_id) = objective.root_turn_id.as_deref() {
        project_chat_objective(
            &pool,
            &app,
            &format!("stream:{}", intent.session_id),
            root_turn_id,
            &objective,
        )
        .await?;
    }
    Ok(())
}

/// Reconcile one claimed Permission remediation. Interrupted channels only
/// reproject the original prompt and typed Objective wait; an already-allowed
/// exact receipt resumes the original Objective/root through the normal chat
/// runner, whose permission gateway reserves that receipt before the action.
pub(crate) async fn reconcile_permission_objective(
    app: AppHandle,
    pool: sqlx::SqlitePool,
    objective: crate::agent::objective::ObjectiveSnapshot,
    mutation_permit: codefactory_agent_loop::tool::MutationPermit,
) -> Result<(), AppError> {
    let now = Utc::now().timestamp_millis();
    let store = crate::agent::permission_intent::PermissionIntentStore::new(pool.clone());
    let binding_id = mutation_permit
        .binding_id
        .as_deref()
        .ok_or_else(|| AppError::Other("Permission remediation has no exact binding".into()))?;
    let resource_generation = mutation_permit.resource_generation.ok_or_else(|| {
        AppError::Other("Permission remediation has no binding generation".into())
    })?;
    let resource_kind: String = sqlx::query_scalar(
        "SELECT resource_kind FROM objective_bindings
         WHERE id=? AND objective_id=? AND resource_generation=?",
    )
    .bind(binding_id)
    .bind(&objective.id)
    .bind(resource_generation)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| AppError::Other("Permission remediation binding is stale".into()))?;
    match store
        .observe_claimed_recovery(
            &mutation_permit,
            &crate::agent::objective::current_process_instance(),
            now + 60_000,
            now,
        )
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
    {
        crate::agent::permission_intent::PermissionClaimAction::ProjectPrompt(observation) => {
            app.emit(
                &format!("stream:{}", observation.snapshot.session_id),
                crate::agent::permission_gateway::DesktopPermissionGateway::projected_prompt_event(
                    &observation,
                ),
            )
            .map_err(|error| AppError::Other(error.to_string()))?;
            let waiting = crate::agent::objective::ObjectiveStore::new(pool.clone())
                .get(&objective.id)
                .await
                .map_err(|error| AppError::Other(error.to_string()))?
                .ok_or_else(|| AppError::Other("permission Objective disappeared".into()))?;
            match resource_kind.as_str() {
                "chat_root_turn" => {
                    let root_turn_id = waiting
                        .root_turn_id
                        .as_deref()
                        .or(waiting.resume_cursor.as_deref())
                        .ok_or_else(|| {
                            AppError::Other("permission Objective has no chat cursor".into())
                        })?;
                    project_chat_objective(
                        &pool,
                        &app,
                        &format!("stream:{}", observation.snapshot.session_id),
                        root_turn_id,
                        &waiting,
                    )
                    .await
                }
                "task_run" if waiting.task_id.is_some() => Ok(()),
                _ => Err(AppError::Other(format!(
                    "Permission remediation cannot project binding kind {resource_kind}"
                ))),
            }
        }
        crate::agent::permission_intent::PermissionClaimAction::ResumeAuthorizedAction => {
            match resource_kind.as_str() {
                "chat_root_turn" => resume_chat_objective(app, objective, mutation_permit).await,
                "task_run" => {
                    crate::commands::tasks::resume_task_objective(app, objective, mutation_permit)
                        .await
                }
                _ => Err(AppError::Other(format!(
                    "Permission remediation cannot execute binding kind {resource_kind}"
                ))),
            }
        }
    }
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
pub async fn cancel_chat(
    app: AppHandle,
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let control = state.chat_cancels.lock().await.get(&session_id).cloned();
    if control.as_ref().is_some_and(|control| !control.durable) {
        control
            .expect("ephemeral control checked above")
            .cancel
            .store(true, Ordering::SeqCst);
        tracing::info!("cancel_chat: requested ephemeral stop for session {session_id}");
        return Ok(());
    }
    let pool = state.db.read().await.clone();
    let store = crate::agent::objective::ObjectiveStore::new(pool.clone());
    // Fence the complete session first. A process-local run may finish while
    // the stop is being handled, but it cannot persist another non-cancel
    // decision after this durable row exists.
    store
        .request_chat_session_cancel(&session_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    if let Some(control) = control.as_ref() {
        if control.durable {
            request_chat_run_cancel(&pool, &control.run_instance_id, &session_id).await?;
        }
        control.cancel.store(true, Ordering::SeqCst);
    }
    let stopped = cancel_system_owned_chat(&app, &pool, &session_id).await?;
    if let Some(control) = control.as_ref() {
        clear_chat_running_if_current(&state.chat_cancels, &session_id, control).await;
    }
    tracing::info!("cancel_chat: durably stopped {stopped} objective(s) for session {session_id}");
    Ok(())
}

/// Every Objective a session still owns, newest turn first. Terminal states are
/// excluded; every non-terminal one — `active`, any `waiting_*` — is something
/// the user's stop must reach. Kept free of `AppHandle` so it stays directly
/// testable: constructing handle-owning values inside the unit-test binary is
/// what previously broke the Windows loader.
#[cfg(test)]
async fn live_chat_objectives(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<Vec<(String, String)>, AppError> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT turn.objective_id, turn.root_turn_id
         FROM chat_turn_state turn
         JOIN objectives objective ON objective.id=turn.objective_id
         WHERE turn.session_id=?
           AND turn.objective_id IS NOT NULL
           AND objective.status NOT IN ('completed', 'cancelled')
         ORDER BY objective.updated_at DESC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?)
}

/// Stop every Objective a session still owns when no run in this process holds
/// it. System-owned recovery and post-restart adoption live only in the
/// database, so the in-memory cancel map cannot reach them, yet those are the
/// states a user cannot otherwise escape.
///
/// Every live Objective is cancelled, not just the newest: the 2026-08-13
/// session grew a second Objective behind the one that had been stopped, so
/// cancelling one at a time left the turn unfinished and the user's queued
/// message stuck behind it. An explicit user stop is also the only authority
/// that may abandon an Objective whose external side effect stayed uncertain —
/// `explicit_cancel` provenance is exactly what the durable store requires.
async fn cancel_system_owned_chat(
    app: &AppHandle,
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<usize, AppError> {
    let store = crate::agent::objective::ObjectiveStore::new(pool.clone());
    let cancelled = store
        .consume_chat_session_cancel(session_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    // Projection is deliberately after durable settlement. A UI event failure
    // must never leave later Objectives alive; hydration/query is the fallback.
    for (objective_id, root_turn_id, snapshot) in &cancelled {
        let Some(root_turn_id) = root_turn_id.as_deref() else {
            continue;
        };
        if let Err(error) = project_chat_objective(
            pool,
            app,
            &format!("stream:{session_id}"),
            root_turn_id,
            snapshot,
        )
        .await
        {
            tracing::warn!(
                %objective_id,
                %error,
                "cancel_chat: durable stop settled but turn projection failed"
            );
        }
    }
    Ok(cancelled.len())
}

async fn cancel_chat_objective_exact(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    root_turn_id: &str,
    objective_id: &str,
) -> Result<crate::agent::objective::ObjectiveSnapshot, AppError> {
    crate::agent::objective::ObjectiveStore::new(pool.clone())
        .cancel_chat_exact(session_id, root_turn_id, objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))
}

async fn request_chat_run_cancel(
    pool: &sqlx::SqlitePool,
    run_instance_id: &str,
    session_id: &str,
) -> Result<Option<ChatRunIdentity>, AppError> {
    let now = Utc::now().timestamp_millis();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO chat_run_controls
         (run_instance_id, session_id, status, created_process_instance,
          cancel_requested_at, created_at, updated_at)
         VALUES (?, ?, 'cancel_requested', ?, ?, ?, ?)
         ON CONFLICT(run_instance_id) DO UPDATE SET
           status=CASE
             WHEN chat_run_controls.status='active' THEN 'cancel_requested'
             ELSE chat_run_controls.status
           END,
           cancel_requested_at=COALESCE(chat_run_controls.cancel_requested_at,
                                        excluded.cancel_requested_at),
           updated_at=excluded.updated_at
         WHERE chat_run_controls.session_id=excluded.session_id
           AND chat_run_controls.status IN ('active','cancel_requested')",
    )
    .bind(run_instance_id)
    .bind(session_id)
    .bind(crate::agent::objective::current_process_instance())
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query(
        "SELECT control.root_turn_id,
                COALESCE(control.objective_id, turn.objective_id) AS objective_id,
                COALESCE(control.objective_revision, objective.revision) AS objective_revision
         FROM chat_run_controls control
         LEFT JOIN chat_turn_state turn
           ON turn.root_turn_id=control.root_turn_id
          AND turn.session_id=control.session_id
         LEFT JOIN objectives objective
           ON objective.id=COALESCE(control.objective_id, turn.objective_id)
         WHERE control.run_instance_id=? AND control.session_id=?
           AND control.status='cancel_requested'",
    )
    .bind(run_instance_id)
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let root_turn_id = row.try_get::<Option<String>, _>("root_turn_id")?;
    let objective_id = row.try_get::<Option<String>, _>("objective_id")?;
    let objective_revision = row.try_get::<Option<i64>, _>("objective_revision")?;
    match (root_turn_id, objective_id, objective_revision) {
        (Some(root_turn_id), Some(objective_id), Some(objective_revision)) => {
            Ok(Some(ChatRunIdentity {
                root_turn_id,
                objective_id,
                objective_revision,
            }))
        }
        (None, None, None) | (Some(_), None, None) => Ok(None),
        _ => Err(AppError::Other(
            "chat run control has an incomplete Objective/root/revision identity".into(),
        )),
    }
}

async fn register_chat_run_control(
    pool: &sqlx::SqlitePool,
    control: &crate::ChatRunControl,
    session_id: &str,
) -> Result<(), AppError> {
    let now = Utc::now().timestamp_millis();
    let cancelled = control.cancel.load(Ordering::SeqCst);
    sqlx::query(
        "INSERT INTO chat_run_controls
         (run_instance_id, session_id, status, created_process_instance,
          cancel_requested_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(run_instance_id) DO NOTHING",
    )
    .bind(&control.run_instance_id)
    .bind(session_id)
    .bind(if cancelled {
        "cancel_requested"
    } else {
        "active"
    })
    .bind(crate::agent::objective::current_process_instance())
    .bind(cancelled.then_some(now))
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    let persisted_session: String =
        sqlx::query_scalar("SELECT session_id FROM chat_run_controls WHERE run_instance_id=?")
            .bind(&control.run_instance_id)
            .fetch_one(pool)
            .await?;
    if persisted_session != session_id {
        return Err(AppError::Other(
            "chat run instance was already bound to another session".into(),
        ));
    }
    Ok(())
}

async fn bind_chat_run_root(
    pool: &sqlx::SqlitePool,
    run_instance_id: &str,
    session_id: &str,
    root_turn_id: &str,
) -> Result<(), AppError> {
    let updated = sqlx::query(
        "UPDATE chat_run_controls
         SET root_turn_id=?, updated_at=?
         WHERE run_instance_id=? AND session_id=?
           AND status IN ('active','cancel_requested')
           AND (root_turn_id IS NULL OR root_turn_id=?)",
    )
    .bind(root_turn_id)
    .bind(Utc::now().timestamp_millis())
    .bind(run_instance_id)
    .bind(session_id)
    .bind(root_turn_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Other(
            "chat run root identity changed during setup".into(),
        ));
    }
    Ok(())
}

async fn bind_chat_run_objective(
    pool: &sqlx::SqlitePool,
    run_instance_id: &str,
    session_id: &str,
    root_turn_id: &str,
    objective_id: &str,
    objective_revision: i64,
) -> Result<Option<crate::agent::objective::ObjectiveSnapshot>, AppError> {
    let updated = sqlx::query(
        "UPDATE chat_run_controls
         SET objective_id=?, objective_revision=?, updated_at=?
         WHERE run_instance_id=? AND session_id=? AND root_turn_id=?
           AND status IN ('active','cancel_requested')
           AND (objective_id IS NULL OR objective_id=?)
           AND EXISTS (
             SELECT 1 FROM chat_turn_state turn
             WHERE turn.root_turn_id=? AND turn.session_id=?
               AND turn.objective_id=?
           )",
    )
    .bind(objective_id)
    .bind(objective_revision)
    .bind(Utc::now().timestamp_millis())
    .bind(run_instance_id)
    .bind(session_id)
    .bind(root_turn_id)
    .bind(objective_id)
    .bind(root_turn_id)
    .bind(session_id)
    .bind(objective_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Other(
            "chat run Objective identity changed during setup".into(),
        ));
    }
    consume_requested_chat_cancel(pool, run_instance_id).await
}

async fn consume_requested_chat_cancel(
    pool: &sqlx::SqlitePool,
    run_instance_id: &str,
) -> Result<Option<crate::agent::objective::ObjectiveSnapshot>, AppError> {
    let identity = sqlx::query_as::<_, (String, String, String, i64)>(
        "SELECT control.session_id, control.root_turn_id,
                COALESCE(control.objective_id, turn.objective_id),
                COALESCE(control.objective_revision, objective.revision)
         FROM chat_run_controls control
         LEFT JOIN chat_turn_state turn
          ON turn.root_turn_id=control.root_turn_id
          AND turn.session_id=control.session_id
         JOIN objectives objective
           ON objective.id=COALESCE(control.objective_id, turn.objective_id)
         WHERE control.run_instance_id=? AND control.status='cancel_requested'
           AND control.root_turn_id IS NOT NULL
           AND COALESCE(control.objective_id, turn.objective_id) IS NOT NULL",
    )
    .bind(run_instance_id)
    .fetch_optional(pool)
    .await?;
    let Some((session_id, root_turn_id, objective_id, objective_revision)) = identity else {
        return Ok(None);
    };
    let bound = sqlx::query(
        "UPDATE chat_run_controls SET objective_id=?, objective_revision=?, updated_at=?
         WHERE run_instance_id=? AND session_id=? AND root_turn_id=?
           AND status='cancel_requested'
           AND (objective_id IS NULL OR objective_id=?)",
    )
    .bind(&objective_id)
    .bind(objective_revision)
    .bind(Utc::now().timestamp_millis())
    .bind(run_instance_id)
    .bind(&session_id)
    .bind(&root_turn_id)
    .bind(&objective_id)
    .execute(pool)
    .await?;
    if bound.rows_affected() != 1 {
        return Err(AppError::Other(
            "chat cancellation identity changed before settlement".into(),
        ));
    }
    let settled =
        cancel_chat_objective_exact(pool, &session_id, &root_turn_id, &objective_id).await?;
    let control_status = match settled.status {
        crate::agent::objective::ObjectiveStatus::Cancelled => "cancelled",
        crate::agent::objective::ObjectiveStatus::Completed => "completed",
        _ => {
            return Err(AppError::Other(
                "chat cancellation did not reach a terminal Objective state".into(),
            ));
        }
    };
    let now = Utc::now().timestamp_millis();
    let updated = sqlx::query(
        "UPDATE chat_run_controls
         SET status=?, settled_at=?, updated_at=?
         WHERE run_instance_id=? AND session_id=? AND root_turn_id=?
           AND objective_id=? AND status='cancel_requested'",
    )
    .bind(control_status)
    .bind(now)
    .bind(now)
    .bind(run_instance_id)
    .bind(&session_id)
    .bind(&root_turn_id)
    .bind(&objective_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Other(
            "chat cancellation intent changed during settlement".into(),
        ));
    }
    Ok(Some(settled))
}

async fn settle_chat_run_control(
    pool: &sqlx::SqlitePool,
    run_instance_id: &str,
) -> Result<Option<crate::agent::objective::ObjectiveSnapshot>, AppError> {
    if let Some(cancelled) = consume_requested_chat_cancel(pool, run_instance_id).await? {
        return Ok(Some(cancelled));
    }
    let now = Utc::now().timestamp_millis();
    sqlx::query(
        "UPDATE chat_run_controls
         SET status=CASE
               WHEN status='cancel_requested' THEN 'cancelled'
               ELSE 'completed'
             END,
             settled_at=?, updated_at=?
         WHERE run_instance_id=? AND status IN ('active','cancel_requested')",
    )
    .bind(now)
    .bind(now)
    .bind(run_instance_id)
    .execute(pool)
    .await?;
    settle_finished_turn_provider_episodes(pool, run_instance_id, now).await?;
    Ok(None)
}

/// A run instance spans every model round of one turn, so its settlement is the
/// exact point where the turn's provider episodes stop being needed. Leaving
/// them live outlived their owner and permanently fenced the binding against
/// any later admission, which is how the 2026-08-13 session spun in system
/// recovery forever. Episodes whose evidence is still uncertain stay open for
/// the supervisor to observe.
async fn settle_finished_turn_provider_episodes(
    pool: &sqlx::SqlitePool,
    run_instance_id: &str,
    now: i64,
) -> Result<(), AppError> {
    let Some((session_id, root_turn_id)) = sqlx::query_as::<_, (String, String)>(
        "SELECT session_id, root_turn_id FROM chat_run_controls WHERE run_instance_id=?",
    )
    .bind(run_instance_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(());
    };
    crate::agent::provider_recovery::ProviderRecoveryStore::new(pool.clone())
        .settle_finished_turn_episodes(&session_id, &root_turn_id, now)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    Ok(())
}

#[tauri::command]
pub async fn is_chat_running(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<bool, AppError> {
    if state.chat_cancels.lock().await.contains_key(&session_id) {
        return Ok(true);
    }
    let pool = state.db.read().await.clone();
    let system_owned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives objective
         WHERE objective.session_id=? AND objective.root_turn_id IS NOT NULL
           AND objective.status IN ('active','waiting_system')
           AND NOT (objective.status='waiting_system'
                    AND objective.failure_code='technical_recovery_exhausted')",
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await?;
    Ok(system_owned > 0)
}

async fn clear_chat_running_if_current(
    chat_cancels: &crate::ChatCancelMap,
    session_id: &str,
    completed_control: &Arc<crate::ChatRunControl>,
) {
    let mut flags = chat_cancels.lock().await;
    if flags
        .get(session_id)
        .is_some_and(|current| Arc::ptr_eq(current, completed_control))
    {
        flags.remove(session_id);
    }
}

async fn admit_chat_run(
    chat_cancels: &crate::ChatCancelMap,
    update_restart_reserved: &std::sync::atomic::AtomicBool,
    session_id: &str,
    control: Arc<crate::ChatRunControl>,
) -> Result<(), AppError> {
    let mut active = chat_cancels.lock().await;
    if update_restart_reserved.load(Ordering::SeqCst) {
        return Err(AppError::Other(
            "应用更新已进入安全重启阶段，请等待自动恢复工作区".into(),
        ));
    }
    if active.contains_key(session_id) {
        return Err(AppError::Other("CHAT_RUN_BUSY".into()));
    }
    active.insert(session_id.to_string(), control);
    Ok(())
}

fn chat_settlement_status(objective: &crate::agent::objective::ObjectiveSnapshot) -> &'static str {
    use crate::agent::objective::{ObjectiveStatus, TECHNICAL_RECOVERY_EXHAUSTED};
    if objective.status == ObjectiveStatus::WaitingSystem
        && objective.failure_code.as_deref() == Some(TECHNICAL_RECOVERY_EXHAUSTED)
    {
        return "system_incident";
    }
    match objective.status {
        ObjectiveStatus::Completed => "completed",
        ObjectiveStatus::Cancelled => "cancelled",
        ObjectiveStatus::WaitingCoreInput
        | ObjectiveStatus::WaitingAuthorization
        | ObjectiveStatus::WaitingBusinessDecision => "waiting_user",
        ObjectiveStatus::Active
        | ObjectiveStatus::WaitingSystem
        | ObjectiveStatus::LegacyOrphan => "waiting_system",
    }
}

fn emit_chat_turn_settled(
    app: &AppHandle,
    event_name: &str,
    control: &crate::ChatRunControl,
    root_turn_id: Option<&str>,
    objective_id: Option<&str>,
    status: &str,
) {
    app.emit(
        event_name,
        StreamEvent::TurnSettled {
            run_instance_id: control.run_instance_id.clone(),
            root_turn_id: root_turn_id.map(str::to_string),
            objective_id: objective_id.map(str::to_string),
            status: status.to_string(),
        },
    )
    .ok();
}

async fn committed_chat_projection_is_current(
    db: &sqlx::SqlitePool,
    root_turn_id: &str,
    objective: &crate::agent::objective::ObjectiveSnapshot,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT 1
         FROM chat_turn_state turn
         JOIN objectives current ON current.id=turn.objective_id
         WHERE turn.root_turn_id=? AND turn.objective_id=?
           AND turn.terminal_revision=?
           AND current.revision=? AND current.status=?
         LIMIT 1",
    )
    .bind(root_turn_id)
    .bind(&objective.id)
    .bind(objective.revision)
    .bind(objective.revision)
    .bind(objective.status.as_str())
    .fetch_optional(db)
    .await?
    .is_some())
}

/// Publish a terminal Objective transition that was committed by the recovery
/// supervisor outside any foreground Chat run. The durable transaction is the
/// authority; this post-commit event only brings the currently-open WebView to
/// the same state. Replaying it is safe because both projections carry the
/// committed root/objective revision.
pub(crate) async fn publish_committed_objective_transition(
    app: &AppHandle,
    objective: &crate::agent::objective::ObjectiveSnapshot,
) -> Result<(), AppError> {
    let session_id = objective
        .session_id
        .as_deref()
        .ok_or_else(|| AppError::Other("committed Chat transition has no session".into()))?;
    let root_turn_id = objective
        .resume_cursor
        .as_deref()
        .or(objective.root_turn_id.as_deref())
        .ok_or_else(|| AppError::Other("committed Chat transition has no root turn".into()))?;
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Other("application state is unavailable".into()))?;
    let pool = state.db.read().await.clone();
    let event_name = format!("stream:{session_id}");
    project_chat_objective(&pool, app, &event_name, root_turn_id, objective).await?;
    if !committed_chat_projection_is_current(&pool, root_turn_id, objective).await? {
        tracing::debug!(
            objective_id = %objective.id,
            objective_revision = objective.revision,
            root_turn_id,
            "did not publish a stale supervisor settlement after the Objective advanced"
        );
        return Ok(());
    }
    app.emit(
        &event_name,
        StreamEvent::TurnSettled {
            run_instance_id: format!(
                "objective-supervisor:{}:{}",
                objective.id, objective.revision
            ),
            root_turn_id: Some(root_turn_id.to_string()),
            objective_id: Some(objective.id.clone()),
            status: chat_settlement_status(objective).to_string(),
        },
    )
    .map_err(|error| {
        AppError::Other(format!(
            "failed to publish committed Chat settlement: {error}"
        ))
    })?;
    Ok(())
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

/// How one durable Objective presents itself on the transport turn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChatTurnProjection {
    phase: &'static str,
    activity_kind: &'static str,
    activity_label: &'static str,
    /// `Some` once the turn has stopped moving on its own — either a terminal
    /// objective, or system-owned recovery that ran out of budget.
    terminal_reason: Option<&'static str>,
}

fn chat_turn_projection(
    objective: &crate::agent::objective::ObjectiveSnapshot,
) -> ChatTurnProjection {
    use crate::agent::objective::{ObjectiveStatus, TECHNICAL_RECOVERY_EXHAUSTED};
    // Recovery that ran out of budget is a settled turn, not a live one. The
    // objective stays non-terminal (the work was never finished), but the
    // transport turn must stop presenting itself as still recovering.
    let recovery_exhausted = objective.status == ObjectiveStatus::WaitingSystem
        && objective.failure_code.as_deref() == Some(TECHNICAL_RECOVERY_EXHAUSTED);
    let (phase, activity_kind, activity_label) = match objective.status {
        ObjectiveStatus::Completed => ("finalizing", "objective_completed", "目标证据已满足"),
        ObjectiveStatus::Cancelled => ("finalizing", "objective_cancelled", "已按用户要求停止"),
        ObjectiveStatus::WaitingSystem if recovery_exhausted => (
            "waiting",
            TECHNICAL_RECOVERY_EXHAUSTED,
            "自动恢复已达到安全上限；系统已登记故障，无需补充输入",
        ),
        ObjectiveStatus::WaitingSystem => ("recovering", "system_recovery", "系统正在恢复并续接"),
        ObjectiveStatus::WaitingCoreInput => ("waiting", "core_input_required", "需要补充核心输入"),
        ObjectiveStatus::WaitingAuthorization => {
            ("waiting", "authorization_required", "等待必要授权")
        }
        ObjectiveStatus::WaitingBusinessDecision => {
            ("waiting", "business_decision_required", "等待业务决策")
        }
        ObjectiveStatus::Active => ("working", "objective_active", "系统正在继续处理"),
        ObjectiveStatus::LegacyOrphan => (
            "recovering",
            "legacy_reconciliation",
            "系统正在核对历史工作",
        ),
    };
    ChatTurnProjection {
        phase,
        activity_kind,
        activity_label,
        terminal_reason: if recovery_exhausted {
            Some(TECHNICAL_RECOVERY_EXHAUSTED)
        } else if objective.status.is_terminal() {
            Some(objective.decision_type.as_str())
        } else {
            None
        },
    }
}

async fn project_chat_objective(
    db: &sqlx::SqlitePool,
    app: &AppHandle,
    event_name: &str,
    root_turn_id: &str,
    objective: &crate::agent::objective::ObjectiveSnapshot,
) -> Result<(), AppError> {
    let now = Utc::now().timestamp_millis();
    let ChatTurnProjection {
        phase,
        activity_kind,
        activity_label,
        terminal_reason,
    } = chat_turn_projection(objective);
    let waiting_reason = objective
        .failure_code
        .as_deref()
        .or(objective.request_key.as_deref())
        .or(objective.decision_key.as_deref());
    let completed_at = terminal_reason.is_some().then_some(now);
    let revision: Option<i64> = sqlx::query_scalar(
        "UPDATE chat_turn_state SET revision=revision+1, phase=?, status=?,
           recent_activity_kind=?, recent_activity_label=?, waiting_reason=?,
           updated_at=?,
           completed_at=CASE WHEN terminal_revision IS NULL THEN ? ELSE completed_at END,
           terminal_reason=CASE WHEN terminal_revision IS NULL THEN ? ELSE terminal_reason END,
           objective_revision=?
         WHERE root_turn_id=? AND objective_id=?
           AND terminal_revision IS NULL
           AND EXISTS (
             SELECT 1 FROM objectives current
             WHERE current.id=? AND current.revision=? AND current.status=?
           )
         RETURNING revision",
    )
    .bind(phase)
    .bind(objective.status.as_str())
    .bind(activity_kind)
    .bind(activity_label)
    .bind(waiting_reason)
    .bind(now)
    .bind(completed_at)
    .bind(terminal_reason)
    .bind(objective.revision)
    .bind(root_turn_id)
    .bind(&objective.id)
    .bind(&objective.id)
    .bind(objective.revision)
    .bind(objective.status.as_str())
    .fetch_optional(db)
    .await?;
    let revision = if let Some(revision) = revision {
        revision
    } else if let Some(revision) = sqlx::query_scalar::<_, i64>(
        "SELECT turn.revision FROM chat_turn_state turn
         WHERE turn.root_turn_id=? AND turn.objective_id=?
           AND turn.terminal_revision=?
           AND EXISTS (
             SELECT 1 FROM objectives current
             WHERE current.id=? AND current.revision=? AND current.status=?
           )",
    )
    .bind(root_turn_id)
    .bind(&objective.id)
    .bind(objective.revision)
    .bind(&objective.id)
    .bind(objective.revision)
    .bind(objective.status.as_str())
    .fetch_optional(db)
    .await?
    {
        tracing::debug!(
            root_turn_id,
            objective_revision = objective.revision,
            "published an already-persisted terminal projection without rewriting it"
        );
        revision
    } else {
        tracing::debug!(
            objective_id = %objective.id,
            objective_revision = objective.revision,
            objective_status = %objective.status.as_str(),
            "ignored stale Chat projection after Objective advanced"
        );
        return Ok(());
    };
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
            terminal_reason: terminal_reason.map(str::to_string),
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
    expected_revision: i64,
    root_turn_id: &str,
    outcome: &codefactory_agent_loop::run::RunOutcome,
    mutation_permit: Option<&codefactory_agent_loop::tool::MutationPermit>,
) -> Result<crate::agent::objective::ObjectiveSnapshot, AppError> {
    let revised = apply_chat_objective_outcome(
        db,
        objective_id,
        expected_revision,
        root_turn_id,
        outcome,
        mutation_permit,
    )
    .await?;
    project_chat_objective(db, app, event_name, root_turn_id, &revised).await?;
    Ok(revised)
}

async fn apply_chat_objective_outcome(
    db: &sqlx::SqlitePool,
    objective_id: &str,
    expected_revision: i64,
    root_turn_id: &str,
    outcome: &codefactory_agent_loop::run::RunOutcome,
    mutation_permit: Option<&codefactory_agent_loop::tool::MutationPermit>,
) -> Result<crate::agent::objective::ObjectiveSnapshot, AppError> {
    let store = crate::agent::objective::ObjectiveStore::new(db.clone());
    let current = store
        .get(objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other(format!("objective {objective_id} missing")))?;
    if current.status.is_terminal() {
        return Ok(current);
    }
    if current.revision != expected_revision {
        return Err(AppError::Other(format!(
            "stale chat runner revision: expected {expected_revision}, actual {}",
            current.revision
        )));
    }
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
    match mutation_permit {
        Some(permit) => {
            store
                .apply_claimed_decision(current.revision, decision, permit)
                .await
        }
        None => store.apply_decision(current.revision, decision).await,
    }
    .map_err(|error| AppError::Other(error.to_string()))
}

/// Settle the production Objective decision path without a Tauri window.
/// Formal executable recovery smokes use this after the real AgentLoop exits;
/// the typed decision and claimed-remediation CAS are unchanged.
#[cfg(not(test))]
pub(crate) async fn settle_headless_chat_objective_from_outcome(
    db: &sqlx::SqlitePool,
    objective_id: &str,
    expected_revision: i64,
    root_turn_id: &str,
    outcome: &codefactory_agent_loop::run::RunOutcome,
    mutation_permit: Option<&codefactory_agent_loop::tool::MutationPermit>,
) -> Result<crate::agent::objective::ObjectiveSnapshot, AppError> {
    apply_chat_objective_outcome(
        db,
        objective_id,
        expected_revision,
        root_turn_id,
        outcome,
        mutation_permit,
    )
    .await
}

async fn settle_chat_objective_from_error(
    db: &sqlx::SqlitePool,
    app: &AppHandle,
    event_name: &str,
    objective_id: &str,
    expected_revision: i64,
    root_turn_id: &str,
    auth_expired: bool,
    error_text: &str,
    mutation_permit: Option<&codefactory_agent_loop::tool::MutationPermit>,
) -> Result<crate::agent::objective::ObjectiveSnapshot, AppError> {
    use crate::agent::objective::{DecisionRouter, RecoveryDomain, RouteSignal};
    let store = crate::agent::objective::ObjectiveStore::new(db.clone());
    let current = store
        .get(objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other(format!("objective {objective_id} missing")))?;
    if current.status.is_terminal() {
        project_chat_objective(db, app, event_name, root_turn_id, &current).await?;
        return Ok(current);
    }
    if current.revision != expected_revision {
        return Err(AppError::Other(format!(
            "stale chat runner revision: expected {expected_revision}, actual {}",
            current.revision
        )));
    }
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
    let revised = match mutation_permit {
        Some(permit) => {
            store
                .apply_claimed_decision(current.revision, decision, permit)
                .await
        }
        None => store.apply_decision(current.revision, decision).await,
    }
    .map_err(|error| AppError::Other(error.to_string()))?;
    project_chat_objective(db, app, event_name, root_turn_id, &revised).await?;
    Ok(revised)
}

/// A detached foreground runner cannot return a settlement error to its IPC
/// caller. Re-read the durable Objective and, only when the same revision is
/// still active, transfer the failure to system-owned recovery. A newer or
/// already-settled revision is projected as-is; the stale runner never applies
/// its result to that newer business state.
async fn recover_chat_settlement_failure(
    db: &sqlx::SqlitePool,
    app: &AppHandle,
    event_name: &str,
    objective_id: &str,
    expected_revision: i64,
    root_turn_id: &str,
    failure: &str,
) -> Result<crate::agent::objective::ObjectiveSnapshot, AppError> {
    use crate::agent::objective::{DecisionRouter, ObjectiveStatus, RecoveryDomain, RouteSignal};
    let store = crate::agent::objective::ObjectiveStore::new(db.clone());
    let current = store
        .get(objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other(format!("objective {objective_id} missing")))?;
    if current.revision != expected_revision || current.status != ObjectiveStatus::Active {
        project_chat_objective(db, app, event_name, root_turn_id, &current).await?;
        return Ok(current);
    }
    let decision = DecisionRouter::route(
        &current,
        RouteSignal::TechnicalFailure {
            domain: RecoveryDomain::Chat,
            failure_code: "chat_settlement_failed".into(),
            failure_signature: format!("sha256:{:x}", Sha256::digest(failure.as_bytes())),
            next_observation_at: Utc::now().timestamp_millis() + 5_000,
            resume_cursor: Some(root_turn_id.to_string()),
        },
    )
    .map_err(|error| AppError::Other(error.to_string()))?;
    let revised = store
        .apply_decision(expected_revision, decision)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    project_chat_objective(db, app, event_name, root_turn_id, &revised).await?;
    Ok(revised)
}

struct ChatRunningSetupGuard {
    chat_cancels: crate::ChatCancelMap,
    session_id: String,
    control: Arc<crate::ChatRunControl>,
    durable_db: Option<sqlx::SqlitePool>,
    #[cfg(not(test))]
    event_app: Option<AppHandle>,
    armed: bool,
}

struct ChatSetupConvergence {
    root_turn_id: String,
    objective: crate::agent::objective::ObjectiveSnapshot,
}

/// Converge an admitted turn whose foreground setup disappeared before the
/// AgentLoop future was handed to its supervisor. This function deliberately
/// derives identity from the durable run receipt instead of values captured by
/// the dropping future: a cancel racing setup and a restart both observe the
/// same root/Objective/revision tuple.
async fn converge_aborted_chat_setup(
    pool: &sqlx::SqlitePool,
    control: &crate::ChatRunControl,
    session_id: &str,
) -> Result<Option<ChatSetupConvergence>, AppError> {
    use crate::agent::objective::{DecisionRouter, ObjectiveStatus, RecoveryDomain, RouteSignal};

    if control.cancel.load(Ordering::SeqCst) {
        // The process-local flag is fast, but only this exact durable intent
        // can fence startup recovery after a crash.
        request_chat_run_cancel(pool, &control.run_instance_id, session_id).await?;
    }
    if let Some(cancelled) = consume_requested_chat_cancel(pool, &control.run_instance_id).await? {
        let root_turn_id = sqlx::query_scalar::<_, Option<String>>(
            "SELECT root_turn_id FROM chat_run_controls
             WHERE run_instance_id=? AND session_id=?",
        )
        .bind(&control.run_instance_id)
        .bind(session_id)
        .fetch_optional(pool)
        .await?
        .flatten()
        .ok_or_else(|| AppError::Other("cancelled chat setup lost its root identity".into()))?;
        return Ok(Some(ChatSetupConvergence {
            root_turn_id,
            objective: cancelled,
        }));
    }

    let identity = sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<i64>)>(
        "SELECT control.status, control.root_turn_id,
                COALESCE(control.objective_id, turn.objective_id),
                COALESCE(control.objective_revision, objective.revision)
         FROM chat_run_controls control
         LEFT JOIN chat_turn_state turn
           ON turn.root_turn_id=control.root_turn_id
          AND turn.session_id=control.session_id
         LEFT JOIN objectives objective
           ON objective.id=COALESCE(control.objective_id, turn.objective_id)
         WHERE control.run_instance_id=? AND control.session_id=?",
    )
    .bind(&control.run_instance_id)
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    let Some((control_status, root_turn_id, objective_id, objective_revision)) = identity else {
        // Admission itself never committed, so there is no durable Objective
        // to recover and no frontend turn receipt to settle.
        return Ok(None);
    };
    let identity = match (root_turn_id, objective_id, objective_revision) {
        (Some(root_turn_id), Some(objective_id), Some(objective_revision)) => {
            Some((root_turn_id, objective_id, objective_revision))
        }
        (None, None, None) | (Some(_), None, None) => None,
        _ => {
            return Err(AppError::Other(
                "aborted chat setup has an incomplete Objective/root/revision identity".into(),
            ));
        }
    };

    let Some((root_turn_id, objective_id, objective_revision)) = identity else {
        // This is the pre-admission registration window used to make an early
        // stop durable. With no committed Objective, retiring the transport is
        // sufficient and cannot orphan business work.
        settle_chat_run_control(pool, &control.run_instance_id).await?;
        return Ok(None);
    };

    let store = crate::agent::objective::ObjectiveStore::new(pool.clone());
    let mut objective = store
        .get(&objective_id)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
        .ok_or_else(|| AppError::Other(format!("objective {objective_id} missing")))?;
    let root_matches_objective = objective.root_turn_id.as_deref() == Some(root_turn_id.as_str())
        || objective.resume_cursor.as_deref() == Some(root_turn_id.as_str());
    if objective.session_id.as_deref() != Some(session_id) || !root_matches_objective {
        return Err(AppError::Other(
            "aborted chat setup durable identity mismatch".into(),
        ));
    }

    if objective.status == ObjectiveStatus::Active && objective.revision == objective_revision {
        // Only the exact admitted revision may be transferred. If another
        // actor already advanced the Objective, this stale setup guard merely
        // retires its own transport receipt and projects the durable winner.
        let decision = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "chat_setup_aborted".into(),
                failure_signature: format!(
                    "sha256:{:x}",
                    Sha256::digest(
                        format!("chat_setup_aborted:{}", control.run_instance_id).as_bytes()
                    )
                ),
                next_observation_at: Utc::now().timestamp_millis() + 5_000,
                resume_cursor: Some(root_turn_id.clone()),
            },
        )
        .map_err(|error| AppError::Other(error.to_string()))?;
        objective = store
            .apply_decision(objective_revision, decision)
            .await
            .map_err(|error| AppError::Other(error.to_string()))?;
    }

    if let Some(cancelled) = settle_chat_run_control(pool, &control.run_instance_id).await? {
        objective = cancelled;
    }
    if control_status == "cancel_requested" && objective.status != ObjectiveStatus::Cancelled {
        return Err(AppError::Other(
            "aborted chat setup cancellation did not converge".into(),
        ));
    }
    Ok(Some(ChatSetupConvergence {
        root_turn_id,
        objective,
    }))
}

#[cfg(not(test))]
async fn converge_and_project_aborted_chat_setup(
    pool: &sqlx::SqlitePool,
    chat_cancels: &crate::ChatCancelMap,
    control: &Arc<crate::ChatRunControl>,
    session_id: &str,
    event_app: Option<&AppHandle>,
) -> Result<(), AppError> {
    let settlement = converge_aborted_chat_setup(pool, control, session_id).await?;
    if let (Some(app), Some(settlement)) = (event_app, settlement) {
        let event_name = format!("stream:{session_id}");
        project_chat_objective(
            pool,
            app,
            &event_name,
            &settlement.root_turn_id,
            &settlement.objective,
        )
        .await?;
        emit_chat_turn_settled(
            app,
            &event_name,
            control,
            Some(&settlement.root_turn_id),
            Some(&settlement.objective.id),
            chat_settlement_status(&settlement.objective),
        );
    }
    clear_chat_running_if_current(chat_cancels, session_id, control).await;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChatContinuationResolution {
    AlreadyBound {
        objective_id: String,
    },
    None,
    Unique {
        objective_id: String,
        continuation_root_turn_id: String,
        previous_segment_id: String,
        driver: String,
    },
    Ambiguous,
}

#[derive(Debug, Clone)]
struct ChatContinuationSetup {
    continuation_root_turn_id: Option<String>,
    previous_segment_id: Option<String>,
    expected_objective_id: Option<String>,
    driver: Option<String>,
    ambiguous: bool,
}

#[derive(Debug)]
struct ChatAdmissionReceipt {
    root_turn_id: String,
    objective: crate::agent::objective::ObjectiveSnapshot,
    contract: crate::agent::dispatch::ChatContract,
    cancel_requested: bool,
    continuation_driver: Option<String>,
    already_settled: bool,
}

/// AppHandle-free admission receipt used by formal executable system smokes.
/// It delegates to the desktop's atomic admission transaction rather than
/// maintaining a second test-only chat schema.
#[cfg(not(test))]
pub(crate) struct HeadlessChatAdmission {
    pub root_turn_id: String,
    pub objective: crate::agent::objective::ObjectiveSnapshot,
}

#[cfg(not(test))]
pub(crate) async fn admit_headless_chat_turn(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    content: &str,
) -> Result<HeadlessChatAdmission, AppError> {
    let control = crate::ChatRunControl::pending();
    let admission = admit_persisted_chat_turn(pool, &control, session_id, None, content).await?;
    Ok(HeadlessChatAdmission {
        root_turn_id: admission.root_turn_id,
        objective: admission.objective,
    })
}

async fn resolve_open_chat_objective_continuation_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: &str,
    root_turn_id: &str,
    current_ordinal: i64,
) -> Result<ChatContinuationResolution, AppError> {
    let current_binding = sqlx::query_scalar::<_, Option<String>>(
        "SELECT objective_id FROM chat_turn_state
         WHERE root_turn_id=? AND session_id=?",
    )
    .bind(root_turn_id)
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?
    .flatten()
    .filter(|value| !value.is_empty());
    if let Some(objective_id) = current_binding {
        return Ok(ChatContinuationResolution::AlreadyBound { objective_id });
    }

    let candidates = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT objective.id, objective.status, turn.root_turn_id, segment.id
         FROM objectives objective
         JOIN chat_turn_state turn ON turn.objective_id=objective.id
         JOIN chat_task_segments segment ON segment.id=turn.task_segment_id
         WHERE turn.session_id=? AND turn.root_turn_id<>?
           AND objective.status NOT IN ('completed', 'cancelled', 'legacy_orphan')
           AND segment.ordinal<?
           AND segment.ordinal=(
             SELECT MAX(prior_segment.ordinal)
             FROM chat_turn_state prior_turn
             JOIN chat_task_segments prior_segment ON prior_segment.id=prior_turn.task_segment_id
             WHERE prior_turn.objective_id=objective.id
               AND prior_turn.session_id=?
               AND prior_turn.root_turn_id<>?
               AND prior_segment.ordinal<?
           )
         ORDER BY objective.id
         LIMIT 2",
    )
    .bind(session_id)
    .bind(root_turn_id)
    .bind(current_ordinal)
    .bind(session_id)
    .bind(root_turn_id)
    .bind(current_ordinal)
    .fetch_all(&mut **tx)
    .await?;
    if candidates.len() > 1 {
        return Ok(ChatContinuationResolution::Ambiguous);
    }
    let Some((objective_id, objective_status, continuation_root_turn_id, previous_segment_id)) =
        candidates.into_iter().next()
    else {
        return Ok(ChatContinuationResolution::None);
    };
    let delivery_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM delivery_runs
         WHERE objective_id=?
           AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
         ORDER BY updated_at DESC, id DESC LIMIT 1",
    )
    .bind(&objective_id)
    .fetch_optional(&mut **tx)
    .await?;
    let driver = match (objective_status.as_str(), delivery_status.as_deref()) {
        ("waiting_system", Some("waiting")) => "recoverable_waiting_open",
        ("waiting_system", Some("awaiting_completion_arbitration")) => {
            "completion_arbitration_open"
        }
        ("waiting_system", _) => "system_owned_remediation_open",
        ("waiting_core_input", _) => "core_input_response",
        ("waiting_authorization", _) => "authorization_response",
        ("waiting_business_decision", _) => "business_decision_response",
        _ => "authorized_objective_still_open",
    }
    .to_string();
    Ok(ChatContinuationResolution::Unique {
        objective_id,
        continuation_root_turn_id,
        previous_segment_id,
        driver,
    })
}

/// Commit the exact user input, execution projection, opaque Objective, and
/// durable run ownership as one receipt before any route, credential,
/// checkpoint, or provider work can fail. A retry may read the same identity;
/// it never guesses the latest unfinished message or reopens a terminal turn.
async fn admit_persisted_chat_turn(
    pool: &sqlx::SqlitePool,
    control: &crate::ChatRunControl,
    session_id: &str,
    requested_root_turn_id: Option<&str>,
    content: &str,
) -> Result<ChatAdmissionReceipt, AppError> {
    if content.trim().is_empty() {
        return Err(AppError::Other("chat input cannot be empty".into()));
    }
    let root_turn_id = requested_root_turn_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let now = Utc::now().timestamp_millis();
    let mut tx = pool.begin().await?;
    let session_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id=?")
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
    if session_exists != 1 {
        return Err(AppError::Other("chat session identity is missing".into()));
    }

    let existing_message = sqlx::query_as::<_, (String, String, String)>(
        "SELECT session_id, role, content FROM messages WHERE id=?",
    )
    .bind(&root_turn_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((bound_session, role, stored_content)) = existing_message {
        if (
            bound_session.as_str(),
            role.as_str(),
            stored_content.as_str(),
        ) != (session_id, "user", content)
        {
            return Err(AppError::Other(
                "chat root message identity/content mismatch".into(),
            ));
        }
    } else {
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES (?, ?, 'user', ?, ?)",
        )
        .bind(&root_turn_id)
        .bind(session_id)
        .bind(content)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }

    let cancelled = control.cancel.load(Ordering::SeqCst);
    sqlx::query(
        "INSERT INTO chat_run_controls
         (run_instance_id, session_id, root_turn_id, status,
          created_process_instance, cancel_requested_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(run_instance_id) DO UPDATE SET
           root_turn_id=excluded.root_turn_id,
           status=CASE WHEN chat_run_controls.status='cancel_requested'
                       THEN 'cancel_requested' ELSE chat_run_controls.status END,
           updated_at=excluded.updated_at
         WHERE chat_run_controls.session_id=excluded.session_id
           AND chat_run_controls.status IN ('active','cancel_requested')
           AND (chat_run_controls.root_turn_id IS NULL
                OR chat_run_controls.root_turn_id=excluded.root_turn_id)",
    )
    .bind(&control.run_instance_id)
    .bind(session_id)
    .bind(&root_turn_id)
    .bind(if cancelled {
        "cancel_requested"
    } else {
        "active"
    })
    .bind(crate::agent::objective::current_process_instance())
    .bind(cancelled.then_some(now))
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    let existing_segment = sqlx::query_as::<_, (String, i64)>(
        "SELECT id, ordinal FROM chat_task_segments
         WHERE session_id=? AND goal_root_turn_id=?",
    )
    .bind(session_id)
    .bind(&root_turn_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (segment_id, ordinal) = if let Some(existing) = existing_segment {
        existing
    } else {
        let ordinal: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM chat_task_segments WHERE session_id=?",
        )
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
        let segment_id = Uuid::new_v4().to_string();
        let title: String = content.chars().take(60).collect();
        sqlx::query(
            "INSERT INTO chat_task_segments
             (id, session_id, ordinal, title, status, goal_root_turn_id,
              previous_segment_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'active', ?, NULL, ?, ?)",
        )
        .bind(&segment_id)
        .bind(session_id)
        .bind(ordinal)
        .bind(if title.trim().is_empty() {
            "新任务"
        } else {
            &title
        })
        .bind(&root_turn_id)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        (segment_id, ordinal)
    };

    let existing_turn = sqlx::query_as::<_, (String, Option<String>, String)>(
        "SELECT session_id, objective_id, status FROM chat_turn_state WHERE root_turn_id=?",
    )
    .bind(&root_turn_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((bound_session, _, _)) = existing_turn.as_ref() {
        if bound_session != session_id {
            return Err(AppError::Other(
                "chat root turn is already bound to another session".into(),
            ));
        }
    } else {
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, task_segment_id, revision, phase, status,
              started_at, updated_at, recent_activity_kind, recent_activity_label)
             VALUES (?, ?, ?, 1, 'planning', 'active', ?, ?,
                     'turn_started', '正在理解任务')",
        )
        .bind(&root_turn_id)
        .bind(session_id)
        .bind(&segment_id)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }

    let resolution =
        resolve_open_chat_objective_continuation_in_tx(&mut tx, session_id, &root_turn_id, ordinal)
            .await?;
    if matches!(resolution, ChatContinuationResolution::Ambiguous) {
        return Err(AppError::Other(
            "CHAT_OBJECTIVE_IDENTITY_AMBIGUOUS: system reconciliation required".into(),
        ));
    }
    let legacy_continuation = if matches!(resolution, ChatContinuationResolution::None)
        && crate::agent::is_contextual_approval(content)
    {
        sqlx::query_as::<_, (String, String)>(
            "SELECT segment.id, segment.goal_root_turn_id
             FROM chat_task_segments segment
             LEFT JOIN chat_turn_state turn
               ON turn.root_turn_id=segment.goal_root_turn_id
             WHERE segment.session_id=? AND segment.ordinal<?
               AND (turn.objective_id IS NULL OR turn.objective_id='')
             ORDER BY segment.ordinal DESC LIMIT 1",
        )
        .bind(session_id)
        .bind(ordinal)
        .fetch_optional(&mut *tx)
        .await?
    } else {
        None
    };
    let setup = match resolution {
        ChatContinuationResolution::AlreadyBound { objective_id } => ChatContinuationSetup {
            continuation_root_turn_id: None,
            previous_segment_id: None,
            expected_objective_id: Some(objective_id),
            driver: None,
            ambiguous: false,
        },
        ChatContinuationResolution::Unique {
            objective_id,
            continuation_root_turn_id,
            previous_segment_id,
            driver,
        } => ChatContinuationSetup {
            continuation_root_turn_id: Some(continuation_root_turn_id),
            previous_segment_id: Some(previous_segment_id),
            expected_objective_id: Some(objective_id),
            driver: Some(driver),
            ambiguous: false,
        },
        ChatContinuationResolution::None => ChatContinuationSetup {
            continuation_root_turn_id: legacy_continuation.as_ref().map(|(_, root)| root.clone()),
            previous_segment_id: legacy_continuation.map(|(segment, _)| segment),
            expected_objective_id: None,
            driver: None,
            ambiguous: false,
        },
        ChatContinuationResolution::Ambiguous => unreachable!(),
    };

    let assistant_candidates = sqlx::query_scalar::<_, String>(
        "SELECT message.content FROM messages message
         WHERE message.session_id=? AND message.role='assistant'
           AND (message.completion_state IS NULL OR message.completion_state='')
           AND NOT EXISTS (
             SELECT 1 FROM gate_events gate
             WHERE gate.message_id=message.id AND gate.kind='rejected_candidate')
         ORDER BY message.created_at DESC, message.rowid DESC",
    )
    .bind(session_id)
    .fetch_all(&mut *tx)
    .await?;
    let previous_assistant = assistant_candidates.into_iter().find(|message| {
        !crate::agent::is_contextual_approval(content)
            || crate::agent::proposal_capability(message).is_some()
    });
    let mut contract = crate::agent::decide_chat_contract(previous_assistant.as_deref(), content);
    let delivery_authorized: i64 =
        sqlx::query_scalar("SELECT delivery_authorized FROM sessions WHERE id=?")
            .bind(session_id)
            .fetch_one(&mut *tx)
            .await?;
    if crate::agent::is_delivery_revocation(content) {
        sqlx::query("UPDATE sessions SET delivery_authorized=0 WHERE id=?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
    } else if contract.capability == crate::agent::TurnCapability::Deliver {
        sqlx::query("UPDATE sessions SET delivery_authorized=1 WHERE id=?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
    } else if delivery_authorized != 0 {
        contract = crate::agent::with_persisted_delivery_authorization(contract, true);
    }

    let objective_kind = chat_objective_kind(contract.capability, content);
    let store = crate::agent::objective::ObjectiveStore::new(pool.clone());
    let objective = store
        .ensure_or_continue_chat_objective_in_tx(
            &mut tx,
            session_id,
            &root_turn_id,
            setup.continuation_root_turn_id.as_deref(),
            objective_kind,
            requested_acceptance(objective_kind),
        )
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    if setup
        .expected_objective_id
        .as_deref()
        .is_some_and(|expected| expected != objective.id)
    {
        return Err(AppError::Other(
            "opaque Objective identity changed during admission".into(),
        ));
    }
    if let Some(previous_segment_id) = setup.previous_segment_id.as_deref() {
        if previous_segment_id == segment_id {
            return Err(AppError::Other(
                "chat continuation refused a self-referential segment".into(),
            ));
        }
        sqlx::query(
            "UPDATE chat_task_segments SET previous_segment_id=?
             WHERE id=? AND session_id=? AND previous_segment_id IS NULL",
        )
        .bind(previous_segment_id)
        .bind(&segment_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(driver) = setup.driver.as_deref() {
        sqlx::query(
            "UPDATE chat_turn_state SET user_reprompt_driver=?
             WHERE root_turn_id=? AND session_id=? AND objective_id=?",
        )
        .bind(driver)
        .bind(&root_turn_id)
        .bind(session_id)
        .bind(&objective.id)
        .execute(&mut *tx)
        .await?;
    }
    let bound = sqlx::query(
        "UPDATE chat_run_controls
         SET objective_id=?, objective_revision=?, updated_at=?
         WHERE run_instance_id=? AND session_id=? AND root_turn_id=?
           AND status IN ('active','cancel_requested')
           AND (objective_id IS NULL
                OR (objective_id=? AND objective_revision=?))
           AND EXISTS (
             SELECT 1 FROM chat_turn_state turn
             WHERE turn.root_turn_id=? AND turn.session_id=?
               AND turn.objective_id=?)",
    )
    .bind(&objective.id)
    .bind(objective.revision)
    .bind(now)
    .bind(&control.run_instance_id)
    .bind(session_id)
    .bind(&root_turn_id)
    .bind(&objective.id)
    .bind(objective.revision)
    .bind(&root_turn_id)
    .bind(session_id)
    .bind(&objective.id)
    .execute(&mut *tx)
    .await?;
    if bound.rows_affected() != 1 {
        return Err(AppError::Other(
            "chat run final Objective binding changed during admission".into(),
        ));
    }
    let control_status: String =
        sqlx::query_scalar("SELECT status FROM chat_run_controls WHERE run_instance_id=?")
            .bind(&control.run_instance_id)
            .fetch_one(&mut *tx)
            .await?;
    let already_settled = existing_turn
        .as_ref()
        .is_some_and(|(_, _, status)| matches!(status.as_str(), "completed" | "cancelled"));
    tx.commit().await?;
    Ok(ChatAdmissionReceipt {
        root_turn_id,
        objective,
        contract,
        cancel_requested: control_status == "cancel_requested",
        continuation_driver: setup.driver,
        already_settled,
    })
}

/// Resolve reprompt continuity from opaque Objective state, never message
/// keywords or a parseable id. DeliveryRun may refine the diagnostic driver
/// only after the Objective has already been uniquely selected.
async fn resolve_open_chat_objective_continuation(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    root_turn_id: &str,
    current_ordinal: i64,
) -> Result<ChatContinuationResolution, AppError> {
    let current_binding = sqlx::query_scalar::<_, Option<String>>(
        "SELECT objective_id FROM chat_turn_state
         WHERE root_turn_id=? AND session_id=?",
    )
    .bind(root_turn_id)
    .bind(session_id)
    .fetch_optional(pool)
    .await?
    .flatten()
    .filter(|value| !value.is_empty());
    if let Some(objective_id) = current_binding {
        return Ok(ChatContinuationResolution::AlreadyBound { objective_id });
    }

    let candidates = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT objective.id, objective.status, turn.root_turn_id, segment.id
         FROM objectives objective
         JOIN chat_turn_state turn ON turn.objective_id=objective.id
         JOIN chat_task_segments segment ON segment.id=turn.task_segment_id
         WHERE turn.session_id=? AND turn.root_turn_id<>?
           AND objective.status NOT IN ('completed', 'cancelled', 'legacy_orphan')
           AND segment.ordinal<?
           AND segment.ordinal=(
             SELECT MAX(prior_segment.ordinal)
             FROM chat_turn_state prior_turn
             JOIN chat_task_segments prior_segment ON prior_segment.id=prior_turn.task_segment_id
             WHERE prior_turn.objective_id=objective.id
               AND prior_turn.session_id=?
               AND prior_turn.root_turn_id<>?
               AND prior_segment.ordinal<?
           )
         ORDER BY objective.id
         LIMIT 2",
    )
    .bind(session_id)
    .bind(root_turn_id)
    .bind(current_ordinal)
    .bind(session_id)
    .bind(root_turn_id)
    .bind(current_ordinal)
    .fetch_all(pool)
    .await?;
    if candidates.len() > 1 {
        return Ok(ChatContinuationResolution::Ambiguous);
    }
    let Some((objective_id, objective_status, continuation_root_turn_id, previous_segment_id)) =
        candidates.into_iter().next()
    else {
        return Ok(ChatContinuationResolution::None);
    };
    let delivery_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM delivery_runs
         WHERE objective_id=?
           AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
         ORDER BY updated_at DESC, id DESC LIMIT 1",
    )
    .bind(&objective_id)
    .fetch_optional(pool)
    .await?;
    let driver = match (objective_status.as_str(), delivery_status.as_deref()) {
        ("waiting_system", Some("waiting")) => "recoverable_waiting_open",
        ("waiting_system", Some("awaiting_completion_arbitration")) => {
            "completion_arbitration_open"
        }
        ("waiting_system", _) => "system_owned_remediation_open",
        ("waiting_core_input", _) => "core_input_response",
        ("waiting_authorization", _) => "authorization_response",
        ("waiting_business_decision", _) => "business_decision_response",
        _ => "authorized_objective_still_open",
    }
    .to_string();
    Ok(ChatContinuationResolution::Unique {
        objective_id,
        continuation_root_turn_id,
        previous_segment_id,
        driver,
    })
}

impl ChatRunningSetupGuard {
    fn new(
        chat_cancels: crate::ChatCancelMap,
        session_id: String,
        control: Arc<crate::ChatRunControl>,
    ) -> Self {
        Self {
            chat_cancels,
            session_id,
            control,
            durable_db: None,
            #[cfg(not(test))]
            event_app: None,
            armed: true,
        }
    }

    fn attach_durable_db(&mut self, db: sqlx::SqlitePool) {
        self.durable_db = Some(db);
    }

    #[cfg(not(test))]
    fn attach_event_app(&mut self, app: AppHandle) {
        self.event_app = Some(app);
    }

    /// Explicit early-return seam for every fallible operation after atomic
    /// admission. Drop remains the panic/cancellation fallback, but ordinary
    /// setup errors await their durable recovery receipt before the command
    /// reports success to the frontend.
    async fn settle_now(&mut self) -> Result<(), AppError> {
        if !self.armed {
            return Ok(());
        }
        let Some(db) = self.durable_db.as_ref() else {
            return Err(AppError::Other(
                "admitted chat setup has no durable database handle".into(),
            ));
        };
        #[cfg(not(test))]
        converge_and_project_aborted_chat_setup(
            db,
            &self.chat_cancels,
            &self.control,
            &self.session_id,
            self.event_app.as_ref(),
        )
        .await?;
        #[cfg(test)]
        {
            converge_aborted_chat_setup(db, &self.control, &self.session_id).await?;
            clear_chat_running_if_current(&self.chat_cancels, &self.session_id, &self.control)
                .await;
        }
        self.armed = false;
        Ok(())
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
        let control = self.control.clone();
        let durable_db = self.durable_db.clone();
        #[cfg(not(test))]
        let event_app = self.event_app.clone();
        tokio::spawn(async move {
            if control.durable {
                if let Some(db) = durable_db {
                    let mut retry_attempt = 0_u32;
                    loop {
                        #[cfg(not(test))]
                        let convergence = converge_and_project_aborted_chat_setup(
                            &db,
                            &chat_cancels,
                            &control,
                            &session_id,
                            event_app.as_ref(),
                        )
                        .await;
                        #[cfg(test)]
                        let convergence = async {
                            converge_aborted_chat_setup(&db, &control, &session_id).await?;
                            clear_chat_running_if_current(&chat_cancels, &session_id, &control)
                                .await;
                            Ok::<(), AppError>(())
                        }
                        .await;
                        match convergence {
                            Ok(()) => break,
                            Err(error) => {
                                retry_attempt = retry_attempt.saturating_add(1);
                                tracing::error!(
                                    run_instance_id = %control.run_instance_id,
                                    retry_attempt,
                                    %error,
                                    "failed to converge admitted chat setup; retrying without user input"
                                );
                                tokio::time::sleep(std::time::Duration::from_millis(
                                    100 * i64::from(retry_attempt.min(50)) as u64,
                                ))
                                .await;
                            }
                        }
                    }
                    return;
                }
            }
            clear_chat_running_if_current(&chat_cancels, &session_id, &control).await;
        });
    }
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    session_id: String,
    content: String,
    root_turn_id: Option<String>,
    state: State<'_, AppState>,
    mcp: State<'_, Arc<McpManager>>,
) -> Result<(), AppError> {
    // Register cancellation before any database, model, or credential work so
    // a stop click cannot race the command's setup phase.
    let run_control = Arc::new(crate::ChatRunControl::pending());
    let cancel_flag = run_control.cancel.clone();
    admit_chat_run(
        &state.chat_cancels,
        &state.update_restart_reserved,
        &session_id,
        run_control.clone(),
    )
    .await?;
    let mut running_setup_guard = ChatRunningSetupGuard::new(
        state.chat_cancels.clone(),
        session_id.clone(),
        run_control.clone(),
    );
    let db = state.db.read().await.clone();
    running_setup_guard.attach_durable_db(db.clone());
    let admission = admit_persisted_chat_turn(
        &db,
        &run_control,
        &session_id,
        root_turn_id.as_deref(),
        &content,
    )
    .await?;
    let root_turn_id = admission.root_turn_id;
    let objective = admission.objective;
    let contract = admission.contract;
    // From this point every early return owns a committed user/root/Objective
    // receipt and therefore must end with one durable recovery/cancel result
    // plus one TurnSettled projection.
    #[cfg(not(test))]
    running_setup_guard.attach_event_app(app.clone());
    macro_rules! setup_or_recover {
        ($expression:expr) => {
            match $expression {
                Ok(value) => value,
                Err(error) => {
                    let setup_error: AppError = error.into();
                    tracing::warn!(
                        run_instance_id = %run_control.run_instance_id,
                        objective_id = %objective.id,
                        error = %setup_error,
                        "chat setup failed after atomic admission; transferring to system recovery"
                    );
                    if let Err(convergence_error) = running_setup_guard.settle_now().await {
                        // The still-armed Drop path keeps retrying the same
                        // durable identity after this command returns.
                        tracing::error!(
                            run_instance_id = %run_control.run_instance_id,
                            error = %convergence_error,
                            "chat setup recovery persistence deferred to guard retry"
                        );
                    }
                    return Ok(());
                }
            }
        };
    }
    if let Some(driver) = admission.continuation_driver.as_deref() {
        tracing::info!(
            objective_id = %objective.id,
            root_turn_id = %root_turn_id,
            continuation_driver = driver,
            "chat turn admitted as an exact Objective continuation"
        );
    }
    if admission.cancel_requested || admission.already_settled {
        let settled = if admission.cancel_requested {
            let cancelled = setup_or_recover!(
                consume_requested_chat_cancel(&db, &run_control.run_instance_id).await
            );
            setup_or_recover!(cancelled.ok_or_else(|| {
                AppError::Other("durable chat cancel disappeared after atomic admission".into())
            }))
        } else {
            setup_or_recover!(settle_chat_run_control(&db, &run_control.run_instance_id).await);
            objective.clone()
        };
        run_control.cancel.store(true, Ordering::SeqCst);
        setup_or_recover!(
            project_chat_objective(
                &db,
                &app,
                &format!("stream:{session_id}"),
                &root_turn_id,
                &settled,
            )
            .await
        );
        clear_chat_running_if_current(&state.chat_cancels, &session_id, &run_control).await;
        emit_chat_turn_settled(
            &app,
            &format!("stream:{session_id}"),
            &run_control,
            Some(&root_turn_id),
            Some(&settled.id),
            chat_settlement_status(&settled),
        );
        running_setup_guard.disarm();
        return Ok(());
    }

    let settings = state.settings.read().await.clone();

    // Fetch session for cwd + model
    let session = {
        let pool = state.db.read().await;
        sqlx::query_as::<_, crate::storage::Session>("SELECT * FROM sessions WHERE id = ?")
            .bind(&session_id)
            .fetch_one(&*pool)
            .await
    };
    let session = setup_or_recover!(session);
    // A placeholder can survive a crash between the first reply and its
    // background title request. Re-read the first real user message on every
    // later turn while the source is still placeholder so the next successful
    // turn repairs it. The title service has an in-process single-flight guard
    // and a database CAS, so recovery cannot overwrite manual titles.
    let title_user_message = if session.title_source == TITLE_SOURCE_PLACEHOLDER {
        let pool = state.db.read().await;
        let result = sqlx::query_scalar::<_, String>(
            "SELECT content FROM messages
             WHERE session_id = ? AND role = 'user'
               AND (completion_state IS NULL OR completion_state = '')
             ORDER BY created_at ASC, rowid ASC LIMIT 1",
        )
        .bind(&session_id)
        .fetch_optional(&*pool)
        .await;
        setup_or_recover!(result)
    } else {
        None
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

    // Freeze all locally usable routes for this turn. This is a runtime
    // availability plan only: it never overwrites the user's preferred
    // endpoint/model in Settings.
    let turn_settings = setup_or_recover!(settings_for_session_route(
        &settings,
        session.endpoint_id.as_deref(),
        &session.model_id,
        &session.model_policy,
    ));
    // Fetch history as the agent should see it — excludes gate-rejected drafts.
    // It is also the capability source for the frozen turn plan.
    let history = {
        let pool = state.db.read().await;
        setup_or_recover!(crate::storage::load_agent_history(&pool, &session_id).await)
    };
    let requires_vision = history.iter().any(|message| {
        !crate::agent::attachments::extract_openai_parts(&message.content).is_empty()
    });
    let (route_plan, excluded_routes) = setup_or_recover!(
        resolve_route_plan(
            &turn_settings,
            &session.model_id,
            &session.model_policy,
            requires_vision,
        )
        .await
    );
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
        setup_or_recover!(
            sqlx::query("UPDATE sessions SET model_id = ?, updated_at = ? WHERE id = ?")
                .bind(&resolved_model)
                .bind(now)
                .bind(&session_id)
                .execute(&*pool)
                .await
        );
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

    // The execution contract and durable delivery grant were frozen in the
    // admission transaction. Recomputing either here would split the control
    // receipt from the provider run and could authorize a different revision.
    tracing::info!(
        "send_message: dispatch mode = {:?}, capability = {:?}",
        contract.mode,
        contract.capability
    );

    let db = state.db.read().await.clone();
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
    let objective_revision_for_settlement = objective.revision;
    let chat_cancels = state.chat_cancels.clone();
    let tracked_run_control = run_control.clone();
    let tracked_cancel_flag = cancel_flag.clone();
    let interjections = state.interjections.clone();
    let interjections_cleanup = state.interjections.clone();
    let completion_title_job =
        title_user_message.map(|message| (primary_route, is_low_information(&message), message));
    let fallback_title_job = completion_title_job.clone();
    let title_cancel_flag = tracked_cancel_flag.clone();
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
        let mut deferred_title_job = None;
        if loop_result.is_ok() && !title_cancel_flag.load(Ordering::SeqCst) {
            if let Some((title_route, needs_summary, title_user_message)) = completion_title_job {
                let assistant_summary = if needs_summary {
                    match sqlx::query_scalar::<_, String>(
                        "SELECT content FROM messages
                         WHERE session_id = ? AND role = 'assistant'
                           AND (completion_state IS NULL OR completion_state = '')
                           AND TRIM(content) <> ''
                         ORDER BY created_at DESC, rowid DESC LIMIT 1",
                    )
                    .bind(&session_for_error)
                    .fetch_optional(&db_for_error)
                    .await
                    {
                        Ok(summary) => summary,
                        Err(error) => {
                            tracing::warn!(
                                "session title assistant summary lookup failed: {error}"
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                if !needs_summary || assistant_summary.is_some() {
                    deferred_title_job = Some((title_route, title_user_message, assistant_summary));
                } else {
                    tracing::debug!(
                        "session title generation deferred: no_visible_assistant_summary"
                    );
                }
            }
        }
        if loop_result.is_err() && !title_cancel_flag.load(Ordering::SeqCst) {
            if let Some((title_route, needs_summary, title_user_message)) = fallback_title_job {
                if !needs_summary {
                    apply_local_title_fallback(
                        &app_clone,
                        &db_for_error,
                        &session_for_error,
                        &title_route,
                        &title_user_message,
                        "primary_turn_failed",
                    )
                    .await;
                }
            }
        }
        let mut settled_objective = match loop_result {
            Ok(outcome) => {
                match settle_chat_objective_from_outcome(
                    &db_for_error,
                    &app_clone,
                    &event_name,
                    &objective_for_settlement,
                    objective_revision_for_settlement,
                    &root_turn_for_error,
                    &outcome,
                    None,
                )
                .await
                {
                    Ok(settled) => Some(settled),
                    Err(error) => {
                        tracing::error!("failed to settle chat objective: {error}");
                        match recover_chat_settlement_failure(
                            &db_for_error,
                            &app_clone,
                            &event_name,
                            &objective_for_settlement,
                            objective_revision_for_settlement,
                            &root_turn_for_error,
                            &error.to_string(),
                        )
                        .await
                        {
                            Ok(settled) => Some(settled),
                            Err(recovery_error) => {
                                tracing::error!(
                                    "failed to recover chat settlement: {recovery_error}"
                                );
                                None
                            }
                        }
                    }
                }
            }
            Err(error_text) => {
                tracing::error!("Agent loop error: {error_text}");
                let auth_expired = is_chatgpt_auth_expired(&endpoint_for_error, &error_text);
                let settled = match settle_chat_objective_from_error(
                    &db_for_error,
                    &app_clone,
                    &event_name,
                    &objective_for_settlement,
                    objective_revision_for_settlement,
                    &root_turn_for_error,
                    auth_expired,
                    &error_text,
                    None,
                )
                .await
                {
                    Ok(settled) => Some(settled),
                    Err(error) => {
                        tracing::error!("failed to persist chat objective recovery: {error}");
                        match recover_chat_settlement_failure(
                            &db_for_error,
                            &app_clone,
                            &event_name,
                            &objective_for_settlement,
                            objective_revision_for_settlement,
                            &root_turn_for_error,
                            &error.to_string(),
                        )
                        .await
                        {
                            Ok(settled) => Some(settled),
                            Err(recovery_error) => {
                                tracing::error!(
                                    "failed to recover chat settlement: {recovery_error}"
                                );
                                None
                            }
                        }
                    }
                };
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
                settled
            }
        };
        match settle_chat_run_control(&db_for_error, &tracked_run_control.run_instance_id).await {
            Ok(cancelled @ Some(_)) => settled_objective = cancelled,
            Ok(None) => {}
            Err(error) => tracing::error!("failed to settle durable chat run control: {error}"),
        }
        clear_chat_running_if_current(&chat_cancels, &session_for_error, &tracked_run_control)
            .await;
        if let Some(settled) = settled_objective.as_ref() {
            emit_chat_turn_settled(
                &app_clone,
                &event_name,
                &tracked_run_control,
                Some(&root_turn_for_error),
                Some(&settled.id),
                chat_settlement_status(settled),
            );
        }
        if let Some((title_route, title_user_message, assistant_summary)) = deferred_title_job {
            // The frontend drains an already-queued next message with a
            // zero-delay timer. Give that primary turn time to register; if it
            // does, leave the placeholder for that turn to repair when idle.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if chat_cancels.lock().await.contains_key(&session_for_error) {
                tracing::debug!("session title generation deferred: next_turn_running");
            } else {
                spawn_title_generation(
                    app_clone.clone(),
                    db_for_error.clone(),
                    session_for_error.clone(),
                    title_route,
                    title_user_message,
                    assistant_summary,
                );
            }
        }
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
    mutation_permit: codefactory_agent_loop::tool::MutationPermit,
) -> Result<(), AppError> {
    resume_chat_objective_inner(app, objective, mutation_permit, None).await
}

pub(crate) async fn resume_context_objective(
    app: AppHandle,
    objective: crate::agent::objective::ObjectiveSnapshot,
    mutation_permit: codefactory_agent_loop::tool::MutationPermit,
    authorization: crate::agent::context_recovery::ContextRecoveryAuthorization,
) -> Result<(), AppError> {
    resume_chat_objective_inner(app, objective, mutation_permit, Some(authorization)).await
}

async fn resume_chat_objective_inner(
    app: AppHandle,
    objective: crate::agent::objective::ObjectiveSnapshot,
    mutation_permit: codefactory_agent_loop::tool::MutationPermit,
    context_authorization: Option<crate::agent::context_recovery::ContextRecoveryAuthorization>,
) -> Result<(), AppError> {
    use crate::agent::objective::{ObjectiveKind, ObjectiveStatus};

    if objective.status != ObjectiveStatus::WaitingSystem {
        return Ok(());
    }
    if mutation_permit.objective_id != objective.id {
        return Err(AppError::Other(
            "chat recovery mutation permit objective mismatch".into(),
        ));
    }
    let session_id = objective
        .session_id
        .clone()
        .ok_or_else(|| AppError::Other("chat objective has no session identity".into()))?;
    let root_turn_id = objective
        .resume_cursor
        .clone()
        .or_else(|| objective.root_turn_id.clone())
        .ok_or_else(|| AppError::Other("chat objective has no active resume cursor".into()))?;

    let state = app.state::<AppState>();
    let run_control = Arc::new(crate::ChatRunControl::pending());
    let cancel_flag = run_control.cancel.clone();
    admit_chat_run(
        &state.chat_cancels,
        &state.update_restart_reserved,
        &session_id,
        run_control.clone(),
    )
    .await?;
    let mut running_guard = ChatRunningSetupGuard::new(
        state.chat_cancels.clone(),
        session_id.clone(),
        run_control.clone(),
    );
    let db = state.db.read().await.clone();
    running_guard.attach_durable_db(db.clone());
    #[cfg(not(test))]
    running_guard.attach_event_app(app.clone());
    let current_binding: Option<String> = sqlx::query_scalar(
        "SELECT objective_id FROM chat_turn_state
         WHERE root_turn_id=? AND session_id=?",
    )
    .bind(&root_turn_id)
    .bind(&session_id)
    .fetch_optional(&db)
    .await?
    .flatten();
    if current_binding.as_deref() != Some(objective.id.as_str()) {
        return Err(AppError::Other(
            "chat objective resume cursor is not bound to the claimed Objective".into(),
        ));
    }
    register_chat_run_control(&db, &run_control, &session_id).await?;
    bind_chat_run_root(
        &db,
        &run_control.run_instance_id,
        &session_id,
        &root_turn_id,
    )
    .await?;
    if let Some(cancelled) = bind_chat_run_objective(
        &db,
        &run_control.run_instance_id,
        &session_id,
        &root_turn_id,
        &objective.id,
        objective.revision,
    )
    .await?
    {
        run_control.cancel.store(true, Ordering::SeqCst);
        project_chat_objective(
            &db,
            &app,
            &format!("stream:{session_id}"),
            &root_turn_id,
            &cancelled,
        )
        .await?;
        clear_chat_running_if_current(&state.chat_cancels, &session_id, &run_control).await;
        emit_chat_turn_settled(
            &app,
            &format!("stream:{session_id}"),
            &run_control,
            Some(&root_turn_id),
            Some(&cancelled.id),
            chat_settlement_status(&cancelled),
        );
        running_guard.disarm();
        return Ok(());
    }
    let settings_snapshot = state.settings.read().await.clone();
    let settings_state = state.settings.clone();
    let pending_permissions = state.pending_permissions.clone();
    let chat_cancels = state.chat_cancels.clone();
    let interjections = state.interjections.clone();
    let tracked_run_control = run_control.clone();
    drop(state);
    let mcp_manager = Arc::clone(&*app.state::<Arc<McpManager>>());

    let session = sqlx::query_as::<_, crate::storage::Session>("SELECT * FROM sessions WHERE id=?")
        .bind(&session_id)
        .fetch_one(&db)
        .await?;
    let original_content: String = sqlx::query_scalar(
        "SELECT content FROM messages WHERE id=? AND session_id=? AND role='user'",
    )
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
    let force_context_compression = context_authorization.and_then(|authorization| {
        crate::agent::claimed_context_compression_authorization(
            &objective,
            &mutation_permit,
            authorization,
        )
    });
    let app_for_run = app.clone();
    let db_for_run = db.clone();
    let session_for_run = session_id.clone();
    let permit_for_run = mutation_permit.clone();
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
            Some(crate::agent::AgentExecutionContext {
                parent_session_id: None,
                task_id: None,
                knowledge_library_ids: Vec::new(),
                usage_surface: crate::agent::UsageSurface::Interactive,
                mutation_permit: Some(permit_for_run),
                force_context_compression,
            }),
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

    let mut settled_objective = match loop_result {
        Ok(outcome) => Some(
            settle_chat_objective_from_outcome(
                &db,
                &app,
                &event_name,
                &objective.id,
                objective.revision,
                &root_turn_id,
                &outcome,
                Some(&mutation_permit),
            )
            .await?,
        ),
        Err(error_text) => {
            let auth_expired = is_chatgpt_auth_expired(&endpoint_for_error, &error_text);
            let settled = settle_chat_objective_from_error(
                &db,
                &app,
                &event_name,
                &objective.id,
                objective.revision,
                &root_turn_id,
                auth_expired,
                &error_text,
                Some(&mutation_permit),
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
            Some(settled)
        }
    };

    if let Some(cancelled) =
        settle_chat_run_control(&db, &tracked_run_control.run_instance_id).await?
    {
        settled_objective = Some(cancelled);
    }

    clear_chat_running_if_current(&chat_cancels, &session_id, &tracked_run_control).await;
    if let Some(settled) = settled_objective.as_ref() {
        emit_chat_turn_settled(
            &app,
            &event_name,
            &tracked_run_control,
            Some(&root_turn_id),
            Some(&settled.id),
            chat_settlement_status(settled),
        );
    }
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
    let run_control = Arc::new(crate::ChatRunControl::ephemeral());
    let cancel_flag = run_control.cancel.clone();
    admit_chat_run(
        &state.chat_cancels,
        &state.update_restart_reserved,
        &session_id,
        run_control.clone(),
    )
    .await?;
    let mut running_setup_guard = ChatRunningSetupGuard::new(
        state.chat_cancels.clone(),
        session_id.clone(),
        run_control.clone(),
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
    let tracked_run_control = run_control.clone();
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
        let transport_succeeded = loop_result.is_ok();
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
        clear_chat_running_if_current(&chat_cancels, &completed_session_id, &tracked_run_control)
            .await;
        let settlement_status = if tracked_run_control.cancel.load(Ordering::SeqCst) {
            "cancelled"
        } else if transport_succeeded {
            "completed"
        } else {
            "failed_setup"
        };
        emit_chat_turn_settled(
            &app_clone,
            &event_name,
            &tracked_run_control,
            None,
            None,
            settlement_status,
        );
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
    use sqlx::sqlite::SqlitePoolOptions;
    use std::time::Duration;

    fn projection_objective(
        status: crate::agent::objective::ObjectiveStatus,
        decision_type: crate::agent::objective::DecisionType,
        failure_code: Option<&str>,
    ) -> crate::agent::objective::ObjectiveSnapshot {
        use crate::agent::objective::{ObjectiveKind, ObjectiveSnapshot, RecoveryDomain};
        let mut objective = ObjectiveSnapshot::new(
            "objective-projection",
            ObjectiveKind::Informational,
            RecoveryDomain::Chat,
            "informational_answer",
        );
        objective.status = status;
        objective.decision_type = decision_type;
        objective.failure_code = failure_code.map(str::to_string);
        objective
    }

    /// A bounded-out recovery must settle the transport turn. Leaving it
    /// `recovering` with no terminal reason is exactly what showed the user an
    /// endless "系统仍在恢复" spinner while nothing was left to observe.
    #[test]
    fn exhausted_recovery_settles_the_turn_instead_of_spinning() {
        use crate::agent::objective::{
            DecisionType, ObjectiveStatus, TECHNICAL_RECOVERY_EXHAUSTED,
        };
        let projection = chat_turn_projection(&projection_objective(
            ObjectiveStatus::WaitingSystem,
            DecisionType::FailedInternal,
            Some(TECHNICAL_RECOVERY_EXHAUSTED),
        ));
        assert_eq!(projection.phase, "waiting");
        assert_eq!(projection.activity_kind, TECHNICAL_RECOVERY_EXHAUSTED);
        assert_eq!(
            projection.terminal_reason,
            Some(TECHNICAL_RECOVERY_EXHAUSTED),
            "the settled turn must name why the system stopped"
        );
        assert!(
            projection.activity_label.contains("无需补充输入"),
            "the user must be told the incident remains system-owned"
        );
    }

    #[tokio::test]
    async fn committed_terminal_projection_rejects_an_advanced_objective() {
        use crate::agent::objective::{
            DecisionType, ObjectiveStatus, TECHNICAL_RECOVERY_EXHAUSTED,
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE objectives (
               id TEXT PRIMARY KEY, revision INTEGER NOT NULL, status TEXT NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, objective_id TEXT,
               terminal_revision INTEGER
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let objective = projection_objective(
            ObjectiveStatus::WaitingSystem,
            DecisionType::FailedInternal,
            Some(TECHNICAL_RECOVERY_EXHAUSTED),
        );
        sqlx::query("INSERT INTO objectives(id, revision, status) VALUES (?, ?, ?)")
            .bind(&objective.id)
            .bind(objective.revision)
            .bind(objective.status.as_str())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, objective_id, terminal_revision)
             VALUES ('root-projection', ?, ?)",
        )
        .bind(&objective.id)
        .bind(objective.revision)
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            committed_chat_projection_is_current(&pool, "root-projection", &objective,)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE objectives SET revision=revision+1, status='active' WHERE id=?")
            .bind(&objective.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !committed_chat_projection_is_current(&pool, "root-projection", &objective,)
                .await
                .unwrap()
        );
    }

    /// Recovery that still has budget stays system-owned and unsettled: the
    /// ceiling must not turn ordinary retries into handbacks.
    #[test]
    fn recovery_with_budget_left_stays_system_owned() {
        use crate::agent::objective::{DecisionType, ObjectiveStatus};
        let projection = chat_turn_projection(&projection_objective(
            ObjectiveStatus::WaitingSystem,
            DecisionType::Waiting,
            Some("completion_evidence_incomplete"),
        ));
        assert_eq!(projection.phase, "recovering");
        assert_eq!(projection.terminal_reason, None);
    }

    /// An ordinary core-input handoff is a different thing and keeps its own
    /// live presentation.
    #[test]
    fn ordinary_core_input_is_not_reported_as_exhausted_recovery() {
        use crate::agent::objective::{DecisionType, ObjectiveStatus};
        let projection = chat_turn_projection(&projection_objective(
            ObjectiveStatus::WaitingCoreInput,
            DecisionType::CoreInputRequired,
            Some("browser_pairing_required"),
        ));
        assert_eq!(projection.activity_kind, "core_input_required");
        assert_eq!(projection.terminal_reason, None);
    }

    async fn reprompt_test_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE sessions (
               id TEXT PRIMARY KEY
             )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "CREATE TABLE chat_task_segments (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                goal_root_turn_id TEXT NOT NULL,
                previous_segment_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
                root_turn_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                task_segment_id TEXT,
                user_reprompt_driver TEXT,
                objective_id TEXT,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE objectives (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE delivery_runs (
                id TEXT PRIMARY KEY,
                objective_id TEXT,
                status TEXT NOT NULL,
                wait_class TEXT,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_reprompt_segment(
        pool: &sqlx::SqlitePool,
        id: &str,
        session_id: &str,
        ordinal: i64,
        root_turn_id: &str,
        previous_segment_id: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO chat_task_segments
             (id, session_id, ordinal, goal_root_turn_id, previous_segment_id)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(session_id)
        .bind(ordinal)
        .bind(root_turn_id)
        .bind(previous_segment_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_reprompt_turn(
        pool: &sqlx::SqlitePool,
        session_id: &str,
        root_turn_id: &str,
        segment_id: &str,
        objective_id: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, task_segment_id, user_reprompt_driver, objective_id, updated_at)
             VALUES (?, ?, ?, NULL, ?, 1)",
        )
        .bind(root_turn_id)
        .bind(session_id)
        .bind(segment_id)
        .bind(objective_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_open_objective(
        pool: &sqlx::SqlitePool,
        id: &str,
        objective_id: &str,
        status: &str,
        wait_class: Option<&str>,
        updated_at: i64,
    ) {
        sqlx::query("INSERT INTO objectives(id, status) VALUES (?, ?)")
            .bind(objective_id)
            .bind(
                if matches!(
                    status,
                    "waiting" | "platform_incident" | "agent_action_required" | "failed_internal"
                ) {
                    "waiting_system"
                } else {
                    "active"
                },
            )
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO delivery_runs
             (id, objective_id, status, wait_class, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(objective_id)
        .bind(status)
        .bind(wait_class)
        .bind(updated_at)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn arbitrary_real_user_message_links_the_unique_open_objective() {
        let pool = reprompt_test_pool().await;
        insert_reprompt_segment(&pool, "objective-a", "session", 1, "turn-a", None).await;
        insert_reprompt_segment(
            &pool,
            "continuation-a",
            "session",
            2,
            "turn-a2",
            Some("objective-a"),
        )
        .await;
        insert_reprompt_segment(&pool, "current", "session", 3, "turn-current", None).await;
        let objective_id = "objective-opaque-a";
        insert_reprompt_turn(
            &pool,
            "session",
            "turn-a",
            "objective-a",
            Some(objective_id),
        )
        .await;
        insert_reprompt_turn(
            &pool,
            "session",
            "turn-a2",
            "continuation-a",
            Some(objective_id),
        )
        .await;
        insert_reprompt_turn(&pool, "session", "turn-current", "current", None).await;
        insert_open_objective(
            &pool,
            "run-a",
            objective_id,
            "waiting",
            Some("external_state_uncertain"),
            10,
        )
        .await;

        let resolution =
            resolve_open_chat_objective_continuation(&pool, "session", "turn-current", 3)
                .await
                .unwrap();
        assert_eq!(
            resolution,
            ChatContinuationResolution::Unique {
                objective_id: objective_id.into(),
                continuation_root_turn_id: "turn-a2".into(),
                previous_segment_id: "continuation-a".into(),
                driver: "recoverable_waiting_open".into(),
            }
        );
        assert!(!objective_id.starts_with("chat:"));
    }

    #[tokio::test]
    async fn user_message_without_an_open_objective_is_not_attributed() {
        let pool = reprompt_test_pool().await;
        insert_reprompt_segment(&pool, "current", "session", 1, "turn-current", None).await;
        insert_reprompt_turn(&pool, "session", "turn-current", "current", None).await;

        let resolution =
            resolve_open_chat_objective_continuation(&pool, "session", "turn-current", 1)
                .await
                .unwrap();
        assert_eq!(resolution, ChatContinuationResolution::None);
    }

    #[tokio::test]
    async fn multiple_open_objectives_fail_closed_as_a_system_owned_incident() {
        let pool = reprompt_test_pool().await;
        insert_reprompt_segment(&pool, "objective-a", "session", 1, "turn-a", None).await;
        insert_reprompt_segment(&pool, "objective-b", "session", 2, "turn-b", None).await;
        insert_reprompt_segment(&pool, "current", "session", 3, "turn-current", None).await;
        insert_reprompt_turn(
            &pool,
            "session",
            "turn-a",
            "objective-a",
            Some("objective-opaque-a"),
        )
        .await;
        insert_reprompt_turn(
            &pool,
            "session",
            "turn-b",
            "objective-b",
            Some("objective-opaque-b"),
        )
        .await;
        insert_reprompt_turn(&pool, "session", "turn-current", "current", None).await;
        insert_open_objective(
            &pool,
            "run-a",
            "objective-opaque-a",
            "waiting",
            Some("wait_retryable"),
            10,
        )
        .await;
        insert_open_objective(
            &pool,
            "run-b",
            "objective-opaque-b",
            "agent_action_required",
            Some("agent_action_required"),
            11,
        )
        .await;

        let resolution =
            resolve_open_chat_objective_continuation(&pool, "session", "turn-current", 3)
                .await
                .unwrap();
        assert_eq!(resolution, ChatContinuationResolution::Ambiguous);
    }

    #[tokio::test]
    async fn already_bound_current_root_is_idempotent_and_cannot_self_link() {
        let pool = reprompt_test_pool().await;
        insert_reprompt_segment(&pool, "prior", "session", 1, "turn-prior", None).await;
        insert_reprompt_segment(&pool, "current", "session", 2, "turn-current", None).await;
        sqlx::query("INSERT INTO objectives(id, status) VALUES ('objective-opaque', 'active')")
            .execute(&pool)
            .await
            .unwrap();
        insert_reprompt_turn(
            &pool,
            "session",
            "turn-prior",
            "prior",
            Some("objective-opaque"),
        )
        .await;
        insert_reprompt_turn(
            &pool,
            "session",
            "turn-current",
            "current",
            Some("objective-opaque"),
        )
        .await;

        let resolution =
            resolve_open_chat_objective_continuation(&pool, "session", "turn-current", 2)
                .await
                .unwrap();
        assert_eq!(
            resolution,
            ChatContinuationResolution::AlreadyBound {
                objective_id: "objective-opaque".into()
            }
        );
    }

    #[tokio::test]
    async fn completed_chat_only_clears_its_own_running_flag() {
        let flags: crate::ChatCancelMap =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let completed = Arc::new(crate::ChatRunControl::pending());
        let replacement = Arc::new(crate::ChatRunControl::pending());
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
        let failed_setup = Arc::new(crate::ChatRunControl::pending());
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

        let stale_setup = Arc::new(crate::ChatRunControl::pending());
        let replacement = Arc::new(crate::ChatRunControl::pending());
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

    #[tokio::test]
    async fn admitted_setup_failure_moves_the_exact_objective_to_system_recovery() {
        use crate::agent::objective::{
            CreateObjective, ObjectiveKind, ObjectiveStatus, ObjectiveStore, RecoveryDomain,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-setup-failure".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-setup-failure".into()),
                root_turn_id: Some("turn-setup-failure".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES ('turn-setup-failure', 'session-setup-failure', ?)",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();

        let flags: crate::ChatCancelMap =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let control = Arc::new(crate::ChatRunControl::pending());
        flags
            .lock()
            .await
            .insert("session-setup-failure".into(), control.clone());
        register_chat_run_control(&pool, &control, "session-setup-failure")
            .await
            .unwrap();
        bind_chat_run_root(
            &pool,
            &control.run_instance_id,
            "session-setup-failure",
            "turn-setup-failure",
        )
        .await
        .unwrap();
        bind_chat_run_objective(
            &pool,
            &control.run_instance_id,
            "session-setup-failure",
            "turn-setup-failure",
            &objective.id,
            objective.revision,
        )
        .await
        .unwrap();

        let mut guard = ChatRunningSetupGuard::new(
            flags.clone(),
            "session-setup-failure".into(),
            control.clone(),
        );
        guard.attach_durable_db(pool.clone());
        guard
            .settle_now()
            .await
            .expect("the explicit early-return seam must persist recovery");
        assert!(!flags.lock().await.contains_key("session-setup-failure"));

        let recovered = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(recovered.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(
            recovered.failure_code.as_deref(),
            Some("chat_setup_aborted")
        );
        assert_eq!(
            recovered.resume_cursor.as_deref(),
            Some("turn-setup-failure")
        );
        let open_remediations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=?
               AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_remediations, 1);
        let control_status: String =
            sqlx::query_scalar("SELECT status FROM chat_run_controls WHERE run_instance_id=?")
                .bind(&control.run_instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(control_status, "completed");

        let repeated = converge_aborted_chat_setup(&pool, &control, "session-setup-failure")
            .await
            .unwrap()
            .expect("the terminal control keeps its exact durable identity");
        assert_eq!(repeated.objective.id, objective.id);
        assert_eq!(repeated.objective.revision, recovered.revision);
        let still_one_remediation: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=?
               AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(still_one_remediation, 1);

        let newer = store
            .create(CreateObjective {
                id: "objective-newer-setup-owner".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-newer-setup-owner".into()),
                root_turn_id: Some("turn-newer-setup-owner".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES ('turn-newer-setup-owner', 'session-newer-setup-owner', ?)",
        )
        .bind(&newer.id)
        .execute(&pool)
        .await
        .unwrap();
        let stale_control = crate::ChatRunControl::pending();
        register_chat_run_control(&pool, &stale_control, "session-newer-setup-owner")
            .await
            .unwrap();
        bind_chat_run_root(
            &pool,
            &stale_control.run_instance_id,
            "session-newer-setup-owner",
            "turn-newer-setup-owner",
        )
        .await
        .unwrap();
        bind_chat_run_objective(
            &pool,
            &stale_control.run_instance_id,
            "session-newer-setup-owner",
            "turn-newer-setup-owner",
            &newer.id,
            newer.revision,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE objectives SET revision=revision+1 WHERE id=?")
            .bind(&newer.id)
            .execute(&pool)
            .await
            .unwrap();
        let projected =
            converge_aborted_chat_setup(&pool, &stale_control, "session-newer-setup-owner")
                .await
                .unwrap()
                .unwrap();
        assert_eq!(projected.objective.status, ObjectiveStatus::Active);
        assert_eq!(projected.objective.revision, newer.revision + 1);
        assert!(projected.objective.failure_code.is_none());
    }

    #[tokio::test]
    async fn aborted_continuation_setup_accepts_the_durable_resume_cursor() {
        use crate::agent::objective::{
            CreateObjective, ObjectiveKind, ObjectiveStatus, ObjectiveStore, RecoveryDomain,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-continuation-setup".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-continuation-setup".into()),
                root_turn_id: Some("turn-original-root".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "UPDATE objectives
             SET status='waiting_system', decision_type='waiting',
                 resume_cursor='turn-continuation-root', revision=revision+1
             WHERE id=?",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES ('turn-continuation-root', 'session-continuation-setup', ?)",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let continued = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(continued.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(
            continued.root_turn_id.as_deref(),
            Some("turn-original-root")
        );
        assert_eq!(
            continued.resume_cursor.as_deref(),
            Some("turn-continuation-root")
        );

        let control = crate::ChatRunControl::pending();
        register_chat_run_control(&pool, &control, "session-continuation-setup")
            .await
            .unwrap();
        bind_chat_run_root(
            &pool,
            &control.run_instance_id,
            "session-continuation-setup",
            "turn-continuation-root",
        )
        .await
        .unwrap();
        bind_chat_run_objective(
            &pool,
            &control.run_instance_id,
            "session-continuation-setup",
            "turn-continuation-root",
            &continued.id,
            continued.revision,
        )
        .await
        .unwrap();

        let settled = converge_aborted_chat_setup(&pool, &control, "session-continuation-setup")
            .await
            .expect("the durable continuation identity should converge")
            .expect("the continuation keeps its Objective receipt");
        assert_eq!(settled.root_turn_id, "turn-continuation-root");
        assert_eq!(settled.objective.id, objective.id);
        assert_eq!(settled.objective.status, ObjectiveStatus::WaitingSystem);
        let control_status: String =
            sqlx::query_scalar("SELECT status FROM chat_run_controls WHERE run_instance_id=?")
                .bind(&control.run_instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(control_status, "completed");
    }

    #[tokio::test]
    async fn admitted_setup_cancel_wins_over_technical_recovery() {
        use crate::agent::objective::{
            CreateObjective, ObjectiveKind, ObjectiveStatus, ObjectiveStore, RecoveryDomain,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-setup-cancel".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-setup-cancel".into()),
                root_turn_id: Some("turn-setup-cancel".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES ('turn-setup-cancel', 'session-setup-cancel', ?)",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let control = crate::ChatRunControl::pending();
        register_chat_run_control(&pool, &control, "session-setup-cancel")
            .await
            .unwrap();
        bind_chat_run_root(
            &pool,
            &control.run_instance_id,
            "session-setup-cancel",
            "turn-setup-cancel",
        )
        .await
        .unwrap();
        bind_chat_run_objective(
            &pool,
            &control.run_instance_id,
            "session-setup-cancel",
            "turn-setup-cancel",
            &objective.id,
            objective.revision,
        )
        .await
        .unwrap();

        request_chat_run_cancel(&pool, &control.run_instance_id, "session-setup-cancel")
            .await
            .unwrap();
        let settled = converge_aborted_chat_setup(&pool, &control, "session-setup-cancel")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.root_turn_id, "turn-setup-cancel");
        assert_eq!(settled.objective.status, ObjectiveStatus::Cancelled);
        assert_eq!(
            settled.objective.cancellation_provenance.as_deref(),
            Some("explicit_cancel")
        );
        let control_status: String =
            sqlx::query_scalar("SELECT status FROM chat_run_controls WHERE run_instance_id=?")
                .bind(&control.run_instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(control_status, "cancelled");
        let active_remediations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_remediations, 0);
    }

    async fn durable_cancel_test_objective(
        test_id: &str,
    ) -> (sqlx::SqlitePool, crate::agent::objective::ObjectiveSnapshot) {
        use crate::agent::objective::{
            CreateObjective, DecisionRouter, ObjectiveKind, ObjectiveStore, RecoveryDomain,
            RouteSignal,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        sqlx::query("INSERT INTO sessions(id) VALUES (?)")
            .bind(format!("session-{test_id}"))
            .execute(&pool)
            .await
            .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: format!("objective-{test_id}"),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(format!("session-{test_id}")),
                root_turn_id: Some(format!("turn-{test_id}")),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES (?, ?, ?)",
        )
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(objective.session_id.as_deref().unwrap())
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "panic".into(),
                failure_signature: format!("panic:{test_id}"),
                next_observation_at: chrono::Utc::now().timestamp_millis() - 1,
                resume_cursor: objective.root_turn_id.clone(),
            },
        )
        .unwrap();
        let waiting = store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        (pool, waiting)
    }

    /// A user pressing stop must reach a turn that no run in this process owns.
    /// The in-memory cancel map is empty after a restart and never holds
    /// system-owned recovery, so `cancel_chat` returned `Ok` without doing
    /// anything and the turn resumed on every launch. Selection therefore has
    /// to come from durable state, and it has to find *every* live Objective:
    /// the 2026-08-13 session grew a second Objective behind the one that had
    /// already been cancelled, which kept the turn unfinished and the user's
    /// queued message stuck behind it.
    #[tokio::test]
    async fn a_user_stop_reaches_every_live_objective_the_session_still_owns() {
        use crate::agent::objective::{
            CreateObjective, ObjectiveKind, ObjectiveStatus, ObjectiveStore, RecoveryDomain,
        };

        let (pool, waiting) = durable_cancel_test_objective("user-stop").await;
        let session_id = waiting.session_id.clone().unwrap();

        // A second Objective appears while the first is still being recovered.
        let second = ObjectiveStore::new(pool.clone())
            .create(CreateObjective {
                id: "objective-user-stop-2".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(session_id.clone()),
                root_turn_id: Some("turn-user-stop-2".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES (?, ?, ?)",
        )
        .bind("turn-user-stop-2")
        .bind(&session_id)
        .bind(&second.id)
        .execute(&pool)
        .await
        .unwrap();

        // A crash may persist the Objective before its chat_turn_state
        // projection. The session fence must still find this row by durable
        // ownership instead of relying on the presentation table.
        let unprojected = ObjectiveStore::new(pool.clone())
            .create(CreateObjective {
                id: "objective-user-stop-unprojected".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(session_id.clone()),
                root_turn_id: Some("turn-user-stop-unprojected".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();

        let live = live_chat_objectives(&pool, &session_id).await.unwrap();
        assert_eq!(
            live.len(),
            2,
            "a stop must reach every live objective, not only the newest: {live:?}"
        );

        let store = ObjectiveStore::new(pool.clone());
        store
            .request_chat_session_cancel(&session_id)
            .await
            .unwrap();
        let cancelled = store
            .consume_chat_session_cancel(&session_id)
            .await
            .unwrap();
        assert_eq!(cancelled.len(), 3);
        for (_, _, cancelled) in cancelled {
            assert_eq!(cancelled.status, ObjectiveStatus::Cancelled);
            assert_eq!(
                cancelled.cancellation_provenance.as_deref(),
                Some("explicit_cancel"),
                "only an explicit user stop may abandon system-owned work"
            );
        }

        assert!(
            live_chat_objectives(&pool, &session_id)
                .await
                .unwrap()
                .is_empty(),
            "a stopped session must leave the supervisor nothing to resume"
        );
        let intent_status: String =
            sqlx::query_scalar("SELECT status FROM chat_session_cancel_intents WHERE session_id=?")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(intent_status, "settled");
        assert_eq!(
            store.get(&unprojected.id).await.unwrap().unwrap().status,
            ObjectiveStatus::Cancelled,
            "an Objective must remain stoppable before turn projection exists"
        );
    }

    #[tokio::test]
    async fn crash_left_session_stop_fences_claim_and_is_consumed_on_restart() {
        use crate::agent::objective::{ObjectiveStatus, ObjectiveStore};

        let (pool, waiting) = durable_cancel_test_objective("session-fence-restart").await;
        let store = ObjectiveStore::new(pool.clone());
        let session_id = waiting.session_id.as_deref().unwrap();
        store.request_chat_session_cancel(session_id).await.unwrap();

        assert!(store
            .claim_due_remediations("stale-supervisor", 8, 60_000)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .consume_pending_chat_session_cancellations()
                .await
                .unwrap(),
            1
        );
        let stopped = store.get(&waiting.id).await.unwrap().unwrap();
        assert_eq!(stopped.status, ObjectiveStatus::Cancelled);
        assert_eq!(
            stopped.cancellation_provenance.as_deref(),
            Some("explicit_cancel")
        );
        assert!(store
            .claim_due_remediations("replacement-supervisor", 8, 60_000)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn durable_cancel_prevents_restart_claim_before_runtime_flag_is_polled() {
        use crate::agent::objective::{ObjectiveStatus, ObjectiveStore};

        let (pool, waiting) = durable_cancel_test_objective("durable-cancel").await;
        let cancelled = cancel_chat_objective_exact(
            &pool,
            waiting.session_id.as_deref().unwrap(),
            waiting.root_turn_id.as_deref().unwrap(),
            &waiting.id,
        )
        .await
        .unwrap();
        assert_eq!(cancelled.status, ObjectiveStatus::Cancelled);
        assert_eq!(
            cancelled.cancellation_provenance.as_deref(),
            Some("explicit_cancel")
        );

        let restart_claims = ObjectiveStore::new(pool.clone())
            .claim_due_remediations("replacement-process", 8, 60_000)
            .await
            .unwrap();
        assert!(restart_claims.is_empty());
        let active_remediations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(&waiting.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_remediations, 0);
    }

    #[tokio::test]
    async fn durable_cancel_refuses_session_root_objective_identity_mismatch() {
        use crate::agent::objective::{ObjectiveStatus, ObjectiveStore};

        let (pool, waiting) = durable_cancel_test_objective("cancel-identity").await;
        let error = cancel_chat_objective_exact(
            &pool,
            waiting.session_id.as_deref().unwrap(),
            "turn-from-another-run",
            &waiting.id,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("identity mismatch"), "{error}");
        let unchanged = ObjectiveStore::new(pool)
            .get(&waiting.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(unchanged.revision, waiting.revision);
    }

    #[tokio::test]
    async fn startup_consumes_crash_left_chat_cancel_before_recovery_claim() {
        use crate::agent::objective::{ObjectiveStatus, ObjectiveStore};

        let (pool, waiting) = durable_cancel_test_objective("startup-cancel").await;
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, status,
              created_process_instance, cancel_requested_at, created_at, updated_at)
             VALUES ('run-startup-cancel', ?, ?, 'cancel_requested',
                     'dead-process', ?, ?, ?)",
        )
        .bind(waiting.session_id.as_deref().unwrap())
        .bind(waiting.root_turn_id.as_deref().unwrap())
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let store = ObjectiveStore::new(pool.clone());
        assert_eq!(store.consume_pending_chat_cancellations().await.unwrap(), 1);
        assert_eq!(store.consume_pending_chat_cancellations().await.unwrap(), 0);
        let cancelled = store.get(&waiting.id).await.unwrap().unwrap();
        assert_eq!(cancelled.status, ObjectiveStatus::Cancelled);
        assert_eq!(
            cancelled.cancellation_provenance.as_deref(),
            Some("explicit_cancel")
        );
        assert!(store
            .claim_due_remediations("replacement-process", 8, 60_000)
            .await
            .unwrap()
            .is_empty());
        let control_status: String = sqlx::query_scalar(
            "SELECT status FROM chat_run_controls WHERE run_instance_id='run-startup-cancel'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(control_status, "cancelled");
    }

    #[tokio::test]
    async fn cancel_before_setup_registration_is_not_lost_or_reassigned() {
        use crate::agent::objective::{ObjectiveStatus, ObjectiveStore};

        let (pool, waiting) = durable_cancel_test_objective("early-cancel").await;
        let control = crate::ChatRunControl::pending();
        let session_id = waiting.session_id.as_deref().unwrap();
        let root_turn_id = waiting.root_turn_id.as_deref().unwrap();

        assert!(
            request_chat_run_cancel(&pool, &control.run_instance_id, session_id)
                .await
                .unwrap()
                .is_none()
        );
        register_chat_run_control(&pool, &control, session_id)
            .await
            .unwrap();
        bind_chat_run_root(&pool, &control.run_instance_id, session_id, root_turn_id)
            .await
            .unwrap();
        let cancelled = bind_chat_run_objective(
            &pool,
            &control.run_instance_id,
            session_id,
            root_turn_id,
            &waiting.id,
            waiting.revision,
        )
        .await
        .unwrap()
        .expect("the pre-registration cancel must settle before model dispatch");
        assert_eq!(cancelled.status, ObjectiveStatus::Cancelled);
        let control_status: String =
            sqlx::query_scalar("SELECT status FROM chat_run_controls WHERE run_instance_id=?")
                .bind(&control.run_instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(control_status, "cancelled");
        assert!(ObjectiveStore::new(pool)
            .claim_due_remediations("replacement-process", 8, 60_000)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn setup_guard_consumes_cancel_after_objective_creation_before_control_binding() {
        use crate::agent::objective::{ObjectiveStatus, ObjectiveStore};

        let (pool, waiting) = durable_cancel_test_objective("guard-cancel-window").await;
        let flags: crate::ChatCancelMap =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let control = Arc::new(crate::ChatRunControl::pending());
        let session_id = waiting.session_id.as_deref().unwrap();
        let root_turn_id = waiting.root_turn_id.as_deref().unwrap();
        flags
            .lock()
            .await
            .insert(session_id.to_string(), control.clone());
        register_chat_run_control(&pool, &control, session_id)
            .await
            .unwrap();
        bind_chat_run_root(&pool, &control.run_instance_id, session_id, root_turn_id)
            .await
            .unwrap();

        let identity = request_chat_run_cancel(&pool, &control.run_instance_id, session_id)
            .await
            .unwrap()
            .expect("chat_turn_state should recover the opaque Objective identity");
        assert_eq!(identity.objective_id, waiting.id);
        let persisted_objective_id: Option<String> = sqlx::query_scalar(
            "SELECT objective_id FROM chat_run_controls WHERE run_instance_id=?",
        )
        .bind(&control.run_instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(persisted_objective_id.is_none());

        {
            let mut guard =
                ChatRunningSetupGuard::new(flags.clone(), session_id.to_string(), control.clone());
            guard.attach_durable_db(pool.clone());
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let status: String = sqlx::query_scalar(
                    "SELECT status FROM chat_run_controls WHERE run_instance_id=?",
                )
                .bind(&control.run_instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
                if status == "cancelled" && !flags.lock().await.contains_key(session_id) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("setup guard must durably settle the crash-left cancel");

        let cancelled = ObjectiveStore::new(pool.clone())
            .get(&waiting.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.status, ObjectiveStatus::Cancelled);
        let bound_objective_id: String = sqlx::query_scalar(
            "SELECT objective_id FROM chat_run_controls WHERE run_instance_id=?",
        )
        .bind(&control.run_instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(bound_objective_id, waiting.id);
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
