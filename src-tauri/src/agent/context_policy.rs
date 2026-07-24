// SPDX-License-Identifier: Apache-2.0
//! Desktop context policy (keystone slice 4.6 sub-step 4).
//!
//! The in-process [`ContextPolicy`] impl: it re-reads the live `Settings` (and,
//! for ChatGPT, the session DB) EACH round, exactly as the loop did inline, so a
//! mid-run model/window change still takes effect. Owns no `AppHandle` (#166) —
//! only the settings lock + pool + config identity. Shared context helpers live
//! in the parent `agent` module and are reached via `super::`.

use codefactory_agent_loop::services::ContextPolicy;
use std::sync::Arc;

use crate::config::settings::{ApiStyle, Settings};

pub(super) struct DesktopContextPolicy {
    pub(super) settings: Arc<tokio::sync::RwLock<Settings>>,
    pub(super) db: sqlx::SqlitePool,
    pub(super) session_id: String,
    pub(super) endpoint_name: String,
    pub(super) model_id: String,
    pub(super) api_style: ApiStyle,
}

#[async_trait::async_trait]
impl ContextPolicy for DesktopContextPolicy {
    async fn context_window(&self, estimated_tokens: u32) -> (u32, u32) {
        let settings = self.settings.read().await;
        let window = super::context::resolve_context_window(
            &settings,
            &self.endpoint_name,
            &self.model_id,
            None,
        );
        (window.select_limit(estimated_tokens), window.max_limit)
    }

    async fn supports_vision(&self) -> bool {
        let settings = self.settings.read().await;
        super::context::model_supports_vision(&settings, &self.endpoint_name, &self.model_id)
    }

    async fn round_reasoning_effort(&self) -> String {
        if !matches!(self.api_style, ApiStyle::Chatgpt) {
            return String::new();
        }
        // Re-read per round (freshness): a mid-run sessions.reasoning_effort
        // change takes effect next round. Verbatim from the old
        // AgentLoop::resolve_round_reasoning_effort.
        let session_effort =
            super::fetch_session_reasoning_effort(&self.db, &self.session_id).await;
        let settings = self.settings.read().await;
        super::resolve_chatgpt_reasoning_effort(
            &settings,
            &self.endpoint_name,
            &self.model_id,
            session_effort.as_deref(),
        )
        .as_str()
        .to_string()
    }
}
