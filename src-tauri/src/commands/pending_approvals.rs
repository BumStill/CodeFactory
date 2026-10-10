// SPDX-License-Identifier: Apache-2.0
//! 后台审批列表读取持久状态，不依赖当前打开的会话或前端缓存。
use serde::Serialize;
use sqlx::{Row, SqlitePool};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    pub approval_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub expires_at: i64,
    pub tool_call_id: String,
}

pub async fn list_pending(pool: &SqlitePool) -> anyhow::Result<Vec<PendingApproval>> {
    let rows = sqlx::query("SELECT intent_id, session_id, tool_name, prompt_args_json, expires_at, provider_tool_call_id FROM permission_intents WHERE status='pending' ORDER BY created_at, intent_id")
        .fetch_all(pool).await?;
    rows.into_iter().map(|row| Ok(PendingApproval {
        approval_id: row.try_get("intent_id")?,
        session_id: row.try_get("session_id")?,
        tool_name: row.try_get("tool_name")?,
        args: serde_json::from_str(row.try_get::<&str, _>("prompt_args_json")?)?,
        expires_at: row.try_get("expires_at")?,
        tool_call_id: row.try_get("provider_tool_call_id")?,
    })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn lists_all_sessions_and_all_pending_tools_without_resolving_any() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE permission_intents (intent_id TEXT, session_id TEXT, tool_name TEXT, prompt_args_json TEXT, expires_at INTEGER, provider_tool_call_id TEXT, status TEXT, created_at INTEGER)").execute(&pool).await.unwrap();
        for (id, session, tool, status) in [("a", "A", "bash", "pending"), ("b", "A", "write_file", "pending"), ("c", "B", "bash", "pending"), ("d", "B", "bash", "denied")] {
            sqlx::query("INSERT INTO permission_intents VALUES (?, ?, ?, '{\"command\":\"printf synthetic\"}', 100, ?, ?, 1)").bind(id).bind(session).bind(tool).bind(id).bind(status).execute(&pool).await.unwrap();
        }
        let approvals = list_pending(&pool).await.unwrap();
        assert_eq!(approvals.iter().map(|a| a.approval_id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(approvals[2].session_id, "B");
        assert_eq!(approvals[0].tool_name, "bash");
        assert_eq!(approvals[0].args["command"], "printf synthetic");
        assert_eq!(approvals[0].expires_at, 100);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM permission_intents WHERE status='pending'").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 3);
    }
}
