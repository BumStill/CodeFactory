// SPDX-License-Identifier: Apache-2.0
//! Desktop permission gateway (keystone slice 4.6 sub-step 6).
//!
//! Folds the loop's `decide_permission` + `request_permission` behind the
//! [`PermissionGateway`] trait: it reads the live permission policy and, on
//! `Ask`, prompts the frontend and waits for a response (or a cancellation /
//! 600s timeout). Owns only `Arc` handles — the settings lock, the event sink,
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

use codefactory_agent_loop::services::{PermissionGateway, PermissionOutcome};

use sqlx::SqlitePool;

use crate::config::settings::Settings;
use crate::openrouter::types::{StreamEvent, ToolCall};
use crate::PendingPermissionMap;

use super::events::EventSink;
use super::{
    await_permission_response, decide_permission, permission_policy_for_mode, PermissionDecision,
    PermissionResponse,
};

pub(super) struct DesktopPermissionGateway {
    pub(super) settings: Arc<tokio::sync::RwLock<Settings>>,
    pub(super) db: SqlitePool,
    pub(super) session_id: String,
    pub(super) events: Arc<dyn EventSink>,
    pub(super) pending_permissions: PendingPermissionMap,
    pub(super) cancel: Option<Arc<AtomicBool>>,
}

async fn resolve_session_permission_policy(db: &SqlitePool, session_id: &str) -> crate::config::settings::PermissionPolicy {
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
        // `bash_command` is the generic per-call detail the gate matches on. For
        // the browser it has to be the action plus the host, so the prompt can
        // name the site and the user can allow-list one host rather than the
        // whole tool.
        let browser_cmd = (tool_call.function.name == "browser_session")
            .then(|| crate::browser::policy::permission_cmd(args))
            .flatten();
        let gate_cmd = browser_cmd.as_deref().or(bash_command);
        match decide_permission(&policy, &tool_call.function.name, gate_cmd) {
            PermissionDecision::Allow => PermissionOutcome::Allow,
            PermissionDecision::Ask => self.request_permission(tool_call, args.clone()).await,
            PermissionDecision::Deny(reason) => {
                tracing::warn!("Tool '{}' denied: {reason}", tool_call.function.name);
                PermissionOutcome::Deny(format!(
                    "Tool call denied: {reason}. Please try a different approach."
                ))
            }
        }
    }
}

impl DesktopPermissionGateway {
    /// Register a pending permission, prompt the frontend, and wait for the
    /// user's response (or a cancellation / 600s timeout). Verbatim from the old
    /// `AgentLoop::request_permission`, mapping the `PermissionResponse` onto the
    /// loop's `PermissionOutcome`.
    async fn request_permission(&self, tc: &ToolCall, args: serde_json::Value) -> PermissionOutcome {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.pending_permissions
            .lock()
            .await
            .insert(tc.id.clone(), sender);

        self.events.emit(StreamEvent::PermissionRequest {
            tool_call_id: tc.id.clone(),
            tool_name: tc.function.name.clone(),
            args,
        });
        {
            let settings = self.settings.read().await;
            crate::notify::send(
                &settings,
                crate::notify::NotifyEvent::PermissionWaiting,
                format!("工具 {} 正在等待你的批准", tc.function.name),
            );
        }

        let response =
            await_permission_response(receiver, self.cancel.as_ref(), Duration::from_secs(600))
                .await;
        self.pending_permissions.lock().await.remove(&tc.id);
        match response {
            PermissionResponse::Allow => PermissionOutcome::Allow,
            PermissionResponse::Deny => PermissionOutcome::Deny(
                "Tool call denied by user. Please try a different approach.".to_string(),
            ),
            PermissionResponse::Cancelled => PermissionOutcome::Cancelled,
        }
    }
}


#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::agent::{decide_permission, PermissionDecision};
    use crate::config::settings::PermissionPolicy;

    /// Build a policy explicitly. PermissionPolicy has no Default on purpose —
    /// an "empty policy" is not a meaningful default for a security type.
    fn policy(allow: &[&str], full_access: bool) -> PermissionPolicy {
        PermissionPolicy {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            ask: Vec::new(),
            deny: Vec::new(),
            full_access,
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
        assert_eq!(decide_permission(&safe, "write_file", None), PermissionDecision::Ask);

        let trusted = resolve_session_permission_policy(&db, "trusted-session").await;
        assert_eq!(
            decide_permission(&trusted, "bash", Some("pnpm test")),
            PermissionDecision::Allow
        );

        let missing = resolve_session_permission_policy(&db, "missing").await;
        assert_eq!(decide_permission(&missing, "bash", Some("pnpm test")), PermissionDecision::Ask);
    }

    #[test]
    fn full_access_does_not_silently_grant_acting_as_the_user_in_a_browser() {
        // A blanket allow-everything is about the user's own machine. Clicking
        // "send" on a site they are signed in to is a different thing, and the
        // browser rules run ahead of full_access precisely so it stays a prompt.
        let trusted = policy(&[], true);
        assert_eq!(
            decide_permission(&trusted, "browser_session", Some("click")),
            PermissionDecision::Ask
        );
        assert_eq!(
            decide_permission(&trusted, "browser_session", Some("open:mail.example.com")),
            PermissionDecision::Ask
        );
        // …but bash still honours full access, so this is scoped to the browser.
        assert_eq!(
            decide_permission(&trusted, "bash", Some("pnpm test")),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn a_host_can_be_allow_listed_without_opening_the_whole_tool() {
        let scoped = policy(&["browser_session(read:github.com)"], false);
        assert_eq!(
            decide_permission(&scoped, "browser_session", Some("read:github.com")),
            PermissionDecision::Allow
        );
        // A different host is not covered by that grant.
        assert_eq!(
            decide_permission(&scoped, "browser_session", Some("read:bank.example.com")),
            PermissionDecision::Ask
        );
    }

    #[test]
    fn the_gate_key_carries_no_scheme_so_the_tool_owns_that_rule() {
        // Documented division of labour: the permission key is action + host, so
        // a non-web scheme cannot be judged here. `browser_session::open` checks
        // the raw URL and refuses before launching anything — asserted there.
        assert_eq!(
            crate::browser::policy::permission_cmd(&serde_json::json!({
                "action": "open",
                "url": "file:///etc/passwd"
            }))
            .as_deref(),
            // No host in a file URL, so the key degrades to the bare action.
            Some("open")
        );
    }
}
