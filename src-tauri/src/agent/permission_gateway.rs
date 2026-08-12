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
        if let Some(outcome) = self
            .authorize_permission_recovery(tool_call, args, bash_command)
            .await
        {
            return outcome;
        }
        let policy = resolve_session_permission_policy(&self.db, &self.session_id).await;
        match decide_permission_for_call(
            &policy,
            &tool_call.function.name,
            args,
            bash_command,
            self.browser_read_granted,
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
        let now = chrono::Utc::now().timestamp_millis();
        let expires_at = now + PERMISSION_WAIT.as_millis() as i64;
        let durable_intent = if self.anonymous {
            None
        } else {
            let scope = match self.resolve_scope().await {
                Ok(scope) => scope,
                Err(error) => {
                    tracing::error!(%error, "permission prompt refused without exact Objective scope");
                    return PermissionOutcome::Deny(PermissionDenial {
                        content: "Permission request could not be bound to the exact Objective revision. No action was executed; the system will reconcile the Objective identity before retrying.".into(),
                        reason: PermissionDenialReason::ChannelClosed,
                        duration_ms: 0,
                    });
                }
            };
            let request = PermissionIntentRequest {
                scope,
                session_id: self.session_id.clone(),
                provider_tool_call_id: tc.id.clone(),
                tool_name: tc.function.name.clone(),
                args: args.clone(),
                bash_command: bash_command.map(str::to_string),
                expires_at,
                created_process_instance: crate::agent::objective::current_process_instance(),
            };
            match PermissionIntentStore::new(self.db.clone())
                .create_pending(&request, now)
                .await
            {
                Ok(intent) => Some(intent),
                Err(error) => {
                    tracing::error!(%error, "durable permission intent admission failed");
                    return PermissionOutcome::Deny(PermissionDenial {
                        content: "Permission request could not be durably admitted. No action was executed; the system retains ownership of the interruption.".into(),
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
        let (sender, receiver) = tokio::sync::oneshot::channel();
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

        self.events.emit(StreamEvent::PermissionRequest {
            intent_id: intent_id.clone(),
            tool_call_id: tc.id.clone(),
            tool_name: tc.function.name.clone(),
            args,
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
        let response =
            await_permission_response(receiver, self.cancel.as_ref(), PERMISSION_WAIT).await;
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
                content: "Permission request timed out after 60 seconds without a user decision. This was not a user denial. The requested action was not executed; do not substitute another source as equivalent evidence.".to_string(),
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
               AND objective.status NOT IN ('completed','cancelled','legacy_orphan')
             ORDER BY binding.resource_generation DESC LIMIT 2",
        )
        .bind(resource.0)
        .bind(resource.1)
        .fetch_all(&self.db)
        .await?;
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

    async fn wait_for_durable_intent(
        gateway: &DesktopPermissionGateway,
    ) -> PermissionIntentSnapshot {
        tokio::time::timeout(Duration::from_secs(1), async {
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
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("durable permission intent was admitted")
    }

    async fn take_pending_sender(gateway: &DesktopPermissionGateway) -> oneshot::Sender<bool> {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let mut pending = gateway.pending_permissions.lock().await;
                if let Some(key) = pending.keys().next().cloned() {
                    let sender = pending.remove(&key).expect("pending key remains present");
                    return sender;
                }
                drop(pending);
                tokio::task::yield_now().await;
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
