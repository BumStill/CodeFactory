// SPDX-License-Identifier: Apache-2.0
//! M236: durable queue for sends that arrive while a turn is already running.
//!
//! ## Why this exists
//!
//! `dispatch send` with `mode: "queue"` used to answer `status: "queued"`
//! while only writing into `runtime[sessionId].queue` — an in-memory array in
//! the renderer. Nothing was persisted and nothing but the *same* renderer
//! observing a `turn_settled` event for that session ever drained it. On the
//! 2026-11-05 incident the turn ended through a scheduled wake-up path, that
//! drain never fired, and the message disappeared with no error anywhere.
//!
//! The queue is now a row set in the app database:
//!
//! * `queued`     — accepted, must be delivered
//! * `delivering` — handed to the renderer, delivery not yet confirmed
//! * `delivered`  — the message started a real turn
//! * `failed`     — terminal, with a human-readable reason
//!
//! Claiming is atomic (`UPDATE ... WHERE status='queued'`), so two drains can
//! race without delivering twice, and a restart re-reads the same rows instead
//! of losing them. `root_turn_id` is minted at enqueue time, which keeps the
//! exact-once identity of the message across process restarts.

use serde::Serialize;
use sqlx::{Row, SqlitePool};
use tauri::{command, State};

use crate::errors::AppError;
use crate::AppState;

/// Same cap the UI enforces, applied server-side so the durable queue cannot
/// grow without bound when a caller enqueues in a loop.
pub const QUEUED_DISPATCH_MAX: i64 = 5;

/// A `delivering` row older than this is treated as an interrupted hand-off and
/// returned to `queued` instead of being stranded.
pub const DELIVERING_STALE_MS: i64 = 300_000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueuedDispatchMessage {
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub root_turn_id: String,
    pub status: String,
    pub attempts: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub delivered_at: Option<i64>,
    pub failure_reason: Option<String>,
    /// 1-based position inside the pending queue; `None` once terminal.
    pub position: Option<i64>,
}

pub async fn ensure_schema(db: &SqlitePool) -> Result<(), AppError> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS queued_dispatch_messages (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            content TEXT NOT NULL,
            root_turn_id TEXT NOT NULL,
            status TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            delivered_at INTEGER,
            failure_reason TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_queued_dispatch_pending
            ON queued_dispatch_messages(session_id, status, created_at);",
    )
    .execute(db)
    .await?;
    Ok(())
}

fn row_to_message(row: &sqlx::sqlite::SqliteRow, position: Option<i64>) -> QueuedDispatchMessage {
    QueuedDispatchMessage {
        id: row.get("id"),
        session_id: row.get("session_id"),
        content: row.get("content"),
        root_turn_id: row.get("root_turn_id"),
        status: row.get("status"),
        attempts: row.get("attempts"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        delivered_at: row.get::<Option<i64>, _>("delivered_at"),
        failure_reason: row.get::<Option<String>, _>("failure_reason"),
        position,
    }
}

/// Persist a message that must be delivered after the running turn.
///
/// Returns only after the row is committed, so a `queued` answer from
/// `dispatch send` is a durability promise rather than a memory promise.
pub async fn enqueue(
    db: &SqlitePool,
    session_id: &str,
    content: &str,
    root_turn_id: Option<&str>,
) -> Result<QueuedDispatchMessage, AppError> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(AppError::Other("queued message cannot be empty".into()));
    }
    ensure_schema(db).await?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM queued_dispatch_messages \
         WHERE session_id=? AND status IN ('queued','delivering')",
    )
    .bind(session_id)
    .fetch_one(db)
    .await?;
    if pending >= QUEUED_DISPATCH_MAX {
        return Err(AppError::Other(QUEUE_FULL.to_string()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let root_turn_id = root_turn_id
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let at = now_ms();
    sqlx::query(
        "INSERT INTO queued_dispatch_messages \
         (id, session_id, content, root_turn_id, status, attempts, created_at, updated_at) \
         VALUES (?, ?, ?, ?, 'queued', 0, ?, ?)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(trimmed)
    .bind(&root_turn_id)
    .bind(at)
    .bind(at)
    .execute(db)
    .await?;
    Ok(QueuedDispatchMessage {
        id,
        session_id: session_id.to_string(),
        content: trimmed.to_string(),
        root_turn_id,
        status: "queued".into(),
        attempts: 0,
        created_at: at,
        updated_at: at,
        delivered_at: None,
        failure_reason: None,
        position: Some(pending + 1),
    })
}

pub async fn pending(
    db: &SqlitePool,
    session_id: &str,
) -> Result<Vec<QueuedDispatchMessage>, AppError> {
    ensure_schema(db).await?;
    let rows = sqlx::query(
        "SELECT * FROM queued_dispatch_messages \
         WHERE session_id=? AND status IN ('queued','delivering') ORDER BY created_at, id",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .iter()
        .enumerate()
        .map(|(index, row)| row_to_message(row, Some(index as i64 + 1)))
        .collect())
}

/// Take the oldest pending message for this session, or `None` when there is
/// nothing to deliver. Stale hand-offs are returned to `queued` first so a
/// crashed renderer cannot strand a message in `delivering` forever.
pub async fn claim_next(
    db: &SqlitePool,
    session_id: &str,
) -> Result<Option<QueuedDispatchMessage>, AppError> {
    ensure_schema(db).await?;
    let at = now_ms();
    sqlx::query(
        "UPDATE queued_dispatch_messages SET status='queued', updated_at=? \
         WHERE session_id=? AND status='delivering' AND updated_at<?",
    )
    .bind(at)
    .bind(session_id)
    .bind(at - DELIVERING_STALE_MS)
    .execute(db)
    .await?;

    let row = sqlx::query(
        "SELECT * FROM queued_dispatch_messages \
         WHERE session_id=? AND status='queued' ORDER BY created_at, id LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let id: String = row.get("id");
    let claimed = sqlx::query(
        "UPDATE queued_dispatch_messages \
         SET status='delivering', attempts=attempts+1, updated_at=? WHERE id=? AND status='queued'",
    )
    .bind(at)
    .bind(&id)
    .execute(db)
    .await?;
    if claimed.rows_affected() != 1 {
        // Another drain won the race; the caller will be woken by its own turn.
        return Ok(None);
    }
    let mut message = row_to_message(&row, None);
    message.status = "delivering".into();
    message.attempts += 1;
    message.updated_at = at;
    Ok(Some(message))
}

pub async fn mark_delivered(
    db: &SqlitePool,
    session_id: &str,
    id: &str,
) -> Result<QueuedDispatchMessage, AppError> {
    ensure_schema(db).await?;
    let at = now_ms();
    let updated = sqlx::query(
        "UPDATE queued_dispatch_messages \
         SET status='delivered', delivered_at=?, updated_at=?, failure_reason=NULL \
         WHERE id=? AND session_id=? AND status IN ('queued','delivering')",
    )
    .bind(at)
    .bind(at)
    .bind(id)
    .bind(session_id)
    .execute(db)
    .await?;
    if updated.rows_affected() == 0 {
        // Idempotent: already delivered (or already failed) is not an error.
        return status_of(db, session_id, id)
            .await?
            .ok_or_else(|| AppError::Other(format!("queued message {id} does not exist")));
    }
    status_of(db, session_id, id)
        .await?
        .ok_or_else(|| AppError::Other(format!("queued message {id} does not exist")))
}

pub async fn mark_failed(
    db: &SqlitePool,
    session_id: &str,
    id: &str,
    reason: &str,
) -> Result<QueuedDispatchMessage, AppError> {
    ensure_schema(db).await?;
    let at = now_ms();
    sqlx::query(
        "UPDATE queued_dispatch_messages \
         SET status='failed', failure_reason=?, updated_at=? \
         WHERE id=? AND session_id=? AND status IN ('queued','delivering')",
    )
    .bind(reason)
    .bind(at)
    .bind(id)
    .bind(session_id)
    .execute(db)
    .await?;
    status_of(db, session_id, id)
        .await?
        .ok_or_else(|| AppError::Other(format!("queued message {id} does not exist")))
}

/// R4/R6: a session that ended (stopped by the user, or cancelled) must not
/// leave messages silently parked. They become visible terminal failures.
pub async fn fail_pending(
    db: &SqlitePool,
    session_id: &str,
    reason: &str,
) -> Result<Vec<QueuedDispatchMessage>, AppError> {
    ensure_schema(db).await?;
    let rows = pending(db, session_id).await?;
    for row in &rows {
        mark_failed(db, session_id, &row.id, reason).await?;
    }
    Ok(rows)
}

pub async fn status_of(
    db: &SqlitePool,
    session_id: &str,
    id: &str,
) -> Result<Option<QueuedDispatchMessage>, AppError> {
    ensure_schema(db).await?;
    let row = sqlx::query("SELECT * FROM queued_dispatch_messages WHERE id=? AND session_id=?")
        .bind(id)
        .bind(session_id)
        .fetch_optional(db)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let status: String = row.get("status");
    let position = if status == "queued" || status == "delivering" {
        let created_at: i64 = row.get("created_at");
        let ahead: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM queued_dispatch_messages \
             WHERE session_id=? AND status IN ('queued','delivering') AND created_at<?",
        )
        .bind(session_id)
        .bind(created_at)
        .fetch_one(db)
        .await?;
        Some(ahead + 1)
    } else {
        None
    };
    Ok(Some(row_to_message(&row, position)))
}

/// Sessions that still owe a delivery — used on app start so a restart (T3)
/// resumes instead of forgetting.
pub async fn sessions_with_pending(db: &SqlitePool) -> Result<Vec<String>, AppError> {
    ensure_schema(db).await?;
    let rows = sqlx::query(
        "SELECT DISTINCT session_id FROM queued_dispatch_messages \
         WHERE status IN ('queued','delivering')",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.iter().map(|row| row.get("session_id")).collect())
}

/// R3: does this session owe a *fresh* user message? A scheduled wake-up must
/// give way to it instead of talking over the user. Stale rows (older than
/// `window_ms`) stop blocking so a message nobody could deliver never freezes
/// the wake-up schedule forever.
pub async fn has_fresh_pending(
    db: &SqlitePool,
    session_id: &str,
    now_ms: i64,
    window_ms: i64,
) -> bool {
    if ensure_schema(db).await.is_err() {
        return false;
    }
    let fresh: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM queued_dispatch_messages \
         WHERE session_id=? AND status IN ('queued','delivering') AND created_at>=?",
    )
    .bind(session_id)
    .bind(now_ms.saturating_sub(window_ms))
    .fetch_one(db)
    .await
    .unwrap_or(0);
    fresh > 0
}

pub const QUEUE_FULL: &str = "queue is full";

#[command]
pub async fn enqueue_queued_dispatch_message(
    session_id: String,
    content: String,
    root_turn_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<QueuedDispatchMessage, AppError> {
    let db = state.db.read().await.clone();
    enqueue(&db, &session_id, &content, root_turn_id.as_deref()).await
}

#[command]
pub async fn list_queued_dispatch_messages(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<QueuedDispatchMessage>, AppError> {
    let db = state.db.read().await.clone();
    pending(&db, &session_id).await
}

#[command]
pub async fn claim_next_queued_dispatch_message(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Option<QueuedDispatchMessage>, AppError> {
    let db = state.db.read().await.clone();
    claim_next(&db, &session_id).await
}

#[command]
pub async fn mark_queued_dispatch_message_delivered(
    session_id: String,
    id: String,
    state: State<'_, AppState>,
) -> Result<QueuedDispatchMessage, AppError> {
    let db = state.db.read().await.clone();
    mark_delivered(&db, &session_id, &id).await
}

#[command]
pub async fn mark_queued_dispatch_message_failed(
    session_id: String,
    id: String,
    reason: String,
    state: State<'_, AppState>,
) -> Result<QueuedDispatchMessage, AppError> {
    let db = state.db.read().await.clone();
    mark_failed(&db, &session_id, &id, &reason).await
}

/// R4/T5: a session that ended keeps no invisible backlog.
#[command]
pub async fn fail_pending_queued_dispatch_messages(
    session_id: String,
    reason: String,
    state: State<'_, AppState>,
) -> Result<Vec<QueuedDispatchMessage>, AppError> {
    let db = state.db.read().await.clone();
    fail_pending(&db, &session_id, &reason).await
}

/// R5: read-only delivery status for one message accepted by `dispatch send`.
#[command]
pub async fn dispatch_message_status(
    session_id: String,
    message_id: String,
    state: State<'_, AppState>,
) -> Result<Option<QueuedDispatchMessage>, AppError> {
    let db = state.db.read().await.clone();
    status_of(&db, &session_id, &message_id).await
}

#[command]
pub async fn list_sessions_with_queued_dispatch_messages(
    state: State<'_, AppState>,
) -> Result<Vec<String>, AppError> {
    let db = state.db.read().await.clone();
    sessions_with_pending(&db).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn temp_db(dir: &std::path::Path) -> SqlitePool {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("queued.sqlite");
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn enqueue_persists_and_restart_keeps_message() {
        let dir = std::env::temp_dir().join(format!("m236-{}", uuid::Uuid::new_v4()));
        let db = temp_db(&dir).await;
        let queued = enqueue(&db, "s1", " 不要合并 ", Some("root-1"))
            .await
            .unwrap();
        assert_eq!(queued.status, "queued");
        assert_eq!(queued.content, "不要合并");
        assert_eq!(queued.position, Some(1));
        db.close().await;

        // T3: a fresh pool on the same file is a restart.
        let reopened = temp_db(&dir).await;
        let pending_rows = pending(&reopened, "s1").await.unwrap();
        assert_eq!(pending_rows.len(), 1);
        assert_eq!(pending_rows[0].root_turn_id, "root-1");
        assert_eq!(sessions_with_pending(&reopened).await.unwrap(), vec!["s1".to_string()]);

        let claimed = claim_next(&reopened, "s1").await.unwrap().unwrap();
        assert_eq!(claimed.id, queued.id);
        assert_eq!(claimed.status, "delivering");
        assert_eq!(claimed.attempts, 1);
        mark_delivered(&reopened, "s1", &claimed.id).await.unwrap();

        // Exactly once: nothing is left to deliver after the confirmation.
        assert!(claim_next(&reopened, "s1").await.unwrap().is_none());
        assert!(pending(&reopened, "s1").await.unwrap().is_empty());
        let status = status_of(&reopened, "s1", &queued.id).await.unwrap().unwrap();
        assert_eq!(status.status, "delivered");
        assert!(status.delivered_at.is_some());
        assert_eq!(status.position, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn claim_is_fifo_and_never_double_claims() {
        let dir = std::env::temp_dir().join(format!("m236-{}", uuid::Uuid::new_v4()));
        let db = temp_db(&dir).await;
        let first = enqueue(&db, "s1", "one", None).await.unwrap();
        let second = enqueue(&db, "s1", "two", None).await.unwrap();
        let claimed = claim_next(&db, "s1").await.unwrap().unwrap();
        assert_eq!(claimed.id, first.id);
        // A second drain in the same state must not hand the same row over again.
        let next = claim_next(&db, "s1").await.unwrap().unwrap();
        assert_eq!(next.id, second.id);
        assert!(claim_next(&db, "s1").await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn queue_is_capped_and_failure_is_visible() {
        let dir = std::env::temp_dir().join(format!("m236-{}", uuid::Uuid::new_v4()));
        let db = temp_db(&dir).await;
        for index in 0..QUEUED_DISPATCH_MAX {
            enqueue(&db, "s1", &format!("msg-{index}"), None).await.unwrap();
        }
        let full = enqueue(&db, "s1", "overflow", None).await;
        assert!(full.is_err());

        let failed = fail_pending(&db, "s1", "session was stopped before delivery")
            .await
            .unwrap();
        assert_eq!(failed.len() as i64, QUEUED_DISPATCH_MAX);
        let rows = pending(&db, "s1").await.unwrap();
        assert!(rows.is_empty());
        let status = status_of(&db, "s1", &failed[0].id).await.unwrap().unwrap();
        assert_eq!(status.status, "failed");
        assert!(status.failure_reason.unwrap().contains("stopped"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_fresh_queued_message_outranks_a_wake_but_a_stale_one_does_not() {
        let dir = std::env::temp_dir().join(format!("m236-{}", uuid::Uuid::new_v4()));
        let db = temp_db(&dir).await;
        let queued = enqueue(&db, "s1", "先听我的", None).await.unwrap();
        let now = now_ms();
        assert!(has_fresh_pending(&db, "s1", now, DELIVERING_STALE_MS * 2).await);
        assert!(!has_fresh_pending(&db, "s1", queued.created_at + DELIVERING_STALE_MS * 4, DELIVERING_STALE_MS * 2).await);
        assert!(!has_fresh_pending(&db, "other", now, DELIVERING_STALE_MS * 2).await);
        mark_delivered(&db, "s1", &queued.id).await.unwrap();
        assert!(!has_fresh_pending(&db, "s1", now, DELIVERING_STALE_MS * 2).await);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stale_handoff_returns_to_queue() {
        let dir = std::env::temp_dir().join(format!("m236-{}", uuid::Uuid::new_v4()));
        let db = temp_db(&dir).await;
        let queued = enqueue(&db, "s1", "retry me", None).await.unwrap();
        claim_next(&db, "s1").await.unwrap().unwrap();
        sqlx::query("UPDATE queued_dispatch_messages SET updated_at=? WHERE id=?")
            .bind(now_ms() - DELIVERING_STALE_MS - 1_000)
            .bind(&queued.id)
            .execute(&db)
            .await
            .unwrap();
        let reclaimed = claim_next(&db, "s1").await.unwrap().unwrap();
        assert_eq!(reclaimed.id, queued.id);
        assert_eq!(reclaimed.attempts, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
