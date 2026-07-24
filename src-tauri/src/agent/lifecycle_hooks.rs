// SPDX-License-Identifier: Apache-2.0
//! Desktop lifecycle hooks (keystone slice 4.6 sub-step 5).
//!
//! Wraps the user's configured [`HookRunner`] so the shared loop fires the
//! pre/post-tool hooks through the [`LifecycleHooks`] trait instead of matching
//! an `Option<HookRunner>` inline. It owns the `HookRunner` — which owns an
//! `AppHandle` — so, exactly like the old inline `hook_runner`, it is
//! constructed ONLY in the provider loops (reached via the dead-stripped
//! `run()`), NEVER in test code: an `AppHandle`-owning struct instantiated in
//! the unit-test EXE trips the Windows loader (`STATUS_ENTRYPOINT_NOT_FOUND`,
//! #166). Headless runs use `NoOpHooks` from agent-loop instead.

use codefactory_agent_loop::services::LifecycleHooks;

use super::hooks::{HookEvent, HookRunner};

pub(super) struct DesktopLifecycleHooks {
    pub(super) runner: HookRunner,
}

#[async_trait::async_trait]
impl LifecycleHooks for DesktopLifecycleHooks {
    async fn pre_tool(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        self.runner
            .fire(HookEvent::PreTool {
                tool_name: tool_name.to_string(),
                args: args.clone(),
            })
            .await
    }

    async fn post_tool(&self, tool_name: &str, result: &str, duration_ms: u64) {
        self.runner
            .fire(HookEvent::PostTool {
                tool_name: tool_name.to_string(),
                result: result.to_string(),
                duration_ms,
            })
            .await;
    }
}
