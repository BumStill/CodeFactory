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
}

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
