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
    /// Per-session endpoint binding. Legacy rows can remain unresolved until
    /// the user chooses an endpoint; new sessions always persist one.
    #[serde(default)]
    #[sqlx(default)]
    pub endpoint_id: Option<String>,
    /// fixed | prefer | auto. Legacy sessions migrate to fixed so an upgrade
    /// never starts sending their history to a different provider.
    #[serde(default = "default_model_policy")]
    #[sqlx(default)]
    pub model_policy: String,
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

fn default_session_kind() -> String {
    "project".into()
}
fn default_model_policy() -> String {
    "fixed".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub endpoint_id: Option<String>,
    pub model_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// Serialised `Vec<ToolCall>` JSON; only set on assistant messages that invoked tools.
    pub tool_calls: Option<String>,
    /// Reasoning trace from thinking-mode models (DeepSeek reasoner, etc).
    /// Must be replayed back to the API on subsequent turns or the provider
    /// rejects the request with HTTP 400.
    pub reasoning_content: Option<String>,
    /// Completion-gate provenance. Internal review rows are persisted for
    /// UI recovery and forensics, but are excluded when a later user turn
    /// rebuilds provider history. NULL for ordinary messages.
    pub completion_state: Option<String>,
    pub created_at: i64,
}

/// Session history as the *agent* should see it, ordered oldest-first.
///
/// Identical to the transcript except for one exclusion: assistant turns the
/// completion gate rejected as premature final answers. Those are real model
/// output and stay visible to the user, but replaying them would feed the model
/// its own withdrawn "I'm done" claims on every later turn — one long session
/// had 48 of them.
///
/// Two mechanisms, because the marker moved: current builds annotate the
/// rejection in `gate_events`, while databases written before that carry
/// `completion_state='rejected_candidate'` on the message itself and are
/// filtered downstream by `replayable_history`. Nothing backfills, so both
/// paths have to keep working.
///
/// UI reads (`get_messages`, `get_message_page`) deliberately do NOT use this —
/// the transcript shows everything the agent actually did.
pub async fn load_agent_history(
    pool: &sqlx::SqlitePool,
    session_id: &str,
) -> Result<Vec<Message>, sqlx::Error> {
    sqlx::query_as::<_, Message>(
        "SELECT m.* FROM messages m \
         WHERE m.session_id = ? \
           AND NOT EXISTS ( \
             SELECT 1 FROM gate_events g \
             WHERE g.message_id = m.id AND g.kind = 'rejected_candidate' \
           ) \
         ORDER BY m.created_at ASC, m.rowid ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
}

// Typed `FromRow` model for the normalized tool lifecycle truth source.
// `messages.tool_calls` remains as a redacted provider-history representation;
// extraction, evidence, and cross-session analysis read this table instead.
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;

    async fn pool() -> SqlitePool {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT, role TEXT, \
             content TEXT, endpoint_id TEXT, model_id TEXT, input_tokens INTEGER, output_tokens INTEGER, \
             tool_calls TEXT, reasoning_content TEXT, completion_state TEXT, created_at INTEGER)",
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
        db
    }

    async fn message(db: &SqlitePool, id: &str, role: &str, content: &str, at: i64) {
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,?,?,?,?)",
        )
        .bind(id)
        .bind("s1")
        .bind(role)
        .bind(content)
        .bind(at)
        .execute(db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn agent_history_drops_gate_rejected_drafts_but_keeps_everything_else() {
        let db = pool().await;
        message(&db, "u1", "user", "do the thing", 1).await;
        message(&db, "a1", "assistant", "premature done claim", 2).await;
        message(&db, "t1", "tool", "probe output", 3).await;
        message(&db, "a2", "assistant", "actually done, verified", 4).await;
        sqlx::query(
            "INSERT INTO gate_events (id, session_id, kind, content, message_id, created_at) \
             VALUES ('g1','s1','rejected_candidate','','a1',2)",
        )
        .execute(&db)
        .await
        .unwrap();

        let history = load_agent_history(&db, "s1").await.unwrap();
        assert_eq!(
            history.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["u1", "t1", "a2"],
        );
    }

    #[tokio::test]
    async fn a_non_rejection_gate_event_never_hides_a_message() {
        let db = pool().await;
        message(&db, "u1", "user", "do the thing", 1).await;
        message(&db, "a1", "assistant", "done", 2).await;
        // Recovery/ready rows carry no message_id; a NULL must not match.
        sqlx::query(
            "INSERT INTO gate_events (id, session_id, kind, content, message_id, created_at) \
             VALUES ('g1','s1','gate_recovery','verify first',NULL,2)",
        )
        .execute(&db)
        .await
        .unwrap();

        let history = load_agent_history(&db, "s1").await.unwrap();
        assert_eq!(
            history.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["u1", "a1"],
        );
    }

    #[tokio::test]
    async fn one_session_rejection_cannot_hide_another_sessions_message() {
        let db = pool().await;
        message(&db, "u1", "user", "do the thing", 1).await;
        message(&db, "a1", "assistant", "done", 2).await;
        sqlx::query(
            "INSERT INTO gate_events (id, session_id, kind, content, message_id, created_at) \
             VALUES ('g1','other-session','rejected_candidate','','a1',2)",
        )
        .execute(&db)
        .await
        .unwrap();

        // The anti-join is by message id, which is globally unique — a stray
        // row naming this id still refers to this message, so it applies.
        let history = load_agent_history(&db, "s1").await.unwrap();
        assert_eq!(
            history.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["u1"]
        );
    }
}
