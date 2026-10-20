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
    if has_table(db, "chat_run_controls").await? {
        let runs: Vec<String> = sqlx::query_scalar(
            "SELECT run_instance_id FROM chat_run_controls WHERE session_id=? AND status='active' AND created_process_instance=?",
        ).bind(session).bind(super::objective::current_process_instance()).fetch_all(db).await?;
        if runs.iter().any(|id| crate::chat_run_executor_alive(id)) {
            return Ok(ThinState::running());
        }
    }
    let tasks: i64 = if has_table(db, "background_tasks").await? {
        sqlx::query_scalar("SELECT COUNT(*) FROM background_tasks WHERE session_id=? AND status='running'")
            .bind(session).fetch_one(db).await?
    } else { 0 };
    let wakes: i64 = if has_table(db, "wakeup_schedules").await? {
        sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id=? AND status='pending' AND next_at>?")
            .bind(session).bind(at).fetch_one(db).await?
    } else { 0 };
    if tasks + wakes > 0 {
        return Ok(super::thin_state::after_final(super::thin_state::WaitNeed::External {
            what: "后台任务或未来定时唤醒".into(), covered: true,
        }, "本会话记录"));
    }
    let mut terminal: Option<(i64, String)> = None;
    let mut stopped_at = 0;
    if has_table(db, "chat_session_cancel_intents").await? {
        let stop: Option<i64> = sqlx::query_scalar("SELECT requested_at FROM chat_session_cancel_intents WHERE session_id=?")
            .bind(session).fetch_optional(db).await?;
        stopped_at = stop.unwrap_or(0);
        terminal = stop.map(|time| (time, "cancelled：用户已停止会话".into()));
    }
    if has_table(db, "chat_turn_state").await? {
        let end: Option<(i64, String)> = sqlx::query_as(
            "SELECT COALESCE(completed_at,updated_at),terminal_reason FROM chat_turn_state WHERE session_id=? AND terminal_reason IS NOT NULL ORDER BY COALESCE(completed_at,updated_at) DESC LIMIT 1",
        ).bind(session).fetch_optional(db).await?;
        if let Some(end) = end { if terminal.as_ref().is_none_or(|old| end.0 > old.0) { terminal = Some(end); } }
    }
    if has_table(db, "messages").await? {
        let answer: Option<i64> = sqlx::query_scalar(
            "SELECT created_at FROM messages WHERE session_id=? AND role='assistant' AND TRIM(content)<>'' AND (tool_calls IS NULL OR tool_calls='' OR tool_calls='[]') AND (completion_state IS NULL OR completion_state='' OR completion_state='final') ORDER BY created_at DESC LIMIT 1",
        ).bind(session).fetch_optional(db).await?;
        if let Some(time) = answer { if terminal.as_ref().is_none_or(|old| time > old.0) { terminal = Some((time, "最终回答已完成".into())); } }
    }
    let cutoff = stopped_at;
    if has_table(db, "permission_intents").await? {
        let question: Option<String> = sqlx::query_scalar("SELECT tool_name FROM permission_intents WHERE session_id=? AND status='pending' AND expires_at>? AND created_at>? ORDER BY created_at DESC LIMIT 1")
            .bind(session).bind(at).bind(cutoff).fetch_optional(db).await?;
        if let Some(tool) = question { return Ok(ThinState { state: SessionState::WaitingUser, question: Some(format!("是否允许 {tool}？")), ..ThinState::running() }); }
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
    Ok(ThinState::ended(terminal.as_ref().map_or("无活跃执行者或未决等待", |end| end.1.as_str()), "本会话记录"))
}

/// 读取端刷新缓存；不受旧缓存的取消保护约束，事实才是权威。
pub async fn refresh(db: &SqlitePool, session: &str) -> sqlx::Result<ThinState> {
    let state = derive_state(db, session).await?;
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
    async fn facts_override_poisoned_cache_and_dead_process_controls() {
        let db = fixture().await;
        super::super::thin_state::save(&db, "derive", &ThinState::running()).await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::Ended);
        let control = crate::ChatRunControl::pending();
        sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,status,created_process_instance,created_at,updated_at) VALUES (?,'derive','active',?,1,1)")
            .bind(&control.run_instance_id).bind(crate::agent::objective::current_process_instance()).execute(&db).await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::Running);
        let waiting = ThinState { state: SessionState::WaitingUser, question: Some("失效的缓存问题".into()), ..ThinState::running() };
        super::super::thin_state::save(&db, "derive", &waiting).await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::Running);
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::Running);
        assert_eq!(super::super::thin_state::load(&db, "derive").await.unwrap().unwrap().state, SessionState::Running);
        drop(control);
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::Ended);
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
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::Ended);
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('final','derive','assistant','完成',300)")
            .execute(&db).await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().reason.as_deref(), Some("最终回答已完成"));
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
