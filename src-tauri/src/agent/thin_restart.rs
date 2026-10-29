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

/// M189：在启动清理旧 transport 之前冻结真实中断身份。
/// 只采集旧进程仍 active 的回合，不能把 completed 控制记录反推为中断。
pub(crate) async fn capture_restart_interruptions(db: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS thin_restart_interruptions(root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, process_instance TEXT NOT NULL)")
        .execute(db).await?;
    sqlx::query("INSERT OR IGNORE INTO thin_restart_interruptions(root_turn_id,session_id,process_instance)
        SELECT turn.root_turn_id,turn.session_id,control.created_process_instance
        FROM chat_turn_state turn JOIN chat_run_controls control ON control.root_turn_id=turn.root_turn_id
        WHERE control.status='active' AND control.created_process_instance<>?
          AND turn.completed_at IS NULL AND turn.terminal_reason IS NULL")
        .bind(crate::agent::objective::current_process_instance()).execute(db).await?;
    Ok(())
}

/// M189：恢复资格必须来自未结束的旧进程回合，而非过期的 running 投影。
async fn filter_restart_candidates(
    db: &SqlitePool,
    rows: Vec<String>,
) -> sqlx::Result<Vec<String>> {
    let full_schema: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_run_controls'")
        .fetch_one(db).await?;
    if full_schema == 0 { return Ok(rows); }
    sqlx::query("CREATE TABLE IF NOT EXISTS thin_restart_interruptions(root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, process_instance TEXT NOT NULL)").execute(db).await?;
    let mut eligible = Vec::new();
    for session in rows {
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_turn_state turn
             JOIN chat_run_controls control ON control.root_turn_id=turn.root_turn_id
             WHERE turn.session_id=? AND turn.completed_at IS NULL AND turn.terminal_reason IS NULL
               AND ((control.status='active' AND control.created_process_instance<>?)
                 OR EXISTS (SELECT 1 FROM thin_restart_interruptions interrupted
                   WHERE interrupted.root_turn_id=turn.root_turn_id))
               AND NOT EXISTS (SELECT 1 FROM messages answer WHERE answer.session_id=turn.session_id
                 AND answer.role='assistant' AND answer.created_at>=turn.started_at
                 AND TRIM(answer.content)<>''
                 AND (answer.tool_calls IS NULL OR answer.tool_calls='' OR answer.tool_calls='[]')
                 AND NOT EXISTS (SELECT 1 FROM tool_calls invocation WHERE invocation.message_id=answer.id)
                 AND (answer.completion_state IS NULL OR answer.completion_state='' OR answer.completion_state='final'))")
            .bind(&session).bind(crate::agent::objective::current_process_instance())
            .fetch_one(db).await?;
        if open > 0 { eligible.push(session); }
    }
    Ok(eligible)
}

/// M189：按工具调用身份（无工具时按根回合）原子认领，跨进程也最多一次。
/// 先落 claim 再调用模型；崩溃后的不确定结果不能靠换时间戳无限重放。
pub(crate) async fn claim_restart(
    db: &SqlitePool,
    session: &str,
    root: &str,
) -> sqlx::Result<Option<String>> {
    sqlx::query("CREATE TABLE IF NOT EXISTS thin_restart_claims (claim_key TEXT PRIMARY KEY, session_id TEXT NOT NULL, root_turn_id TEXT NOT NULL, claimed_at INTEGER NOT NULL)")
        .execute(db).await?;
    if filter_restart_candidates(db, vec![session.to_string()]).await?.is_empty() {
        return Ok(None);
    }
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS thin_restart_claim_root ON thin_restart_claims(root_turn_id)").execute(db).await?;
    let tool: Option<String> = sqlx::query_scalar(
        "SELECT tool.id FROM tool_calls tool JOIN messages message ON message.id=tool.message_id
         JOIN chat_turn_state turn ON turn.root_turn_id=? AND turn.session_id=message.session_id
         WHERE message.session_id=? AND tool.created_at>=turn.started_at
           AND tool.status IN ('pending','approved') AND (tool.result IS NULL OR TRIM(tool.result)='')
         ORDER BY tool.created_at DESC,tool.id DESC LIMIT 1")
        .bind(root).bind(session).fetch_optional(db).await?;
    let key = tool.map_or_else(|| format!("turn:{root}"), |id| format!("tool:{id}"));
    let written = sqlx::query("INSERT OR IGNORE INTO thin_restart_claims(claim_key,session_id,root_turn_id,claimed_at) VALUES (?,?,?,?)")
        .bind(&key).bind(session).bind(root).bind(chrono::Utc::now().timestamp_millis())
        .execute(db).await?.rows_affected();
    if written == 1 {
        sqlx::query("DELETE FROM thin_restart_interruptions WHERE root_turn_id=?")
            .bind(root).execute(db).await?;
    }
    Ok((written == 1).then_some(key))
}

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
        return filter_restart_candidates(db, rows.into_iter().filter(|session| !has_live_executor(session)).collect()).await;
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
    filter_restart_candidates(db, rows
        .into_iter()
        .filter(|session| !has_live_executor(session))
        .collect()).await
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
    filter_restart_candidates(db, rows
        .into_iter()
        .filter(|session| !has_live_executor(session))
        .collect()).await
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

    #[tokio::test]
    async fn m200_restart_model_wait_is_not_orphaned_and_wakes_once() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('m200','synthetic','/synthetic','model',1,1,'thin');
            INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at) VALUES ('m200-root','m200','working','active',1,1);
            INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,status,created_process_instance,created_at,updated_at) VALUES ('m200-run','m200','m200-root','active','synthetic-prior-process',1,1);")
            .execute(&db).await.unwrap();
        crate::agent::thin_state::save(&db, "m200", &crate::agent::thin_state::ThinState::running()).await.unwrap();
        capture_restart_interruptions(&db).await.unwrap();
        assert!(claim_restart(&db, "m200", "m200-root").await.unwrap().is_some());
        let due = chrono::Utc::now().timestamp_millis() + 120_000;
        let error = crate::agent::failover::ModelUnavailable { resume_at_ms: Some(due), notice: "合成模型等待，系统自动重试/自愈".into() }.encode();
        crate::agent::thin_state::settle_error(&db, "m200", "m200-root", &error).await.unwrap();
        // The startup/heartbeat reader must not retire this root while its wake owns it.
        crate::agent::thin_derive::reconcile_orphan_turns(&db).await.unwrap();
        let reason: Option<String> = sqlx::query_scalar("SELECT terminal_reason FROM chat_turn_state WHERE root_turn_id='m200-root'").fetch_one(&db).await.unwrap();
        assert!(reason.is_none(), "a model wait is not an orphan: {reason:?}");
        let state = crate::agent::thin_state::load(&db, "m200").await.unwrap().unwrap();
        assert_eq!(state.state, crate::agent::thin_state::SessionState::WaitingWake);
        let wakes = crate::agent::background_tasks::list_wakeups(&db, "m200").await.unwrap();
        assert_eq!(wakes.len(), 1);
        assert_eq!(wakes[0].status, "pending");
        struct Sink(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl crate::agent::background_tasks::WakeSink for Sink {
            fn enqueue(&self, session: String, text: String) -> crate::agent::background_tasks::WakeFuture {
                assert_eq!(session, "m200");
                assert!(text.contains("系统自动重试/自愈"));
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async { true })
            }
        }
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink = Sink(calls.clone());
        let at = (due + 999) / 1000;
        assert_eq!(crate::agent::background_tasks::drain_due_wakeups_at(&db, &sink, at - 1).await, 0);
        assert_eq!(crate::agent::background_tasks::drain_due_wakeups_at(&db, &sink, at).await, 1);
        assert_eq!(crate::agent::background_tasks::drain_due_wakeups_at(&db, &sink, at + 1).await, 0);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

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
    async fn m189_final_answer_survives_three_recovery_sweeps() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('m189-final','synthetic','/synthetic','model',1,1,'thin');
            INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at) VALUES ('m189-root','m189-final','running','running',1,1);
            INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,status,created_process_instance,created_at,updated_at,settled_at) VALUES ('m189-run','m189-final','m189-root','completed','old-process',1,1,2);
            INSERT INTO messages(id,session_id,role,content,completion_state,created_at) VALUES ('m189-answer','m189-final','assistant','synthetic finished','final',2);")
            .execute(&db).await.unwrap();
        crate::agent::thin_state::save(&db, "m189-final", &crate::agent::thin_state::ThinState::running()).await.unwrap();
        sqlx::query("UPDATE thin_session_state SET updated_at=1 WHERE session_id='m189-final'").execute(&db).await.unwrap();
        for tick in 1..=3 {
            let now = tick * 10_000_000;
            assert!(running_without_executor(&db, now, 0, |_| false).await.unwrap().is_empty(), "completed turn must never dispatch another model run");
            assert!(running_interrupted_by_restart(&db, now, |_| false).await.unwrap().is_empty(), "restart must not reopen a final answer");
        }
        crate::agent::thin_derive::refresh(&db, "m189-final").await.unwrap();
        assert_eq!(crate::agent::thin_state::load(&db, "m189-final").await.unwrap().unwrap().state, crate::agent::thin_state::SessionState::Ended);
        let notices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id='m189-final' AND role='system'").fetch_one(&db).await.unwrap();
        assert_eq!(notices, 0);
    }

    #[test]
    fn m189_restart_claim_survives_real_process_restart() {
        const ENV: &str = "CODEFACTORY_M189_SYNTHETIC_DB";
        if let Some(url) = std::env::var_os(ENV) {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let db = crate::storage::db::connect(url.to_str().unwrap()).await.unwrap();
                capture_restart_interruptions(&db).await.unwrap();
                crate::agent::objective::ObjectiveStore::new(db.clone())
                    .reconcile_stale_chat_run_controls(&crate::agent::objective::current_process_instance()).await.unwrap();
                crate::agent::thin_derive::reconcile_orphan_turns(&db).await.unwrap();
                let claim = claim_restart(&db, "m189-restart", "m189-root").await.unwrap();
                if std::env::var("CODEFACTORY_M189_EXPECT_CLAIM").unwrap() == "yes" {
                    assert_eq!(claim.as_deref(), Some("tool:m189-tool"));
                    crate::commands::chat::persist_resume_interrupt_notice(&db, "m189-restart", claim.as_deref().unwrap()).await.unwrap();
                    mark_interrupted_tool_calls(&db, "m189-restart").await.unwrap();
                } else { assert!(claim.is_none()); }
                for _ in 0..3 {
                    assert!(claim_restart(&db, "m189-restart", "m189-root").await.unwrap().is_none());
                }
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM thin_restart_claims").fetch_one(&db).await.unwrap();
                assert_eq!(count, 1);
                let notices: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE role='system'").fetch_one(&db).await.unwrap();
                assert_eq!(notices, 1);
            });
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite:{}?mode=rwc", dir.path().join("synthetic.db").display());
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let db = crate::storage::db::connect(&url).await.unwrap();
            sqlx::raw_sql("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('m189-restart','synthetic','/synthetic','model',1,1,'thin');
                INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at) VALUES ('m189-root','m189-restart','running','running',1,1);
                INSERT INTO messages(id,session_id,role,content,tool_calls,created_at) VALUES ('m189-call','m189-restart','assistant','','[]',2);
                INSERT INTO tool_calls(id,message_id,tool_name,arguments,status,created_at) VALUES ('m189-tool','m189-call','background_task_list','{}','pending',2);")
                .execute(&db).await.unwrap();
            sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,status,created_process_instance,created_at,updated_at) VALUES ('m189-run','m189-restart','m189-root','active',?,1,1)")
                .bind(crate::agent::objective::current_process_instance()).execute(&db).await.unwrap();
            crate::agent::thin_state::save(&db, "m189-restart", &crate::agent::thin_state::ThinState::running()).await.unwrap();
            assert!(claim_restart(&db, "m189-restart", "m189-root").await.unwrap().is_none(), "same process is not a restart");
            db.close().await;
        });
        for expected in ["yes", "no", "no"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "agent::thin_restart::tests::m189_restart_claim_survives_real_process_restart", "--nocapture"])
                .env(ENV, &url).env("CODEFACTORY_M189_EXPECT_CLAIM", expected)
                .current_dir(dir.path()).output().unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
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
