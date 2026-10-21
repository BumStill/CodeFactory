// SPDX-License-Identifier: Apache-2.0
//! 四态由运行事实推导，thin_session_state 只作投影。
use sqlx::SqlitePool;
use super::thin_state::{SessionState, ThinState};

async fn has_table(db: &SqlitePool, name: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
        .bind(name).fetch_one(db).await? != 0)
}

/// 唯一四态判定。缓存中的 state/question/reason 不参与事实选择。
pub async fn derive_state(db: &SqlitePool, session: &str) -> sqlx::Result<ThinState> {
    let at = chrono::Utc::now().timestamp_millis();
    if has_table(db, "chat_session_cancel_intents").await? {
        let stop: Option<i64> = sqlx::query_scalar("SELECT requested_at FROM chat_session_cancel_intents WHERE session_id=?")
            .bind(session).fetch_optional(db).await?;
        if let Some(stop) = stop {
            let newer_user: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id=? AND role='user' AND created_at>?")
                .bind(session).bind(stop).fetch_one(db).await?;
            if newer_user == 0 {
                return Ok(ThinState::ended("cancelled：用户已停止会话", "本会话记录"));
            }
        }
    }
    // M139: permission waits are facts even while the executor is alive.
    // Cancellation above remains authoritative; resolved/expired prompts do not wait.
    if has_table(db, "permission_intents").await? {
        let pending: Option<(String, String)> = sqlx::query_as(
            "SELECT tool_name,prompt_args_json FROM permission_intents WHERE session_id=? AND status='pending' AND expires_at>? ORDER BY created_at DESC LIMIT 1",
        ).bind(session).bind(at).fetch_optional(db).await?;
        if let Some((tool, args)) = pending {
            let args = serde_json::from_str::<serde_json::Value>(&args).unwrap_or_default();
            let summary = args.get("command").and_then(serde_json::Value::as_str).unwrap_or(&tool);
            let summary: String = summary.chars().take(240).collect();
            return Ok(ThinState {
                state: SessionState::WaitingUser,
                question: Some(format!("需要你批准执行：{summary}")),
                ..ThinState::running()
            });
        }
    }
    match crate::session_executor_alive(session) {
        Some(true) => return Ok(ThinState::running()),
        None => return Ok(super::thin_state::load(db, session).await?.unwrap_or_else(ThinState::running)),
        Some(false) => {}
    }
    let tasks: i64 = if has_table(db, "background_tasks").await? {
        sqlx::query_scalar("SELECT COUNT(*) FROM background_tasks WHERE session_id=? AND status='running'")
            .bind(session).fetch_one(db).await?
    } else { 0 };
    let wakes: i64 = if has_table(db, "wakeup_schedules").await? {
        sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id=? AND status='pending' AND next_at>?")
            .bind(session).bind(at).fetch_one(db).await?
    } else { 0 };
    let mut terminal: Option<(i64, String)> = None;
    let mut stopped_at = 0;
    if has_table(db, "chat_session_cancel_intents").await? {
        let stop: Option<i64> = sqlx::query_scalar("SELECT requested_at FROM chat_session_cancel_intents WHERE session_id=?")
            .bind(session).fetch_optional(db).await?;
        stopped_at = stop.unwrap_or(0);
        terminal = stop.map(|time| (time, "cancelled：用户已停止会话".into()));
    }
    if has_table(db, "chat_turn_state").await? {
        // 阶段 2 P0/R1：只有真实终止事实才算数。`objective_already_terminal` 与
        // `superseded_at_startup` 是 legacy 目标收敛在重启时写给回合投影的，
        // 不代表 thin 会话结束——v1.91.7 就是这样把还在跑 `sleep 60` 的会话判成
        // “已结束（目标已终态）”。已写坏的投影在这里被忽略。
        let end: Option<(i64, String)> = sqlx::query_as(
            "SELECT COALESCE(completed_at,updated_at),terminal_reason FROM chat_turn_state WHERE session_id=? AND terminal_reason IS NOT NULL AND terminal_reason NOT IN ('objective_already_terminal','superseded_at_startup') ORDER BY COALESCE(completed_at,updated_at) DESC LIMIT 1",
        ).bind(session).fetch_optional(db).await?;
        if let Some(end) = end { if terminal.as_ref().is_none_or(|old| end.0 > old.0) { terminal = Some(end); } }
    }
    let mut pending_question_at = 0;
    if has_table(db, "thin_wait_notes").await? {
        pending_question_at = sqlx::query_scalar::<_, Option<i64>>("SELECT MAX(question.created_at) FROM thin_wait_notes note JOIN messages question ON question.id=note.root_turn_id WHERE note.session_id=? AND NOT EXISTS (SELECT 1 FROM messages answer WHERE answer.session_id=note.session_id AND answer.role='user' AND answer.created_at>question.created_at)")
            .bind(session).fetch_one(db).await?.unwrap_or(0);
    }
    if has_table(db, "messages").await? {
        let answer: Option<i64> = sqlx::query_scalar(
            "SELECT created_at FROM messages WHERE session_id=? AND role='assistant' AND TRIM(content)<>'' AND (tool_calls IS NULL OR tool_calls='' OR tool_calls='[]') AND (completion_state IS NULL OR completion_state='' OR completion_state='final') ORDER BY created_at DESC LIMIT 1",
        ).bind(session).fetch_optional(db).await?;
        if let Some(time) = answer.filter(|_| pending_question_at == 0) { if terminal.as_ref().is_none_or(|old| time > old.0) { terminal = Some((time, "最终回答已完成".into())); } }
    }
    let latest_user: Option<i64> = if has_table(db, "messages").await? {
        sqlx::query_scalar("SELECT MAX(created_at) FROM messages WHERE session_id=? AND role='user'")
            .bind(session).fetch_one(db).await?
    } else { None };
    let cutoff = terminal.as_ref().map_or(0, |end| end.0);
    if latest_user.is_some_and(|time| time > cutoff) {
        // A newly admitted message is a fact even before the transport is registered.
        if terminal.is_some() {
            return Ok(ThinState::running());
        }
    } else if let Some((_, reason)) = &terminal {
        let previous = super::thin_state::load(db, session).await?;
        // A final answer may deliberately settle to waiting_wake. A stop never does.
        if stopped_at > 0 || previous.as_ref().is_none_or(|state| state.state != SessionState::WaitingWake) {
            return Ok(ThinState::ended(reason, "本会话记录"));
        }
    }
    if tasks + wakes > 0 {
        return Ok(super::thin_state::after_final(super::thin_state::WaitNeed::External {
            what: "后台任务或未来定时唤醒".into(), covered: true,
        }, "本会话记录"));
    }
    if has_table(db, "thin_wait_notes").await? {
        // A note names an observed question; a subsequent user answer consumes it.
        let note: Option<(String, Option<String>)> = sqlx::query_as("SELECT waiting_for,root_turn_id FROM thin_wait_notes WHERE session_id=?")
            .bind(session).fetch_optional(db).await?;
        if let Some((what, Some(root))) = note {
            let unanswered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages question WHERE question.id=? AND question.session_id=? AND question.created_at>? AND NOT EXISTS (SELECT 1 FROM messages answer WHERE answer.session_id=question.session_id AND answer.role='user' AND answer.created_at>question.created_at)")
                .bind(root).bind(session).bind(cutoff).fetch_one(db).await?;
            if unanswered > 0 && !super::thin::is_external_object(&what) {
                return Ok(ThinState { state: SessionState::WaitingUser, question: Some(what), ..ThinState::running() });
            }
        }
    }
    if let Some(previous) = super::thin_state::load(db, session).await? {
        return Ok(previous);
    }
    if latest_user.is_some() {
        return Ok(ThinState::running());
    }
    Ok(ThinState::ended(terminal.as_ref().map_or("无活跃执行者或未决等待", |end| end.1.as_str()), "本会话记录"))
}

/// 读取端刷新缓存；不受旧缓存的取消保护约束，事实才是权威。
pub async fn refresh(db: &SqlitePool, session: &str) -> sqlx::Result<ThinState> {
    let mut state = derive_state(db, session).await?;
    if let Some(previous) = super::thin_state::load(db, session).await? {
        if state.state == previous.state {
            state.updated_at = previous.updated_at;
        }
    }
    super::thin_state::ensure(db).await?;
    sqlx::query("INSERT INTO thin_session_state(session_id,state,waiting_for,wake_source,question,reason,outcome_location,consecutive_failures,updated_at) VALUES (?,?,?,?,?,?,?,0,?) ON CONFLICT(session_id) DO UPDATE SET state=excluded.state,waiting_for=excluded.waiting_for,wake_source=excluded.wake_source,question=excluded.question,reason=excluded.reason,outcome_location=excluded.outcome_location,updated_at=excluded.updated_at")
        .bind(session).bind(state.state.as_str()).bind(&state.waiting_for).bind(&state.wake_source).bind(&state.question).bind(&state.reason).bind(&state.outcome_location).bind(state.updated_at).execute(db).await?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn fixture() -> SqlitePool {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('derive','synthetic','/synthetic','model',1,1,'thin')")
            .execute(&db).await.unwrap();
        db
    }
    #[tokio::test]
    async fn m139_pending_approval_overrides_live_executor_and_resumes() {
        let db = fixture().await;
        let control = crate::ChatRunControl::pending();
        crate::register_session_executor("derive", &control);
        sqlx::raw_sql("DROP TABLE permission_intents; CREATE TABLE permission_intents(session_id TEXT, status TEXT, expires_at INTEGER, created_at INTEGER, tool_name TEXT, prompt_args_json TEXT);
            INSERT INTO permission_intents VALUES ('derive','pending',9223372036854775807,100,'bash','{\"command\":\"rm -rf generated\"}');")
            .execute(&db).await.unwrap();
        let waiting = refresh(&db, "derive").await.unwrap();
        assert_eq!(waiting.state, SessionState::WaitingUser);
        assert_eq!(waiting.question.as_deref(), Some("需要你批准执行：rm -rf generated"));
        sqlx::query("UPDATE permission_intents SET status='allowed'").execute(&db).await.unwrap();
        let resumed = refresh(&db, "derive").await.unwrap();
        assert_eq!(resumed.state, SessionState::Running);
        assert!(resumed.question.is_none());
        crate::unregister_session_executor("derive", &control);
    }

    #[tokio::test]
    async fn facts_override_poisoned_cache_and_dead_process_controls() {
        let db = fixture().await;
        sqlx::query("UPDATE sessions SET id='derive-live' WHERE id='derive'").execute(&db).await.unwrap();
        super::super::thin_state::save(&db, "derive-live", &ThinState::running()).await.unwrap();
        assert_eq!(derive_state(&db, "derive-live").await.unwrap().state, SessionState::Running);
        let control = crate::ChatRunControl::pending();
        crate::register_session_executor("derive-live", &control);
        sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,status,created_process_instance,created_at,updated_at) VALUES (?,'derive-live','active',?,1,1)")
            .bind(&control.run_instance_id).bind(crate::agent::objective::current_process_instance()).execute(&db).await.unwrap();
        assert_eq!(derive_state(&db, "derive-live").await.unwrap().state, SessionState::Running);
        let waiting = ThinState { state: SessionState::WaitingUser, question: Some("失效的缓存问题".into()), ..ThinState::running() };
        super::super::thin_state::save(&db, "derive-live", &waiting).await.unwrap();
        assert_eq!(derive_state(&db, "derive-live").await.unwrap().state, SessionState::Running);
        assert_eq!(refresh(&db, "derive-live").await.unwrap().state, SessionState::Running);
        assert_eq!(super::super::thin_state::load(&db, "derive-live").await.unwrap().unwrap().state, SessionState::Running);
        crate::unregister_session_executor("derive-live", &control);
        drop(control);
        assert_eq!(derive_state(&db, "derive-live").await.unwrap().state, SessionState::Running);
    }
    #[tokio::test]
    async fn future_wakes_and_unanswered_questions_follow_fact_precedence() {
        let db = fixture().await;
        sqlx::raw_sql("CREATE TABLE wakeup_schedules(id TEXT PRIMARY KEY,session_id TEXT,status TEXT,next_at INTEGER);
            INSERT INTO wakeup_schedules VALUES ('future','derive','pending',9223372036854775807);
            CREATE TABLE IF NOT EXISTS thin_wait_notes(session_id TEXT PRIMARY KEY,root_turn_id TEXT,waiting_for TEXT,hinted INTEGER);
            INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('question','derive','user','选择目录',100);
            INSERT INTO thin_wait_notes VALUES ('derive','question','等你确认目录',1);")
            .execute(&db).await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
        sqlx::query("UPDATE wakeup_schedules SET next_at=1").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('ask-text','derive','assistant','请选择目录',150)")
            .execute(&db).await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::WaitingUser);
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('answer','derive','user','使用 synthetic',200)")
            .execute(&db).await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::Running);
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('final','derive','assistant','完成',300)")
            .execute(&db).await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().reason.as_deref(), Some("最终回答已完成"));
    }

    /// 阶段 2 P0/R1：legacy 目标收敛在重启时写进回合投影的原因不是 thin 的终止
    /// 事实。v1.91.7 现场（会话 `0dcf23e1`）就是这样把还在跑 `sleep 60` 的会话
    /// 判成 `ended / objective_already_terminal`。
    #[tokio::test]
    async fn legacy_convergence_reasons_never_end_a_running_thin_session() {
        let db = fixture().await;
        super::super::thin_state::save(&db, "derive", &ThinState::running())
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('mid-turn','derive','user','只读演练：用 bash 执行 sleep 60',1000)")
            .execute(&db)
            .await
            .unwrap();
        for reason in ["objective_already_terminal", "superseded_at_startup"] {
            sqlx::raw_sql("DELETE FROM chat_turn_state")
                .execute(&db)
                .await
                .unwrap();
            sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at,completed_at,terminal_reason) VALUES ('turn','derive','settled','completed',1000,2000,2000,?)")
                .bind(reason)
                .execute(&db)
                .await
                .unwrap();
            let derived = derive_state(&db, "derive").await.unwrap();
            assert_eq!(
                derived.state,
                SessionState::Running,
                "{reason} 不得结束 thin 会话（reason={:?}）",
                derived.reason
            );
        }
    }

    /// R1 的另一半：真实终止事实仍然结束会话（停止意图 + 三次真实失败）。
    #[tokio::test]
    async fn real_termination_still_ends_a_thin_session() {
        let db = fixture().await;
        sqlx::query("INSERT INTO chat_session_cancel_intents(session_id,status,requested_at,updated_at) VALUES ('derive','requested',50,50)")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(
            derive_state(&db, "derive").await.unwrap().state,
            SessionState::Ended
        );
    }

    #[tokio::test]
    async fn terminal_facts_invalidate_old_questions_and_background_work() {
        let db = fixture().await;
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS thin_wait_notes(session_id TEXT PRIMARY KEY,root_turn_id TEXT,waiting_for TEXT,hinted INTEGER);
            INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('old-question','derive','user','选择目录',100);
            INSERT INTO thin_wait_notes VALUES ('derive','old-question','等你确认目录',1);
            INSERT INTO chat_session_cancel_intents(session_id,status,requested_at,updated_at,settled_at) VALUES ('derive','settled',200,200,200);
            CREATE TABLE wakeup_schedules(id TEXT PRIMARY KEY,session_id TEXT,status TEXT,next_at INTEGER);
            INSERT INTO wakeup_schedules VALUES ('old-wake','derive','pending',9223372036854775807);")
            .execute(&db).await.unwrap();
        for _ in 0..3 {
            assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::Ended);
        }
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('new-user','derive','user','新任务',300)")
            .execute(&db).await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::Running);
    }

    #[tokio::test]
    async fn uncertain_running_projection_is_not_ended_or_retimestamped() {
        let db = fixture().await;
        let mut state = ThinState::running();
        state.updated_at = 123;
        super::super::thin_state::save(&db, "derive", &state).await.unwrap();
        for _ in 0..3 {
            let derived = refresh(&db, "derive").await.unwrap();
            assert_eq!(derived.state, SessionState::Running);
            assert_eq!(derived.updated_at, 123);
        }
    }

    #[tokio::test]
    async fn stop_reason_is_a_fact_even_when_cache_says_running() {
        let db = fixture().await;
        sqlx::query("INSERT INTO chat_session_cancel_intents(session_id,status,requested_at,updated_at) VALUES ('derive','requested',50,50)")
            .execute(&db).await.unwrap();
        let state = derive_state(&db, "derive").await.unwrap();
        assert_eq!(state.state, SessionState::Ended);
        assert!(state.reason.unwrap().contains("cancelled"));
    }
}
