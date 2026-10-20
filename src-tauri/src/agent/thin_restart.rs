// SPDX-License-Identifier: Apache-2.0
//! 阶段 2 P0/R2：`running` 但没有活着的回合执行者的 thin 会话必须被续跑。
//!
//! v1.91.1 现场（会话 6f5e87a2）：应用因安装重启时该会话停在 `running`，
//! 重启后既没有执行者也没有任何恢复动作——阶段 1 R3 的读现场恢复只覆盖「用户
//! 再发一条消息」，阶段 2 R4 的心跳只扫 `waiting_wake`，于是行永久停在
//! `running`。这里补上两处扫描（启动 + 心跳）并用同一份续跑实现收口。
//!
//! 选择条件是**权威表** `thin_session_state`，不是回合投影：
//! - `state='running'`；
//! - 已经超过宽限期（启动时为 0，心跳为 [`RUNNING_WITHOUT_EXECUTOR_GRACE_MS`]）；
//! - 当前进程没有该会话的活执行者（`chat_cancels` 里的在跑回合）。

use sqlx::SqlitePool;
use tauri::Manager;

/// 心跳判定「执行者已死」的宽限期：`running` 超过这么久还没有活执行者就续跑。
pub(crate) const RUNNING_WITHOUT_EXECUTOR_GRACE_MS: i64 = 10 * 60 * 1_000;

/// 纯查询 + 可注入的「活执行者」判定，合成 fixture 可直接驱动。
pub(crate) async fn running_without_executor(
    db: &SqlitePool,
    now_ms: i64,
    grace_ms: i64,
    mut has_live_executor: impl FnMut(&str) -> bool,
) -> sqlx::Result<Vec<String>> {
    crate::agent::thin_state::ensure(db).await?;
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT state.session_id FROM thin_session_state state
         JOIN sessions session ON session.id=state.session_id
         WHERE state.state='running' AND state.updated_at <= ?
         ORDER BY state.updated_at ASC",
    )
    .bind(now_ms.saturating_sub(grace_ms))
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|session| !has_live_executor(session))
        .collect())
}

/// 启动与心跳共用的入口：扫描并把每个候选交给续跑实现（各自一个任务，
/// 启动流程不在模型调用上阻塞）。返回被接管的数量。
pub(crate) async fn resume_running_without_executor(
    app: tauri::AppHandle,
    pool: &SqlitePool,
    grace_ms: i64,
) -> usize {
    let live: std::collections::HashSet<String> = app
        .state::<crate::AppState>()
        .chat_cancels
        .lock()
        .await
        .keys()
        .cloned()
        .collect();
    let candidates = match running_without_executor(
        pool,
        chrono::Utc::now().timestamp_millis(),
        grace_ms,
        |session| live.contains(session),
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(%error, "thin restart: running-session scan failed");
            return 0;
        }
    };
    for session_id in &candidates {
        tracing::info!(
            session = %session_id,
            grace_ms,
            "thin restart: resuming a running session with no live executor"
        );
        let app = app.clone();
        let session_id = session_id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) =
                crate::commands::chat::resume_thin_running_session(app, session_id.clone()).await
            {
                tracing::warn!(%error, session = %session_id, "thin restart: resume failed");
            }
        });
    }
    candidates.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE sessions (id TEXT PRIMARY KEY, delivery_mode TEXT);")
            .execute(&db)
            .await
            .unwrap();
        crate::agent::thin_state::ensure(&db).await.unwrap();
        db
    }

    async fn session(db: &SqlitePool, id: &str, state: &str, updated_at: i64) {
        sqlx::query("INSERT INTO sessions (id, delivery_mode) VALUES (?, 'thin')")
            .bind(id)
            .execute(db)
            .await
            .unwrap();
        let mut value = crate::agent::thin_state::ThinState::running();
        value.state = match state {
            "running" => crate::agent::thin_state::SessionState::Running,
            "waiting_wake" => crate::agent::thin_state::SessionState::WaitingWake,
            _ => crate::agent::thin_state::SessionState::Ended,
        };
        crate::agent::thin_state::save(db, id, &value)
            .await
            .unwrap();
        sqlx::query("UPDATE thin_session_state SET updated_at=? WHERE session_id=?")
            .bind(updated_at)
            .bind(id)
            .execute(db)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn startup_selects_running_sessions_without_executor_and_ignores_the_rest() {
        let db = fixture().await;
        let now = 10_000_000_i64;
        session(&db, "stale-running", "running", now - 60 * 60 * 1_000).await;
        session(&db, "live-running", "running", now - 60 * 60 * 1_000).await;
        session(&db, "fresh-running", "running", now - 1_000).await;
        session(&db, "waiting", "waiting_wake", now - 60 * 60 * 1_000).await;
        session(&db, "ended", "ended", now - 60 * 60 * 1_000).await;

        let selected = running_without_executor(&db, now, 0, |session| session == "live-running")
            .await
            .unwrap();
        // Startup has no grace window: every running row with no executor is
        // ours, including one updated a second before the crash.
        assert_eq!(
            selected,
            vec!["stale-running".to_string(), "fresh-running".to_string()]
        );

        // The heartbeat grace keeps a freshly-updated `running` row alone, but a
        // dead executor past the grace window is still picked up.
        let heartbeat =
            running_without_executor(&db, now, RUNNING_WITHOUT_EXECUTOR_GRACE_MS, |_| false)
                .await
                .unwrap();
        assert_eq!(
            heartbeat,
            vec!["stale-running".to_string(), "live-running".to_string()]
        );
    }

    #[tokio::test]
    async fn a_running_row_is_never_left_unwatched_once_its_executor_is_gone() {
        let db = fixture().await;
        let now = 10_000_000_i64;
        session(
            &db,
            "restart",
            "running",
            now - RUNNING_WITHOUT_EXECUTOR_GRACE_MS,
        )
        .await;
        let selected =
            running_without_executor(&db, now, RUNNING_WITHOUT_EXECUTOR_GRACE_MS, |_| false)
                .await
                .unwrap();
        assert_eq!(selected, vec!["restart".to_string()]);
    }

    async fn turn_fixture(root_turn_id: &str) -> SqlitePool {
        let db = fixture().await;
        sqlx::raw_sql(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT, revision INTEGER DEFAULT 0,
               phase TEXT, status TEXT, recent_activity_kind TEXT, recent_activity_label TEXT,
               waiting_reason TEXT, updated_at INTEGER, completed_at INTEGER,
               terminal_revision INTEGER, terminal_reason TEXT);",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO sessions (id, delivery_mode) VALUES ('s1', 'thin')")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO chat_turn_state (root_turn_id, session_id) VALUES (?, 's1')")
            .bind(root_turn_id)
            .execute(&db)
            .await
            .unwrap();
        db
    }

    /// R1: a bounded `Blocked` stop that already delivered the final answer
    /// settles as a final answer — the v1.91.1 defect wrote `running` here.
    #[tokio::test]
    async fn a_final_answer_never_settles_running() {
        let db = turn_fixture("turn-final").await;
        let settled = crate::agent::thin_state::settle_outcome(
            &db,
            "s1",
            "turn-final",
            false,
            true,
            "页面标题是 Example Domain",
            "Blocked",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            settled.state,
            crate::agent::thin_state::SessionState::Ended,
            "a delivered final answer must end the session, not leave it running"
        );
        assert!(settled.failure_signature.is_none());
        assert_eq!(settled.consecutive_failures, 0);
        let terminal: Option<String> =
            sqlx::query_scalar("SELECT terminal_reason FROM chat_turn_state WHERE root_turn_id=?")
                .bind("turn-final")
                .fetch_one(&db)
                .await
                .unwrap();
        assert!(
            terminal.is_some(),
            "the turn projection must be terminal too"
        );
    }

    /// The other half of the same rule: no answer means a real technical
    /// failure, which the heartbeat then has to pick up.
    #[tokio::test]
    async fn a_blocked_stop_without_an_answer_stays_a_technical_failure() {
        let db = turn_fixture("turn-silent").await;
        let settled = crate::agent::thin_state::settle_outcome(
            &db,
            "s1",
            "turn-silent",
            false,
            true,
            "   ",
            "Blocked",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            settled.state,
            crate::agent::thin_state::SessionState::Running
        );
        assert_eq!(
            settled.failure_signature.as_deref(),
            Some("agent loop stopped: Blocked")
        );
        assert_eq!(settled.consecutive_failures, 1);
    }

    #[test]
    fn settlement_predicate_is_finished_or_blocked_with_text() {
        use crate::agent::thin_state::outcome_settles_final;
        assert!(outcome_settles_final(true, false, ""));
        assert!(outcome_settles_final(false, true, "done"));
        assert!(!outcome_settles_final(false, true, ""));
        assert!(!outcome_settles_final(false, false, "partial"));
    }

    /// R2: nothing persisted to continue — the row must ask, never stay running.
    #[tokio::test]
    async fn a_running_row_with_nothing_to_resume_asks_the_user() {
        let db = turn_fixture("turn-orphan").await;
        let state = crate::agent::thin_state::ask_user(&db, "s1", "要我继续吗？")
            .await
            .unwrap();
        assert_eq!(
            state.state,
            crate::agent::thin_state::SessionState::WaitingUser
        );
        assert_eq!(state.question.as_deref(), Some("要我继续吗？"));
        let loaded = crate::agent::thin_state::load(&db, "s1")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            loaded.state,
            crate::agent::thin_state::SessionState::Running,
            "a session with no resumable turn must not stay running"
        );
    }
}
