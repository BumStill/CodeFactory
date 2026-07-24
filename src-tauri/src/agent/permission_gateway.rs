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

use crate::config::settings::Settings;
use crate::openrouter::types::{StreamEvent, ToolCall};
use crate::PendingPermissionMap;

use super::events::EventSink;
use super::{await_permission_response, decide_permission, PermissionDecision, PermissionResponse};

pub(super) struct DesktopPermissionGateway {
    pub(super) settings: Arc<tokio::sync::RwLock<Settings>>,
    pub(super) events: Arc<dyn EventSink>,
    pub(super) pending_permissions: PendingPermissionMap,
    pub(super) cancel: Option<Arc<AtomicBool>>,
}

#[async_trait::async_trait]
impl PermissionGateway for DesktopPermissionGateway {
    async fn authorize(
        &self,
        tool_call: &ToolCall,
        args: &serde_json::Value,
        bash_command: Option<&str>,
    ) -> PermissionOutcome {
        let policy = {
            let settings = self.settings.read().await;
            settings.permissions.clone()
        };
        match decide_permission(&policy, &tool_call.function.name, bash_command) {
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
