// SPDX-License-Identifier: Apache-2.0
//! M151：会话级诊断与状态重置。
//!
//! 现场（v1.91.21，Windows 升级后存量会话）：会话永远显示「正在做 · 等待中」，
//! 用户既发不出新消息，编排方也拿不到任何事实去判断为什么卡住——
//! 只有设置页的扩展诊断，没有「把这个会话为什么卡住」导出的能力。
//!
//! 本模块只做两件事：
//! 1. `collect`——把「这个会话现在到底卡在哪」的全部依据导出成脱敏 JSON
//!    （不含消息正文、不含凭据），这是外部唯一可用的观测事实；
//! 2. `reset_session_state`——在会话**没有**活执行者时，结算残留取消意图、
//!    回收无主运行控制行、把状态重算成 `ended`/`waiting_user`，让下一条消息
//!    能正常开始。它绝不删除消息、工具结果、摘要或交付记录（R9）。
use serde::Serialize;
use sqlx::SqlitePool;

use super::thin_state::{SessionState, ThinState};

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(super) async fn has_table(db: &SqlitePool, name: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
    )
    .bind(name)
    .fetch_one(db)
    .await?
        != 0)
}

async fn has_column(db: &SqlitePool, table: &str, column: &str) -> sqlx::Result<bool> {
    let rows: Vec<(String,)> =
        sqlx::query_as(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .fetch_all(db)
            .await?;
    Ok(rows.iter().any(|row| row.0 == column))
}

/// 存活判据：`chat_run_controls` 里仍处于 live 状态、且由**当前进程实例**创建的行。
///
/// 这是「回合真的被准入过」的持久事实。进程重启后旧实例的行不算数——
/// 它们只是无主的排他锁，不是活着的回合。
pub(crate) async fn live_run_control(db: &SqlitePool, session_id: &str) -> sqlx::Result<bool> {
    if !has_table(db, "chat_run_controls").await? {
        return Ok(false);
    }
    let live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_run_controls
         WHERE session_id=? AND status IN ('active','cancel_requested')
           AND created_process_instance=?",
    )
    .bind(session_id)
    .bind(super::objective::current_process_instance())
    .fetch_one(db)
    .await?;
    Ok(live > 0)
}

/// 是否存在「上一进程留下、仍占着会话级排他锁」的 live 控制行。
///
/// 这是判定「缓存里的 running 已经死了」的唯一硬证据：没有控制行的会话是
/// **含糊**的（thin 会话并不都用控制行），含糊不得降级成终态。
pub(crate) async fn dead_process_run_control(db: &SqlitePool, session_id: &str) -> sqlx::Result<bool> {
    if !has_table(db, "chat_run_controls").await? {
        return Ok(false);
    }
    let foreign: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_run_controls
         WHERE session_id=? AND status IN ('active','cancel_requested')
           AND created_process_instance<>?",
    )
    .bind(session_id)
    .bind(super::objective::current_process_instance())
    .fetch_one(db)
    .await?;
    Ok(foreign > 0)
}

/// 无主 live 控制行：会话维度上的排他锁，却没有任何活着的执行者。
pub(crate) async fn orphan_run_control_count(db: &SqlitePool, session_id: &str) -> sqlx::Result<i64> {
    if !has_table(db, "chat_run_controls").await? {
        return Ok(0);
    }
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT run_instance_id FROM chat_run_controls
         WHERE session_id=? AND status IN ('active','cancel_requested')",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .iter()
        .filter(|run| !crate::chat_run_is_live(run))
        .count() as i64)
}

#[derive(Debug, Serialize, Default)]
pub struct Evidence {
    /// 注册表是否可靠可观测（`None` 表示观测本身不可靠，恢复不得据此行动）。
    pub executor_alive: Option<bool>,
    pub live_run_control_this_process: bool,
    pub orphan_run_controls: i64,
    pub background_tasks_running: i64,
    pub pending_wakeups: i64,
    pub overdue_wakeups: i64,
    pub oldest_wakeup_delay_secs: i64,
    pub wakeup_retry_count: i64,
    pub pending_approvals: i64,
    pub pending_cancel_intent_at: Option<i64>,
    pub delivered_pr: Option<i64>,
    pub in_flight_activity: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct StateRow {
    pub state: String,
    pub waiting_for: Option<String>,
    pub wake_source: Option<String>,
    pub question: Option<String>,
    pub reason: Option<String>,
    pub outcome_location: Option<String>,
    pub updated_at: i64,
}

impl From<&ThinState> for StateRow {
    fn from(state: &ThinState) -> Self {
        Self {
            state: state.state.as_str().to_string(),
            waiting_for: state.waiting_for.clone(),
            wake_source: state.wake_source.clone(),
            question: state.question.clone(),
            reason: state.reason.clone(),
            outcome_location: state.outcome_location.clone(),
            updated_at: state.updated_at,
        }
    }
}

#[derive(Debug, Serialize, Default)]
pub struct RunControlRow {
    pub run_instance_id: String,
    pub status: String,
    pub created_process_instance: String,
    pub objective_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub settled_at: Option<i64>,
    pub live_in_this_process: bool,
}

#[derive(Debug, Serialize, Default)]
pub struct EventRow {
    pub kind: String,
    pub actor: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Default)]
pub struct MessageStats {
    /// 只有计数与时间范围：诊断绝不携带消息正文（R1 脱敏）。
    pub count: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    pub assistant_finals: i64,
    pub user_last_at: Option<i64>,
}

#[derive(Debug, Serialize, Default)]
pub struct SessionDiagnostics {
    pub schema: String,
    pub session_id: String,
    pub generated_at: i64,
    pub app_version: String,
    pub platform: String,
    pub shell: String,
    pub delivery_mode: Option<String>,
    /// 权威表里的投影（缓存）。
    pub thin_session_state: Option<StateRow>,
    /// 由运行事实重新推导的结果。
    pub derived_state: StateRow,
    /// 缓存与事实是否一致；false 就是「界面显示可能已经骗人」的信号。
    pub cache_matches_facts: bool,
    pub evidence: Evidence,
    pub run_controls: Vec<RunControlRow>,
    pub recent_events: Vec<EventRow>,
    pub messages: MessageStats,
}

async fn count(db: &SqlitePool, sql: &str, session: &str, table: &str) -> sqlx::Result<i64> {
    if !has_table(db, table).await? {
        return Ok(0);
    }
    sqlx::query_scalar(sql).bind(session).fetch_one(db).await
}

/// R1：导出这个会话卡住的全部依据。
pub async fn collect(
    db: &SqlitePool,
    session_id: &str,
    app_version: &str,
    platform: &str,
    shell: &str,
) -> anyhow::Result<SessionDiagnostics> {
    let derived = super::thin_derive::derive_state(db, session_id).await?;
    let cached = super::thin_state::load(db, session_id).await?;
    let delivery_mode: Option<String> =
        sqlx::query_scalar("SELECT delivery_mode FROM sessions WHERE id=?")
            .bind(session_id)
            .fetch_optional(db)
            .await?
            .flatten();

    let mut run_controls = Vec::new();
    if has_table(db, "chat_run_controls").await? {
        let objective = has_column(db, "chat_run_controls", "objective_id").await?;
        let sql = if objective {
            "SELECT run_instance_id,status,created_process_instance,objective_id,root_turn_id,created_at,updated_at,settled_at
             FROM chat_run_controls WHERE session_id=? ORDER BY created_at DESC LIMIT 20"
        } else {
            "SELECT run_instance_id,status,created_process_instance,NULL,root_turn_id,created_at,updated_at,settled_at
             FROM chat_run_controls WHERE session_id=? ORDER BY created_at DESC LIMIT 20"
        };
        let rows: Vec<(String, String, String, Option<String>, Option<String>, i64, i64, Option<i64>)> =
            sqlx::query_as(sql).bind(session_id).fetch_all(db).await?;
        run_controls = rows
            .into_iter()
            .map(|row| RunControlRow {
                live_in_this_process: crate::chat_run_is_live(&row.0),
                run_instance_id: row.0,
                status: row.1,
                created_process_instance: row.2,
                objective_id: row.3,
                root_turn_id: row.4,
                created_at: row.5,
                updated_at: row.6,
                settled_at: row.7,
            })
            .collect();
    }

    let mut recent_events = Vec::new();
    if has_table(db, "session_audit_events").await? {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT kind,actor,created_at FROM session_audit_events
             WHERE session_id=? ORDER BY created_at DESC LIMIT 30",
        )
        .bind(session_id)
        .fetch_all(db)
        .await?;
        recent_events = rows
            .into_iter()
            .map(|row| EventRow { kind: row.0, actor: row.1, created_at: row.2 })
            .collect();
    }

    let in_flight_activity: Option<String> = if has_table(db, "chat_turn_state").await? {
        sqlx::query_scalar(
            "SELECT recent_activity_label FROM chat_turn_state WHERE session_id=? ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(session_id)
        .fetch_optional(db)
        .await?
        .flatten()
    } else {
        None
    };

    let messages = if has_table(db, "messages").await? {
        let (count, first, last): (i64, Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT COUNT(*),MIN(created_at),MAX(created_at) FROM messages WHERE session_id=?",
        )
        .bind(session_id)
        .fetch_one(db)
        .await?;
        let assistant_finals: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages WHERE session_id=? AND role='assistant' AND TRIM(content)<>'' AND (tool_calls IS NULL OR tool_calls='' OR tool_calls='[]')",
        )
        .bind(session_id)
        .fetch_one(db)
        .await?;
        let user_last_at: Option<i64> =
            sqlx::query_scalar("SELECT MAX(created_at) FROM messages WHERE session_id=? AND role='user'")
                .bind(session_id)
                .fetch_one(db)
                .await?;
        MessageStats { count, first_at: first, last_at: last, assistant_finals, user_last_at }
    } else {
        MessageStats::default()
    };

    let pending_cancel_intent_at: Option<i64> = if has_table(db, "chat_session_cancel_intents").await? {
        sqlx::query_scalar("SELECT requested_at FROM chat_session_cancel_intents WHERE session_id=?")
            .bind(session_id)
            .fetch_optional(db)
            .await?
    } else {
        None
    };
    let pending_approvals = count(
        db,
        "SELECT COUNT(*) FROM permission_intents WHERE session_id=? AND status='pending'",
        session_id,
        "permission_intents",
    )
    .await?;
    let delivered_pr: Option<i64> = if has_table(db, "thin_delivery_refs").await? {
        sqlx::query_scalar("SELECT pr_number FROM thin_delivery_refs WHERE session_id=?")
            .bind(session_id)
            .fetch_optional(db)
            .await?
    } else {
        None
    };

    let cached_row = cached.as_ref().map(StateRow::from);
    let cache_matches_facts = cached_row.as_ref().is_some_and(|row| row.state == derived.state.as_str());
    Ok(SessionDiagnostics {
        schema: "codefactory.session_diagnostics.v1".into(),
        session_id: session_id.to_string(),
        generated_at: now_ms(),
        app_version: app_version.to_string(),
        platform: platform.to_string(),
        shell: shell.to_string(),
        delivery_mode,
        thin_session_state: cached_row,
        derived_state: StateRow::from(&derived),
        cache_matches_facts,
        evidence: Evidence {
            executor_alive: crate::session_executor_alive(session_id),
            live_run_control_this_process: live_run_control(db, session_id).await?,
            orphan_run_controls: orphan_run_control_count(db, session_id).await?,
            background_tasks_running: count(
                db,
                "SELECT COUNT(*) FROM background_tasks WHERE session_id=? AND status='running'",
                session_id,
                "background_tasks",
            )
            .await?,
            pending_wakeups: count(
                db,
                "SELECT COUNT(*) FROM wakeup_schedules WHERE session_id=? AND status='pending'",
                session_id,
                "wakeup_schedules",
            )
            .await?,
            overdue_wakeups: if has_table(db, "wakeup_schedules").await? {
                sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id=? AND status='pending' AND next_at<?")
                    .bind(session_id).bind(now_ms() / 1000 - 120).fetch_one(db).await?
            } else { 0 },
            oldest_wakeup_delay_secs: if has_table(db, "wakeup_schedules").await? {
                sqlx::query_scalar("SELECT MAX(0,COALESCE(?-MIN(next_at),0)) FROM wakeup_schedules WHERE session_id=? AND status='pending'")
                    .bind(now_ms() / 1000).bind(session_id).fetch_one(db).await?
            } else { 0 },
            wakeup_retry_count: if has_table(db, "wakeup_alerts").await? {
                sqlx::query_scalar("SELECT retry_count FROM wakeup_alerts WHERE session_id=?")
                    .bind(session_id).fetch_optional(db).await?.unwrap_or(0)
            } else { 0 },
            pending_approvals,
            pending_cancel_intent_at,
            delivered_pr,
            in_flight_activity,
        },
        run_controls,
        recent_events,
        messages,
    })
}

#[derive(Debug, Serialize, Default)]
pub struct ResetOutcome {
    pub session_id: String,
    /// false 表示拒绝重置；`reason` 说明为什么。
    pub reset: bool,
    pub reason: Option<String>,
    pub resulting_state: String,
    pub settled_cancel_intents: u64,
    pub retired_run_controls: u64,
    /// R9：重置前后消息条数必须一致。
    pub messages_before: i64,
    pub messages_after: i64,
    pub reset_at: i64,
}

/// R3/R6/R9：把一个没有活执行者的会话拉回可用的状态。
///
/// 只结算控制面（取消意图、运行控制行）与状态投影；消息、工具结果、摘要、
/// 交付记录一行都不动。
pub async fn reset_session_state(db: &SqlitePool, session_id: &str) -> anyhow::Result<ResetOutcome> {
    let at = now_ms();
    let messages_before = count(db, "SELECT COUNT(*) FROM messages WHERE session_id=?", session_id, "messages").await?;
    let mut outcome = ResetOutcome {
        session_id: session_id.to_string(),
        reset_at: at,
        messages_before,
        messages_after: messages_before,
        ..ResetOutcome::default()
    };

    if crate::session_executor_alive(session_id) == Some(true) {
        outcome.reason = Some("会话仍有活着的执行者，请先停止该回合再重置".into());
        outcome.resulting_state = super::thin_derive::derive_state(db, session_id).await?.state.as_str().to_string();
        return Ok(outcome);
    }

    if has_table(db, "chat_session_cancel_intents").await? {
        outcome.settled_cancel_intents = sqlx::query(
            "DELETE FROM chat_session_cancel_intents WHERE session_id=?",
        )
        .bind(session_id)
        .execute(db)
        .await?
        .rows_affected();
    }

    if has_table(db, "chat_run_controls").await? {
        let live: Vec<String> = sqlx::query_scalar(
            "SELECT run_instance_id FROM chat_run_controls WHERE session_id=? AND status IN ('active','cancel_requested')",
        )
        .bind(session_id)
        .fetch_all(db)
        .await?;
        for run in live {
            // 进程内仍然活着的行不动：那是真的在跑（会话级注册表可能已先一步注销）。
            if crate::chat_run_is_live(&run) {
                continue;
            }
            outcome.retired_run_controls += sqlx::query(
                "UPDATE chat_run_controls SET status='completed', settled_at=COALESCE(settled_at,?), updated_at=? WHERE run_instance_id=?",
            )
            .bind(at)
            .bind(at)
            .bind(&run)
            .execute(db)
            .await?
            .rows_affected();
        }
    }

    // 事实重算；仍被判成 running 说明缓存自持，这里直接落一个诚实的终态。
    let mut state = super::thin_derive::derive_state(db, session_id).await?;
    if state.state == SessionState::Running
        || (state.state == SessionState::WaitingWake
            && state.wake_source.as_deref() == Some("系统周期恢复扫描")) {
        state = ThinState::ended("已手动重置会话状态：残留运行状态已清理", "本会话记录");
        super::thin_state::save(db, session_id, &state).await?;
    } else {
        super::thin_derive::refresh(db, session_id).await?;
    }
    outcome.reset = true;
    outcome.resulting_state = state.state.as_str().to_string();
    outcome.messages_after = count(db, "SELECT COUNT(*) FROM messages WHERE session_id=?", session_id, "messages").await?;

    if has_table(db, "session_audit_events").await? {
        let _ = sqlx::query(
            "INSERT INTO session_audit_events(id,session_id,kind,actor,detail_json,created_at) VALUES (?,?,?,?,?,?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(session_id)
        .bind("session_state_reset")
        .bind("user")
        .bind(serde_json::json!({
            "settled_cancel_intents": outcome.settled_cancel_intents,
            "retired_run_controls": outcome.retired_run_controls,
            "resulting_state": outcome.resulting_state,
        }).to_string())
        .bind(at)
        .execute(db)
        .await;
    }
    tracing::info!(
        session = session_id,
        settled_cancel_intents = outcome.settled_cancel_intents,
        retired_run_controls = outcome.retired_run_controls,
        resulting_state = %outcome.resulting_state,
        "M151 手动重置会话状态"
    );
    Ok(outcome)
}

/// R4/R6：准入前的自愈——回收该会话的无主 live 控制行。
///
/// `chat_run_controls` 的 live 唯一索引是会话级排他锁；没有这一步，上一进程
/// 留下的一行 `active` 会让新回合永远排「等待中」。
pub(crate) async fn retire_orphan_run_controls(
    db: &SqlitePool,
    session_id: &str,
) -> sqlx::Result<u64> {
    if !has_table(db, "chat_run_controls").await? {
        return Ok(0);
    }
    let at = now_ms();
    let live: Vec<String> = sqlx::query_scalar(
        "SELECT run_instance_id FROM chat_run_controls WHERE session_id=? AND status IN ('active','cancel_requested')",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;
    let mut retired = 0u64;
    for run in live {
        if crate::chat_run_is_live(&run) {
            continue;
        }
        retired += sqlx::query(
            "UPDATE chat_run_controls SET status='completed', settled_at=COALESCE(settled_at,?), updated_at=? WHERE run_instance_id=? AND status IN ('active','cancel_requested')",
        )
        .bind(at)
        .bind(at)
        .bind(&run)
        .execute(db)
        .await?
        .rows_affected();
    }
    if retired > 0 {
        tracing::warn!(session = session_id, retired, "M151 准入前回收无主运行控制行");
    }
    Ok(retired)
}

/// R4：启动时对每个 thin 会话重算状态，修正被写坏的缓存。
pub(crate) async fn reconcile_all_thin_states(db: &SqlitePool) -> sqlx::Result<usize> {
    if !has_table(db, "thin_session_state").await? {
        return Ok(0);
    }
    let sessions: Vec<String> = sqlx::query_scalar("SELECT session_id FROM thin_session_state")
        .fetch_all(db)
        .await?;
    let mut fixed = 0usize;
    for session in sessions {
        let before = super::thin_state::load(db, &session).await?;
        let after = match super::thin_derive::refresh(db, &session).await {
            Ok(state) => state,
            Err(error) => {
                tracing::warn!(session = %session, %error, "M151 启动重算状态失败");
                continue;
            }
        };
        if before.as_ref().is_none_or(|state| state.state != after.state) {
            fixed += 1;
        }
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture(session: &str) -> SqlitePool {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES (?,'synthetic','/synthetic','model',1,1,'thin')")
            .bind(session)
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m1',?,'user','帮我改一下这个函数',100)")
            .bind(session)
            .execute(&db)
            .await
            .unwrap();
        db
    }

    #[tokio::test]
    async fn m183_diagnostics_expose_overdue_wakes_without_prompt_content() {
        let db = fixture("diag-overdue").await;
        super::super::background_tasks::schedule_wakeup_at(&db, super::super::background_tasks::WakeupRequest {
            session_id: "diag-overdue".into(), root_turn_id: None,
            prompt: "synthetic-private-reason".into(),
            source: super::super::background_tasks::TaskSource::AgentDecided,
            after_sec: Some(60), every_sec: None,
        }, 1).await.unwrap();
        let report = collect(&db, "diag-overdue", "test", "test", "test").await.unwrap();
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["evidence"]["overdue_wakeups"], 1);
        assert!(json["evidence"]["oldest_wakeup_delay_secs"].as_i64().unwrap() > 120);
        assert!(!json.to_string().contains("synthetic-private-reason"));
    }

    /// R2：缓存 running 但没有任何执行者证据时，诊断要指出缓存与事实不一致。
    #[tokio::test]
    async fn m151_diagnostics_flag_poisoned_cache_and_stay_content_free() {
        let db = fixture("diag").await;
        super::super::thin_state::save(&db, "diag", &ThinState::running()).await.unwrap();
        sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,status,created_process_instance,created_at,updated_at) VALUES ('left-over','diag','active','process-that-died',1,1)")
            .execute(&db)
            .await
            .unwrap();
        let report = collect(&db, "diag", "1.0.0", "windows", "powershell").await.unwrap();
        assert_eq!(report.schema, "codefactory.session_diagnostics.v1");
        assert_eq!(report.thin_session_state.as_ref().unwrap().state, "running");
        assert_eq!(report.derived_state.state, "ended");
        assert!(!report.cache_matches_facts);
        assert_eq!(report.messages.count, 1);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("帮我改一下这个函数"), "诊断不得携带消息正文");
        assert!(json.contains("已随进程退出"), "诊断要说明为什么被判成结束");
        assert_eq!(report.evidence.orphan_run_controls, 1);
        assert_eq!(report.run_controls.len(), 1);
        assert!(!report.run_controls[0].live_in_this_process);
    }

    /// R3/R6/R9：重置结算取消意图、回收无主控制行、保留全部消息。
    #[tokio::test]
    async fn m151_reset_makes_a_stuck_session_usable_without_losing_content() {
        let db = fixture("stuck").await;
        super::super::thin_state::save(&db, "stuck", &ThinState::running()).await.unwrap();
        sqlx::query("INSERT INTO chat_session_cancel_intents(session_id,status,requested_at,updated_at) VALUES ('stuck','requested',900,900)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,status,created_process_instance,created_at,updated_at) VALUES ('orphan','stuck','active',?,1,1)")
            .bind("process-that-died")
            .execute(&db)
            .await
            .unwrap();

        let outcome = reset_session_state(&db, "stuck").await.unwrap();
        assert!(outcome.reset, "无活执行者时必须允许重置：{:?}", outcome.reason);
        assert_eq!(outcome.settled_cancel_intents, 1);
        assert_eq!(outcome.retired_run_controls, 1);
        assert_eq!(outcome.resulting_state, "ended");
        assert_eq!(outcome.messages_before, 1);
        assert_eq!(outcome.messages_after, 1, "R9：重置绝不删除消息");
        let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_session_cancel_intents WHERE session_id='stuck'")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(intents, 0);
        let status: String = sqlx::query_scalar("SELECT status FROM chat_run_controls WHERE run_instance_id='orphan'")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(status, "completed");
        // 重置后下一条消息不会再被残留状态挡住。
        assert_eq!(super::super::thin_derive::derive_state(&db, "stuck").await.unwrap().state, SessionState::Ended);
        let settled = super::super::thin_state::load(&db, "stuck").await.unwrap().unwrap();
        assert_eq!(settled.state, SessionState::Ended);
        let reason = settled.reason.unwrap_or_default();
        assert!(
            reason.contains("没有活着的执行者") || reason.contains("已手动重置"),
            "重置后必须给出可解释的终态，实际：{reason}"
        );
    }

    /// R3：活着的回合不允许被重置。
    #[tokio::test]
    async fn m151_reset_refuses_while_the_turn_is_alive() {
        let db = fixture("live").await;
        super::super::thin_state::save(&db, "live", &ThinState::running()).await.unwrap();
        let control = crate::ChatRunControl::pending();
        crate::register_session_executor("live", &control);
        let outcome = reset_session_state(&db, "live").await.unwrap();
        assert!(!outcome.reset);
        assert!(outcome.reason.unwrap().contains("活着的执行者"));
        crate::unregister_session_executor("live", &control);
    }

    /// R6：准入前回收无主 live 控制行——包括上一进程留下的 `cancel_requested`。
    #[tokio::test]
    async fn m151_orphan_controls_are_retired_before_admission() {
        let db = fixture("admit").await;
        sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,status,created_process_instance,created_at,updated_at,cancel_requested_at) VALUES ('dead','admit','cancel_requested',?,1,1,2)")
            .bind("previous-process")
            .execute(&db)
            .await
            .unwrap();
        // 会话级 live 唯一索引：同一会话同时只允许一条 live 控制行——正是这条
        // 排他锁让上一进程留下的 `cancel_requested` 把新回合永远挡在「等待中」。
        assert_eq!(orphan_run_control_count(&db, "admit").await.unwrap(), 1);
        let retired = retire_orphan_run_controls(&db, "admit").await.unwrap();
        assert_eq!(retired, 1);
        assert_eq!(orphan_run_control_count(&db, "admit").await.unwrap(), 0);
        assert!(!live_run_control(&db, "admit").await.unwrap());
    }
}
