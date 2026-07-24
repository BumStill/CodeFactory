// SPDX-License-Identifier: Apache-2.0
//! Loop capability seams (keystone slice 4.6): the desktop-only concerns the
//! shared loop reaches through trait objects instead of touching `Settings`,
//! the DB, or an `AppHandle` directly. Each has a desktop impl in the bin (under
//! `#[cfg(not(test))]`, #166) and a headless/no-op impl for the sidecar.

/// Per-round context decisions that read the live `Settings` (and, for ChatGPT,
/// the session DB). Re-queried EACH round by the loop so a mid-run model/window
/// change takes effect — a frozen snapshot would regress that. Headless returns
/// a fixed window / no vision / no reasoning effort.
#[async_trait::async_trait]
pub trait ContextPolicy: Send + Sync {
    /// `(select_limit(estimated), max_limit)` for the current model's window,
    /// in tokens (matches `context::ContextWindow`'s `u32` fields).
    async fn context_window(&self, estimated_tokens: u32) -> (u32, u32);
    /// Whether the active model accepts image input this round.
    async fn supports_vision(&self) -> bool;
    /// Pre-resolved ChatGPT reasoning effort for this round (empty for api
    /// styles that ignore it), so the transport stays DB-pure (slice 4.4d).
    async fn round_reasoning_effort(&self) -> String;
}

/// Tool lifecycle callbacks the loop fires around each tool call. Desktop wraps
/// the user's configured `HookRunner`; headless uses [`NoOpHooks`], which allows
/// every tool and records nothing. Both default to allow/no-op so a partial impl
/// degrades safely.
#[async_trait::async_trait]
pub trait LifecycleHooks: Send + Sync {
    /// Fired before a tool runs; returning `false` cancels the call.
    async fn pre_tool(&self, _tool_name: &str, _args: &serde_json::Value) -> bool {
        true
    }
    /// Fired after a tool completes (fire-and-forget; the result is already
    /// truncated by the caller).
    async fn post_tool(&self, _tool_name: &str, _result: &str, _duration_ms: u64) {}
}

/// Headless/no-op hooks: every tool is allowed, nothing is recorded. Owns no
/// `AppHandle`, so the sidecar and the unit-test EXE can construct it freely.
pub struct NoOpHooks;

impl LifecycleHooks for NoOpHooks {}

/// The loop's per-tool authorization outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// Run the tool.
    Allow,
    /// Skip the tool; feed this content back to the model as the tool result.
    Deny(String),
    /// The user cancelled while a prompt was pending: the loop finishes the
    /// remaining tool batch as cancelled and stops.
    Cancelled,
}

/// Decides whether a tool call may run, prompting the user when the policy
/// requires it. Desktop reads the live permission policy and, on `Ask`, emits a
/// prompt to the frontend and waits (or observes a cancellation); headless
/// auto-allows ([`AllowAllPermissions`]) since there is no user and the eval
/// sandbox is the boundary.
#[async_trait::async_trait]
pub trait PermissionGateway: Send + Sync {
    /// `bash_command` is the extracted shell command (for finer-grained
    /// matching), if the call is a `bash` invocation.
    async fn authorize(
        &self,
        tool_call: &crate::types::ToolCall,
        args: &serde_json::Value,
        bash_command: Option<&str>,
    ) -> PermissionOutcome;
}

/// Headless permission gateway: every tool is allowed. Owns no `AppHandle`.
pub struct AllowAllPermissions;

#[async_trait::async_trait]
impl PermissionGateway for AllowAllPermissions {
    async fn authorize(
        &self,
        _tool_call: &crate::types::ToolCall,
        _args: &serde_json::Value,
        _bash_command: Option<&str>,
    ) -> PermissionOutcome {
        PermissionOutcome::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedContext;

    #[async_trait::async_trait]
    impl ContextPolicy for FixedContext {
        async fn context_window(&self, _estimated: u32) -> (u32, u32) {
            (100_000, 200_000)
        }
        async fn supports_vision(&self) -> bool {
            false
        }
        async fn round_reasoning_effort(&self) -> String {
            String::new()
        }
    }

    #[tokio::test]
    async fn context_policy_is_object_safe() {
        let p: std::sync::Arc<dyn ContextPolicy> = std::sync::Arc::new(FixedContext);
        assert_eq!(p.context_window(1_000).await, (100_000, 200_000));
        assert!(!p.supports_vision().await);
        assert!(p.round_reasoning_effort().await.is_empty());
    }

    #[tokio::test]
    async fn noop_hooks_allow_all_and_are_object_safe() {
        let h: std::sync::Arc<dyn LifecycleHooks> = std::sync::Arc::new(NoOpHooks);
        assert!(h.pre_tool("bash", &serde_json::json!({"cmd": "ls"})).await);
        // post_tool is fire-and-forget: it must simply not panic.
        h.post_tool("bash", "output", 12).await;
    }

    #[tokio::test]
    async fn allow_all_permissions_allow_and_are_object_safe() {
        let g: std::sync::Arc<dyn PermissionGateway> = std::sync::Arc::new(AllowAllPermissions);
        let tc = crate::types::ToolCall {
            id: "t".into(),
            r#type: "function".into(),
            function: crate::types::FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        };
        assert_eq!(
            g.authorize(&tc, &serde_json::json!({"command": "rm -rf /"}), Some("rm -rf /"))
                .await,
            PermissionOutcome::Allow
        );
    }
}
