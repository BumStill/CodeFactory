// SPDX-License-Identifier: Apache-2.0
//! Desktop persistence backend (keystone slice 4.4b).
//!
//! `SqlitePersistence` is the in-process implementation of agent-loop's
//! [`Persistence`] trait. It owns ONLY `{db, session_id, anonymous}` — NO
//! `AppHandle` — so the unit-test EXE can construct it from a bare pool and it
//! links no Tauri entrypoints (#166). The SQL bodies here are MOVED verbatim
//! from the `AgentLoop` inherent helpers, which now delegate to this; call
//! sites are unchanged, so the existing anonymous / #135-#136 / persist-cancelled
//! tests pin behaviour byte-for-byte.
//!
//! The `anonymous` flag lives here and every DB write returns the "not written"
//! value when set — centralizing the no-DB-trace guarantee. (The three NON-DB
//! anonymous guards — the KB-tool strip and hook disabling — deliberately stay
//! in the loop; folding them here would silently re-enable KB tools / hooks in
//! anonymous runs.)

use chrono::Utc;
use codefactory_agent_loop::journal::{PersistError, PersistResult, Persistence, UsageRow};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::openrouter::types::ToolCall;

/// Map any error into the tauri-free `PersistError` (agent-loop can't see
/// `sqlx::Error` or the bin's `AppError`, so we stringify at the boundary).
fn perr<E: std::fmt::Display>(e: E) -> PersistError {
    PersistError {
        message: e.to_string(),
    }
}

/// Convert a `PersistError` back into the bin's error type for the inherent
/// delegators. The user-facing message is preserved verbatim.
pub(super) fn to_app_error(e: PersistError) -> crate::errors::AppError {
    crate::errors::AppError::Other(e.message)
}

/// In-process persistence for the desktop app. Owns the pool + session + the
/// `anonymous` no-trace flag. No `AppHandle` (#166).
pub(super) struct SqlitePersistence {
    pub(super) db: SqlitePool,
    pub(super) session_id: String,
    pub(super) anonymous: bool,
}

#[async_trait::async_trait]
impl Persistence for SqlitePersistence {
    async fn persist_message(
        &self,
        role: &str,
        content: &str,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        tool_calls: Option<&[ToolCall]>,
        reasoning_content: Option<&str>,
        usage_request_id: Option<&str>,
    ) -> PersistResult<Option<String>> {
        // Anonymous runs never touch the DB — the assistant turn lives only in
        // the in-memory `messages` vec for the rest of this run. The `None`
        // return is load-bearing: callers key mark_rejected_candidate /
        // tool-start off the id.
        if self.anonymous {
            return Ok(None);
        }
        let msg_id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        let persisted_content = crate::trajectory::redact_derived_message_for_storage(content);
        let persisted_reasoning =
            reasoning_content.map(crate::trajectory::redact_derived_message_for_storage);
        let tool_calls_json = tool_calls
            .filter(|tcs| !tcs.is_empty())
            .map(|tcs| crate::trajectory::redact_tool_calls_for_storage(tcs).unwrap_or_default());

        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, input_tokens, output_tokens, tool_calls, reasoning_content, usage_request_id, created_at) \
             VALUES (?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&msg_id)
        .bind(&self.session_id)
        .bind(role)
        .bind(persisted_content)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(tool_calls_json)
        .bind(persisted_reasoning)
        .bind(usage_request_id)
        .bind(now)
        .execute(&self.db)
        .await
        .map_err(perr)?;
        Ok(Some(msg_id))
    }

    async fn persist_gate_message(&self, content: &str, state: &str) -> PersistResult<()> {
        if self.anonymous {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, completion_state, created_at) \
             VALUES (?,?,?,?,?,?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&self.session_id)
        .bind("user")
        .bind(content)
        .bind(state)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.db)
        .await
        .map_err(perr)?;
        Ok(())
    }

    async fn persist_gate_message_once(
        &self,
        marker: &str,
        content: &str,
        state: &str,
    ) -> PersistResult<()> {
        if self.anonymous {
            return Ok(());
        }
        let existing: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM messages WHERE session_id = ? AND completion_state = ? \
             AND content LIKE ?",
        )
        .bind(&self.session_id)
        .bind(state)
        .bind(format!("%{marker}%"))
        .fetch_one(&self.db)
        .await
        .map_err(perr)?;
        if existing.0 > 0 {
            return Ok(());
        }
        self.persist_gate_message(content, state).await
    }

    async fn mark_rejected_candidate(&self, message_id: Option<&str>) -> PersistResult<()> {
        let Some(message_id) = message_id else {
            return Ok(());
        };
        if self.anonymous {
            return Ok(());
        }
        sqlx::query("UPDATE messages SET completion_state='rejected_candidate' WHERE id=?")
            .bind(message_id)
            .execute(&self.db)
            .await
            .map_err(perr)?;
        Ok(())
    }

    async fn record_tool_call_outcome(
        &self,
        tool_call: &ToolCall,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
        duration_ms: u64,
    ) -> PersistResult<()> {
        if self.anonymous {
            return Ok(());
        }
        crate::trajectory::record_terminal_tool_outcome(
            &self.db,
            &self.session_id,
            &tool_call.id,
            status,
            result,
            error,
            duration_ms.min(i64::MAX as u64) as i64,
        )
        .await
        .map_err(perr)
    }

    async fn persist_cancelled_tool_batch(
        &self,
        remaining: &[ToolCall],
    ) -> PersistResult<Vec<String>> {
        // Delegates to the already-extracted free fn (tested at mod.rs:4379);
        // the content strings come back even in anonymous runs (UI path needs
        // them), only the per-item DB write is anonymous-gated.
        super::persist_cancelled_tool_batch(&self.db, &self.session_id, self.anonymous, remaining)
            .await
            .map_err(perr)
    }

    async fn record_usage(&self, row: UsageRow<'_>) -> PersistResult<bool> {
        if self.anonymous {
            return Ok(false);
        }
        let event = crate::commands::costs::UsageEventInput {
            request_id: row.request_id,
            session_id: row.session_id.to_string(),
            task_id: row.task_id,
            surface: row.surface.to_string(),
            provider: row.provider,
            endpoint: row.endpoint.to_string(),
            model: row.model.to_string(),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            reasoning_tokens: row.reasoning_tokens,
            cached_tokens: row.cached_tokens,
            actual_cost_usd: row.actual_cost_usd,
            estimated_cost_usd: None,
            cost_source: row.cost_source,
            created_at: None,
        };
        crate::commands::costs::record_usage_event(&self.db, event)
            .await
            .map_err(perr)
    }
}

#[cfg(test)]
mod tests {
    //! `SqlitePersistence` owns no `AppHandle`, so these construct it from a
    //! bare in-memory pool — the #166-safe pattern — and lock the invariants the
    //! delegators depend on: a written gate row, and the anonymous no-op path.
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT, role TEXT, \
             content TEXT, input_tokens INTEGER, output_tokens INTEGER, tool_calls TEXT, \
             reasoning_content TEXT, usage_request_id TEXT, completion_state TEXT, created_at INTEGER)",
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn persist_gate_message_writes_a_raw_tagged_row() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };
        p.persist_gate_message("recover: verify then finish", "gate_recovery")
            .await
            .unwrap();
        let (content, state): (String, String) = sqlx::query_as(
            "SELECT content, completion_state FROM messages WHERE session_id='s1'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(content, "recover: verify then finish"); // RAW, not redacted
        assert_eq!(state, "gate_recovery");
    }

    #[tokio::test]
    async fn anonymous_writes_nothing_and_persist_message_returns_none() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: true,
        };
        assert_eq!(
            p.persist_message("assistant", "secret", Some(1), Some(2), None, None, Some("r"))
                .await
                .unwrap(),
            None
        );
        p.persist_gate_message("x", "gate_ready").await.unwrap();
        p.mark_rejected_candidate(Some("m1")).await.unwrap();
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count.0, 0, "anonymous run left DB rows");
    }
}
