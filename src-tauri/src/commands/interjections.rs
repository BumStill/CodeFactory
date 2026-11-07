// SPDX-License-Identifier: Apache-2.0
//! Mid-execution interjections — short user notes the scheduler picks up
//! before dispatching the **next** pending task. Honest scope: we can't
//! safely surgery into a sub-agent that's already mid-tool-call, but
//! redirecting before the next task starts catches most "wait, change X"
//! moments cheaply and predictably.
//!
//! Queue lives in-memory per session — interjections are transient by
//! design. The scheduler drains the queue when it begins dispatching a
//! task and appends the notes to that task's `SubagentBrief.parent_summary`.
//!
//! If you find yourself wanting durable interjections, write them to
//! memory.md or as a preference instead — that's the persistent channel.

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{command, State};
use tokio::sync::Mutex;

use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interjection {
    /// Durable row id (empty for entries that only ever lived in memory).
    #[serde(default)]
    pub id: String,
    pub session_id: String,
    pub message: String,
    pub at: i64, // unix ms
}

/// One session's steering state.
///
/// `delivered_ids` is CF-STOP-R4 (M63) idempotency: a client message id that has
/// already been handed to this session can never be handed over a second time.
/// M63's shape was: the loop was parked inside a tool call, the steer sat
/// unread, the user stopped the session, and the continuation re-sent the same
/// message — the same line landed twice. Identity now travels with the message
/// instead of being inferred from its text.
#[derive(Debug, Default)]
pub struct SessionInterjections {
    pub pending: Vec<Interjection>,
    pub delivered_ids: std::collections::HashSet<String>,
}

/// session_id → pending interjections + the ids already delivered.
pub type InterjectionQueue = Arc<Mutex<HashMap<String, SessionInterjections>>>;

// ---------------------------------------------------------------------------
// M261 — the durable half.
//
// Before this, a steer lived ONLY in `InterjectionQueue`: an in-memory map that
// the agent loop drains at its round top. A turn that spends its whole life
// inside one round — approval waits plus tool execution, exactly the 18:15 run
// in the 2026-11-07 report — never reaches that boundary, so the correction was
// never persisted, never shown anywhere, and died with the process when the run
// was stopped.
//
// Every steer is now a row in `pending_steer_messages` *before* the command
// answers. The row is the record of the promise: `pending` means "accepted, not
// yet handed to a model call", `delivered` means a run read it. A run that dies
// leaves `pending` rows behind, and the next turn's inbox recovers them, so a
// steer can no longer disappear into RAM.
// ---------------------------------------------------------------------------

pub async fn ensure_steer_schema(db: &SqlitePool) -> Result<(), AppError> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS pending_steer_messages (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            content TEXT NOT NULL,
            client_message_id TEXT,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            delivered_at INTEGER
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_pending_steer_client_message
            ON pending_steer_messages(session_id, client_message_id)
            WHERE client_message_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_pending_steer_pending
            ON pending_steer_messages(session_id, status, created_at);",
    )
    .execute(db)
    .await?;
    Ok(())
}

fn row_to_interjection(row: &sqlx::sqlite::SqliteRow) -> Interjection {
    Interjection {
        id: row.get("id"),
        session_id: row.get("session_id"),
        message: row.get("content"),
        at: row.get("created_at"),
    }
}

/// Persist a steer that must reach this session's next model call.
///
/// Returns `None` when this exact client message id was already accepted for
/// the session — the durable twin of the in-memory idempotency rule.
pub async fn persist_steer(
    db: &SqlitePool,
    session_id: &str,
    id: &str,
    message: &str,
    client_message_id: Option<&str>,
) -> Result<Option<Interjection>, AppError> {
    ensure_steer_schema(db).await?;
    let at = chrono::Utc::now().timestamp_millis();
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO pending_steer_messages \
         (id, session_id, content, client_message_id, status, created_at) \
         VALUES (?, ?, ?, ?, 'pending', ?)",
    )
    .bind(id)
    .bind(session_id)
    .bind(message.trim())
    .bind(client_message_id)
    .bind(at)
    .execute(db)
    .await?;
    if inserted.rows_affected() == 0 {
        return Ok(None);
    }
    Ok(Some(Interjection {
        id: id.to_string(),
        session_id: session_id.to_string(),
        message: message.trim().to_string(),
        at,
    }))
}

/// Steers this session still owes a model call, oldest first. Recovered rows
/// are exactly the ones a dead or still-parked run never read.
pub async fn unrecovered_steers(
    db: &SqlitePool,
    session_id: &str,
) -> Result<Vec<Interjection>, AppError> {
    ensure_steer_schema(db).await?;
    let rows = sqlx::query(
        "SELECT * FROM pending_steer_messages \
         WHERE session_id=? AND status='pending' ORDER BY created_at, id",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(row_to_interjection).collect())
}

/// Hand-over confirmation: the text is now part of the session's model history.
pub async fn mark_steer_delivered(db: &SqlitePool, id: &str) -> Result<(), AppError> {
    ensure_steer_schema(db).await?;
    sqlx::query(
        "UPDATE pending_steer_messages SET status='delivered', delivered_at=? \
         WHERE id=? AND status='pending'",
    )
    .bind(chrono::Utc::now().timestamp_millis())
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// `true` = queued, `false` = this exact client message was already accepted
/// for this session (a duplicate must not be handed over twice).
#[command]
pub async fn queue_interjection(
    session_id: String,
    message: String,
    client_message_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<bool, AppError> {
    let db = state.db.read().await.clone();
    if crate::agent::is_delivery_revocation(&message) {
        sqlx::query("UPDATE sessions SET delivery_authorized=0, delivery_ceiling=NULL WHERE id=?")
            .bind(&session_id).execute(&db).await?;
    }
    enqueue_durable_steer(
        &db,
        &state.interjections,
        &session_id,
        &message,
        client_message_id.as_deref(),
    )
    .await
}

/// The command's rule, split out so it is testable without `AppState`:
/// commit the durable row first, mirror it into the in-memory queue second.
/// A failed write is a failed accept — a caller must never be told `queued`
/// for a message that only exists in RAM.
pub async fn enqueue_durable_steer(
    db: &SqlitePool,
    queue: &InterjectionQueue,
    session_id: &str,
    message: &str,
    client_message_id: Option<&str>,
) -> Result<bool, AppError> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err(AppError::Other("interjection cannot be empty".into()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let Some(row) = persist_steer(db, session_id, &id, trimmed, client_message_id).await? else {
        return Ok(false);
    };
    enqueue_item(queue, session_id, row).await?;
    Ok(true)
}

/// The queueing rule itself, free of `AppState` so it stays directly testable.
/// Memory-only variant: used by the scheduler's own drain consumer.
pub async fn enqueue_interjection(
    queue: &InterjectionQueue,
    session_id: &str,
    message: &str,
    client_message_id: Option<&str>,
) -> Result<bool, AppError> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err(AppError::Other("interjection cannot be empty".into()));
    }
    let entry = Interjection {
        id: String::new(),
        session_id: session_id.to_string(),
        message: trimmed.into(),
        at: chrono::Utc::now().timestamp_millis(),
    };
    if let Some(id) = client_message_id.map(str::trim).filter(|id| !id.is_empty()) {
        let mut q = queue.lock().await;
        let session = q.entry(session_id.to_string()).or_default();
        if !session.delivered_ids.insert(id.to_string()) {
            return Ok(false);
        }
        session.pending.push(entry);
        return Ok(true);
    }
    enqueue_item(queue, session_id, entry).await
}

/// Push one already-built entry into a session's in-memory queue, with the
/// M63 idempotency rule applied to its durable id.
async fn enqueue_item(
    queue: &InterjectionQueue,
    session_id: &str,
    entry: Interjection,
) -> Result<bool, AppError> {
    let mut q = queue.lock().await;
    let session = q.entry(session_id.to_string()).or_default();
    if !entry.id.is_empty() && !session.delivered_ids.insert(entry.id.clone()) {
        return Ok(false);
    }
    session.pending.push(entry);
    Ok(true)
}

/// Drain (return + clear) the pending queue for a session. Used by the
/// scheduler at the start of each task dispatch. The delivered-id set stays:
/// draining means "the message was handed over", not "it may come again".
pub async fn drain_for_session(
    queue: &InterjectionQueue,
    session_id: &str,
) -> Vec<Interjection> {
    let mut q = queue.lock().await;
    let Some(session) = q.get_mut(session_id) else {
        return Vec::new();
    };
    std::mem::take(&mut session.pending)
}

/// The chat loop's view of the same queue. Both consumers drain at their own
/// safe boundary — the scheduler before dispatching the next task, the agent
/// loop before its next model request — so one `queue_interjection` call means
/// the same thing to the user no matter which is running.
pub struct SessionSteerInbox {
    pub queue: InterjectionQueue,
    pub session_id: String,
    /// The session's standing delivery grant, read once when the loop starts.
    /// `capability_override` is synchronous, so it cannot query the row itself,
    /// and the flag is settled at turn entry before the loop is built.
    pub delivery_authorized: bool,
    /// M261: the durable record. Rows still `pending` for this session are ones
    /// an earlier (or still-parked) run never read; they are recovered here so a
    /// steer survives the round that never ended and the process that died.
    pub db: Option<SqlitePool>,
}

#[async_trait::async_trait]
impl codefactory_agent_loop::services::SteerInbox for SessionSteerInbox {
    async fn drain(&self) -> Vec<String> {
        let mut texts: Vec<(String, String)> = Vec::new();
        if let Some(db) = &self.db {
            match unrecovered_steers(db, &self.session_id).await {
                Ok(rows) => texts.extend(rows.into_iter().map(|row| (row.id, row.message))),
                Err(error) => tracing::warn!(
                    "steer recovery failed for session {}: {}",
                    self.session_id,
                    error
                ),
            }
        }
        for item in drain_for_session(&self.queue, &self.session_id).await {
            if item.id.is_empty() || !texts.iter().any(|(id, _)| *id == item.id) {
                texts.push((item.id, item.message));
            }
        }
        if let Some(db) = &self.db {
            for (id, _) in &texts {
                if !id.is_empty() {
                    if let Err(error) = mark_steer_delivered(db, id).await {
                        tracing::warn!("steer {} delivered but not marked: {}", id, error);
                    }
                }
            }
        }
        texts.into_iter().map(|(_, message)| message).collect()
    }

    fn capability_override(
        &self,
        content: &str,
    ) -> Option<codefactory_agent_loop::run::TurnCapability> {
        crate::agent::steer_capability_override_with_authorization(
            content,
            self.delivery_authorized,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_clears_queue() {
        let q: InterjectionQueue = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut g = q.lock().await;
            g.entry("s1".into()).or_default().pending.push(Interjection {
                id: String::new(),
                session_id: "s1".into(),
                message: "hi".into(),
                at: 0,
            });
        }
        let drained = drain_for_session(&q, "s1").await;
        assert_eq!(drained.len(), 1);
        let drained_again = drain_for_session(&q, "s1").await;
        assert!(drained_again.is_empty());
    }

    /// CF-STOP-R4 最低证据（幂等的一半）：同一条客户端消息 id 只会被交付一次——
    /// 即使它在被读到之前又被送了一次，也只会落一条。
    #[tokio::test]
    async fn the_same_client_message_is_never_handed_over_twice() {
        let q: InterjectionQueue = Arc::new(Mutex::new(HashMap::new()));
        assert!(enqueue_interjection(&q, "s1", "改用 chrome channel", Some("m-1"))
            .await
            .unwrap());
        assert!(
            !enqueue_interjection(&q, "s1", "改用 chrome channel", Some("m-1"))
                .await
                .unwrap(),
            "同一条消息第二次必须被拒绝"
        );
        // 换一条消息（新 id）当然可以进。
        assert!(enqueue_interjection(&q, "s1", "然后跑测试", Some("m-2"))
            .await
            .unwrap());

        let drained = drain_for_session(&q, "s1").await;
        assert_eq!(
            drained.iter().map(|i| i.message.as_str()).collect::<Vec<_>>(),
            vec!["改用 chrome channel", "然后跑测试"],
            "重复的一条只能落一次"
        );

        // 「读到之后又续跑」也不能把同一条再送一次：id 集合不随 drain 清空。
        assert!(
            !enqueue_interjection(&q, "s1", "改用 chrome channel", Some("m-1"))
                .await
                .unwrap(),
            "已经交付过的消息在续跑时仍然必须被拒绝"
        );
        assert!(drain_for_session(&q, "s1").await.is_empty());
        // 另一个会话不受影响：id 是会话内身份，不是全局身份。
        assert!(enqueue_interjection(&q, "s2", "改用 chrome channel", Some("m-1"))
            .await
            .unwrap());
    }

    /// The inbox is where the 2026-09-08 regression actually bit: it asked
    /// "what does this message mean on its own" and threw away what the
    /// session already held, so `继续` demoted a delivery-authorized session to
    /// local-only work and every commit, PR and release was then refused.
    #[tokio::test]
    async fn a_continuation_steer_does_not_revoke_the_sessions_delivery_grant() {
        use codefactory_agent_loop::services::SteerInbox;
        let authorized = SessionSteerInbox {
            queue: Arc::new(Mutex::new(HashMap::new())),
            session_id: "s1".into(),
            delivery_authorized: true,
            db: None,
        };
        assert_eq!(
            authorized.capability_override("继续"),
            None,
        );
        let unauthorized = SessionSteerInbox {
            queue: Arc::new(Mutex::new(HashMap::new())),
            session_id: "s1".into(),
            delivery_authorized: false,
            db: None,
        };
        assert_eq!(
            unauthorized.capability_override("继续"),
            None,
        );
    }

    /// M261 回归：会话一个回合长时间卡在审批/工具里时，steer 只会进内存队列，
    /// 而队列只在回合顶部被读。这一回合没走到顶部，消息就没有任何记录——
    /// 停止后再发才通。这里断言：接受到的消息先落库，新的 inbox 能把它捡回来，
    /// 且只交付一次。
    #[tokio::test]
    async fn a_steer_survives_the_round_that_never_read_it() {
        use codefactory_agent_loop::services::SteerInbox;

        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let queue: InterjectionQueue = Arc::new(Mutex::new(HashMap::new()));

        // 18:15：发给一个正在跑的回合。没有人读它，回合随后被停止。
        assert!(enqueue_durable_steer(&pool, &queue, "s1", "改用 chrome channel", Some("m-1"))
            .await
            .unwrap());
        assert!(
            !enqueue_durable_steer(&pool, &queue, "s1", "改用 chrome channel", Some("m-1"))
                .await
                .unwrap(),
            "同一个 client_message_id 不能被接受两次"
        );

        // 内存队列被丢弃（进程重启 / 停止那一回合），库里仍然欠着这条消息。
        let fresh_queue: InterjectionQueue = Arc::new(Mutex::new(HashMap::new()));
        let inbox = SessionSteerInbox {
            queue: fresh_queue.clone(),
            session_id: "s1".into(),
            delivery_authorized: true,
            db: Some(pool.clone()),
        };
        assert_eq!(inbox.drain().await, vec!["改用 chrome channel".to_string()]);

        // 交给模型之后就是 delivered：下一个回合不能再重复注入。
        let again = SessionSteerInbox {
            queue: fresh_queue,
            session_id: "s1".into(),
            delivery_authorized: true,
            db: Some(pool.clone()),
        };
        assert!(again.drain().await.is_empty());
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pending_steer_messages WHERE session_id='s1' AND status='pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending, 0);
    }

    /// 状态必须是可查的：接受之后、被读走之前，行留在库里的 `pending`，
    /// 而不是只在内存里"说已排队"。
    #[tokio::test]
    async fn an_accepted_steer_is_visible_as_pending_before_any_run_reads_it() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let queue: InterjectionQueue = Arc::new(Mutex::new(HashMap::new()));
        enqueue_durable_steer(&pool, &queue, "s1", "先别改 UI", None)
            .await
            .unwrap();
        let rows = unrecovered_steers(&pool, "s1").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message, "先别改 UI");
        assert!(unrecovered_steers(&pool, "other").await.unwrap().is_empty());
    }
}
