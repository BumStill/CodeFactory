// SPDX-License-Identifier: Apache-2.0
//! Desktop permission gateway (keystone slice 4.6 sub-step 6).
//!
//! Folds the loop's `decide_permission` + `request_permission` behind the
//! [`PermissionGateway`] trait: it reads the live permission policy and, on
//! `Ask`, prompts the frontend and waits for a response (or a cancellation /
//! bounded timeout). Owns only `Arc` handles — the settings lock, the event sink,
//! the pending-permission map, and the cancel flag. It holds NO `AppHandle`
//! directly (the `AppHandle` stays inside the `dyn EventSink`), so — unlike the
//! tool/hook backends — it needs no `#[cfg(not(test))]` gating for #166.
//!
//! `decide_permission` stays a free fn in the parent module (it is bin-crate
//! bound — `PermissionPolicy`, `shell_policy` — and directly unit-tested); this
//! gateway calls it via `super::`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use codefactory_agent_loop::services::{
    PermissionDenial, PermissionDenialReason, PermissionGateway, PermissionOutcome,
};

use sqlx::SqlitePool;

use crate::config::settings::Settings;
use crate::openrouter::types::{StreamEvent, ToolCall};
use crate::PendingPermissionMap;

use super::events::EventSink;
use super::permission_intent::{
    PermissionIntentRequest, PermissionIntentStatus, PermissionIntentStore, PermissionPromptKey,
    PermissionScope,
};
#[cfg(test)]
use super::permission_intent::{
    PermissionIntentSnapshot, PermissionPromptResponse, PermissionRecoveryDisposition,
};
use super::{
    await_permission_response, decide_permission_for_call, permission_policy_for_mode,
    PermissionDecision, PermissionResponse,
};

#[cfg(not(test))]
const PERMISSION_WAIT: Duration = Duration::from_secs(60);
// Keep timeout-path unit tests fast while production retains the full prompt
// window above.
#[cfg(test)]
const PERMISSION_WAIT: Duration = Duration::from_millis(250);

/// CF-TPP-R4: how long an unanswered prompt stays answerable instead of being
/// voided. A user who stepped away for a while comes back to a request they can
/// still approve — the task continues rather than failing.
#[cfg(not(test))]
const PERMISSION_LATE_WAIT: Duration = Duration::from_secs(30 * 60);
#[cfg(test)]
const PERMISSION_LATE_WAIT: Duration = Duration::from_millis(250);

pub(crate) struct DesktopPermissionGateway {
    pub(super) settings: Arc<tokio::sync::RwLock<Settings>>,
    pub(super) db: SqlitePool,
    pub(super) session_id: String,
    pub(super) root_turn_id: Option<String>,
    pub(super) task_id: Option<String>,
    pub(super) mutation_permit: Option<codefactory_agent_loop::tool::MutationPermit>,
    pub(super) anonymous: bool,
    pub(super) events: Arc<dyn EventSink>,
    pub(super) pending_permissions: PendingPermissionMap,
    pub(super) cancel: Option<Arc<AtomicBool>>,
    pub(super) browser_read_granted: bool,
    /// Compatibility field removed with the response-map integration. It must
    /// never be read as authority: an allow applies to one exact action only.
    #[allow(dead_code)]
    pub(super) browser_act_granted: AtomicBool,
}

pub(super) async fn registered_git_roots(db: &SqlitePool, session: &str) -> Option<Vec<std::path::PathBuf>> {
    super::execution_workspace::ensure_schema(db).await.ok()?;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT objective_id, worktree_path FROM execution_workspaces WHERE session_id=? AND state IN ('active','delivering')",
    ).bind(session).fetch_all(db).await.ok()?;
    let mut roots = Vec::new();
    for (objective, path) in rows {
        let path = std::path::PathBuf::from(path);
        super::execution_workspace::verify_objective_workspace(db, &objective, &path).await.ok()?;
        roots.push(path);
    }
    (!roots.is_empty()).then_some(roots)
}

async fn resolve_session_permission_policy(
    db: &SqlitePool,
    session_id: &str,
) -> crate::config::settings::PermissionPolicy {
    let mode = sqlx::query_scalar::<_, Option<String>>(
        "SELECT permission_mode FROM sessions WHERE id = ?",
    )
    .bind(session_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .flatten()
    .unwrap_or_else(|| "standard".to_string());
    permission_policy_for_mode(&mode)
}

#[async_trait::async_trait]
impl PermissionGateway for DesktopPermissionGateway {
    async fn authorize(
        &self,
        tool_call: &ToolCall,
        args: &serde_json::Value,
        bash_command: Option<&str>,
    ) -> PermissionOutcome {
        // M130: never infer remote authority from model/user wording or the
        // loop's cached capability. Shell and structured delivery share this gate.
        let remote = tool_call.function.name == "deliver_changes"
            || bash_command.is_some_and(codefactory_agent_loop::policy::is_delivery_command);
        if let Some(outcome) = self
            .authorize_permission_recovery(tool_call, args, bash_command)
            .await
        {
            if remote && matches!(outcome, PermissionOutcome::Allow)
                && !super::fetch_session_delivery_authorized(&self.db, &self.session_id).await {
                return permission_recovery_rejected();
            }
            return outcome;
        }
        let policy = resolve_session_permission_policy(&self.db, &self.session_id).await;
        if policy.full_access && tool_call.function.name == "bash" {
            if let Some(number) = bash_command.and_then(super::trusted_git::own_pr_metadata_target) {
                let key = format!("bash({})", bash_command.unwrap_or_default());
                let denied = policy.deny.iter().any(|p| super::glob_match(p, "bash") || super::glob_match(p, &key));
                let owned = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM thin_delivery_refs r JOIN execution_workspaces w ON w.session_id=r.session_id AND w.branch_name=r.branch WHERE r.session_id=? AND r.pr_number=? AND w.state IN ('active','delivering')")
                    .bind(&self.session_id).bind(number).fetch_one(&self.db).await.unwrap_or(0) > 0;
                if !denied && owned && super::fetch_session_delivery_authorized(&self.db, &self.session_id).await {
                    return PermissionOutcome::Allow;
                }
                return self.request_permission(tool_call, args.clone(), bash_command).await;
            }
        }
        // M198: heredoc is executable interpreter input, not shell segments.
        // Require the same identity-verified registration used by local Git.
        if policy.full_access && tool_call.function.name == "bash"
            && bash_command.is_some_and(|c| c.contains("<<"))
        {
            let command = bash_command.unwrap_or_default();
            let key = format!("bash({command})");
            if policy.deny.iter().any(|p| super::glob_match(p, "bash") || super::glob_match(p, &key))
                || matches!(crate::tools::shell_policy::classify_command(command), crate::tools::shell_policy::ShellCommandPolicy::Deny { .. })
            { return permission_recovery_rejected(); }
            if let Some(root) = self.managed_workspace_root().await {
                if let Some(roots) = registered_git_roots(&self.db, &self.session_id).await {
                    if roots.iter().any(|r| r.canonicalize().ok() == root.canonicalize().ok())
                        && super::trusted_git::python_heredoc_proved(Some(&root), command)
                    { return PermissionOutcome::Allow; }
                }
            }
            return self.request_permission(tool_call, args.clone(), bash_command).await;
        }
        // M159: delivery authority is not authority to force-push, delete a
        // remote ref, or discard local work. Prove and snapshot first.
        if policy.full_access && tool_call.function.name == "bash"
            && bash_command.is_some_and(super::trusted_git::sensitive)
        {
            let key = format!("bash({})", bash_command.unwrap_or_default());
            if policy.deny.iter().any(|pattern| super::glob_match(pattern, "bash") || super::glob_match(pattern, &key))
                || matches!(crate::tools::shell_policy::classify_command(bash_command.unwrap_or_default()), crate::tools::shell_policy::ShellCommandPolicy::Deny { .. })
            {
                return permission_recovery_rejected();
            }
            if let (Some(root), Some(command)) = (self.managed_workspace_root().await, bash_command) {
                if let Some(roots) = registered_git_roots(&self.db, &self.session_id).await {
                    if super::trusted_git::backup_registered(&roots, &root, command).is_some() {
                        return PermissionOutcome::Allow;
                    }
                }
            }
            return self.request_permission(tool_call, args.clone(), bash_command).await;
        }
        if remote {
            // The session switch never overrides an explicit per-tool deny.
            if let PermissionDecision::Deny(reason) = decide_permission_for_call(
                &policy, &tool_call.function.name, args, bash_command,
                self.browser_read_granted, None,
            ) {
                return PermissionOutcome::Deny(PermissionDenial {
                    content: format!("此操作被会话权限策略禁止：{reason}"),
                    reason: PermissionDenialReason::PolicyDenied,
                    duration_ms: 0,
                });
            }
            // M180: explicit deny above wins; delivery authority cannot
            // bypass shell Ask for credentials or a direct remote merge.
            if bash_command.is_some_and(|command| matches!(
                crate::tools::shell_policy::classify_command(command),
                crate::tools::shell_policy::ShellCommandPolicy::Ask { .. }
            )) {
                return self.request_permission(tool_call, args.clone(), bash_command).await;
            }
            if super::fetch_session_delivery_authorized(&self.db, &self.session_id).await {
                return PermissionOutcome::Allow;
            }
            let waiting = super::thin_state::ThinState {
                state: super::thin_state::SessionState::WaitingUser,
                question: Some("允许向远端推送并创建 PR / MR 吗？".into()),
                ..super::thin_state::ThinState::running()
            };
            if super::thin_state::save(&self.db, &self.session_id, &waiting).await.is_err() {
                return permission_recovery_rejected();
            }
            let outcome = self.request_permission(tool_call, args.clone(), bash_command).await;
            if super::thin_state::save(&self.db, &self.session_id, &super::thin_state::ThinState::running()).await.is_err() {
                return permission_recovery_rejected();
            }
            return match outcome {
                PermissionOutcome::Allow if self.anonymous => {
                    let saved = sqlx::query("UPDATE sessions SET delivery_authorized=1 WHERE id=?")
                        .bind(&self.session_id).execute(&self.db).await;
                    if matches!(saved, Ok(ref result) if result.rows_affected() == 1) {
                        PermissionOutcome::Allow
                    } else {
                        permission_recovery_rejected()
                    }
                }
                PermissionOutcome::Allow => {
                    // Durable decision already stored the grant atomically.
                    // Never reopen it here after a concurrent user revocation.
                    if super::fetch_session_delivery_authorized(&self.db, &self.session_id).await {
                        PermissionOutcome::Allow
                    } else { permission_recovery_rejected() }
                }
                PermissionOutcome::Deny(mut denial) => {
                    if denial.reason == PermissionDenialReason::DeniedByUser {
                        denial.content = "用户不允许推送，请在本地完成并说明".into();
                    }
                    PermissionOutcome::Deny(denial)
                }
                other => other,
            };
        }
        // CF-TPP-R1 only ever widens the shell gate for `full_access` (trusted)
        // sessions, and only for actions the workspace-local proof accepts.
        // Anything else — including "we could not resolve the workspace root" —
        // keeps the existing confirmation.
        let workspace_root = if policy.full_access {
            self.managed_workspace_root().await
        } else {
            None
        };
        match decide_permission_for_call(
            &policy,
            &tool_call.function.name,
            args,
            bash_command,
            self.browser_read_granted,
            workspace_root.as_deref(),
        ) {
            PermissionDecision::Allow => PermissionOutcome::Allow,
            PermissionDecision::Ask => {
                self.request_permission(tool_call, args.clone(), bash_command)
                    .await
            }
            PermissionDecision::Deny(reason) => {
                tracing::warn!("Tool '{}' denied: {reason}", tool_call.function.name);
                PermissionOutcome::Deny(PermissionDenial {
                    content: format!(
                        "Tool call denied by policy: {reason}. Choose only an action that stays inside the current policy."
                    ),
                    reason: PermissionDenialReason::PolicyDenied,
                    duration_ms: 0,
                })
            }
        }
    }
}

impl DesktopPermissionGateway {
    pub(crate) fn projected_prompt_event(
        observation: &super::permission_intent::PermissionIntentObservation,
    ) -> StreamEvent {
        StreamEvent::PermissionRequest {
            intent_id: observation.projection.intent_id.clone(),
            tool_call_id: observation.projection.provider_tool_call_id.clone(),
            tool_name: observation.projection.tool_name.clone(),
            args: observation.projection.args.clone(),
            expires_at: observation.projection.expires_at,
        }
    }

    /// A Permission-domain recovery runner may cross only the exact action
    /// authorized by its durable receipt. A consumed/mismatched/stale receipt
    /// fails closed here and never creates a fresh prompt that could authorize
    /// a duplicate mutation.
    async fn authorize_permission_recovery(
        &self,
        tc: &ToolCall,
        args: &serde_json::Value,
        bash_command: Option<&str>,
    ) -> Option<PermissionOutcome> {
        let permit = self.mutation_permit.as_ref()?;
        let now = chrono::Utc::now().timestamp_millis();
        let is_permission_recovery: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM objectives objective
               JOIN objective_remediations remediation
                 ON remediation.id=objective.remediation_id
                AND remediation.objective_id=objective.id
               WHERE objective.id=? AND objective.status='waiting_system'
                 AND objective.domain='permission' AND objective.remediation_id=?
                 AND objective.lease_owner=? AND objective.lease_expires_at>?
                 AND remediation.status='claimed' AND remediation.lease_owner=?
                 AND remediation.attempt_index=? AND remediation.lease_expires_at>?
             )",
        )
        .bind(&permit.objective_id)
        .bind(&permit.remediation_id)
        .bind(&permit.owner)
        .bind(now)
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .fetch_one(&self.db)
        .await
        .unwrap_or(false);
        if !is_permission_recovery {
            return None;
        }
        let scope = match self.resolve_scope().await {
            Ok(scope) => scope,
            Err(error) => {
                tracing::warn!(%error, "Permission recovery scope is no longer authoritative");
                return Some(permission_recovery_rejected());
            }
        };
        let action_signature = super::permission_intent::permission_action_signature(
            &tc.function.name,
            args,
            bash_command,
        );
        match PermissionIntentStore::new(self.db.clone())
            .reserve_exact_recovery_allow(&scope, &action_signature, permit, now)
            .await
        {
            Ok(true) => Some(PermissionOutcome::Allow),
            Ok(false) => {
                let exact_receipt: Option<String> = sqlx::query_scalar(
                    "SELECT status FROM permission_action_receipts
                     WHERE objective_id=? AND remediation_id=?
                       AND binding_id=? AND resource_generation=?
                       AND action_signature=?",
                )
                .bind(&scope.objective_id)
                .bind(&permit.remediation_id)
                .bind(&scope.binding_id)
                .bind(scope.resource_generation)
                .bind(&action_signature)
                .fetch_optional(&self.db)
                .await
                .unwrap_or(None);
                // A different action has no receipt and falls through to the
                // live session policy; the already-authorized exact action can
                // never be prompted or executed twice.
                exact_receipt.map(|_| permission_recovery_rejected())
            }
            Err(error) => {
                tracing::warn!(%error, "Permission recovery receipt could not be reserved");
                Some(permission_recovery_rejected())
            }
        }
    }

    /// Register a pending permission, prompt the frontend, and wait for the
    /// user's response (or a cancellation / bounded timeout).
    async fn request_permission(
        &self,
        tc: &ToolCall,
        args: serde_json::Value,
        bash_command: Option<&str>,
    ) -> PermissionOutcome {
        let mut args = args;
        // Diagnostic metadata is server-owned and travels in the existing
        // durable prompt JSON; never trust a model-supplied `_permission`.
        let (rule, reason) = if bash_command.is_some_and(|c| c.contains("<<")) {
            ("trusted.heredoc.unproved", "无法证明解释器仅访问已登记工作区内的文件；可能包含越界路径、外部操作、未闭合输入或不支持的语法。".to_string())
        } else if let Some(command) = bash_command {
            match crate::tools::shell_policy::classify_command(command) {
                crate::tools::shell_policy::ShellCommandPolicy::Ask { reason, .. }
                | crate::tools::shell_policy::ShellCommandPolicy::Deny { reason } => ("shell.safety", format!("命令命中安全规则：{reason}")),
                _ if super::trusted_git::sensitive(command) => ("trusted.git.unproved", "无法证明 Git 操作仅影响本会话已登记工作区，或无法完成可恢复备份。".to_string()),
                _ if codefactory_agent_loop::policy::is_delivery_command(command) => ("delivery.authorization", "此命令会修改远端，需要本会话的交付授权。".to_string()),
                _ => ("session.policy.ask", "当前会话权限策略要求对此工具调用逐次确认。".to_string()),
            }
        } else if tc.function.name == "deliver_changes" {
            ("delivery.authorization", "向远端推送并创建 PR / MR 需要本会话的交付授权。".to_string())
        } else {
            ("session.policy.ask", "当前会话权限策略要求对此工具调用逐次确认。".to_string())
        };
        if let Some(object) = args.as_object_mut() {
            object.insert("_permission".into(), serde_json::json!({"rule":rule, "reason":reason}));
        } else {
            args = serde_json::json!({"arguments":args, "_permission":{"rule":rule, "reason":reason}});
        }
        let now = chrono::Utc::now().timestamp_millis();
        let expires_at = now + PERMISSION_WAIT.as_millis() as i64;
        let durable_intent = if self.anonymous {
            None
        } else {
            match self.admit_permission_intent(tc, &args, bash_command, expires_at, now).await {
                Ok(intent) => Some(intent),
                Err(error) => {
                    tracing::error!(%error, "permission identity reconciliation exhausted one retry");
                    return PermissionOutcome::Deny(PermissionDenial {
                        content: "权限请求暂时无法与当前任务对齐，尚未执行任何操作；系统自动重试/自愈，工作区和改动保留。".into(),
                        reason: PermissionDenialReason::ChannelClosed,
                        duration_ms: 0,
                    });
                }
            }
        };
        let intent_id = durable_intent
            .as_ref()
            .map(|intent| intent.intent_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        let registry_collision = {
            use std::collections::hash_map::Entry;
            let mut pending = self.pending_permissions.lock().await;
            match pending.entry(intent_id.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(sender);
                    false
                }
                Entry::Occupied(_) => true,
            }
        };
        if registry_collision {
            tracing::error!(intent_id, "permission intent sender collision");
            let intent_store = PermissionIntentStore::new(self.db.clone());
            let intent_key = durable_intent.as_ref().map(|intent| intent.prompt_key());
            persist_interruption(
                &intent_store,
                intent_key.as_ref(),
                PermissionIntentStatus::ChannelClosed,
            )
            .await;
            return PermissionOutcome::Deny(PermissionDenial {
                content: "Permission request ownership conflicted before display. No action was executed.".into(),
                reason: PermissionDenialReason::ChannelClosed,
                duration_ms: 0,
            });
        }

        let args_for_reemit = args.clone();
        self.events.emit(StreamEvent::PermissionRequest {
            intent_id: intent_id.clone(),
            tool_call_id: tc.id.clone(),
            tool_name: tc.function.name.clone(),
            args: args_for_reemit.clone(),
            expires_at,
        });
        {
            let settings = self.settings.read().await;
            crate::notify::send(
                &settings,
                crate::notify::NotifyEvent::PermissionWaiting,
                format!("工具 {} 正在等待你的批准", tc.function.name),
            );
        }

        let started = std::time::Instant::now();
        // CF-TPP-R4 / CF-TPP-R5: the prompt window closing is not a decision.
        // Keep the request owned, visible and answerable for a much longer
        // window instead of voiding it, and never record it as a user choice.
        let mut response =
            await_permission_response(&mut receiver, self.cancel.as_ref(), PERMISSION_WAIT).await;
        if matches!(response, PermissionResponse::TimedOut) {
            tracing::info!(
                intent_id,
                "permission prompt is still unanswered; keeping it waiting for the user"
            );
            // Keep the durable request answerable: the prompt clock closing is
            // not a decision, so the recorded deadline moves with the window.
            if let Some(key) = durable_intent.as_ref().map(|intent| intent.prompt_key()) {
                let late_expires_at = expires_at + PERMISSION_LATE_WAIT.as_millis() as i64;
                if let Err(error) = PermissionIntentStore::new(self.db.clone())
                    .extend_pending(&key, late_expires_at, chrono::Utc::now().timestamp_millis())
                    .await
                {
                    tracing::warn!(%error, "permission waiting window could not be extended");
                }
            }
            // Re-publish the exact same intent (same id, later expiry) so a
            // user who left the screen, or switched to another session, still
            // sees the request when they come back — CF-TPP-R6.
            self.events.emit(StreamEvent::PermissionRequest {
                intent_id: intent_id.clone(),
                tool_call_id: tc.id.clone(),
                tool_name: tc.function.name.clone(),
                args: args_for_reemit.clone(),
                expires_at: expires_at + PERMISSION_LATE_WAIT.as_millis() as i64,
            });
            let settings = self.settings.read().await;
            crate::notify::send(
                &settings,
                crate::notify::NotifyEvent::PermissionWaiting,
                format!(
                    "工具 {} 仍在等待你的批准；无人应答不会记成你拒绝，回到电脑后批准即可继续。",
                    tc.function.name
                ),
            );
            drop(settings);
            response =
                await_permission_response(&mut receiver, self.cancel.as_ref(), PERMISSION_LATE_WAIT)
                    .await;
        }
        let duration_ms = started.elapsed().as_millis() as u64;
        self.pending_permissions.lock().await.remove(&intent_id);
        let intent_store = PermissionIntentStore::new(self.db.clone());
        let intent_key = durable_intent.as_ref().map(|intent| intent.prompt_key());
        match response {
            PermissionResponse::Allow => {
                if let Some(key) = intent_key.as_ref() {
                    match intent_store
                        .consume_exact_allow(key, chrono::Utc::now().timestamp_millis())
                        .await
                    {
                        Ok(true) => PermissionOutcome::Allow,
                        Ok(false) | Err(_) => PermissionOutcome::Deny(PermissionDenial {
                            content: "The recorded permission no longer owns this exact Objective revision. No action was executed.".into(),
                            reason: PermissionDenialReason::ChannelClosed,
                            duration_ms,
                        }),
                    }
                } else {
                    PermissionOutcome::Allow
                }
            }
            PermissionResponse::DeniedByUser => PermissionOutcome::Deny(PermissionDenial {
                content: "Tool call denied by the user. The requested action was not executed; do not bypass this decision with another tool.".to_string(),
                reason: PermissionDenialReason::DeniedByUser,
                duration_ms,
            }),
            PermissionResponse::TimedOut => {
                persist_interruption(&intent_store, intent_key.as_ref(), PermissionIntentStatus::TimedOut).await;
                PermissionOutcome::Deny(PermissionDenial {
                content: "No one answered this permission request before it expired, so it was recorded as not answered — that is not a user denial. The requested action was not executed; do not substitute another source as equivalent evidence.".to_string(),
                reason: PermissionDenialReason::TimedOut,
                duration_ms,
            })
            }
            PermissionResponse::ChannelClosed => {
                persist_interruption(&intent_store, intent_key.as_ref(), PermissionIntentStatus::ChannelClosed).await;
                PermissionOutcome::Deny(PermissionDenial {
                content: "Permission request was interrupted because its response channel closed. No user decision was recorded and the requested action was not executed.".to_string(),
                reason: PermissionDenialReason::ChannelClosed,
                duration_ms,
            })
            }
            PermissionResponse::Cancelled => {
                persist_interruption(&intent_store, intent_key.as_ref(), PermissionIntentStatus::Cancelled).await;
                PermissionOutcome::Cancelled
            }
        }
    }

    /// The session's own managed workspace root, when one is recorded. `None`
    /// keeps every existing confirmation: an unknown root can never widen the
    /// automatic-allow rule.
    async fn managed_workspace_root(&self) -> Option<std::path::PathBuf> {
        let scope = self.resolve_scope().await.ok()?;
        let view = super::execution_workspace::latest_for_session(&self.db, &self.session_id)
            .await
            .ok()
            .flatten()?;
        if view.objective_id != scope.objective_id || !matches!(view.state.as_str(), "active" | "delivering") {
            return None;
        }
        let path = std::path::PathBuf::from(view.worktree_path);
        super::execution_workspace::verify_objective_workspace(&self.db, &scope.objective_id, &path).await.ok()?;
        path.is_dir().then_some(path)
    }

    /// Resolve again once after admission loses a revision race. This never
    /// carries an old allow forward: every attempt admits a fresh exact scope.
    async fn admit_permission_intent(
        &self, tc: &ToolCall, args: &serde_json::Value,
        bash_command: Option<&str>, expires_at: i64, now: i64,
    ) -> anyhow::Result<super::permission_intent::PermissionIntentSnapshot> {
        let store = PermissionIntentStore::new(self.db.clone());
        let mut failure = None;
        for attempt in 0..2 {
            let result = async {
                let scope = self.resolve_scope().await?;
                store.create_pending(&PermissionIntentRequest {
                    scope, session_id: self.session_id.clone(),
                    provider_tool_call_id: tc.id.clone(), tool_name: tc.function.name.clone(),
                    args: args.clone(), bash_command: bash_command.map(str::to_string),
                    expires_at,
                    created_process_instance: crate::agent::objective::current_process_instance(),
                }, now).await
            }.await;
            match result {
                Ok(intent) => return Ok(intent),
                Err(error) => {
                    tracing::warn!(attempt, %error, "permission admission will reconcile live scope");
                    failure = Some(error);
                }
            }
        }
        Err(failure.expect("two unsuccessful admission attempts"))
    }

    async fn resolve_scope(&self) -> anyhow::Result<PermissionScope> {
        use sqlx::Row;
        let resource = if let Some(task_id) = self.task_id.as_deref() {
            ("task_run", task_id)
        } else if let Some(root_turn_id) = self.root_turn_id.as_deref() {
            ("chat_root_turn", root_turn_id)
        } else {
            anyhow::bail!("permission request has no task or chat root identity");
        };
        let row = sqlx::query(
            "SELECT objective.id AS objective_id, objective.revision,
                    binding.id AS binding_id, binding.resource_generation
             FROM objective_bindings binding
             JOIN objectives objective ON objective.id=binding.objective_id
             WHERE binding.resource_kind=? AND binding.resource_id=?
               AND objective.status NOT IN ('completed','cancelled','failed','legacy_orphan')
             ORDER BY binding.resource_generation DESC LIMIT 2",
        )
        .bind(resource.0)
        .bind(resource.1)
        .fetch_all(&self.db)
        .await?;
        if row.is_empty() && resource.0 == "chat_root_turn" {
            // A synthetic wake message need not be the persisted root turn.
            // Recover only one active Objective in this session, never a task
            // or a different session, and read its live revision from SQLite.
            let candidates = sqlx::query(
                "SELECT DISTINCT objective.id AS objective_id, objective.revision
                 FROM objectives objective JOIN chat_turn_state turn
                   ON turn.objective_id=objective.id AND turn.session_id=objective.session_id
                 WHERE objective.session_id=? AND objective.status='active'",
            ).bind(&self.session_id).fetch_all(&self.db).await?;
            if candidates.len() == 1 {
                let objective_id: String = candidates[0].try_get("objective_id")?;
                let bindings = sqlx::query(
                    "SELECT binding.id AS binding_id,binding.resource_generation
                     FROM objective_bindings binding JOIN chat_turn_state turn
                       ON turn.root_turn_id=binding.resource_id
                     WHERE binding.objective_id=? AND turn.objective_id=?
                       AND turn.session_id=? AND binding.resource_kind='chat_root_turn'
                     ORDER BY binding.resource_generation DESC,turn.updated_at DESC LIMIT 1",
                ).bind(&objective_id).bind(&objective_id).bind(&self.session_id)
                    .fetch_optional(&self.db).await?;
                if let Some(binding) = bindings {
                    return Ok(PermissionScope {
                        objective_id,
                        objective_revision: candidates[0].try_get("revision")?,
                        binding_id: binding.try_get("binding_id")?,
                        resource_generation: binding.try_get("resource_generation")?,
                    });
                }
            }
        }
        if row.len() != 1 {
            anyhow::bail!("permission request requires one authoritative Objective binding");
        }
        let row = &row[0];
        Ok(PermissionScope {
            objective_id: row.try_get("objective_id")?,
            objective_revision: row.try_get("revision")?,
            binding_id: row.try_get("binding_id")?,
            resource_generation: row.try_get("resource_generation")?,
        })
    }
}

fn permission_recovery_rejected() -> PermissionOutcome {
    PermissionOutcome::Deny(PermissionDenial {
        content: "The resumed action did not match the exact durable permission receipt. No action was executed and no replacement authorization was created.".into(),
        reason: PermissionDenialReason::ChannelClosed,
        duration_ms: 0,
    })
}

async fn persist_interruption(
    store: &PermissionIntentStore,
    key: Option<&PermissionPromptKey>,
    status: PermissionIntentStatus,
) {
    let Some(key) = key else { return };
    if let Err(error) = store
        .record_interruption(key, status, chrono::Utc::now().timestamp_millis())
        .await
    {
        tracing::warn!(intent_id = %key.intent_id, %error, "permission interruption CAS lost to another terminal");
    }
}

#[cfg(test)]
mod tests {
    use crate::util::no_window::NoWindow;
    use std::collections::HashMap;

    use sqlx::sqlite::SqlitePoolOptions;
    use tokio::sync::oneshot;

    use super::*;
    use crate::agent::events::CollectingEventSink;
    use crate::agent::{decide_permission, PermissionDecision};
    use crate::openrouter::types::FunctionCall;

    fn browser_call(id: &str, action: &str) -> (ToolCall, serde_json::Value) {
        let args = serde_json::json!({"action": action});
        (
            ToolCall {
                id: id.to_string(),
                r#type: "function".to_string(),
                function: FunctionCall {
                    name: "browser_session".to_string(),
                    arguments: args.to_string(),
                },
            },
            args,
        )
    }

    #[tokio::test]
    async fn m179_rebinds_latest_active_session_revision_before_permission() {
        let mut gateway = durable_test_gateway().await;
        sqlx::raw_sql("ALTER TABLE objectives ADD COLUMN session_id TEXT; UPDATE objectives SET session_id='permission-test',revision=8; CREATE TABLE chat_turn_state(root_turn_id TEXT,session_id TEXT,objective_id TEXT,updated_at INTEGER); INSERT INTO chat_turn_state VALUES ('root-permission-test','permission-test','5eea633e-59f9-42bc-91f8-0a19a5c49711',1);")
            .execute(&gateway.db).await.unwrap();
        Arc::get_mut(&mut gateway).unwrap().root_turn_id = Some("synthetic-wake-message".into());
        let scope = gateway.resolve_scope().await.expect("same-session active Objective must rebind");
        assert_eq!(scope.objective_revision, 8);
        assert_eq!(scope.binding_id, "42cfb353-5f4e-4809-8338-9b49b9806894");
        let (call, args) = browser_call("m179-call", "click");
        let other = gateway.clone();
        let task = tokio::spawn(async move { other.authorize(&call, &args, None).await });
        let intent = wait_for_durable_intent(&gateway).await;
        assert_eq!(intent.scope.objective_revision, 8);
        PermissionIntentStore::new(gateway.db.clone()).record_user_response(&intent.prompt_key(), PermissionPromptResponse::Allow, chrono::Utc::now().timestamp_millis()).await.unwrap();
        take_pending_sender(&gateway).await.send(true).unwrap();
        assert_eq!(task.await.unwrap(), PermissionOutcome::Allow);
    }

    #[tokio::test]
    async fn m139_readonly_compositions_skip_approval() {
        let command = "git status --short; git log -1 --oneline; gh pr view --json number,state,title; ps -eo pid,etime,args | grep '[s]leep 60'";
        for mode in ["trusted", "standard"] {
            let gateway = test_gateway(mode).await;
            let args = serde_json::json!({"command": command});
            let call = ToolCall { id: "m139-read".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
            assert!(matches!(gateway.authorize(&call, &args, Some(command)).await, PermissionOutcome::Allow), "{mode}: readonly command must not prompt");
            assert!(gateway.pending_permissions.lock().await.is_empty());
        }
    }

    #[tokio::test]
    async fn m139_standard_write_waits_then_approval_continues() {
        let gateway = durable_test_gateway().await;
        let control = crate::ChatRunControl::pending();
        crate::register_session_executor(&gateway.session_id, &control);
        let command = "rm -rf generated";
        let args = serde_json::json!({"command": command});
        let call = ToolCall { id: "m139-write".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        let task_gateway = gateway.clone();
        let authorization = tokio::spawn(async move { task_gateway.authorize(&call, &args, Some(command)).await });
        let intent = wait_for_durable_intent(&gateway).await;
        let state = super::super::thin_derive::refresh(&gateway.db, &gateway.session_id).await.unwrap();
        assert_eq!(state.state, super::super::thin_state::SessionState::WaitingUser);
        assert_eq!(state.question.as_deref(), Some("需要你批准执行：rm -rf generated"));
        assert!(!authorization.is_finished());
        PermissionIntentStore::new(gateway.db.clone()).record_user_response(
            &intent.prompt_key(), PermissionPromptResponse::Allow, chrono::Utc::now().timestamp_millis(),
        ).await.unwrap();
        take_pending_sender(&gateway).await.send(true).unwrap();
        assert_eq!(authorization.await.unwrap(), PermissionOutcome::Allow);
        let state = super::super::thin_derive::refresh(&gateway.db, &gateway.session_id).await.unwrap();
        assert_eq!(state.state, super::super::thin_state::SessionState::Running);
        assert!(state.question.is_none());
        crate::unregister_session_executor(&gateway.session_id, &control);
    }

    async fn test_gateway(mode: &str) -> Arc<DesktopPermissionGateway> {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY, permission_mode TEXT NOT NULL)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions (id, permission_mode) VALUES ('permission-test', ?)")
            .bind(mode)
            .execute(&db)
            .await
            .unwrap();

        Arc::new(DesktopPermissionGateway {
            settings: Arc::new(tokio::sync::RwLock::new(Settings::default())),
            db,
            session_id: "permission-test".to_string(),
            root_turn_id: None,
            task_id: None,
            mutation_permit: None,
            anonymous: true,
            events: Arc::new(CollectingEventSink::new()),
            pending_permissions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            cancel: None,
            browser_read_granted: false,
            browser_act_granted: AtomicBool::new(false),
        })
    }

    async fn durable_test_gateway() -> Arc<DesktopPermissionGateway> {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&db)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE sessions (
               id TEXT PRIMARY KEY, permission_mode TEXT NOT NULL
             );
             CREATE TABLE objectives (
               id TEXT PRIMARY KEY, revision INTEGER NOT NULL, status TEXT NOT NULL
             );
             CREATE TABLE objective_bindings (
               id TEXT PRIMARY KEY, objective_id TEXT NOT NULL,
               resource_kind TEXT NOT NULL, resource_id TEXT NOT NULL,
               resource_generation INTEGER NOT NULL,
               FOREIGN KEY(objective_id) REFERENCES objectives(id) ON DELETE CASCADE
             );
             INSERT INTO sessions VALUES ('permission-test', 'standard');
             INSERT INTO objectives VALUES (
               '5eea633e-59f9-42bc-91f8-0a19a5c49711', 7, 'active'
             );
             INSERT INTO objective_bindings VALUES (
               '42cfb353-5f4e-4809-8338-9b49b9806894',
               '5eea633e-59f9-42bc-91f8-0a19a5c49711',
               'chat_root_turn', 'root-permission-test', 3
             );",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../migrations/0010_permission_intents.sql"))
            .execute(&db)
            .await
            .unwrap();

        Arc::new(DesktopPermissionGateway {
            settings: Arc::new(tokio::sync::RwLock::new(Settings::default())),
            db,
            session_id: "permission-test".to_string(),
            root_turn_id: Some("root-permission-test".into()),
            task_id: None,
            mutation_permit: None,
            anonymous: false,
            events: Arc::new(CollectingEventSink::new()),
            pending_permissions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            cancel: None,
            browser_read_granted: false,
            browser_act_granted: AtomicBool::new(false),
        })
    }

    fn m159_git(root: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git").current_dir(root).args(args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    #[tokio::test]
    async fn m159_gateway_session_identity_backup_and_remote_fence() {
        let gateway = durable_test_gateway().await;
        sqlx::query("UPDATE sessions SET permission_mode='trusted'").execute(&gateway.db).await.unwrap();
        sqlx::query("ALTER TABLE sessions ADD COLUMN delivery_authorized INTEGER NOT NULL DEFAULT 1").execute(&gateway.db).await.unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        m159_git(&root, &["init", "-q"]);
        m159_git(&root, &["config", "user.name", "Synthetic"]);
        m159_git(&root, &["config", "user.email", "synthetic@example.invalid"]);
        // Synthetic byte assertions must not inherit runner-global CRLF conversion.
        m159_git(&root, &["config", "core.autocrlf", "false"]);
        std::fs::write(root.join("tracked.txt"), "base").unwrap();
        m159_git(&root, &["add", "."]);
        m159_git(&root, &["commit", "-qm", "synthetic"]);
        let remote = temp.path().join("remote.git");
        m159_git(temp.path(), &["init", "--bare", "-q", remote.to_str().unwrap()]);
        m159_git(&root, &["remote", "add", "origin", remote.to_str().unwrap()]);
        super::super::execution_workspace::ensure_schema(&gateway.db).await.unwrap();
        // Register exact synthetic Git identity without invoking network allocation.
        let common = m159_git(&root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]);
        let gitdir = m159_git(&root, &["rev-parse", "--absolute-git-dir"]);
        let branch = m159_git(&root, &["branch", "--show-current"]);
        let head = m159_git(&root, &["rev-parse", "HEAD"]);
        // The identity implementation is exercised through the public verifier;
        // obtain its ordinary digest inputs exactly as workspace allocation does.
        let repo = super::super::delivery::resolve_repo(&root, None).unwrap();
        let identity = super::super::delivery::capture_delivery_identity(&repo).unwrap();
        let (repo_identity, worktree_identity) = (identity.repo_identity, identity.worktree_identity);
        sqlx::query("INSERT INTO execution_workspaces(id,objective_id,session_id,repo_identity,repo_root,git_common_dir,worktree_path,worktree_identity,branch_name,base_ref,base_sha,head_sha,state,created_at,updated_at) VALUES ('synthetic-workspace','5eea633e-59f9-42bc-91f8-0a19a5c49711','permission-test',?,?,?,?,?,?, 'HEAD',?,?,'active',1,1)")
            .bind(repo_identity).bind(root.to_string_lossy().as_ref()).bind(common).bind(root.to_string_lossy().as_ref()).bind(worktree_identity).bind(branch).bind(&head).bind(&head).execute(&gateway.db).await.unwrap();
        assert!(!gitdir.is_empty());
        // M198 R4: real interpreter execution in an identity-verified synthetic
        // trusted session; no model/network/production database is involved.
        let local = "cd \"$(pwd)\"; python3 - <<'PY'\np='tracked.txt'\ns=open(p).read()\ns=s.replace('base', 'm198 rewritten')\nopen(p,'w').write(s)\nPY";
        let local = if cfg!(windows) { local.replace("python3 -", "python -") } else { local.into() };
        let args = serde_json::json!({"command":local});
        let call = ToolCall { id: "m198-safe".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        assert_eq!(gateway.authorize(&call, &args, Some(&local)).await, PermissionOutcome::Allow);
        let shell = if cfg!(windows) {
            let exec = std::path::PathBuf::from(m159_git(&root, &["--exec-path"]));
            exec.ancestors().map(|p| p.join("bin/bash.exe")).find(|p| p.is_file()).expect("Git Bash")
        } else { "sh".into() };
        let output = std::process::Command::new(&shell).no_window().current_dir(&root).args(["-c", &local]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(std::fs::read_to_string(root.join("tracked.txt")).unwrap(), "m198 rewritten");
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM permission_intents").fetch_one(&gateway.db).await.unwrap(), 0);
        let outside = temp.path().join("outside.txt");
        std::fs::write(&outside, "unchanged").unwrap();
        let unsafe_command = "python3 - <<'PY'\nopen('../outside.txt','w').write('bad')\nPY";
        let args = serde_json::json!({"command":unsafe_command, "_permission":{"rule":"forged", "reason":"safe"}});
        let call = ToolCall { id: "m198-outside".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        assert!(matches!(gateway.authorize(&call, &args, Some(unsafe_command)).await, PermissionOutcome::Deny(_)));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "unchanged");
        let prompt: String = sqlx::query_scalar("SELECT prompt_args_json FROM permission_intents WHERE provider_tool_call_id='m198-outside'").fetch_one(&gateway.db).await.unwrap();
        let prompt: serde_json::Value = serde_json::from_str(&prompt).unwrap();
        assert_eq!(prompt["_permission"]["rule"], "trusted.heredoc.unproved");
        assert!(prompt["_permission"]["reason"].as_str().unwrap().contains("工作区"));
        assert_eq!(super::super::permission_intent::permission_action_signature("bash", &args, Some(unsafe_command)), super::super::permission_intent::permission_action_signature("bash", &prompt, Some(unsafe_command)));
        // Restore synthetic state for the existing independent Git checks.
        std::fs::write(root.join("tracked.txt"), "base").unwrap();
        let node = "node <<'JS'\nconst fs = require('fs');\nconst p = 'tracked.txt';\nlet s = fs.readFileSync(p, 'utf8');\ns = s.replace('base', 'node rewritten');\nfs.writeFileSync(p, s);\nJS";
        let args = serde_json::json!({"command":node});
        let call = ToolCall { id: "m198-node".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        assert_eq!(gateway.authorize(&call, &args, Some(node)).await, PermissionOutcome::Allow);
        // Reuse the resolved Git Bash: on Windows `bash` on PATH can be the WSL
        // launcher, which cannot see the runner's node/pnpm installation.
        let output = std::process::Command::new(&shell).no_window().current_dir(&root).args(["-c", node]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(std::fs::read_to_string(root.join("tracked.txt")).unwrap(), "node rewritten");
        std::fs::write(root.join("tracked.txt"), "base").unwrap();
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS thin_delivery_refs(session_id TEXT PRIMARY KEY,branch TEXT NOT NULL,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL); INSERT INTO thin_delivery_refs SELECT 'permission-test',branch_name,777,'through_merge' FROM execution_workspaces WHERE id='synthetic-workspace';").execute(&gateway.db).await.unwrap();
        for command in ["gh pr edit 777 --remove-label 'merge-queue: armed'", "gh pr edit 777 --add-label 'synthetic'", "gh pr comment 777 --body 'synthetic comment'"] {
            let args = serde_json::json!({"command":command});
            let call = ToolCall { id: "m198-owned-pr".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
            assert_eq!(gateway.authorize(&call, &args, Some(command)).await, PermissionOutcome::Allow);
        }
        // M198 family regressions: mixed discovery must not make a local
        // synchronization or the mandated local hook configuration prompt.
        for command in [
            "git fetch --prune origin main && git merge-base --is-ancestor origin/main HEAD; rg trusted src; cat docs/repo-governance-profile.md; ls src; git config core.hooksPath .githooks",
            "git fetch --prune origin main && git merge origin/main && git diff --check",
            "git merge --autostash origin/main && git status --short && git diff --check",
        ] {
            let args = serde_json::json!({"command":command});
            let call = ToolCall { id: "m198-family".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
            assert_eq!(gateway.authorize(&call, &args, Some(command)).await, PermissionOutcome::Allow, "{command}");
        }
        // M180: exercise the exact production gateway, not just the proof helper.
        for command in [
            "sed -n '175,244p' src-tauri/src/agent/usage_windows.rs; sed -n '223,257p' src-tauri/src/vision/mod.rs; sed -n '35,100p' src-tauri/src/agent/internal_text.rs; grep -n 'attach_usage_settings' src-tauri/src/agent/mod.rs src-tauri/src/agent/model_transport.rs; grep -n 'settings' src-tauri/src/agent/internal_text.rs | head -15; git worktree list; git log -1 --oneline; git log -1 --oneline origin/main",
            "git fetch --prune origin main && git stash push -u -m x && git merge origin/main && git stash pop",
            "git checkout origin/main -- tracked.txt && git diff --stat && git status --short && sed -n '1,10p' tracked.txt",
            "git add -A && git commit -m synthetic",
            "node -e 'console.log(require(\"./docs/testing/scenario-registry.json\"))' | head -20",
            "git fetch --prune origin main; find src -name '*.rs'; grep -R settings src | head -90; ls src; command -v pnpm; gh auth status",
            "node -e 'console.log(2 + 3)'",
        ] {
            let args = serde_json::json!({"command":command});
            let call = ToolCall { id: "m180-local".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
            let outcome = gateway.authorize(&call, &args, Some(command)).await;
            assert_eq!(outcome, PermissionOutcome::Allow, "M180 trusted command: {command}");
            assert!(gateway.pending_permissions.lock().await.is_empty());
        }
        // M182 R1/R5: exact redirect/pipeline incidents must traverse the
        // production gateway, create durable backups and leave no prompt.
        for command in [
            "git fetch --prune origin main && git merge origin/main --no-edit 2>&1 | tail -25; git status --short | head -20",
            "git reset -q --hard HEAD && git status --short && git merge --no-edit origin/main 2>&1 | tail -5",
            "git merge --no-edit origin/main > merge.log || git status --short",
        ] {
            let before = m159_git(&root, &["for-each-ref", "--format=%(refname)", "refs/codefactory/backup/"]);
            let args = serde_json::json!({"command": command});
            let call = ToolCall { id: format!("m182-{}", before.lines().count()), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
            let outcome = tokio::time::timeout(Duration::from_secs(10), gateway.authorize(&call, &args, Some(command))).await.expect("trusted incident must not wait for approval");
            assert_eq!(outcome, PermissionOutcome::Allow, "{command}");
            assert!(gateway.pending_permissions.lock().await.is_empty());
            let after = m159_git(&root, &["for-each-ref", "--format=%(refname)", "refs/codefactory/backup/"]);
            assert!(after.lines().count() > before.lines().count(), "backup must precede Allow: {command}");
        }
        // R6 equivalent synthetic execution: use only a local bare remote,
        // authorize each exact R5 command through the production gateway,
        // then actually execute it and assert both effects and no approvals.
        m159_git(&root, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "synthetic original\n").unwrap();
        m159_git(&root, &["add", "."]);
        m159_git(&root, &["commit", "-qm", "synthetic source"]);
        m159_git(&root, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
        let r5 = [
            "git fetch --prune origin main && git merge origin/main --no-edit 2>&1 | tail -25; git status --short | head -20",
            "git fetch --prune origin main && git stash push -u -m x && git merge origin/main && git stash pop",
            "git checkout origin/main -- src/main.rs && git diff --stat && git status --short && sed -n '1,20p' src/main.rs",
            "git add -A && git commit -m 'synthetic change'",
            "git reset -q --hard HEAD && git status --short && git merge --no-edit origin/main 2>&1 | tail -5",
        ];
        let shell = if cfg!(windows) {
            let exec_path = std::path::PathBuf::from(m159_git(&root, &["--exec-path"]));
            exec_path.ancestors().map(|p| p.join("bin/bash.exe")).find(|p| p.is_file()).expect("Git Bash")
        } else { std::path::PathBuf::from("sh") };
        for (index, command) in r5.iter().enumerate() {
            if index == 1 { std::fs::write(root.join("tracked.txt"), "synthetic dirty stash\n").unwrap(); }
            if index == 2 { std::fs::write(root.join("src/main.rs"), "synthetic to restore\n").unwrap(); }
            if index == 3 { std::fs::write(root.join("commit.txt"), "synthetic committed\n").unwrap(); }
            if index == 4 { std::fs::write(root.join("commit.txt"), "synthetic discarded\n").unwrap(); }
            let before = m159_git(&root, &["for-each-ref", "--format=%(refname)", "refs/codefactory/backup/"]);
            let args = serde_json::json!({"command":command});
            let call = ToolCall { id: format!("m182-r6-{index}"), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
            assert_eq!(tokio::time::timeout(Duration::from_secs(10), gateway.authorize(&call, &args, Some(command))).await.expect("R5 must not wait"), PermissionOutcome::Allow);
            let after = m159_git(&root, &["for-each-ref", "--format=%(refname)", "refs/codefactory/backup/"]);
            let reference = after.lines().find(|r| !before.lines().any(|old| old == *r)).expect("new snapshot before execution");
            let output = std::process::Command::new(&shell).no_window().current_dir(&root).args(["-c", command]).output().unwrap();
            assert!(output.status.success(), "R5[{index}] {command}: {}", String::from_utf8_lossy(&output.stderr));
            assert!(gateway.pending_permissions.lock().await.is_empty(), "R5[{index}]");
            match index {
                0 => assert!(String::from_utf8_lossy(&output.stdout).contains("Already up to date")),
                1 => assert_eq!(std::fs::read_to_string(root.join("tracked.txt")).unwrap(), "synthetic dirty stash\n"),
                2 => assert_eq!(std::fs::read_to_string(root.join("src/main.rs")).unwrap(), "synthetic original\n"),
                3 => assert_eq!(m159_git(&root, &["show", "HEAD:commit.txt"]), "synthetic committed"),
                4 => {
                    assert_eq!(std::fs::read_to_string(root.join("commit.txt")).unwrap(), "synthetic committed\n");
                    assert_eq!(m159_git(&root, &["show", &format!("{reference}:commit.txt")]), "synthetic discarded");
                }
                _ => unreachable!(),
            }
            println!("M182-R6 fixture command[{index}] executed; snapshot={reference}; approvals=[]");
        }
        let target = temp.path().join("owned-worktree");
        m159_git(&root, &["worktree", "add", "-qb", "synthetic-owned", target.to_str().unwrap()]);
        let target_repo = super::super::delivery::resolve_repo(&target, None).unwrap();
        let target_identity = super::super::delivery::capture_delivery_identity(&target_repo).unwrap();
        sqlx::query("INSERT INTO objectives(id,revision,status) VALUES ('synthetic-owned-objective',1,'active')").execute(&gateway.db).await.unwrap();
        sqlx::query("INSERT INTO execution_workspaces(id,objective_id,session_id,repo_identity,repo_root,git_common_dir,worktree_path,worktree_identity,branch_name,base_ref,base_sha,head_sha,state,created_at,updated_at) VALUES ('synthetic-owned-workspace','synthetic-owned-objective','permission-test',?,?,?,?,?,'synthetic-owned','HEAD',?,?,'active',1,1)")
            .bind(target_identity.repo_identity).bind(root.to_string_lossy().as_ref())
            .bind(m159_git(&root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]))
            .bind(target.to_string_lossy().as_ref()).bind(target_identity.worktree_identity)
            .bind(&head).bind(&head).execute(&gateway.db).await.unwrap();
        std::fs::write(target.join("tracked.txt"), "synthetic owned dirty data\n").unwrap();
        let command = format!("S=\"{}\"; A=\"{}\"; git -C \"$S\" worktree remove --force \"$A\"; git -C \"$S\" checkout {}", root.display(), target.display(), m159_git(&root, &["branch", "--show-current"]));
        let before = m159_git(&root, &["for-each-ref", "--format=%(refname)", "refs/codefactory/backup/"]);
        let args = serde_json::json!({"command":command});
        let call = ToolCall { id: "m182-r6-5".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        assert_eq!(tokio::time::timeout(Duration::from_secs(10), gateway.authorize(&call, &args, Some(&command))).await.expect("owned worktree must not prompt"), PermissionOutcome::Allow);
        let output = std::process::Command::new(&shell).no_window().current_dir(&root).args(["-c", &command]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(!target.exists());
        assert!(gateway.pending_permissions.lock().await.is_empty());
        let after = m159_git(&root, &["for-each-ref", "--format=%(refname)", "refs/codefactory/backup/"]);
        assert!(after.lines().filter(|r| !before.lines().any(|old| old == *r)).any(|reference| m159_git(&root, &["show", &format!("{reference}:tracked.txt")]) == "synthetic owned dirty data"));
        sqlx::query("UPDATE execution_workspaces SET state='closed' WHERE id='synthetic-owned-workspace'").execute(&gateway.db).await.unwrap();
        println!("M182-R6 fixture command[5] executed; owned worktree removed; dirty content recovered from snapshot; approvals=[]");
        std::fs::write(root.join("tracked.txt"), "uncommitted").unwrap();
        let command = "git reset --hard HEAD";
        let args = serde_json::json!({"command": command});
        let call = ToolCall { id: "m159-local".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        assert_eq!(gateway.authorize(&call, &args, Some(command)).await, PermissionOutcome::Allow);
        let references = m159_git(&root, &["for-each-ref", "--sort=refname", "--format=%(refname)", "refs/codefactory/backup/"]);
        let reference = references.lines().last().expect("snapshot exists");
        assert_eq!(m159_git(&root, &["show", &format!("{reference}:tracked.txt")]), "uncommitted");
        assert!(gateway.pending_permissions.lock().await.is_empty());
        let compound = format!("cd \"{}\"; git stash push -m prefill; git fetch --prune origin main; git merge origin/main --no-edit; git stash pop; git status --porcelain | head", root.display());
        let compound_args = serde_json::json!({"command": compound});
        let compound_call = ToolCall { id: "m166-compound".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: compound_args.to_string() } };
        assert_eq!(gateway.authorize(&compound_call, &compound_args, Some(&compound)).await, PermissionOutcome::Allow);
        assert!(gateway.pending_permissions.lock().await.is_empty());
        for command in ["gh auth token", "gh pr merge 7 --squash", "gh pr edit 778 --remove-label 'merge-queue: armed'", "gh pr comment 778 --body 'not ours'", "git config --global core.hooksPath /tmp/hooks", "git push --force origin main", "git push origin :main", "git checkout -- ../other", "git reset --hard HEAD && rm -rf ../other"] {
            let args = serde_json::json!({"command":command});
            let call = ToolCall { id: "m159-remote".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
            let other = gateway.clone();
            let command = command.to_string();
            let task = tokio::spawn(async move { other.authorize(&call, &args, Some(&command)).await });
            let intent = wait_for_durable_intent(&gateway).await;
            assert!(!task.is_finished());
            PermissionIntentStore::new(gateway.db.clone()).record_user_response(&intent.prompt_key(), PermissionPromptResponse::Deny, chrono::Utc::now().timestamp_millis()).await.unwrap();
            take_pending_sender(&gateway).await.send(false).unwrap();
            assert!(matches!(task.await.unwrap(), PermissionOutcome::Deny(_)));
        }
        // Another session's registered workspace never establishes ownership.
        sqlx::query("UPDATE execution_workspaces SET session_id='other-session'").execute(&gateway.db).await.unwrap();
        assert!(gateway.managed_workspace_root().await.is_none());
    }

    #[tokio::test]
    async fn m159_standard_reset_still_requests_approval() {
        let gateway = test_gateway("standard").await;
        let command = "git reset --hard HEAD";
        let args = serde_json::json!({"command":command});
        let call = ToolCall { id: "m159-standard".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        let other = gateway.clone();
        let task = tokio::spawn(async move { other.authorize(&call, &args, Some(command)).await });
        take_pending_sender(&gateway).await.send(false).unwrap();
        assert!(matches!(task.await.unwrap(), PermissionOutcome::Deny(_)));
    }

    async fn wait_for_durable_intent(
        gateway: &DesktopPermissionGateway,
    ) -> PermissionIntentSnapshot {
        // Poll with a real sleep rather than a yield_now spin: the admitting task
        // needs the executor and the DB to make progress, and a spin loop on a
        // saturated CI runner starves exactly the work it is waiting for. The
        // budget is generous because this is a liveness wait, not a latency
        // assertion.
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Some(intent_id) = gateway
                    .pending_permissions
                    .lock()
                    .await
                    .keys()
                    .next()
                    .cloned()
                {
                    let store = PermissionIntentStore::new(gateway.db.clone());
                    if let Some(intent) = store.get(&intent_id).await.unwrap() {
                        return intent;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("durable permission intent was admitted")
    }

    async fn take_pending_sender(gateway: &DesktopPermissionGateway) -> oneshot::Sender<bool> {
        // Same liveness budget and cooperative sleep as wait_for_durable_intent.
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let mut pending = gateway.pending_permissions.lock().await;
                if let Some(key) = pending.keys().next().cloned() {
                    let sender = pending.remove(&key).expect("pending key remains present");
                    return sender;
                }
                drop(pending);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("permission prompt was not emitted"))
    }

    async fn authorize_with_response(
        gateway: Arc<DesktopPermissionGateway>,
        id: &str,
        action: &str,
        allow: bool,
    ) -> PermissionOutcome {
        let (tool_call, args) = browser_call(id, action);
        let task_gateway = gateway.clone();
        let authorization =
            tokio::spawn(async move { task_gateway.authorize(&tool_call, &args, None).await });
        take_pending_sender(&gateway)
            .await
            .send(allow)
            .expect("permission receiver remains live");
        authorization.await.expect("authorization task")
    }

    fn assert_denied_for(outcome: PermissionOutcome, expected: PermissionDenialReason) {
        match outcome {
            PermissionOutcome::Deny(denial) => assert_eq!(denial.reason, expected),
            other => panic!("expected {expected:?} denial, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn m130_real_local_commit_then_approved_push_updates_synthetic_remote() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let remote = dir.path().join("remote.git");
        fn git(cwd: &std::path::Path, args: &[&str]) -> String {
            use crate::util::no_window::NoWindow;
            let output = std::process::Command::new("git").no_window().current_dir(cwd).args(args).output().unwrap();
            assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap()
        }
        git(dir.path(), &["init", "--bare", remote.to_str().unwrap()]);
        git(dir.path(), &["init", repo.to_str().unwrap()]);
        git(&repo, &["config", "user.email", "synthetic@example.invalid"]);
        git(&repo, &["config", "user.name", "Synthetic"]);
        git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
        let contract = super::super::resolve_turn_delivery_authorization(None, "实施", super::super::TurnIntent::default(), false).0;
        assert!(codefactory_agent_loop::policy::capability_denial(contract.capability, "bash", "git commit -m synthetic", &codefactory_agent_core::ToolKind::Mutation, &serde_json::json!({})).is_none());
        git(&repo, &["commit", "--allow-empty", "-m", "synthetic"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        assert!(git(&repo, &["ls-remote", "origin"]).is_empty());
        let gateway = test_gateway("trusted").await;
        sqlx::query("ALTER TABLE sessions ADD COLUMN delivery_authorized INTEGER NOT NULL DEFAULT 0").execute(&gateway.db).await.unwrap();
        let args = serde_json::json!({"command":"git push origin HEAD:refs/heads/synthetic"});
        let call = ToolCall { id: "synthetic-push-real".into(), r#type: "function".into(), function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        let cloned = gateway.clone();
        let handle = tokio::spawn(async move { cloned.authorize(&call, &args, Some("git push origin HEAD:refs/heads/synthetic")).await });
        let sender = take_pending_sender(&gateway).await;
        let waiting = super::super::thin_state::load(&gateway.db, "permission-test").await.unwrap().unwrap();
        assert_eq!(waiting.state, super::super::thin_state::SessionState::WaitingUser);
        assert!(waiting.question.unwrap().contains("允许"));
        assert!(git(&repo, &["ls-remote", "origin"]).is_empty(), "no remote mutation before approval");
        sender.send(true).unwrap();
        assert_eq!(handle.await.unwrap(), PermissionOutcome::Allow);
        git(&repo, &["push", "origin", "HEAD:refs/heads/synthetic"]);
        assert_eq!(git(&repo, &["ls-remote", "origin", "refs/heads/synthetic"]).split_whitespace().next(), Some(head.trim()));
    }

    #[tokio::test]
    async fn m130_remote_approval_persists_and_resumes_same_call() {
        let gateway = test_gateway("trusted").await;
        sqlx::query("ALTER TABLE sessions ADD COLUMN delivery_authorized INTEGER NOT NULL DEFAULT 0")
            .execute(&gateway.db).await.unwrap();
        let args = serde_json::json!({"command":"git push origin synthetic"});
        let call = ToolCall { id: "synthetic-push".into(), r#type: "function".into(),
            function: FunctionCall { name: "bash".into(), arguments: args.to_string() } };
        let cloned = gateway.clone();
        let handle = tokio::spawn(async move { cloned.authorize(&call, &args, Some("git push origin synthetic")).await });
        assert!(!super::super::fetch_session_delivery_authorized(&gateway.db, "permission-test").await);
        take_pending_sender(&gateway).await.send(true).unwrap();
        assert_eq!(handle.await.unwrap(), PermissionOutcome::Allow);
        assert!(super::super::fetch_session_delivery_authorized(&gateway.db, "permission-test").await);
    }

    #[tokio::test]
    async fn m130_remote_denial_keeps_grant_off_and_names_user_decision() {
        let gateway = test_gateway("trusted").await;
        sqlx::query("ALTER TABLE sessions ADD COLUMN delivery_authorized INTEGER NOT NULL DEFAULT 0")
            .execute(&gateway.db).await.unwrap();
        let args = serde_json::json!({"ceiling":"pr_only"});
        let call = ToolCall { id: "synthetic-deliver".into(), r#type: "function".into(),
            function: FunctionCall { name: "deliver_changes".into(), arguments: args.to_string() } };
        let cloned = gateway.clone();
        let handle = tokio::spawn(async move { cloned.authorize(&call, &args, None).await });
        take_pending_sender(&gateway).await.send(false).unwrap();
        match handle.await.unwrap() {
            PermissionOutcome::Deny(denial) => assert_eq!(denial.content, "用户不允许推送，请在本地完成并说明"),
            other => panic!("expected denial, got {other:?}"),
        }
        assert!(!super::super::fetch_session_delivery_authorized(&gateway.db, "permission-test").await);
    }

    #[tokio::test]
    async fn durable_gateway_persists_allow_before_channel_and_consumes_it_once() {
        let gateway = durable_test_gateway().await;
        let (tool_call, args) = browser_call("durable-allow", "click");
        let task_gateway = gateway.clone();
        let authorization =
            tokio::spawn(async move { task_gateway.authorize(&tool_call, &args, None).await });
        let intent = wait_for_durable_intent(&gateway).await;
        let store = PermissionIntentStore::new(gateway.db.clone());
        store
            .record_user_response(
                &intent.prompt_key(),
                PermissionPromptResponse::Allow,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        gateway
            .pending_permissions
            .lock()
            .await
            .remove(&intent.intent_id)
            .unwrap()
            .send(true)
            .unwrap();
        assert_eq!(
            authorization.await.unwrap(),
            PermissionOutcome::Allow,
            "the exact durable allow must authorize this one call"
        );
        assert_eq!(
            store.get(&intent.intent_id).await.unwrap().unwrap().status,
            PermissionIntentStatus::Consumed
        );
        assert!(!store
            .consume_exact_allow(&intent.prompt_key(), chrono::Utc::now().timestamp_millis())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn durable_gateway_timeout_is_recoverable_projection_not_user_denial() {
        let gateway = durable_test_gateway().await;
        let (tool_call, args) = browser_call("durable-timeout", "press");
        let outcome = gateway.authorize(&tool_call, &args, None).await;
        assert_denied_for(outcome, PermissionDenialReason::TimedOut);
        let row = sqlx::query(
            "SELECT status, failure_code FROM permission_intents
             WHERE provider_tool_call_id='durable-timeout'",
        )
        .fetch_one(&gateway.db)
        .await
        .unwrap();
        use sqlx::Row;
        assert_eq!(row.get::<String, _>("status"), "timed_out");
        assert_eq!(row.get::<String, _>("failure_code"), "permission_timed_out");
    }

    #[tokio::test]
    async fn durable_gateway_channel_close_is_persisted_for_rehydration() {
        let gateway = durable_test_gateway().await;
        let (tool_call, args) = browser_call("durable-channel-close", "fill");
        let task_gateway = gateway.clone();
        let authorization =
            tokio::spawn(async move { task_gateway.authorize(&tool_call, &args, None).await });
        let intent = wait_for_durable_intent(&gateway).await;
        drop(
            gateway
                .pending_permissions
                .lock()
                .await
                .remove(&intent.intent_id)
                .unwrap(),
        );
        assert_denied_for(
            authorization.await.unwrap(),
            PermissionDenialReason::ChannelClosed,
        );
        assert_eq!(
            PermissionIntentStore::new(gateway.db.clone())
                .observe_exact(&intent.intent_id, chrono::Utc::now().timestamp_millis())
                .await
                .unwrap()
                .unwrap()
                .disposition,
            PermissionRecoveryDisposition::ReprojectInterrupted
        );
    }

    #[tokio::test]
    async fn session_permission_policy_is_read_from_session_row() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY, permission_mode TEXT NOT NULL)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions (id, permission_mode) VALUES ('safe-session', 'safe'), ('trusted-session', 'trusted')")
            .execute(&db)
            .await
            .unwrap();

        let safe = resolve_session_permission_policy(&db, "safe-session").await;
        assert_eq!(
            decide_permission(&safe, "write_file", None),
            PermissionDecision::Ask
        );

        let trusted = resolve_session_permission_policy(&db, "trusted-session").await;
        assert_eq!(
            decide_permission(&trusted, "bash", Some("pnpm test")),
            PermissionDecision::Allow
        );

        let missing = resolve_session_permission_policy(&db, "missing").await;
        assert_eq!(
            decide_permission(&missing, "bash", Some("pnpm test")),
            PermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn browser_act_allow_is_consumed_by_only_the_exact_call() {
        let gateway = test_gateway("standard").await;
        assert_eq!(
            authorize_with_response(gateway.clone(), "first-click", "click", true).await,
            PermissionOutcome::Allow
        );

        let denied = authorize_with_response(gateway, "follow-up-fill", "fill", false).await;
        assert_denied_for(denied, PermissionDenialReason::DeniedByUser);
    }

    #[tokio::test]
    async fn trusted_mode_still_requires_the_first_browser_act_grant() {
        let gateway = test_gateway("trusted").await;
        assert_eq!(
            authorize_with_response(gateway.clone(), "trusted-first", "press", true).await,
            PermissionOutcome::Allow,
            "trusted mode must surface the first external act authorization"
        );

        let denied = authorize_with_response(gateway, "trusted-follow-up", "click", false).await;
        assert_denied_for(denied, PermissionDenialReason::DeniedByUser);
    }

    /// CF-TPP-R4: a user who steps away and comes back can still approve, and
    /// nothing already done is undone. The prompt window closing must not turn
    /// into a denial, a cancellation, or a lost request.
    #[tokio::test]
    async fn late_approval_after_the_prompt_window_still_continues() {
        let gateway = durable_test_gateway().await;
        let (tool_call, args) = browser_call("late-approval", "click");
        let task_gateway = gateway.clone();
        let authorization =
            tokio::spawn(async move { task_gateway.authorize(&tool_call, &args, None).await });
        let intent = wait_for_durable_intent(&gateway).await;

        // Nobody answers for longer than the first prompt window.
        tokio::time::sleep(PERMISSION_WAIT + Duration::from_millis(150)).await;
        assert!(
            gateway
                .pending_permissions
                .lock()
                .await
                .contains_key(&intent.intent_id),
            "an unanswered request must stay owned and answerable, not be voided"
        );
        let status: String = sqlx::query_scalar("SELECT status FROM permission_intents WHERE intent_id=?")
            .bind(&intent.intent_id)
            .fetch_one(&gateway.db)
            .await
            .unwrap();
        assert_eq!(
            status, "pending",
            "an unanswered request stays pending — it is never recorded as a user decision"
        );
        assert_ne!(status, "cancelled");

        // The user comes back and approves through the same durable path the
        // real command uses: record the exact decision, then wake the waiter.
        let store = PermissionIntentStore::new(gateway.db.clone());
        store
            .record_user_response(
                &intent.prompt_key(),
                PermissionPromptResponse::Allow,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .expect("a late approval is still an admissible decision");
        take_pending_sender(&gateway)
            .await
            .send(true)
            .expect("permission receiver remains live");
        assert_eq!(
            authorization.await.expect("authorization task"),
            PermissionOutcome::Allow,
            "a late approval must still let the task continue"
        );
    }

    #[tokio::test]
    async fn browser_act_rejection_does_not_create_a_grant() {
        let gateway = test_gateway("standard").await;
        let rejected =
            authorize_with_response(gateway.clone(), "rejected-click", "click", false).await;
        assert_denied_for(rejected, PermissionDenialReason::DeniedByUser);

        assert_eq!(
            authorize_with_response(gateway, "after-rejection", "fill", true).await,
            PermissionOutcome::Allow,
            "a later act must ask again after an explicit rejection"
        );
    }

    #[tokio::test]
    async fn browser_act_timeout_does_not_create_a_grant() {
        let gateway = test_gateway("standard").await;
        let (tool_call, args) = browser_call("timed-out-click", "click");
        let timed_out = gateway.authorize(&tool_call, &args, None).await;
        assert_denied_for(timed_out, PermissionDenialReason::TimedOut);

        assert_eq!(
            authorize_with_response(gateway, "after-timeout", "press", true).await,
            PermissionOutcome::Allow,
            "a later act must ask again after a timed-out prompt"
        );
    }

    #[tokio::test]
    async fn browser_act_channel_close_does_not_create_a_grant() {
        let gateway = test_gateway("standard").await;
        let (tool_call, args) = browser_call("closed-click", "click");
        let task_gateway = gateway.clone();
        let authorization =
            tokio::spawn(async move { task_gateway.authorize(&tool_call, &args, None).await });
        drop(take_pending_sender(&gateway).await);
        assert_denied_for(
            authorization.await.expect("authorization task"),
            PermissionDenialReason::ChannelClosed,
        );

        assert_eq!(
            authorize_with_response(gateway, "after-close", "fill", true).await,
            PermissionOutcome::Allow,
            "a later act must ask again after the response channel closes"
        );
    }

    #[tokio::test]
    async fn screenshot_does_not_reuse_the_browser_act_grant() {
        let gateway = test_gateway("standard").await;
        assert_eq!(
            authorize_with_response(gateway.clone(), "grant-act", "click", true).await,
            PermissionOutcome::Allow
        );
        let screenshot =
            authorize_with_response(gateway, "screenshot-still-asks", "screenshot", false).await;
        assert_denied_for(screenshot, PermissionDenialReason::DeniedByUser);
    }
}
