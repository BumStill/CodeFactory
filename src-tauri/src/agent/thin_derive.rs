// SPDX-License-Identifier: Apache-2.0
//! 四态由运行事实推导，thin_session_state 只作投影。
use sqlx::SqlitePool;
use super::thin_state::{SessionState, ThinState};

async fn has_table(db: &SqlitePool, name: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
        .bind(name).fetch_one(db).await? != 0)
}

/// 外部事实是有时效的观测，不在 derive_state 中访问网络。
async fn ensure_external(db: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS thin_external_observations(session_id TEXT PRIMARY KEY,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL,observed_at INTEGER NOT NULL DEFAULT 0,completed_reason TEXT,closeout_queued INTEGER NOT NULL DEFAULT 0)")
        .execute(db).await?;
    Ok(())
}

fn completion_reason(pr: &serde_json::Value, ceiling: &str, released: bool, number: i64) -> Option<String> {
    let terminal = pr["state"] == "MERGED";
    if !terminal || (matches!(ceiling, "ThroughRelease" | "through_release") && !released) {
        return None;
    }
    Some(format!("PR #{number} 已合并"))
}

fn failure_reason(pr: &serde_json::Value, number: i64) -> Option<String> {
    if pr["state"] == "CLOSED" { return Some(format!("PR #{number} 已关闭但未合并")); }
    if pr["state"] != "OPEN" { return None; }
    let failed: Vec<&str> = pr["statusCheckRollup"].as_array()?.iter().filter(|check| {
        matches!(check["conclusion"].as_str(), Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" | "STALE"))
            || matches!(check["state"].as_str(), Some("FAILURE" | "ERROR"))
    }).map(|check| check["name"].as_str().or(check["context"].as_str()).unwrap_or("未知检查")).collect();
    (!failed.is_empty()).then(|| format!("PR #{number} 检查失败：{}", failed.join("、")))
}

fn is_external_failure(reason: &str) -> bool {
    reason.contains("检查失败：") || reason.contains("已关闭但未合并")
}

// 进程内共享滚动窗口；同时计入既有全局观测计数，限额耗尽时不发请求。
fn reserve_external_call() -> bool {
    use super::delivery::github_api_budget as budget;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static WINDOW: OnceLock<Mutex<(Instant, u64, u64)>> = OnceLock::new();
    let mut window = match WINDOW.get_or_init(|| Mutex::new((Instant::now(), budget::total_gh_calls(), 0))).lock() {
        Ok(window) => window, Err(_) => return false,
    };
    if window.0.elapsed() >= Duration::from_secs(3600) {
        *window = (Instant::now(), budget::total_gh_calls(), 0);
    }
    if !budget::current_rate_limit_policy().allows_observation()
        || window.2.max(budget::total_gh_calls().saturating_sub(window.1)) >= 600 { return false; }
    window.2 += 1;
    budget::record_gh_call(budget::CALLER_OBSERVE);
    true
}

fn observe_command(cwd: &str, args: &[&str]) -> Option<String> {
    if !reserve_external_call() { return None; }
    super::delivery::bounded_delivery_output(super::delivery::dev_command("gh").current_dir(cwd).args(args)).ok()
}

/// 心跳先刷新事实，再由持久唤醒入口投递一次 §8 收尾。
/// 查询失败/额度不足不伪造完成；observed_at 在请求前 CAS，重启也遵守 180s。
pub(crate) async fn heartbeat_external(db: &SqlitePool, session: &str) -> sqlx::Result<()> {
    if !super::thin::is_thin(db, session).await { return Ok(()); }
    let state = super::thin_state::load(db, session).await?;
    if !state.is_some_and(|state| matches!(state.state, SessionState::WaitingWake | SessionState::WaitingUser))
        || crate::session_executor_alive(session) != Some(false) { return Ok(()); }
    ensure_external(db).await?;
    if has_table(db, "chat_session_cancel_intents").await? {
        let stopped: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_session_cancel_intents stop WHERE stop.session_id=? AND NOT EXISTS (SELECT 1 FROM messages user WHERE user.session_id=stop.session_id AND user.role='user' AND user.created_at>stop.requested_at)")
            .bind(session).fetch_one(db).await?;
        if stopped > 0 { return Ok(()); }
    }
    let reference: Option<(i64, String, String)> = sqlx::query_as("SELECT ref.pr_number,ref.ceiling,session.cwd FROM thin_delivery_refs ref JOIN sessions session ON session.id=ref.session_id WHERE ref.session_id=?")
        .bind(session).fetch_optional(db).await?;
    let Some((number, ceiling, cwd)) = reference else { return Ok(()); };
    sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling) VALUES (?,?,?) ON CONFLICT(session_id) DO UPDATE SET pr_number=excluded.pr_number,ceiling=excluded.ceiling,observed_at=0,completed_reason=NULL,closeout_queued=0 WHERE pr_number<>excluded.pr_number OR ceiling<>excluded.ceiling")
        .bind(session).bind(number).bind(&ceiling).execute(db).await?;
    let at = chrono::Utc::now().timestamp_millis();
    let claimed = sqlx::query("UPDATE thin_external_observations SET observed_at=? WHERE session_id=? AND (completed_reason IS NULL OR completed_reason LIKE '%检查失败：%' OR completed_reason LIKE '%已关闭但未合并%') AND observed_at<=?")
        .bind(at).bind(session).bind(at - 180_000).execute(db).await?.rows_affected() == 1;
    if claimed {
        let observe_ceiling = ceiling.clone();
        let reason = tokio::task::spawn_blocking(move || {
            let raw = observe_command(&cwd, &["pr", "view", &number.to_string(), "--json", "state,mergeCommit,statusCheckRollup"])?;
            let pr: serde_json::Value = serde_json::from_str(&raw).ok()?;
            let released = if matches!(observe_ceiling.as_str(), "ThroughRelease" | "through_release") && pr["state"] == "MERGED" {
                // 不把任意最新版本当作本 PR 的版本：必须确认 merge commit 被已发布 tag 包含。
                let release = observe_command(&cwd, &["api", "repos/{owner}/{repo}/releases/latest"])?;
                let release: serde_json::Value = serde_json::from_str(&release).ok()?;
                if release["draft"] != false || !release["published_at"].is_string() { return None; }
                let tag = release["tag_name"].as_str()?;
                let sha = pr["mergeCommit"]["oid"].as_str()?;
                let comparison = observe_command(&cwd, &["api", &format!("repos/{{owner}}/{{repo}}/compare/{sha}...{tag}")])?;
                let comparison: serde_json::Value = serde_json::from_str(&comparison).ok()?;
                matches!(comparison["status"].as_str(), Some("ahead" | "identical"))
            } else { false };
            Some(failure_reason(&pr, number).or_else(|| completion_reason(&pr, &observe_ceiling, released, number)))
        }).await.ok().flatten();
        if let Some(reason) = reason {
            sqlx::query("UPDATE thin_external_observations SET closeout_queued=CASE WHEN completed_reason IS ? THEN closeout_queued ELSE 0 END,completed_reason=? WHERE session_id=? AND pr_number=? AND ceiling=?")
                .bind(&reason).bind(&reason).bind(session).bind(number).bind(&ceiling).execute(db).await?;
        }
    }
    let reason: Option<String> = sqlx::query_scalar("SELECT completed_reason FROM thin_external_observations WHERE session_id=? AND closeout_queued=0")
        .bind(session).fetch_optional(db).await?.flatten();
    if let Some(reason) = reason {
        super::background_tasks::wake_source_cover(db, session).await;
        // 同一事务写唤醒与去重事实，不留下“标记成功但没排唤醒”的崩溃窗口。
        let mut tx = db.begin().await?;
        let queued = sqlx::query("UPDATE thin_external_observations SET closeout_queued=1 WHERE session_id=? AND closeout_queued=0")
            .bind(session).execute(&mut *tx).await?.rows_affected() == 1;
        if queued {
            let failed = is_external_failure(&reason);
            let prefix = if failed { "thin-repair" } else { "thin-closeout" };
            let prompt = if failed {
                format!("{reason}。系统自动重试/自愈：诊断失败检查，修复后在同一分支重新交付并继续监视；PR 关闭时先核查原因再恢复交付。不向用户询问是否继续监视。")
            } else {
                format!("{reason}，交付外部事项已完成。现在执行任务模板 §8 收尾：确认合并与 squash commit、检查并清理远端分支、检查本地未提交/未推送状态、清理本任务产物与后台进程，然后给出收尾回答。不要重复交付；若已无事可做直接报告完成。")
            };
            sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,after_sec,next_at,status,created_at) VALUES (?,?,?,'agent_decided',60,?,'pending',?)")
                .bind(format!("{prefix}-{}", uuid::Uuid::new_v4())).bind(session).bind(prompt)
                .bind(at / 1000 + 60).bind(at / 1000).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

async fn completed_external(db: &SqlitePool, session: &str) -> sqlx::Result<Option<String>> {
    if !has_table(db, "thin_external_observations").await? { return Ok(None); }
    sqlx::query_scalar("SELECT observation.completed_reason FROM thin_external_observations observation JOIN thin_delivery_refs ref ON ref.session_id=observation.session_id AND ref.pr_number=observation.pr_number AND ref.ceiling=observation.ceiling WHERE observation.session_id=? AND observation.closeout_queued=1")
        .bind(session).fetch_optional(db).await.map(Option::flatten)
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
            .bind(session).bind(at / 1000).fetch_one(db).await?
    } else { 0 };
    let completed = completed_external(db, session).await?;
    if let Some(reason) = completed.as_ref().filter(|reason| !is_external_failure(reason)) {
        let final_after_wake: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules wake JOIN messages answer ON answer.session_id=wake.session_id AND answer.role='assistant' AND answer.created_at>=wake.last_fired_at*1000 AND TRIM(answer.content)<>'' AND (answer.tool_calls IS NULL OR answer.tool_calls='' OR answer.tool_calls='[]') AND (answer.completion_state IS NULL OR answer.completion_state='' OR answer.completion_state='final') WHERE wake.session_id=? AND wake.id LIKE 'thin-closeout-%' AND wake.status='done' AND NOT EXISTS (SELECT 1 FROM messages user WHERE user.session_id=answer.session_id AND user.role='user' AND user.created_at>answer.created_at)")
            .bind(session).fetch_one(db).await?;
        if final_after_wake > 0 {
            return Ok(ThinState::ended(reason, "本会话记录"));
        }
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
    // M142：已交付引用本身就是系统心跳监视源；最终回答不终止未达 ceiling 的 PR。
    // 真实提问仍由下面的未回答问题事实处理，审批和停止已在最前面处理。
    if completed.as_deref().is_some_and(is_external_failure) {
        let repaired: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules wake JOIN messages answer ON answer.session_id=wake.session_id AND answer.role='assistant' AND answer.created_at>=wake.last_fired_at*1000 AND TRIM(answer.content)<>'' AND (answer.tool_calls IS NULL OR answer.tool_calls='' OR answer.tool_calls='[]') AND (answer.completion_state IS NULL OR answer.completion_state='' OR answer.completion_state='final') WHERE wake.session_id=? AND wake.id LIKE 'thin-repair-%' AND wake.status='done' AND wake.prompt LIKE (SELECT completed_reason || '%' FROM thin_external_observations WHERE session_id=?)")
            .bind(session).bind(session).fetch_one(db).await?;
        if repaired == 0 && pending_question_at == 0 { return Ok(ThinState::running()); }
    }
    if pending_question_at == 0 && (completed.is_none() || completed.as_deref().is_some_and(is_external_failure))
        && has_table(db, "thin_delivery_refs").await? {
        let number: Option<i64> = sqlx::query_scalar("SELECT pr_number FROM thin_delivery_refs WHERE session_id=?")
            .bind(session).fetch_optional(db).await?;
        if let Some(number) = number {
            return Ok(ThinState {
                state: SessionState::WaitingWake,
                waiting_for: Some(format!("等 PR #{number} 合并/CI")),
                wake_source: Some("系统 PR 监视（心跳，间隔不少于 180 秒）".into()),
                ..ThinState::running()
            });
        }
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
        if previous.state == SessionState::WaitingWake {
            return Ok(ThinState {
                state: SessionState::WaitingUser,
                waiting_for: previous.waiting_for,
                question: Some("外部事项尚无法确认完成，且没有活着的唤醒源。请选择继续监视或结束任务。".into()),
                reason: Some("唤醒源已失效；系统自动重试/自愈未取得完成事实".into()),
                ..ThinState::running()
            });
        }
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
    async fn m142_delivered_pr_is_a_system_wake_source_even_after_final() {
        let db = fixture().await;
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',73,'ThroughMerge')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('delivered','derive','assistant','PR 已交付',100)").execute(&db).await.unwrap();
        let state = refresh(&db, "derive").await.unwrap();
        assert_eq!(state.state, SessionState::WaitingWake);
        assert_eq!(state.waiting_for.as_deref(), Some("等 PR #73 合并/CI"));
        assert!(state.wake_source.unwrap().contains("系统 PR 监视"));
        assert!(state.question.is_none());
    }

    #[test]
    fn m142_closed_pr_is_failure_not_successful_closeout() {
        assert!(completion_reason(&serde_json::json!({"state":"CLOSED"}), "ThroughMerge", false, 73).is_none());
    }

    #[tokio::test]
    async fn m142_failed_check_wakes_for_repair_once_and_resumes_system_watch() {
        let db = fixture().await;
        super::super::thin_state::save(&db, "derive", &super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "PR / CI".into(), covered: false }, "")).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',73,'ThroughMerge')").execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason) VALUES ('derive',73,'ThroughMerge',?,'PR #73 检查失败：unit-tests')")
            .bind(chrono::Utc::now().timestamp_millis()).execute(&db).await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::Running);
        struct Sink;
        impl super::super::background_tasks::WakeSink for Sink {
            fn enqueue(&self, _: String, text: String) -> super::super::background_tasks::WakeFuture {
                assert!(text.contains("unit-tests"));
                assert!(text.contains("修复"));
                assert!(!text.contains("§8 收尾"));
                Box::pin(async { true })
            }
        }
        let due: i64 = sqlx::query_scalar("SELECT next_at FROM wakeup_schedules").fetch_one(&db).await.unwrap();
        assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &Sink, due).await, 1);
        assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &Sink, due + 1).await, 0);
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('repair','derive','assistant','修复并重新交付',?)")
            .bind(due * 1000 + 1).execute(&db).await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
    }

    #[test]
    fn m142_check_names_and_closed_failure_ignore_merged_stale_checks() {
        for conclusion in ["FAILURE", "TIMED_OUT", "CANCELLED", "ACTION_REQUIRED", "STARTUP_FAILURE", "STALE"] {
            let mut pr = serde_json::json!({"state":"OPEN","statusCheckRollup":[{"name":"synthetic-build","conclusion":conclusion},{"context":"legacy-ci","state":"ERROR"}]});
            let reason = failure_reason(&pr, 91).unwrap();
            assert!(reason.contains("synthetic-build") && reason.contains("legacy-ci"));
            pr["state"] = "MERGED".into();
            assert!(failure_reason(&pr, 91).is_none());
        }
        for conclusion in ["SUCCESS", "NEUTRAL", "SKIPPED", ""] {
            assert!(failure_reason(&serde_json::json!({"state":"OPEN","statusCheckRollup":[{"name":"ok","conclusion":conclusion}]}), 91).is_none());
        }
        assert!(failure_reason(&serde_json::json!({"state":"CLOSED"}), 91).unwrap().contains("未合并"));
    }

    #[tokio::test]
    async fn m142_real_question_and_stop_override_system_monitor() {
        let db = fixture().await;
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',73,'ThroughMerge')").execute(&db).await.unwrap();
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS thin_wait_notes(session_id TEXT PRIMARY KEY,root_turn_id TEXT,waiting_for TEXT,hinted INTEGER);
            INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('human-question','derive','assistant','请选择目标目录',100);
            INSERT INTO thin_wait_notes VALUES ('derive','human-question','请选择目标目录',1);").execute(&db).await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingUser);
        sqlx::query("INSERT INTO chat_session_cancel_intents(session_id,status,requested_at,updated_at) VALUES ('derive','requested',200,200)").execute(&db).await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::Ended);
        heartbeat_external(&db, "derive").await.unwrap();
        assert!(!has_table(&db, "thin_external_observations").await.unwrap());
    }

    #[tokio::test]
    async fn m140_unwatched_wait_is_immediately_surfaced_but_live_monitor_stays() {
        let db = fixture().await;
        let waiting = super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "交付 PR / CI".into(), covered: false }, "");
        super::super::thin_state::save(&db, "derive", &waiting).await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingUser);
        super::super::thin_state::save(&db, "derive", &waiting).await.unwrap();
        super::super::background_tasks::wake_source_cover(&db, "derive").await;
        sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,created_at) VALUES ('monitor','derive','synthetic','agent_decided',9223372036854775807,'pending',1)")
            .execute(&db).await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
        sqlx::query("UPDATE wakeup_schedules SET next_at=1").execute(&db).await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingUser);
    }

    #[test]
    fn m140_ceiling_requires_release_and_ignores_stale_ci_after_merge() {
        for number in [7, 42, 913] {
            for state in ["OPEN", "MERGED", "CLOSED", "UNKNOWN"] {
                let pr = serde_json::json!({"state":state,"statusCheckRollup":[{"status":"IN_PROGRESS"}]});
                let terminal = state == "MERGED";
                assert_eq!(completion_reason(&pr, "ThroughMerge", false, number).is_some(), terminal);
                assert!(completion_reason(&pr, "ThroughRelease", false, number).is_none());
                assert_eq!(completion_reason(&pr, "ThroughRelease", true, number).is_some(), terminal);
            }
        }
    }

    #[tokio::test]
    async fn m140_completed_pr_wakes_once_then_final_answer_ends() {
        let db = fixture().await;
        let waiting = super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "交付 PR / CI".into(), covered: false }, "");
        super::super::thin_state::save(&db, "derive", &waiting).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',42,'ThroughMerge')").execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason) VALUES ('derive',42,'ThroughMerge',1,'PR #42 已合并')")
            .execute(&db).await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        let wakes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE id LIKE 'thin-closeout-%'").fetch_one(&db).await.unwrap();
        assert_eq!(wakes, 1);
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
        struct Sink;
        impl super::super::background_tasks::WakeSink for Sink {
            fn enqueue(&self, session: String, text: String) -> super::super::background_tasks::WakeFuture {
                assert_eq!(session, "derive");
                assert!(text.contains("§8 收尾"));
                Box::pin(async { true })
            }
        }
        assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &Sink, 1).await, 0);
        let due: i64 = sqlx::query_scalar("SELECT next_at FROM wakeup_schedules").fetch_one(&db).await.unwrap();
        assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &Sink, due).await, 1);
        assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &Sink, due + 1).await, 0);
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('closeout','derive','assistant','§8 收尾完成',?)")
            .bind(due * 1000 + 1).execute(&db).await.unwrap();
        let ended = refresh(&db, "derive").await.unwrap();
        assert_eq!(ended.state, SessionState::Ended);
        assert_eq!(ended.reason.as_deref(), Some("PR #42 已合并"));
        heartbeat_external(&db, "derive").await.unwrap();
        let wakes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules").fetch_one(&db).await.unwrap();
        assert_eq!(wakes, 1);
    }

    #[tokio::test]
    async fn m139_pending_approval_overrides_live_executor_and_resumes() {
        let db = fixture().await;
        sqlx::query("UPDATE sessions SET id='derive-approval' WHERE id='derive'").execute(&db).await.unwrap();
        let control = crate::ChatRunControl::pending();
        crate::register_session_executor("derive-approval", &control);
        sqlx::raw_sql("DROP TABLE permission_intents; CREATE TABLE permission_intents(session_id TEXT, status TEXT, expires_at INTEGER, created_at INTEGER, tool_name TEXT, prompt_args_json TEXT);
            INSERT INTO permission_intents VALUES ('derive-approval','pending',9223372036854775807,100,'bash','{\"command\":\"rm -rf generated\"}');")
            .execute(&db).await.unwrap();
        let waiting = refresh(&db, "derive-approval").await.unwrap();
        assert_eq!(waiting.state, SessionState::WaitingUser);
        assert_eq!(waiting.question.as_deref(), Some("需要你批准执行：rm -rf generated"));
        sqlx::query("UPDATE permission_intents SET status='allowed'").execute(&db).await.unwrap();
        let resumed = refresh(&db, "derive-approval").await.unwrap();
        assert_eq!(resumed.state, SessionState::Running);
        assert!(resumed.question.is_none());
        crate::unregister_session_executor("derive-approval", &control);
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
