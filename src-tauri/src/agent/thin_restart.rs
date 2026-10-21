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
//! - 启动与心跳均至少超过 [`RUNNING_WITHOUT_EXECUTOR_GRACE_MS`] 宽限；
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
    let grace_ms = grace_ms.max(RUNNING_WITHOUT_EXECUTOR_GRACE_MS);
    crate::agent::thin_state::ensure(db).await?;
    let has_controls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_run_controls'")
        .fetch_one(db).await?;
    if has_controls == 1 {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT session.id FROM sessions session
             WHERE session.delivery_mode='thin'
               AND EXISTS (SELECT 1 FROM chat_turn_state turn
                 JOIN chat_run_controls control ON control.root_turn_id=turn.root_turn_id
                 WHERE turn.session_id=session.id AND turn.completed_at IS NULL
                   AND turn.terminal_reason IS NULL AND control.status<>'cancelled'
                   AND control.updated_at<=? AND turn.updated_at<=?)
               AND EXISTS (SELECT 1 FROM thin_session_state state
                 WHERE state.session_id=session.id AND state.state='running' AND state.updated_at<=?)
               AND NOT EXISTS (SELECT 1 FROM messages message
                 WHERE message.session_id=session.id AND message.created_at>?)
               AND NOT EXISTS (SELECT 1 FROM chat_turn_state recent
                 WHERE recent.session_id=session.id AND recent.updated_at>?)
               AND NOT EXISTS (SELECT 1 FROM chat_run_controls recent
                 WHERE recent.session_id=session.id AND recent.updated_at>?)
               AND NOT EXISTS (SELECT 1 FROM chat_session_cancel_intents stop
                 WHERE stop.session_id=session.id AND stop.status='requested')
             ORDER BY session.id",
        ).bind(now_ms.saturating_sub(grace_ms))
            .bind(now_ms.saturating_sub(grace_ms))
            .bind(now_ms.saturating_sub(grace_ms))
            .bind(now_ms.saturating_sub(grace_ms))
            .bind(now_ms.saturating_sub(grace_ms))
            .bind(now_ms.saturating_sub(grace_ms)).fetch_all(db).await?;
        return Ok(rows.into_iter().filter(|session| !has_live_executor(session)).collect());
    }
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

/// 阶段 2 P0/R2：启动扫描。进程刚起来时执行者注册表天然为空，所以任何
/// `running` 且没有活执行者的 thin 会话都是上一个进程留下的回合，必须立刻续跑——
/// 不能套用心跳的 10 分钟宽限（v1.91.7 现场是回合开始后 66 秒重启，宽限把它挡掉了）。
pub(crate) async fn running_interrupted_by_restart(
    db: &SqlitePool,
    now_ms: i64,
    mut has_live_executor: impl FnMut(&str) -> bool,
) -> sqlx::Result<Vec<String>> {
    crate::agent::thin_state::ensure(db).await?;
    let has_stop: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_session_cancel_intents'",
    )
    .fetch_one(db)
    .await?;
    let rows: Vec<String> = if has_stop == 1 {
        sqlx::query_scalar(
            "SELECT state.session_id FROM thin_session_state state
             JOIN sessions session ON session.id=state.session_id
             WHERE session.delivery_mode='thin'
               AND state.state='running'
               AND state.updated_at<=?
               AND NOT EXISTS (SELECT 1 FROM chat_session_cancel_intents stop
                 WHERE stop.session_id=state.session_id AND stop.status='requested')
             ORDER BY state.updated_at ASC",
        )
        .bind(now_ms)
        .fetch_all(db)
        .await?
    } else {
        sqlx::query_scalar(
            "SELECT state.session_id FROM thin_session_state state
             JOIN sessions session ON session.id=state.session_id
             WHERE session.delivery_mode='thin'
               AND state.state='running'
               AND state.updated_at<=?
             ORDER BY state.updated_at ASC",
        )
        .bind(now_ms)
        .fetch_all(db)
        .await?
    };
    Ok(rows
        .into_iter()
        .filter(|session| !has_live_executor(session))
        .collect())
}

/// 阶段 2 P0/R2：中断现场标记。`pending`/`approved` 且没有结果的工具调用在重启后
/// 不可能再拿到真实结果，必须显式写成「中断，结果未知」，而不是一直挂着 pending。
pub(crate) const INTERRUPTED_TOOL_NOTE: &str =
    "中断，结果未知：应用重启，工具可能已执行也可能未执行";

pub(crate) async fn mark_interrupted_tool_calls(
    db: &SqlitePool,
    session: &str,
) -> sqlx::Result<u64> {
    let has_tools: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tool_calls'",
    )
    .fetch_one(db)
    .await?;
    if has_tools == 0 {
        return Ok(0);
    }
    let marked = sqlx::query(
        "UPDATE tool_calls SET status='error', error=?
         WHERE message_id IN (SELECT id FROM messages WHERE session_id=?)
           AND status IN ('pending','approved')
           AND (result IS NULL OR TRIM(result)='')",
    )
    .bind(INTERRUPTED_TOOL_NOTE)
    .bind(session)
    .execute(db)
    .await?;
    Ok(marked.rows_affected())
}

/// 扫描来源决定「没有活执行者」的时间口径：启动不设宽限，心跳必须守宽限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResumeOrigin {
    Startup,
    Heartbeat,
}

impl ResumeOrigin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Heartbeat => "heartbeat",
        }
    }

    /// 与扫描入口同一份判定，续跑实现内部复验时复用，避免两处口径分叉。
    pub(crate) async fn candidates(
        self,
        db: &SqlitePool,
        now_ms: i64,
        has_live_executor: impl FnMut(&str) -> bool,
    ) -> sqlx::Result<Vec<String>> {
        match self {
            Self::Startup => running_interrupted_by_restart(db, now_ms, has_live_executor).await,
            Self::Heartbeat => {
                running_without_executor(
                    db,
                    now_ms,
                    RUNNING_WITHOUT_EXECUTOR_GRACE_MS,
                    has_live_executor,
                )
                .await
            }
        }
    }
}

/// 启动与心跳共用的入口：扫描并把每个候选交给续跑实现（各自一个任务，
/// 启动流程不在模型调用上阻塞）。返回被接管的数量。
pub(crate) async fn resume_running_without_executor(
    app: tauri::AppHandle,
    pool: &SqlitePool,
    origin: ResumeOrigin,
) -> usize {
    let live: std::collections::HashSet<String> = app
        .state::<crate::AppState>()
        .chat_cancels
        .lock()
        .await
        .keys()
        .cloned()
        .collect();
    let candidates = match origin
        .candidates(
            pool,
            chrono::Utc::now().timestamp_millis(),
            |session| live.contains(session) || crate::session_executor_alive(session) != Some(false),
        )
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(%error, origin = origin.label(), "thin restart: running-session scan failed");
            return 0;
        }
    };
    for session_id in &candidates {
        tracing::info!(
            session = %session_id,
            origin = origin.label(),
            "thin restart: resuming a running session with no live executor"
        );
        let app = app.clone();
        let session_id = session_id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) =
                crate::commands::chat::resume_thin_running_session(app, session_id.clone(), origin)
                    .await
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
        // R2: startup and heartbeat both protect newly started turns.
        assert_eq!(selected, vec!["stale-running".to_string()]);

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
    async fn recovery_never_reopens_ended_even_with_stale_unfinished_controls() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('ended-recovery','synthetic','/synthetic','model',1,1,'thin')")
            .execute(&db).await.unwrap();
        crate::agent::thin_state::save(&db, "ended-recovery", &crate::agent::thin_state::ThinState::ended("cancelled", "synthetic")).await.unwrap();
        sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at) VALUES ('old-root','ended-recovery','running','running',1,1)")
            .execute(&db).await.unwrap();
        sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,status,created_process_instance,created_at,updated_at) VALUES ('old-run','ended-recovery','old-root','active','old-process',1,1)")
            .execute(&db).await.unwrap();
        for grace in [0, RUNNING_WITHOUT_EXECUTOR_GRACE_MS] {
            assert!(running_without_executor(&db, 10_000_000, grace, |_| false).await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn recovery_uses_latest_user_activity_not_an_old_control() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('recent-user','synthetic','/synthetic','model',1,1,'thin')")
            .execute(&db).await.unwrap();
        let mut state = crate::agent::thin_state::ThinState::running();
        state.updated_at = 1;
        crate::agent::thin_state::save(&db, "recent-user", &state).await.unwrap();
        sqlx::raw_sql("INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at) VALUES ('recent-root','recent-user','running','running',1,1);
            INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,status,created_process_instance,created_at,updated_at) VALUES ('recent-run','recent-user','recent-root','active','old-process',1,1);
            INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('recent-message','recent-user','user','synthetic new task',9999000);")
            .execute(&db).await.unwrap();
        assert!(running_without_executor(&db, 10_000_000, 0, |_| false).await.unwrap().is_empty());
        assert_eq!(running_without_executor(&db, 10_600_001, 0, |_| false).await.unwrap(), vec!["recent-user"]);
    }

    #[tokio::test]
    async fn recovery_protects_fresh_session_even_when_startup_passes_zero_grace() {
        let db = fixture().await;
        session(&db, "fresh", "running", 9_999_000).await;
        assert!(running_without_executor(&db, 10_000_000, 0, |_| false).await.unwrap().is_empty());
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

    /// 阶段 2 P0/R2：重启时，66 秒前还在跑的回合必须立刻续跑。v1.91.7 现场是
    /// `sleep 60` 中途重启（回合开始后 66 秒），心跳的 10 分钟宽限把它挡在门外。
    #[tokio::test]
    async fn restart_resumes_a_turn_interrupted_a_minute_ago() {
        let db = fixture().await;
        let now = 10_000_000_i64;
        session(&db, "interrupted", "running", now - 66_000).await;
        let selected = running_interrupted_by_restart(&db, now, |_| false)
            .await
            .unwrap();
        assert_eq!(selected, vec!["interrupted".to_string()]);
        // 同一个会话在心跳口径下仍然守宽限，不会被重复接管。
        assert!(running_without_executor(&db, now, RUNNING_WITHOUT_EXECUTOR_GRACE_MS, |_| false)
            .await
            .unwrap()
            .is_empty());
    }

    /// R2：有活执行者、用户已要求停止、已经结束的会话都不在启动扫描里。
    #[tokio::test]
    async fn restart_skips_live_executors_stops_and_finished_sessions() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES
               ('live','s','/s','m',1,1,'thin'),
               ('stopped','s','/s','m',1,1,'thin'),
               ('finished','s','/s','m',1,1,'thin');",
        )
        .execute(&db)
        .await
        .unwrap();
        for session_id in ["live", "stopped", "finished"] {
            let mut state = crate::agent::thin_state::ThinState::running();
            state.updated_at = 1000;
            let state = if session_id == "finished" {
                crate::agent::thin_state::ThinState::ended("最终回答已完成", "")
            } else {
                state
            };
            crate::agent::thin_state::save(&db, session_id, &state)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO chat_session_cancel_intents(session_id,status,requested_at,updated_at) VALUES ('stopped','requested',900,900)")
            .execute(&db)
            .await
            .unwrap();
        let selected = running_interrupted_by_restart(&db, 10_000_000, |id| id == "live")
            .await
            .unwrap();
        assert!(selected.is_empty(), "不该接管：{selected:?}");
    }

    /// R2：重启后不可能再拿到真实结果的工具调用必须写成「中断，结果未知」，
    /// 而不是一直挂在 `pending` 上让界面显示“正在执行”。
    #[tokio::test]
    async fn restart_marks_pending_tool_calls_as_interrupted() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode)
               VALUES ('s','s','/s','m',1,1,'thin');
             INSERT INTO messages(id,session_id,role,content,created_at)
               VALUES ('m','s','assistant','准备执行',1);
             INSERT INTO tool_calls(id,message_id,tool_name,arguments,status,created_at) VALUES
               ('pending','m','bash','{\"command\":\"sleep 60; echo LIVENESS-DONE\"}','pending',1),
               ('done','m','bash','{\"command\":\"pwd\"}','done',2);",
        )
        .execute(&db)
        .await
        .unwrap();
        assert_eq!(mark_interrupted_tool_calls(&db, "s").await.unwrap(), 1);
        let (status, error): (String, Option<String>) =
            sqlx::query_as("SELECT status, error FROM tool_calls WHERE id='pending'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(status, "error");
        assert!(
            error.unwrap_or_default().contains("中断，结果未知"),
            "pending 工具必须显式标成中断、结果未知"
        );
        let done: String = sqlx::query_scalar("SELECT status FROM tool_calls WHERE id='done'")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(done, "done", "已有结果的调用不得被改写");
    }

    /// R1：终态目标不关闭 thin 的活跃回合（启动收敛形态 2 必须跳过 thin）。
    #[tokio::test]
    async fn reconcile_never_closes_a_thin_turn_because_its_objective_is_terminal() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('session-thin-live','t','/t','m',1,1,'thin')")
            .execute(&db)
            .await
            .unwrap();
        crate::agent::thin_state::save(&db, "session-thin-live", &crate::agent::thin_state::ThinState::running())
            .await
            .unwrap();
        let store = crate::agent::objective::ObjectiveStore::new(db.clone());
        let objective = store
            .create(crate::agent::objective::CreateObjective {
                id: "objective-thin-live".into(),
                kind: crate::agent::objective::ObjectiveKind::Informational,
                session_id: Some("session-thin-live".into()),
                root_turn_id: Some("turn-thin-live".into()),
                domain: crate::agent::objective::RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET status='legacy_orphan', completed_at=1 WHERE id=?")
            .bind(&objective.id)
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at,objective_id) VALUES ('turn-thin-live','session-thin-live','running','active',1,2,?)")
            .bind(&objective.id)
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(store.reconcile_stale_chat_turns().await.unwrap(), 0);
        let terminal: Option<String> = sqlx::query_scalar(
            "SELECT terminal_reason FROM chat_turn_state WHERE root_turn_id='turn-thin-live'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(
            terminal.is_none(),
            "thin 回合不能被 legacy 目标终态关闭：{terminal:?}"
        );
    }
}
