// SPDX-License-Identifier: Apache-2.0
pub mod db;
pub mod tasks;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub model_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    /// If this session was spawned by a subagent, the id of its parent (root) session.
    /// Top-level chat sessions have this set to `None` and should be shown in the sidebar.
    #[serde(default)]
    pub parent_session_id: Option<String>,
    /// "project" (full software-factory flow, default) or "quick" (one-off
    /// ephemeral chat from Home's Quick Task entry). Quick sessions are
    /// hidden from the Recent Projects list and reused across visits.
    #[serde(default = "default_session_kind")]
    pub kind: String,
    /// Per-session reasoning effort override (minimal/low/medium/high).
    /// None → fall back to the global Settings.reasoning_effort default.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

fn default_session_kind() -> String { "project".into() }

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub model_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// Serialised `Vec<ToolCall>` JSON; only set on assistant messages that invoked tools.
    pub tool_calls: Option<String>,
    /// Reasoning trace from thinking-mode models (DeepSeek reasoner, etc).
    /// Must be replayed back to the API on subsequent turns or the provider
    /// rejects the request with HTTP 400.
    pub reasoning_content: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ToolCallRecord {
    pub id: String,
    pub message_id: String,
    pub tool_name: String,
    pub arguments: String,
    pub result: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: i64,
}
