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

use std::sync::atomic::{AtomicBool, Ordering};
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

pub(super) struct DesktopPermissionGateway {
    pub(super) settings: Arc<tokio::sync::RwLock<Settings>>,
    pub(super) db: SqlitePool,
    pub(super) session_id: String,
    pub(super) events: Arc<dyn EventSink>,
    pub(super) pending_permissions: PendingPermissionMap,
    pub(super) cancel: Option<Arc<AtomicBool>>,
    pub(super) browser_read_granted: bool,
    /// One explicit browser-act approval covers later click/fill/press calls in
    /// this gateway (one AgentLoop run). It is never persisted across runs.
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
        let policy = resolve_session_permission_policy(&self.db, &self.session_id).await;
        let browser_act = is_browser_act(tool_call, args);
        if browser_act && self.browser_act_granted.load(Ordering::SeqCst) {
            return PermissionOutcome::Allow;
        }
        match decide_permission_for_call(
            &policy,
            &tool_call.function.name,
            args,
            bash_command,
            self.browser_read_granted,
        ) {
            PermissionDecision::Allow => PermissionOutcome::Allow,
            PermissionDecision::Ask => {
                let outcome = self.request_permission(tool_call, args.clone()).await;
                if browser_act && matches!(outcome, PermissionOutcome::Allow) {
                    self.browser_act_granted.store(true, Ordering::SeqCst);
                }
                outcome
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

fn is_browser_act(tool_call: &ToolCall, args: &serde_json::Value) -> bool {
    tool_call.function.name == "browser_session"
        && args
            .get("action")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|action| matches!(action, "click" | "fill" | "press"))
}

impl DesktopPermissionGateway {
    /// Register a pending permission, prompt the frontend, and wait for the
    /// user's response (or a cancellation / bounded timeout).
    async fn request_permission(
        &self,
        tc: &ToolCall,
        args: serde_json::Value,
    ) -> PermissionOutcome {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.pending_permissions
            .lock()
            .await
            .insert(tc.id.clone(), sender);

        let expires_at = chrono::Utc::now().timestamp_millis() + PERMISSION_WAIT.as_millis() as i64;
        self.events.emit(StreamEvent::PermissionRequest {
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
        self.pending_permissions.lock().await.remove(&tc.id);
        match response {
            PermissionResponse::Allow => PermissionOutcome::Allow,
            PermissionResponse::DeniedByUser => PermissionOutcome::Deny(PermissionDenial {
                content: "Tool call denied by the user. The requested action was not executed; do not bypass this decision with another tool.".to_string(),
                reason: PermissionDenialReason::DeniedByUser,
                duration_ms,
            }),
            PermissionResponse::TimedOut => PermissionOutcome::Deny(PermissionDenial {
                content: "Permission request timed out after 60 seconds without a user decision. This was not a user denial. The requested action was not executed; do not substitute another source as equivalent evidence.".to_string(),
                reason: PermissionDenialReason::TimedOut,
                duration_ms,
            }),
            PermissionResponse::ChannelClosed => PermissionOutcome::Deny(PermissionDenial {
                content: "Permission request was interrupted because its response channel closed. No user decision was recorded and the requested action was not executed.".to_string(),
                reason: PermissionDenialReason::ChannelClosed,
                duration_ms,
            }),
            PermissionResponse::Cancelled => PermissionOutcome::Cancelled,
        }
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
            events: Arc::new(CollectingEventSink::new()),
            pending_permissions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            cancel: None,
            browser_read_granted: false,
            browser_act_granted: AtomicBool::new(false),
        })
    }

    async fn take_pending_sender(
        gateway: &DesktopPermissionGateway,
        id: &str,
    ) -> oneshot::Sender<bool> {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(sender) = gateway.pending_permissions.lock().await.remove(id) {
                    return sender;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("permission prompt {id} was not emitted"))
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
        take_pending_sender(&gateway, id)
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
    async fn browser_act_grant_is_reused_for_follow_up_act_calls() {
        let gateway = test_gateway("standard").await;
        assert_eq!(
            authorize_with_response(gateway.clone(), "first-click", "click", true).await,
            PermissionOutcome::Allow
        );

        for (id, action) in [("follow-up-fill", "fill"), ("follow-up-press", "press")] {
            let (tool_call, args) = browser_call(id, action);
            let outcome = tokio::time::timeout(
                Duration::from_secs(1),
                gateway.authorize(&tool_call, &args, None),
            )
            .await
            .unwrap_or_else(|_| panic!("{action} asked again after the browser act grant"));
            assert_eq!(outcome, PermissionOutcome::Allow);
        }
    }

    #[tokio::test]
    async fn trusted_mode_still_requires_the_first_browser_act_grant() {
        let gateway = test_gateway("trusted").await;
        assert_eq!(
            authorize_with_response(gateway.clone(), "trusted-first", "press", true).await,
            PermissionOutcome::Allow,
            "trusted mode must surface the first external act authorization"
        );

        let (tool_call, args) = browser_call("trusted-follow-up", "click");
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(1),
                gateway.authorize(&tool_call, &args, None),
            )
            .await
            .expect("the granted follow-up must not prompt"),
            PermissionOutcome::Allow
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
        drop(take_pending_sender(&gateway, "closed-click").await);
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
