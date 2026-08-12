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
use codefactory_agent_loop::journal::{
    PersistError, PersistResult, Persistence, RecoveryAttemptRow, TurnActivityUpdate, UsageRow,
};
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

pub async fn persist_model_route_attempts(
    db: &SqlitePool,
    session_id: &str,
    root_turn_id: &str,
    policy: &str,
    attempts: &[super::failover::RouteAttemptSnapshot],
    output_started: bool,
    side_effect_started: bool,
) -> PersistResult<()> {
    if attempts.is_empty() {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    let mut tx = db.begin().await.map_err(perr)?;
    for attempt in attempts {
        sqlx::query(
            "INSERT INTO model_route_attempts
             (id, root_turn_id, session_id, endpoint, model, policy, status,
              failure_code, output_started, side_effect_started, created_at, completed_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(root_turn_id)
        .bind(session_id)
        .bind(&attempt.endpoint)
        .bind(&attempt.model)
        .bind(policy)
        .bind(&attempt.status)
        .bind(&attempt.failure_code)
        .bind(i64::from(output_started))
        .bind(i64::from(side_effect_started))
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(perr)?;
    }
    tx.commit().await.map_err(perr)?;
    Ok(())
}

/// Gate control states that are pure forensics: the loop injects them as
/// prompts, `replayable_history` excludes them from the model's context, and
/// the UI excludes them from the transcript. They belong in `gate_events`, not
/// in the conversation store — a control loop decides what the agent does next,
/// it does not get to write rows into the user's conversation.
///
/// `gate_warning` / `turn_notice` / `turn_error` are deliberately NOT here:
/// those are user-facing notices that the transcript is supposed to show.
///
/// `rejected_candidate` is not a state passed through here either — it marks
/// an existing assistant row rather than injecting a new one, so it arrives
/// via [`Persistence::mark_rejected_candidate`] and is recorded as a
/// `gate_events` row that points at the message id.
fn is_gate_control_state(state: &str) -> bool {
    matches!(state, "gate_recovery" | "gate_ready" | "gate_blocked")
}

/// In-process persistence for the desktop app. Owns the pool + session + the
/// `anonymous` no-trace flag. No `AppHandle` (#166).
pub(super) struct SqlitePersistence {
    pub(super) db: SqlitePool,
    pub(super) session_id: String,
    pub(super) anonymous: bool,
}

impl SqlitePersistence {
    /// Move the session's "last activity" marker forward.
    ///
    /// The sidebar shows and orders by `sessions.updated_at`
    /// (`SessionSidebar.tsx`, `ORDER BY updated_at DESC`,
    /// `groupSessionsByProject`), so that column has to mean "when did anything
    /// real last happen here". Nothing advanced it when messages arrived — only
    /// changing the model or setting the title did, and the title is generated
    /// once at the start — so it froze near creation time and a long
    /// conversation sank below sessions nobody had touched in days.
    ///
    /// Deliberately best-effort: a session list that is one row stale is a far
    /// smaller problem than losing the message that was just written, so a
    /// failure here logs and moves on rather than failing the persist.
    async fn touch_session(&self, at_ms: i64) {
        if let Err(error) = sqlx::query("UPDATE sessions SET updated_at = ? WHERE id = ?")
            .bind(at_ms)
            .bind(&self.session_id)
            .execute(&self.db)
            .await
        {
            tracing::warn!(
                session_id = %self.session_id,
                "could not advance session activity time: {error}"
            );
        }
    }

    /// Append one gate control event to the side table. Content is stored raw:
    /// these rows exist to answer "why did the loop keep going", so redacting
    /// them would defeat the only reason to keep them.
    async fn record_gate_event(
        &self,
        kind: &str,
        content: &str,
        message_id: Option<&str>,
    ) -> PersistResult<()> {
        sqlx::query(
            "INSERT INTO gate_events (id, session_id, kind, content, message_id, created_at) \
             VALUES (?,?,?,?,?,?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&self.session_id)
        .bind(kind)
        .bind(content)
        .bind(message_id)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.db)
        .await
        .map_err(perr)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Persistence for SqlitePersistence {
    async fn record_recovery_attempt(&self, attempt: &RecoveryAttemptRow) -> PersistResult<()> {
        if self.anonymous {
            return Ok(());
        }
        let now = Utc::now().timestamp_millis();
        let process_instance = format!(
            "{}:{}",
            std::process::id(),
            crate::storage::db::current_process_start_token()
                .unwrap_or_else(|| "unknown-start".into())
        );
        let mut tx = self.db.begin().await.map_err(perr)?;
        sqlx::query(
            "UPDATE chat_turn_state
             SET recovery_attempt=recovery_attempt+1, updated_at=?
             WHERE root_turn_id=?",
        )
        .bind(now)
        .bind(&attempt.root_turn_id)
        .execute(&mut *tx)
        .await
        .map_err(perr)?;
        let objective_id = sqlx::query_scalar::<_, Option<String>>(
            "SELECT objective_id FROM chat_turn_state WHERE root_turn_id=?",
        )
        .bind(&attempt.root_turn_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(perr)?
        .flatten()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            perr(format!(
                "recovery attempt refused without unified objective identity for root turn {}",
                attempt.root_turn_id
            ))
        })?;
        sqlx::query(
            "INSERT INTO objective_recovery_attempts
             (id, objective_id, root_turn_id, delivery_run_id, domain, attempt_index,
              failure_code, failure_class, output_started, side_effect_started,
              queue_wait_ms, runtime_ms, process_instance, resume_owner,
              terminal_decision, created_at)
             VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, 'agent_loop', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(objective_id)
        .bind(&attempt.root_turn_id)
        .bind(&attempt.domain)
        .bind(attempt.attempt_index)
        .bind(&attempt.failure_code)
        .bind(&attempt.failure_class)
        .bind(i64::from(attempt.output_started))
        .bind(i64::from(attempt.side_effect_started))
        .bind(&process_instance)
        .bind(&attempt.terminal_decision)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(perr)?;
        tx.commit().await.map_err(perr)?;
        Ok(())
    }

    async fn update_turn_activity(&self, update: &TurnActivityUpdate) -> PersistResult<i64> {
        if self.anonymous {
            return Ok(0);
        }
        let now = Utc::now().timestamp_millis();
        let terminal = matches!(
            update.status.as_str(),
            "completed"
                | "blocked"
                | "cancelled"
                | "interrupted"
                | "failed_internal"
                | "platform_incident"
        );
        sqlx::query(
            "UPDATE chat_turn_state SET
               revision=revision+1, phase=?, status=?,
               recent_activity_kind=?, recent_activity_label=?,
               waiting_reason=?, updated_at=?,
               completed_at=CASE WHEN ? THEN COALESCE(completed_at, ?) ELSE NULL END,
               terminal_reason=?
             WHERE root_turn_id=?",
        )
        .bind(&update.phase)
        .bind(&update.status)
        .bind(&update.recent_activity_kind)
        .bind(&update.recent_activity_label)
        .bind(&update.waiting_reason)
        .bind(now)
        .bind(terminal)
        .bind(now)
        .bind(&update.terminal_reason)
        .bind(&update.root_turn_id)
        .execute(&self.db)
        .await
        .map_err(perr)?;
        // `chat_task_segments` is the durable objective projection, not the
        // transport-turn lifecycle. A Done/failed_internal/platform_incident
        // settles the current turn above but cannot complete or fail the
        // business objective. Only reopening work or an explicit user cancel
        // changes the segment here; CompletionArbiter owns completed.
        if update.status == "active" || update.status == "cancelled" {
            sqlx::query(
                "UPDATE chat_task_segments SET status=?, updated_at=?
                 WHERE id=(SELECT task_segment_id FROM chat_turn_state WHERE root_turn_id=?)",
            )
            .bind(&update.status)
            .bind(now)
            .bind(&update.root_turn_id)
            .execute(&self.db)
            .await
            .map_err(perr)?;
        }
        sqlx::query_scalar("SELECT revision FROM chat_turn_state WHERE root_turn_id=?")
            .bind(&update.root_turn_id)
            .fetch_optional(&self.db)
            .await
            .map_err(perr)
            .map(|value| value.unwrap_or(0))
    }

    async fn persist_message(
        &self,
        role: &str,
        content: &str,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        tool_calls: Option<&[ToolCall]>,
        reasoning_content: Option<&str>,
        endpoint_id: Option<&str>,
        model_id: Option<&str>,
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
            "INSERT INTO messages (id, session_id, role, content, endpoint_id, model_id, input_tokens, output_tokens, tool_calls, reasoning_content, usage_request_id, created_at) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&msg_id)
        .bind(&self.session_id)
        .bind(role)
        .bind(persisted_content)
        .bind(endpoint_id)
        .bind(model_id)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(tool_calls_json)
        .bind(persisted_reasoning)
        .bind(usage_request_id)
        .bind(now)
        .execute(&self.db)
        .await
        .map_err(perr)?;
        self.touch_session(now).await;
        Ok(Some(msg_id))
    }

    async fn persist_gate_message(&self, content: &str, state: &str) -> PersistResult<()> {
        if self.anonymous {
            return Ok(());
        }
        if is_gate_control_state(state) {
            return self.record_gate_event(state, content, None).await;
        }
        // What's left is user-facing: a warning, a runtime notice, or a turn
        // error. Notices are internal provenance rather than new user intent,
        // so they stay role=system and out of replay.
        let role = if state == "turn_notice" {
            "system"
        } else {
            "user"
        };
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, completion_state, created_at) \
             VALUES (?,?,?,?,?,?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&self.session_id)
        .bind(role)
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
    ) -> PersistResult<bool> {
        if self.anonymous {
            return Ok(false);
        }
        let marker_id = format!("notice-marker:{}:{marker}", self.session_id);
        let mut tx = self.db.begin().await.map_err(perr)?;
        let inserted = if is_gate_control_state(state) {
            sqlx::query(
                "INSERT OR IGNORE INTO gate_events \
                 (id, session_id, kind, content, message_id, created_at) \
                 VALUES (?,?,?,?,NULL,?)",
            )
            .bind(&marker_id)
            .bind(&self.session_id)
            .bind(state)
            .bind(content)
            .bind(Utc::now().timestamp_millis())
            .execute(&mut *tx)
            .await
            .map_err(perr)?
            .rows_affected()
        } else {
            sqlx::query(
                "INSERT OR IGNORE INTO gate_events \
                 (id, session_id, kind, content, message_id, created_at) \
                 VALUES (?,?,'notice_marker',?,NULL,?)",
            )
            .bind(&marker_id)
            .bind(&self.session_id)
            .bind(marker)
            .bind(Utc::now().timestamp_millis())
            .execute(&mut *tx)
            .await
            .map_err(perr)?
            .rows_affected()
        };
        if inserted == 0 {
            tx.rollback().await.map_err(perr)?;
            return Ok(false);
        }
        if !is_gate_control_state(state) {
            let role = if state == "turn_notice" {
                "system"
            } else {
                "user"
            };
            sqlx::query(
                "INSERT INTO messages \
                 (id, session_id, role, content, completion_state, created_at) \
                 VALUES (?,?,?,?,?,?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&self.session_id)
            .bind(role)
            .bind(content)
            .bind(state)
            .bind(Utc::now().timestamp_millis())
            .execute(&mut *tx)
            .await
            .map_err(perr)?;
        }
        tx.commit().await.map_err(perr)?;
        Ok(true)
    }

    async fn root_turn_tool_call_count(&self, root_turn_id: &str) -> PersistResult<usize> {
        if self.anonymous {
            return Ok(0);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM tool_calls tc
             JOIN messages assistant ON assistant.id = tc.message_id
             WHERE assistant.session_id = ?
               AND assistant.rowid > COALESCE(
                 (SELECT rowid FROM messages WHERE id = ? AND session_id = ?),
                 9223372036854775807
               )
               AND assistant.rowid < COALESCE(
                 (
                   SELECT MIN(next_user.rowid)
                   FROM messages next_user
                   WHERE next_user.session_id = ?
                     AND next_user.role = 'user'
                     AND (next_user.completion_state IS NULL OR next_user.completion_state = '')
                     AND EXISTS (
                       SELECT 1 FROM chat_turn_state registered_root
                       WHERE registered_root.root_turn_id = next_user.id
                     )
                     AND next_user.rowid > (
                       SELECT rowid FROM messages WHERE id = ? AND session_id = ?
                     )
                 ),
                 9223372036854775807
               )",
        )
        .bind(&self.session_id)
        .bind(root_turn_id)
        .bind(&self.session_id)
        .bind(&self.session_id)
        .bind(root_turn_id)
        .bind(&self.session_id)
        .fetch_one(&self.db)
        .await
        .map_err(perr)?;
        Ok(count.max(0) as usize)
    }

    /// Record that the gate rejected this assistant turn as a premature final
    /// answer. The assistant row itself is left exactly as written — a control
    /// loop does not get to rewrite what already happened. The rejection is an
    /// annotation beside the conversation, and `load_agent_history` reads it to
    /// keep rejected drafts out of the model's replayed context.
    async fn mark_rejected_candidate(&self, message_id: Option<&str>) -> PersistResult<()> {
        let Some(message_id) = message_id else {
            return Ok(());
        };
        if self.anonymous {
            return Ok(());
        }
        self.record_gate_event("rejected_candidate", "", Some(message_id))
            .await
    }

    async fn record_tool_call_started(
        &self,
        message_id: &str,
        tool_call: &ToolCall,
    ) -> PersistResult<()> {
        if self.anonymous {
            return Ok(());
        }
        let args = serde_json::from_str(&tool_call.function.arguments).unwrap_or_default();
        crate::trajectory::record_tool_call_started(
            &self.db,
            &self.session_id,
            message_id,
            &tool_call.id,
            &tool_call.function.name,
            &args,
        )
        .await
        .map_err(perr)
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

    async fn record_tool_call_metadata(
        &self,
        tool_call: &ToolCall,
        metadata: &serde_json::Value,
    ) -> PersistResult<()> {
        if self.anonymous {
            return Ok(());
        }
        crate::trajectory::record_tool_call_metadata(
            &self.db,
            &self.session_id,
            &tool_call.id,
            metadata,
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
             content TEXT, endpoint_id TEXT, model_id TEXT, input_tokens INTEGER,
             output_tokens INTEGER, tool_calls TEXT, \
             reasoning_content TEXT, usage_request_id TEXT, completion_state TEXT, created_at INTEGER)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, created_at INTEGER, updated_at INTEGER)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE gate_events (id TEXT PRIMARY KEY, session_id TEXT, kind TEXT, \
             content TEXT, message_id TEXT, created_at INTEGER)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE tool_calls (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, \
             tool_name TEXT NOT NULL, arguments TEXT NOT NULL DEFAULT '{}', result TEXT, \
             metadata TEXT, status TEXT NOT NULL DEFAULT 'pending', error TEXT, \
             duration_ms INTEGER, created_at INTEGER NOT NULL)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE model_route_attempts (
                id TEXT PRIMARY KEY, root_turn_id TEXT, session_id TEXT,
                endpoint TEXT, model TEXT, policy TEXT, status TEXT,
                failure_code TEXT, output_started INTEGER,
                side_effect_started INTEGER, created_at TEXT, completed_at TEXT
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_task_segments (
                id TEXT PRIMARY KEY, previous_segment_id TEXT,
                status TEXT NOT NULL, updated_at INTEGER NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
                root_turn_id TEXT PRIMARY KEY, task_segment_id TEXT, revision INTEGER NOT NULL,
                objective_id TEXT,
                recovery_attempt INTEGER NOT NULL DEFAULT 0,
                phase TEXT NOT NULL, status TEXT NOT NULL, recent_activity_kind TEXT,
                recent_activity_label TEXT, waiting_reason TEXT, updated_at INTEGER NOT NULL,
                completed_at INTEGER, terminal_reason TEXT
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE objective_recovery_attempts (
                id TEXT PRIMARY KEY, objective_id TEXT, root_turn_id TEXT,
                delivery_run_id TEXT, domain TEXT NOT NULL, attempt_index INTEGER NOT NULL,
                failure_code TEXT NOT NULL, failure_class TEXT NOT NULL,
                output_started INTEGER NOT NULL, side_effect_started INTEGER NOT NULL,
                queue_wait_ms INTEGER, runtime_ms INTEGER, process_instance TEXT NOT NULL,
                resume_owner TEXT NOT NULL, terminal_decision TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn rejecting_a_draft_annotates_it_without_rewriting_the_assistant_row() {
        let db = pool().await;
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at) \
             VALUES ('a1','s1','assistant','premature done claim',1)",
        )
        .execute(&db)
        .await
        .unwrap();
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };

        p.mark_rejected_candidate(Some("a1")).await.unwrap();

        // The conversation row is untouched — content AND completion_state.
        let (content, state): (String, Option<String>) =
            sqlx::query_as("SELECT content, completion_state FROM messages WHERE id='a1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(content, "premature done claim");
        assert_eq!(state, None, "the gate must not rewrite history in place");

        let (kind, message_id): (String, String) =
            sqlx::query_as("SELECT kind, message_id FROM gate_events WHERE session_id='s1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(kind, "rejected_candidate");
        assert_eq!(message_id, "a1");
    }

    #[tokio::test]
    async fn anonymous_rejection_writes_nothing() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: true,
        };
        p.mark_rejected_candidate(Some("a1")).await.unwrap();
        let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gate_events")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(rows.0, 0);
    }

    #[tokio::test]
    async fn gate_control_prompts_go_to_the_side_table_not_the_conversation() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };
        for state in ["gate_recovery", "gate_ready", "gate_blocked"] {
            p.persist_gate_message("recover: verify then finish", state)
                .await
                .unwrap();
        }

        // The conversation store stays untouched — this is the whole point.
        let messages: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(messages.0, 0);

        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT kind, content FROM gate_events WHERE session_id='s1' ORDER BY kind",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(
            rows.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["gate_blocked", "gate_ready", "gate_recovery"],
        );
        // RAW, not redacted — forensics is the only reason these rows exist.
        assert!(rows.iter().all(|(_, c)| c == "recover: verify then finish"));
    }

    #[tokio::test]
    async fn user_facing_gate_notices_stay_in_the_conversation() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };
        p.persist_gate_message("⚠ 以上回复未经完整验证", "gate_warning")
            .await
            .unwrap();
        let (role, state): (String, String) =
            sqlx::query_as("SELECT role, completion_state FROM messages WHERE session_id='s1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(role, "user");
        assert_eq!(state, "gate_warning");
        let side: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM gate_events")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(side.0, 0);
    }

    #[tokio::test]
    async fn persist_gate_message_once_dedups_against_the_table_it_writes() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };
        for _ in 0..2 {
            p.persist_gate_message_once("已在发送前", "已在发送前移除图片", "turn_notice")
                .await
                .unwrap();
            p.persist_gate_message_once("verify", "verify then finish", "gate_recovery")
                .await
                .unwrap();
        }
        let notices: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM messages")
            .fetch_one(&db)
            .await
            .unwrap();
        let control: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM gate_events WHERE kind != 'notice_marker'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(notices.0, 1, "turn notices dedup within messages");
        assert_eq!(control.0, 1, "gate prompts dedup within gate_events");
    }

    #[tokio::test]
    async fn opaque_notice_marker_dedups_without_leaking_into_the_notice_body() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };
        for _ in 0..2 {
            p.persist_gate_message_once(
                "tool_amplification:root-1:40",
                "本回合工具调用较多",
                "turn_notice",
            )
            .await
            .unwrap();
        }

        let bodies: Vec<String> =
            sqlx::query_scalar("SELECT content FROM messages WHERE session_id='s1' ORDER BY rowid")
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(bodies, ["本回合工具调用较多"]);
        assert!(!bodies[0].contains("root-1"));
    }

    #[tokio::test]
    async fn root_turn_tool_count_crosses_steers_and_stops_at_the_next_registered_root() {
        let db = pool().await;
        for (id, role, state, created_at) in [
            ("root-1", "user", None, 1_i64),
            ("assistant-1", "assistant", None, 2),
            ("steer-1", "user", None, 3),
            ("assistant-2", "assistant", None, 4),
            ("notice", "system", Some("turn_notice"), 5),
            ("root-2", "user", None, 6),
            ("assistant-3", "assistant", None, 7),
        ] {
            sqlx::query(
                "INSERT INTO messages \
                 (id, session_id, role, content, completion_state, created_at) \
                 VALUES (?,'s1',?,'content',?,?)",
            )
            .bind(id)
            .bind(role)
            .bind(state)
            .bind(created_at)
            .execute(&db)
            .await
            .unwrap();
        }
        for (id, message_id, created_at) in [
            ("tool-1", "assistant-1", 2_i64),
            ("tool-2", "assistant-2", 4),
            ("tool-3", "assistant-3", 7),
        ] {
            sqlx::query(
                "INSERT INTO tool_calls \
                 (id, message_id, tool_name, arguments, status, created_at) \
                 VALUES (?,?,'bash','{}','done',?)",
            )
            .bind(id)
            .bind(message_id)
            .bind(created_at)
            .execute(&db)
            .await
            .unwrap();
        }
        for root_turn_id in ["root-1", "root-2"] {
            sqlx::query(
                "INSERT INTO chat_turn_state \
                 (root_turn_id, revision, phase, status, updated_at) \
                 VALUES (?,1,'working','active',1)",
            )
            .bind(root_turn_id)
            .execute(&db)
            .await
            .unwrap();
        }
        let p = SqlitePersistence {
            db,
            session_id: "s1".into(),
            anonymous: false,
        };

        assert_eq!(p.root_turn_tool_call_count("root-1").await.unwrap(), 2);
        assert_eq!(p.root_turn_tool_call_count("root-2").await.unwrap(), 1);
        assert_eq!(p.root_turn_tool_call_count("missing").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn recoverable_tool_failure_reopens_a_stale_blocked_segment() {
        let db = pool().await;
        sqlx::query(
            "INSERT INTO chat_task_segments (id, status, updated_at) \
             VALUES ('segment-1','blocked',1)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state \
             (root_turn_id, task_segment_id, revision, phase, status, updated_at, completed_at, terminal_reason) \
             VALUES ('root-1','segment-1',1,'working','blocked',1,1,'tool_error')",
        )
        .execute(&db)
        .await
        .unwrap();
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };

        p.update_turn_activity(&TurnActivityUpdate {
            root_turn_id: "root-1".into(),
            phase: "working".into(),
            status: "active".into(),
            recent_activity_kind: "tool_failed".into(),
            recent_activity_label: "长时工具执行失败，正在尝试恢复".into(),
            waiting_reason: None,
            terminal_reason: None,
        })
        .await
        .unwrap();

        let turn: (String, Option<i64>, Option<String>) = sqlx::query_as(
            "SELECT status, completed_at, terminal_reason FROM chat_turn_state \
             WHERE root_turn_id='root-1'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        let segment: String =
            sqlx::query_scalar("SELECT status FROM chat_task_segments WHERE id='segment-1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(turn, ("active".into(), None, None));
        assert_eq!(segment, "active");
    }

    async fn assert_system_owned_failure_does_not_terminalize_objective(status: &str) {
        let db = pool().await;
        sqlx::query(
            "INSERT INTO chat_task_segments (id, status, updated_at) \
             VALUES ('segment-1', 'active', 1)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state \
             (root_turn_id, task_segment_id, revision, phase, status, updated_at) \
             VALUES ('root-1', 'segment-1', 1, 'working', 'active', 1)",
        )
        .execute(&db)
        .await
        .unwrap();
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };

        p.update_turn_activity(&TurnActivityUpdate {
            root_turn_id: "root-1".into(),
            phase: "finalizing".into(),
            status: status.into(),
            recent_activity_kind: status.into(),
            recent_activity_label: "系统恢复仍由 CodeFactory 持有".into(),
            waiting_reason: Some("等待持久恢复器接管".into()),
            terminal_reason: Some("technical_recovery_exhausted".into()),
        })
        .await
        .unwrap();

        let turn: (String, Option<i64>, Option<String>) = sqlx::query_as(
            "SELECT status, completed_at, terminal_reason FROM chat_turn_state \
             WHERE root_turn_id='root-1'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        let objective_status: String =
            sqlx::query_scalar("SELECT status FROM chat_task_segments WHERE id='segment-1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(turn.0, status, "the current transport turn may settle");
        assert!(turn.1.is_some(), "the settled turn keeps its own timestamp");
        assert_eq!(turn.2.as_deref(), Some("technical_recovery_exhausted"));
        assert_eq!(
            objective_status, "active",
            "{status} is system-owned recovery, not an objective terminal state"
        );
    }

    #[tokio::test]
    async fn failed_internal_does_not_terminalize_the_objective() {
        assert_system_owned_failure_does_not_terminalize_objective("failed_internal").await;
    }

    #[tokio::test]
    async fn platform_incident_does_not_terminalize_the_objective() {
        assert_system_owned_failure_does_not_terminalize_objective("platform_incident").await;
    }

    #[tokio::test]
    async fn completed_transport_turn_does_not_complete_the_objective_without_arbitration() {
        let db = pool().await;
        sqlx::query(
            "INSERT INTO chat_task_segments (id, status, updated_at) \
             VALUES ('segment-1', 'active', 1)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state \
             (root_turn_id, task_segment_id, revision, phase, status, updated_at) \
             VALUES ('root-1', 'segment-1', 1, 'working', 'active', 1)",
        )
        .execute(&db)
        .await
        .unwrap();
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };

        p.update_turn_activity(&TurnActivityUpdate {
            root_turn_id: "root-1".into(),
            phase: "finalizing".into(),
            status: "completed".into(),
            recent_activity_kind: "completed".into(),
            recent_activity_label: "transport Done".into(),
            waiting_reason: None,
            terminal_reason: None,
        })
        .await
        .unwrap();

        let turn: (String, Option<i64>) = sqlx::query_as(
            "SELECT status, completed_at FROM chat_turn_state WHERE root_turn_id='root-1'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        let objective_status: String =
            sqlx::query_scalar("SELECT status FROM chat_task_segments WHERE id='segment-1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(turn.0, "completed");
        assert!(turn.1.is_some(), "the transport turn itself is settled");
        assert_eq!(
            objective_status, "active",
            "Done/RunOutcome must not bypass the objective completion arbiter"
        );
    }

    #[tokio::test]
    async fn turn_notice_is_persisted_as_internal_system_provenance() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };
        p.persist_gate_message("runtime correction", "turn_notice")
            .await
            .unwrap();
        let (role, state): (String, String) =
            sqlx::query_as("SELECT role, completion_state FROM messages WHERE session_id='s1'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(role, "system");
        assert_eq!(state, "turn_notice");
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
            p.persist_message(
                "assistant",
                "secret",
                Some(1),
                Some(2),
                None,
                None,
                Some("chatgpt"),
                Some("gpt-5.5"),
                Some("r"),
            )
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

    #[tokio::test]
    async fn assistant_message_persists_the_effective_endpoint_and_model() {
        let db = pool().await;
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };
        p.persist_message(
            "assistant",
            "ok",
            Some(11),
            Some(7),
            None,
            None,
            Some("deepseek"),
            Some("deepseek-v4-pro"),
            Some("request-1"),
        )
        .await
        .unwrap();

        let actual: (String, String) =
            sqlx::query_as("SELECT endpoint_id, model_id FROM messages LIMIT 1")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(actual, ("deepseek".into(), "deepseek-v4-pro".into()));
    }

    #[tokio::test]
    async fn route_attempt_journal_contains_provenance_but_no_credentials() {
        let db = pool().await;
        let attempts = vec![
            super::super::failover::RouteAttemptSnapshot {
                endpoint: "chatgpt".into(),
                model: "gpt-5.5".into(),
                status: "failed".into(),
                failure_code: Some("AUTH_EXPIRED".into()),
            },
            super::super::failover::RouteAttemptSnapshot {
                endpoint: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                status: "succeeded".into(),
                failure_code: None,
            },
        ];
        persist_model_route_attempts(&db, "s1", "user-1", "prefer", &attempts, true, false)
            .await
            .unwrap();

        let rows: Vec<(String, String, String, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT endpoint, model, status, failure_code,
                    output_started, side_effect_started
             FROM model_route_attempts ORDER BY rowid",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].3.as_deref(), Some("AUTH_EXPIRED"));
        assert_eq!(rows[1].0, "deepseek");
        assert_eq!(rows[1].4, 1);
        assert_eq!(rows[1].5, 0);
        let serialized = serde_json::to_string(&rows).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("api_key"));
    }

    /// The session list answers "where was I working", so its timestamp and its
    /// order must follow the last real message.
    ///
    /// Display and sorting already read `updated_at`
    /// (`SessionSidebar.tsx`, `ORDER BY updated_at DESC`,
    /// `groupSessionsByProject`), but nothing advanced it when messages
    /// arrived — only changing the model or setting the title did, and the
    /// title is generated once at the start. So it froze at roughly creation
    /// time and a two-hour conversation still showed its opening minute while
    /// sinking below idle sessions.
    #[tokio::test]
    async fn persisting_a_message_advances_the_session_activity_time() {
        let db = pool().await;
        sqlx::query("INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 100, 100)")
            .execute(&db)
            .await
            .unwrap();
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: false,
        };

        p.persist_message("user", "hello", None, None, None, None, None, None, None)
            .await
            .unwrap();

        let updated: i64 = sqlx::query_scalar("SELECT updated_at FROM sessions WHERE id='s1'")
            .fetch_one(&db)
            .await
            .unwrap();
        assert!(
            updated > 100,
            "a real message must move the session to the top of the list, got {updated}"
        );
    }

    #[tokio::test]
    async fn an_anonymous_session_still_leaves_no_trace() {
        let db = pool().await;
        sqlx::query("INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 100, 100)")
            .execute(&db)
            .await
            .unwrap();
        let p = SqlitePersistence {
            db: db.clone(),
            session_id: "s1".into(),
            anonymous: true,
        };

        p.persist_message("user", "hello", None, None, None, None, None, None, None)
            .await
            .unwrap();

        let updated: i64 = sqlx::query_scalar("SELECT updated_at FROM sessions WHERE id='s1'")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(
            updated, 100,
            "anonymous sessions leave no trace by definition"
        );
    }

    #[tokio::test]
    async fn provider_recovery_attempt_persists_stable_code_and_objective_owner() {
        let db = pool().await;
        sqlx::query(
            "INSERT INTO chat_task_segments(id, previous_segment_id, status, updated_at)
             VALUES ('objective', NULL, 'active', 1)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, task_segment_id, objective_id, revision, recovery_attempt, phase, status, updated_at)
             VALUES ('turn', 'objective', 'objective-opaque-uuid', 1, 0, 'executing', 'active', 1)",
        )
        .execute(&db)
        .await
        .unwrap();
        let persistence = SqlitePersistence {
            db: db.clone(),
            session_id: "session".into(),
            anonymous: false,
        };
        persistence
            .record_recovery_attempt(&RecoveryAttemptRow {
                root_turn_id: "turn".into(),
                domain: "provider".into(),
                attempt_index: 1,
                failure_code: "PROVIDER_OVERLOADED".into(),
                failure_class: "transient_provider".into(),
                output_started: false,
                side_effect_started: false,
                terminal_decision: "continue".into(),
            })
            .await
            .unwrap();

        let row: (String, String, String, i64, i64, String) = sqlx::query_as(
            "SELECT objective_id, failure_code, failure_class,
                    output_started, side_effect_started, terminal_decision
             FROM objective_recovery_attempts",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(row.0, "objective-opaque-uuid");
        assert_eq!(row.1, "PROVIDER_OVERLOADED");
        assert_eq!(row.2, "transient_provider");
        assert_eq!((row.3, row.4), (0, 0));
        assert_eq!(row.5, "continue");
        let attempts: i64 = sqlx::query_scalar(
            "SELECT recovery_attempt FROM chat_turn_state WHERE root_turn_id='turn'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn recovery_attempt_refuses_to_invent_an_objective_identity() {
        let db = pool().await;
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, task_segment_id, objective_id, revision, recovery_attempt, phase, status, updated_at)
             VALUES ('legacy-turn', NULL, NULL, 1, 0, 'executing', 'active', 1)",
        )
        .execute(&db)
        .await
        .unwrap();
        let persistence = SqlitePersistence {
            db: db.clone(),
            session_id: "session".into(),
            anonymous: false,
        };
        let error = persistence
            .record_recovery_attempt(&RecoveryAttemptRow {
                root_turn_id: "legacy-turn".into(),
                domain: "provider".into(),
                attempt_index: 1,
                failure_code: "PROVIDER_OVERLOADED".into(),
                failure_class: "transient_provider".into(),
                output_started: false,
                side_effect_started: false,
                terminal_decision: "continue".into(),
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("objective identity"));
        let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objective_recovery_attempts")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(attempts, 0);
    }
}
