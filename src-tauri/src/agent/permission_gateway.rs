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
use super::{
    await_permission_response, decide_permission_for_call, permission_policy_for_mode,
    PermissionDecision, PermissionResponse,
};

const PERMISSION_WAIT: Duration = Duration::from_secs(60);

pub(super) struct DesktopPermissionGateway {
    pub(super) settings: Arc<tokio::sync::RwLock<Settings>>,
    pub(super) db: SqlitePool,
    pub(super) session_id: String,
    pub(super) events: Arc<dyn EventSink>,
    pub(super) pending_permissions: PendingPermissionMap,
    pub(super) cancel: Option<Arc<AtomicBool>>,
    pub(super) browser_read_granted: bool,
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
        match decide_permission_for_call(
            &policy,
            &tool_call.function.name,
            args,
            bash_command,
            self.browser_read_granted,
        ) {
            PermissionDecision::Allow => PermissionOutcome::Allow,
            PermissionDecision::Ask => self.request_permission(tool_call, args.clone()).await,
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
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::agent::{decide_permission, PermissionDecision};

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
            PermissionDecision::Ask
        );
    }
}
