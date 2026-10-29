// SPDX-License-Identifier: Apache-2.0
//! R1/R2/R4/R5/R7: the four authoritative thin-session states.
//!
//! Thin sessions no longer borrow legacy Objective states, failure codes,
//! remediations or decision rows. A final answer is recorded exactly once and
//! every wait names its wake source; a wait with no wake source is a question
//! for the user, never a silent wait. The supervisor loop only heartbeats these
//! rows: it wakes a due wait and turns an expired wake source into a question.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// R4: a `waiting_wake` row whose wake source disappeared is surfaced to the
/// user after this long instead of waiting forever for a caller that is gone.
pub const WAKE_EXPIRY_MS: i64 = 30 * 60 * 1_000;
/// R4: the heartbeat cadence is the supervisor's own slow sweep interval.
pub const HEARTBEAT_MS: i64 = 30 * 60 * 1_000;
/// R5: the third identical real technical failure ends the task.
pub const FAILURE_LIMIT: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Running,
    WaitingWake,
    WaitingUser,
    Ended,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::WaitingWake => "waiting_wake",
            Self::WaitingUser => "waiting_user",
            Self::Ended => "ended",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "running" => Some(Self::Running),
            "waiting_wake" => Some(Self::WaitingWake),
            "waiting_user" => Some(Self::WaitingUser),
            "ended" => Some(Self::Ended),
            _ => None,
        }
    }
}

/// What a final answer says is still outstanding. `None` means nothing external
/// is in flight, so the one completion rule ends the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitNeed {
    None,
    External { what: String, covered: bool },
    /// The answer waits on a person or an invented confirmation. The user must
    /// see the question; there is no wake source to invent for them.
    Unwatched { what: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinState {
    pub state: SessionState,
    pub waiting_for: Option<String>,
    pub wake_source: Option<String>,
    pub question: Option<String>,
    pub reason: Option<String>,
    pub outcome_location: Option<String>,
    pub failure_signature: Option<String>,
    pub consecutive_failures: i64,
    pub updated_at: i64,
}

impl ThinState {
    pub fn running() -> Self {
        Self {
            state: SessionState::Running,
            waiting_for: None,
            wake_source: None,
            question: None,
            reason: None,
            outcome_location: None,
            failure_signature: None,
            consecutive_failures: 0,
            updated_at: now(),
        }
    }

    pub fn ended(reason: &str, location: &str) -> Self {
        Self {
            state: SessionState::Ended,
            reason: Some(reason.to_string()),
            outcome_location: Some(location.to_string()),
            ..Self::running()
        }
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// R2/R3: one completion rule. External work with a wake source waits; external
/// work without one, or a wait on someone who is not the user, asks the user.
pub fn after_final(need: WaitNeed, outcome_location: &str) -> ThinState {
    match need {
        WaitNeed::None => ThinState::ended("任务已结束", outcome_location),
        WaitNeed::External { what, covered: true } => ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some(what.clone()),
            wake_source: Some(format!("已登记的唤醒源：{what}")),
            ..ThinState::running()
        },
        WaitNeed::External { what, covered: false } => ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some(what.clone()),
            // R5：外部事项（PR/CI/发版）是模型自己能监视的对象。没有唤醒源时，
            // 工具结果层面已经提示挂监视（background_task_start / schedule_wakeup），
            // 这里不再把它升级成「需要用户决定」的问题；R4 的失效守卫仍在 30 分钟
            // 后兜底，避免真的无人看管。
            reason: Some(format!(
                "外部事项未挂监视：等 {what}，可用 background_task_start 或 schedule_wakeup 监视"
            )),
            ..ThinState::running()
        },
        WaitNeed::Unwatched { what } => ThinState {
            state: SessionState::WaitingUser,
            waiting_for: Some(what.clone()),
            question: Some(format!(
                "我在等「{what}」，但这不是我能自己确认的对象。请你决定是否继续，或告诉我下一步。"
            )),
            ..ThinState::running()
        },
    }
}

/// R5: the same real technical failure three times ends the task; anything
/// earlier is still running.
pub fn after_failure(streak: i64, what: &str) -> ThinState {
    if streak >= FAILURE_LIMIT {
        ThinState {
            state: SessionState::Ended,
            reason: Some(format!("同一个技术失败连续 {streak} 次：{what}")),
            outcome_location: Some("试过的步骤与保留的改动见本会话记录".to_string()),
            failure_signature: Some(what.to_string()),
            consecutive_failures: streak,
            ..ThinState::running()
        }
    } else {
        ThinState {
            failure_signature: Some(what.to_string()),
            consecutive_failures: streak,
            ..ThinState::running()
        }
    }
}

/// R5/M123: no model is a wait with a real wake source (the model coming back),
/// not a technical failure and not a prompt for the user.
pub fn after_no_model(lead_ms: i64) -> ThinState {
    let at = now().saturating_add(lead_ms.max(1_000));
    ThinState {
        state: SessionState::WaitingWake,
        waiting_for: Some("模型恢复可用".to_string()),
        reason: Some("模型暂不可用，系统将在恢复时间自动继续，无需操作。".into()),
        wake_source: Some(format!("模型恢复时间 {}", at)),
        ..ThinState::running()
    }
}

pub async fn ensure(db: &SqlitePool) -> sqlx::Result<()> {
    sqlx::raw_sql(include_str!("../../migrations/0024_thin_session_state.sql"))
        .execute(db)
        .await?;
    Ok(())
}

pub async fn save(db: &SqlitePool, session_id: &str, state: &ThinState) -> sqlx::Result<()> {
    ensure(db).await?;
    sqlx::query(
        "INSERT INTO thin_session_state
         (session_id, state, waiting_for, wake_source, question, reason,
          outcome_location, failure_signature, consecutive_failures, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(session_id) DO UPDATE SET
           state=excluded.state, waiting_for=excluded.waiting_for,
           wake_source=excluded.wake_source, question=excluded.question,
           reason=excluded.reason, outcome_location=excluded.outcome_location,
           failure_signature=excluded.failure_signature,
           consecutive_failures=excluded.consecutive_failures,
           updated_at=excluded.updated_at
         WHERE NOT (thin_session_state.state='ended'
                    AND thin_session_state.reason LIKE 'cancelled%')",
    )
    .bind(session_id)
    .bind(state.state.as_str())
    .bind(&state.waiting_for)
    .bind(&state.wake_source)
    .bind(&state.question)
    .bind(&state.reason)
    .bind(&state.outcome_location)
    .bind(&state.failure_signature)
    .bind(state.consecutive_failures)
    .bind(state.updated_at)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn load(db: &SqlitePool, session_id: &str) -> sqlx::Result<Option<ThinState>> {
    ensure(db).await?;
    let row: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64)> =
        sqlx::query_as(
            "SELECT state, waiting_for, wake_source, question, reason,
                    outcome_location, failure_signature, consecutive_failures, updated_at
             FROM thin_session_state WHERE session_id=?",
        )
        .bind(session_id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|(state, waiting_for, wake_source, question, reason, outcome_location, failure_signature, consecutive_failures, updated_at)| ThinState {
        state: SessionState::parse(&state).unwrap_or(SessionState::Running),
        waiting_for,
        wake_source,
        question,
        reason,
        outcome_location,
        failure_signature,
        consecutive_failures,
        updated_at,
    }))
}

/// Count one real technical failure against the same signature, resetting the
/// streak when the failure is a different one.
pub async fn record_technical_failure(
    db: &SqlitePool,
    session_id: &str,
    what: &str,
) -> sqlx::Result<ThinState> {
    let previous = load(db, session_id).await?.unwrap_or_else(ThinState::running);
    let streak = if previous.failure_signature.as_deref() == Some(what) {
        previous.consecutive_failures + 1
    } else {
        1
    };
    let next = after_failure(streak, what);
    save(db, session_id, &next).await?;
    Ok(next)
}

pub async fn clear_failure(db: &SqlitePool, session_id: &str) -> sqlx::Result<()> {
    let mut state = load(db, session_id).await?.unwrap_or_else(ThinState::running);
    state.failure_signature = None;
    state.consecutive_failures = 0;
    state.updated_at = now();
    save(db, session_id, &state).await
}

/// R4 heartbeat: a waiting_wake row keeps running while its wake source exists.
/// Once the source is gone and the wait outlived [`WAKE_EXPIRY_MS`], the user
/// is asked what to do instead of the session waiting invisibly.
pub async fn sweep(db: &SqlitePool, now_ms: i64) -> sqlx::Result<usize> {
    ensure(db).await?;
    let rows: Vec<(String, Option<String>, String, Option<String>, Option<String>, i64)> =
        sqlx::query_as(
            "SELECT session_id, waiting_for, state, question, reason, updated_at
             FROM thin_session_state WHERE state='waiting_wake'",
        )
        .fetch_all(db)
        .await?;
    let mut surfaced = 0;
    for (session_id, waiting_for, _state, _question, _reason, updated_at) in rows {
        let cover = super::background_tasks::wake_source_cover(db, &session_id).await;
        if cover.is_covered() || now_ms.saturating_sub(updated_at) < WAKE_EXPIRY_MS {
            continue;
        }
        let what = waiting_for.unwrap_or_else(|| "未命名事项".to_string());
        let next = ThinState {
            state: SessionState::WaitingUser,
            waiting_for: Some(what.clone()),
            question: Some(format!(
                "你等的 {what} 已失效（唤醒源不存在或已过期）。需要你决定是否继续等待或结束。"
            )),
            ..ThinState::running()
        };
        save(db, &session_id, &next).await?;
        surfaced += 1;
    }
    Ok(surfaced)
}

/// R7: thin settlement writes this one table plus the opaque turn projection.
/// It never inserts an Objective remediation, recovery attempt, binding or
/// decision row, so no legacy failure code can surface on a thin session.
pub async fn settle_final(
    db: &SqlitePool,
    session_id: &str,
    root_turn_id: &str,
    external_what: Option<String>,
) -> sqlx::Result<ThinState> {
    if let Some(state) = load(db, session_id).await?.filter(|state| {
        state.state == SessionState::Ended
            && state.reason.as_deref().is_some_and(|reason| reason.starts_with("cancelled"))
    }) {
        return Ok(state);
    }
    let cover = super::background_tasks::wake_source_cover(db, session_id).await;
    let need = match external_what {
        Some(what) => WaitNeed::External {
            what,
            covered: cover.is_covered(),
        },
        None if cover.is_covered() => WaitNeed::External {
            what: "已登记的唤醒源（后台任务或定时唤醒）".to_string(),
            covered: true,
        },
        None => match super::thin::unwatched_wait(db, session_id).await {
            // R5：名字里是外部事项（PR/CI/发版/部署/流水线）的等待，是模型能自己
            // 监视的对象，不能被 R3 当成「等一个人」而升级成给用户的问题。
            Some(what) if super::thin::is_external_object(&what) => WaitNeed::External {
                what,
                covered: false,
            },
            Some(what) => WaitNeed::Unwatched { what },
            None => WaitNeed::None,
        },
    };
    let state = after_final(need, "本会话记录与工作区改动");
    save(db, session_id, &state).await?;
    project_turn(db, root_turn_id, &state).await?;
    Ok(state)
}

pub async fn settle_error(db: &SqlitePool, session_id: &str, root_turn_id: &str, error_text: &str) -> sqlx::Result<ThinState> {
    if let Some(state) = load(db, session_id).await?.filter(|state| {
        state.state == SessionState::Ended
            && state.reason.as_deref().is_some_and(|reason| reason.starts_with("cancelled"))
    }) {
        return Ok(state);
    }
    let state = if error_text == "agent loop stopped: Cancelled" {
        ThinState::ended("cancelled：回合已取消", "本会话记录")
    } else if let Some(unavailable) = super::failover::ModelUnavailable::parse(error_text) {
        let at = unavailable.resume_at_ms.unwrap_or_else(|| now().saturating_add(60_000));
        super::background_tasks::persist_model_wakeup(db, session_id, root_turn_id, at, &unavailable.notice).await?;
        ThinState {
            waiting_for: Some(unavailable.notice.clone()),
            reason: Some(unavailable.notice),
            wake_source: Some(format!("model-availability-{session_id}")),
            ..after_no_model(at.saturating_sub(now()))
        }
    } else if error_text.contains("no_model_available") {
        let at = now().saturating_add(60_000);
        super::background_tasks::persist_model_wakeup(db, session_id, root_turn_id, at, "模型暂不可用").await?;
        after_no_model(60_000)
    } else {
        record_technical_failure(db, session_id, &first_line(error_text)).await?
    };
    save(db, session_id, &state).await?;
    project_turn(db, root_turn_id, &state).await?;
    Ok(state)
}

/// 阶段 2 P0/R1：thin 回合**唯一**的结算判定。
///
/// 只要本轮已经产出了最终回答（`final_text` 非空），状态就必须结算为最终回答
/// （→ `ended`/`waiting_*`）。`StopReason::Blocked` 是完成闸门在拿到最终文本后
/// 收束回合的有界停止，把用户可见的最终回答当成技术失败会让行永久停在
/// `running`（v1.91.1 会话 bee5d6b7 的现场），而且之后再也没有回合来改写它。
pub fn outcome_settles_final(finished: bool, blocked: bool, final_text: &str) -> bool {
    finished || (blocked && !final_text.trim().is_empty())
}

/// 阶段 2 P0/R1：最终回答只在这一处结算。
///
/// `stop_label` 只用于技术失败签名，调用方不得再自行判断「finished 才算最终」。
#[allow(clippy::too_many_arguments)]
pub async fn settle_outcome(
    db: &SqlitePool,
    session_id: &str,
    root_turn_id: &str,
    finished: bool,
    blocked: bool,
    final_text: &str,
    stop_label: &str,
    external_what: Option<String>,
) -> sqlx::Result<ThinState> {
    // M179: permission transport/identity reconciliation is system-owned,
    // not a real technical failure. Use the persisted typed turn fact, never
    // model prose or the broad PlatformIncident label alone.
    if !finished && stop_label == "PlatformIncident" {
        let permission_wait: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM chat_turn_state WHERE root_turn_id=?
             AND session_id=? AND status='waiting'
             AND terminal_reason IN ('permission_channel_closed','permission_timed_out'))",
        ).bind(root_turn_id).bind(session_id).fetch_one(db).await?;
        if permission_wait {
            // list_wakeups initializes the durable scheduler schema as well.
            super::background_tasks::list_wakeups(db, session_id).await
                .map_err(sqlx::Error::Protocol)?;
            let wake_id = format!("permission-reconcile-{session_id}");
            let notice = "权限请求与当前任务暂未对齐，未执行操作；系统自动重试/自愈，工作区和改动保留";
            sqlx::query(
                "INSERT INTO wakeup_schedules(id,session_id,root_turn_id,prompt,source,next_at,status,created_at)
                 VALUES (?,?,?,?,'agent_decided',?,'pending',?)
                 ON CONFLICT(id) DO UPDATE SET root_turn_id=excluded.root_turn_id,
                   prompt=excluded.prompt,next_at=excluded.next_at,status='pending',last_fired_at=NULL",
            ).bind(&wake_id).bind(session_id).bind(root_turn_id)
                .bind(format!("{notice}。重新核对当前会话 Objective 及最新 revision，继续原任务；不得重放已执行操作或沿用失效授权。"))
                .bind(now().div_euclid(1000) + 60).bind(now().div_euclid(1000)).execute(db).await?;
            let state = ThinState {
                state: SessionState::WaitingWake,
                waiting_for: Some(notice.into()), wake_source: Some(wake_id),
                reason: Some(notice.into()), ..ThinState::running()
            };
            save(db, session_id, &state).await?;
            project_turn(db, root_turn_id, &state).await?;
            return Ok(state);
        }
    }
    // M160: a bounded delivery stop is not a completed task. Read the typed
    // turn fact, never classify the model's final text.
    if blocked {
        let pending: Option<String> = sqlx::query_scalar("SELECT waiting_reason FROM chat_turn_state WHERE root_turn_id=? AND terminal_reason='delivery_pending'")
            .bind(root_turn_id).fetch_optional(db).await?.flatten();
        if let Some(reason) = pending {
            let state = ask_user(db, session_id, &reason).await?;
            project_turn(db, root_turn_id, &state).await?;
            return Ok(state);
        }
    }
    if outcome_settles_final(finished, blocked, final_text) {
        settle_final(db, session_id, root_turn_id, external_what).await
    } else {
        settle_error(
            db,
            session_id,
            root_turn_id,
            &format!("agent loop stopped: {stop_label}"),
        )
        .await
    }
}

/// 阶段 2 P0/R2：没有任何可续跑的回合时，thin 会话必须发问，不能留在 `running`。
pub async fn ask_user(db: &SqlitePool, session_id: &str, question: &str) -> sqlx::Result<ThinState> {
    let mut state = ThinState::running();
    state.state = SessionState::WaitingUser;
    state.question = Some(question.to_string());
    state.updated_at = now();
    save(db, session_id, &state).await?;
    Ok(state)
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("agent loop error").chars().take(160).collect()
}

/// M197/R4：等待标签必须同时写明「在等什么」和「预计何时回来」；两者都不能是空的。
fn waiting_wake_label(state: &ThinState) -> String {
    let what = state.waiting_for.clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "外部事项".to_string());
    let when = state.wake_source.clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "系统会在下一次检查时自动继续".to_string());
    format!("等唤醒：{what}（{when}）")
}

pub(crate) async fn project_turn(db: &SqlitePool, root_turn_id: &str, state: &ThinState) -> sqlx::Result<()> {
    let (phase, kind, label) = match state.state {
        SessionState::Running => ("working", "thin_running", "运行中".to_string()),
        // M197/R4：「等待中」必须写明在等什么、预计何时回来，不允许空原因。
        SessionState::WaitingWake => ("waiting", "thin_waiting_wake", waiting_wake_label(state)),
        SessionState::WaitingUser => ("waiting", "thin_waiting_user", state.question.clone().unwrap_or_else(|| "需要你决定下一步".into())),
        SessionState::Ended => ("finalizing", "thin_ended", state.reason.clone().unwrap_or_else(|| "已结束".into())),
    };
    let terminal = (state.state == SessionState::Ended).then(|| state.reason.clone().unwrap_or_else(|| "已结束".into()));
    sqlx::query(
        "UPDATE chat_turn_state SET revision=revision+1, phase=?, status=?,
           recent_activity_kind=?, recent_activity_label=?, waiting_reason=?,
           updated_at=?,
           completed_at=CASE WHEN terminal_revision IS NULL AND ? IS NOT NULL THEN ? ELSE completed_at END,
           terminal_reason=CASE WHEN terminal_revision IS NULL AND ? IS NOT NULL THEN ? ELSE terminal_reason END
         WHERE root_turn_id=?",
    )
    .bind(phase).bind(phase).bind(kind).bind(label)
    .bind(state.question.clone().or(state.waiting_for.clone()))
    .bind(now())
    .bind(terminal.as_deref()).bind(now())
    .bind(terminal.as_deref()).bind(terminal.as_deref())
    .bind(root_turn_id)
    .execute(db).await?;
    Ok(())
}

pub async fn close_objective(db: &SqlitePool, objective_id: &str, state: &ThinState) -> sqlx::Result<()> {
    if state.state != SessionState::Ended { return Ok(()); }
    sqlx::query(
        // `legacy_orphan` is the only terminal status that claims no legacy
        // completion evidence. The four-state row above is the actual outcome.
        "UPDATE objectives SET status='legacy_orphan',
           failure_code=NULL, failure_signature=NULL, remediation_id=NULL,
           next_observation_at=NULL, completed_at=?, updated_at=?
         WHERE id=? AND status NOT IN ('completed','cancelled','legacy_orphan')",
    ).bind(now()).bind(now()).bind(objective_id).execute(db).await?;
    Ok(())
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    #[tokio::test]
    async fn m178_wait_has_reason_and_survives_real_process_restart() {
        const ENV: &str = "CODEFACTORY_M178_SYNTHETIC_DB";
        if let Ok(url) = std::env::var(ENV) {
            let db = crate::storage::db::connect(&url).await.unwrap();
            if std::env::var("CODEFACTORY_M178_PHASE").unwrap() == "write" {
                sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('m178','synthetic','/synthetic','model',1,1,'thin')").execute(&db).await.unwrap();
                let at = chrono::Utc::now().timestamp_millis() + 60_000;
                let error = super::super::failover::ModelUnavailable { resume_at_ms: Some(at), notice: "DeepSeek 时段外，系统自动重试/自愈，到点自动继续".into() }.encode();
                for _ in 0..3 {
                    let state = settle_error(&db, "m178", "m178-root", &error).await.unwrap();
                    assert_eq!(state.state, SessionState::WaitingWake);
                    assert_eq!(state.consecutive_failures, 0);
                    assert!(state.failure_signature.is_none());
                    assert!(state.reason.as_deref().is_some_and(|reason| !reason.trim().is_empty()));
                }
            } else {
                let state = load(&db, "m178").await.unwrap().unwrap();
                assert_eq!(state.state, SessionState::WaitingWake);
                assert_eq!(state.consecutive_failures, 0);
                assert!(state.reason.as_deref().unwrap().contains("自动继续"));
                let wakes = super::super::background_tasks::list_wakeups(&db, "m178").await.unwrap();
                assert_eq!(wakes.len(), 1);
                assert_eq!(wakes[0].root_turn_id.as_deref(), Some("m178-root"));
                struct Sink(std::sync::Arc<std::sync::atomic::AtomicUsize>);
                impl super::super::background_tasks::WakeSink for Sink {
                    fn enqueue(&self, session: String, text: String) -> super::super::background_tasks::WakeFuture {
                        assert_eq!(session, "m178");
                        assert!(text.contains("自动继续"));
                        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Box::pin(async { true })
                    }
                }
                let sink = Sink(Default::default());
                let at = wakes[0].next_at;
                assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &sink, at - 1).await, 0);
                assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &sink, at).await, 1);
                assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &sink, at).await, 0);
                assert_eq!(sink.0.load(std::sync::atomic::Ordering::SeqCst), 1);
            }
            db.close().await;
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("m178.db").display());
        for phase in ["write", "resume"] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "agent::thin_state::settle_tests::m178_wait_has_reason_and_survives_real_process_restart", "--nocapture"])
                .env(ENV, &url).env("CODEFACTORY_M178_PHASE", phase).output().unwrap();
            assert!(result.status.success(), "{phase}: {} {}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        }
        assert!(after_no_model(60_000).reason.as_deref().is_some_and(|reason| reason.contains("自动继续")));
    }

    async fn db() -> SqlitePool {
        let db = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, revision INTEGER NOT NULL DEFAULT 0,
               phase TEXT, status TEXT, recent_activity_kind TEXT,
               recent_activity_label TEXT, waiting_reason TEXT, updated_at INTEGER,
               completed_at INTEGER, terminal_revision INTEGER, terminal_reason TEXT, session_id TEXT);
             CREATE TABLE objectives (id TEXT PRIMARY KEY, status TEXT NOT NULL, decision_type TEXT,
               requires_user_action INTEGER, attention_request_json TEXT, failure_code TEXT,
               failure_signature TEXT, remediation_id TEXT, next_observation_at INTEGER,
               completed_at INTEGER, updated_at INTEGER);
             INSERT INTO chat_turn_state (root_turn_id, session_id) VALUES ('root-1','s-1');
             INSERT INTO objectives (id,status) VALUES ('o-1','active');",
        ).execute(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn m179_three_permission_reconciliations_keep_session_and_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("permission.db").display());
        let db = crate::storage::db::connect(&url).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('m179','synthetic','/synthetic','model',1,1,'thin')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,status,phase,revision,started_at,updated_at,terminal_reason,waiting_reason) VALUES ('m179-root','m179','waiting','waiting',1,1,1,'permission_channel_closed','系统正在核对权限绑定')").execute(&db).await.unwrap();
        for _ in 0..3 {
            let state = settle_outcome(&db, "m179", "m179-root", false, false, "", "PlatformIncident", None).await.unwrap();
            assert_eq!(state.state, SessionState::WaitingWake);
            assert_eq!(state.consecutive_failures, 0);
            assert!(state.waiting_for.as_deref().unwrap().contains("系统自动重试/自愈"));
            assert_eq!(super::super::background_tasks::list_wakeups(&db, "m179").await.unwrap().len(), 1);
        }
        let completed: Option<i64> = sqlx::query_scalar("SELECT completed_at FROM chat_turn_state WHERE root_turn_id='m179-root'").fetch_one(&db).await.unwrap();
        assert!(completed.is_none());
        db.close().await;
    }

    #[tokio::test]
    async fn m160_delivery_limit_is_waiting_user_not_completed() {
        let db = db().await;
        sqlx::query("UPDATE chat_turn_state SET terminal_reason='delivery_pending', waiting_reason='交付仍未完成，改动保留' WHERE root_turn_id='root-1'").execute(&db).await.unwrap();
        let state = settle_outcome(&db, "s-1", "root-1", false, true, "not delivered", "Blocked", None).await.unwrap();
        assert_eq!(state.state, SessionState::WaitingUser);
        assert_eq!(state.question.as_deref(), Some("交付仍未完成，改动保留"));
        assert_eq!(load(&db, "s-1").await.unwrap().unwrap().state, SessionState::WaitingUser);
        close_objective(&db, "o-1", &state).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM objectives WHERE id='o-1'").fetch_one(&db).await.unwrap();
        assert_eq!(status, "active", "pending delivery must not terminalize the objective");
    }

    #[tokio::test]
    async fn m164_model_wait_persists_exact_wake_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("restart.db").display());
        let db = crate::storage::db::connect(&url).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('m164','synthetic','/synthetic','model',1,1,'thin')").execute(&db).await.unwrap();
        let due = now() + 7_200_250;
        let unavailable = super::super::failover::ModelUnavailable {
            resume_at_ms: Some(due), notice: "GPT 已到你设的 80% 上限，约到点自动继续".into(),
        };
        let state = settle_error(&db, "m164", "root-m164", &unavailable.encode()).await.unwrap();
        assert_eq!(state.state, SessionState::WaitingWake);
        let wakes = super::super::background_tasks::list_wakeups(&db, "m164").await.unwrap();
        assert_eq!(wakes.len(), 1, "waiting_wake 必须有真实落盘调度，而非仅 wake_source 文本");
        assert_eq!(wakes[0].next_at, (due + 999) / 1000);
        settle_error(&db, "m164", "root-m164", &unavailable.encode()).await.unwrap();
        assert_eq!(super::super::background_tasks::list_wakeups(&db, "m164").await.unwrap().len(), 1);
        db.close().await;
        let db = crate::storage::db::connect(&url).await.unwrap();
        struct Sink(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl super::super::background_tasks::WakeSink for Sink {
            fn enqueue(&self, session: String, text: String) -> super::super::background_tasks::WakeFuture {
                assert_eq!(session, "m164");
                assert!(text.contains("系统自动重试/自愈"));
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async { true })
            }
        }
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink = Sink(calls.clone());
        let at = (due + 999) / 1000;
        assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &sink, at - 1).await, 0);
        assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &sink, at).await, 1);
        assert_eq!(super::super::background_tasks::drain_due_wakeups_at(&db, &sink, at + 1).await, 0);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        db.close().await;
    }

    #[tokio::test]
    async fn r6_stop_ends_thin_sessions_and_late_errors_cannot_reopen_them() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        let store = crate::agent::objective::ObjectiveStore::new(db.clone());
        for mode in ["thin", "legacy"] {
            sqlx::query("INSERT INTO sessions (id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES (?, 'synthetic', '/synthetic', 'model', 1, 1, ?)")
                .bind(mode).bind(mode).execute(&db).await.unwrap();
            if mode == "thin" { save(&db, mode, &ThinState::running()).await.unwrap(); }
            store.request_chat_session_cancel(mode).await.unwrap();
            if mode == "thin" {
                let stopped = load(&db, mode).await.unwrap().unwrap();
                assert_eq!(stopped.state, SessionState::Ended);
                assert_eq!(stopped.reason.as_deref(), Some("cancelled：用户已停止会话"));
                let late = settle_error(&db, mode, "missing-root", "late transport error").await.unwrap();
                assert_eq!(late.state, SessionState::Ended);
                assert_eq!(late.reason, stopped.reason);
                let late_final = settle_final(&db, mode, "missing-root", None).await.unwrap();
                assert_eq!(late_final.reason, stopped.reason);
                save(&db, mode, &ThinState::running()).await.unwrap();
                assert_eq!(load(&db, mode).await.unwrap().unwrap().reason, stopped.reason);
            } else {
                assert!(load(&db, mode).await.unwrap().is_none());
            }
            store.consume_chat_session_cancel(mode).await.unwrap();
        }
    }

    #[tokio::test]
    async fn r6_exact_objective_cancel_ends_thin_state() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO sessions (id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('exact','synthetic','/synthetic','model',1,1,'thin')")
            .execute(&db).await.unwrap();
        use crate::agent::objective::{CreateObjective, ObjectiveKind, ObjectiveStore, RecoveryDomain};
        let store = ObjectiveStore::new(db.clone());
        store.create(CreateObjective {
            id: "exact-objective".into(), kind: ObjectiveKind::Informational,
            session_id: Some("exact".into()), root_turn_id: Some("exact-root".into()),
            domain: RecoveryDomain::Chat, requested_acceptance: "answer".into(), created_surface: "synthetic".into(),
        }).await.unwrap();
        sqlx::query("INSERT INTO chat_turn_state (root_turn_id,session_id,objective_id,phase,status,started_at,updated_at) VALUES ('exact-root','exact','exact-objective','working','active',1,1)")
            .execute(&db).await.unwrap();
        save(&db, "exact", &ThinState::running()).await.unwrap();
        store.cancel_chat_exact("exact", "exact-root", "exact-objective").await.unwrap();
        let ended = load(&db, "exact").await.unwrap().unwrap();
        assert_eq!(ended.state, SessionState::Ended);
        assert!(ended.reason.unwrap().contains("cancelled"));
    }

    #[tokio::test]
    async fn r6_cancelled_outcome_is_terminal_not_a_technical_failure() {
        let db = db().await;
        let state = settle_outcome(&db, "s-1", "root-1", false, false, "", "Cancelled", None).await.unwrap();
        assert_eq!(state.state, SessionState::Ended);
        assert!(state.reason.as_deref().unwrap().contains("cancelled"));
        assert_eq!(state.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn thin_final_settlement_writes_only_the_four_state_authority() {
        let db = db().await;
        let state = settle_final(&db, "s-1", "root-1", None).await.unwrap();
        assert_eq!(state.state, SessionState::Ended);
        close_objective(&db, "o-1", &state).await.unwrap();
        let (kind, terminal): (String, Option<String>) = sqlx::query_as(
            "SELECT recent_activity_kind, terminal_reason FROM chat_turn_state WHERE root_turn_id='root-1'")
            .fetch_one(&db).await.unwrap();
        assert_eq!(kind, "thin_ended");
        assert_eq!(terminal.as_deref(), Some("任务已结束"));
        let failure: Option<String> = sqlx::query_scalar("SELECT failure_code FROM objectives WHERE id='o-1'")
            .fetch_one(&db).await.unwrap();
        assert!(failure.is_none(), "thin settlement must not project a legacy failure code");
    }

    #[tokio::test]
    async fn second_and_third_identical_failure_are_visible_and_terminal() {
        let db = db().await;
        let first = settle_error(&db, "s-1", "root-1", "bash exited 1").await.unwrap();
        assert_eq!(first.state, SessionState::Running);
        let second = settle_error(&db, "s-1", "root-1", "bash exited 1").await.unwrap();
        assert_eq!(second.state, SessionState::Running);
        let third = settle_error(&db, "s-1", "root-1", "bash exited 1").await.unwrap();
        assert_eq!(third.state, SessionState::Ended);
        assert!(third.reason.unwrap().contains("bash exited 1"));
        let model = settle_error(&db, "s-1", "root-1", "no endpoints are available: no_model_available").await.unwrap();
        assert_eq!(model.state, SessionState::WaitingWake);
    }
}

#[cfg(test)]
mod m197_tests {
    use super::*;

    /// R4：等待标签不允许只有「等唤醒：」这种空原因。
    #[test]
    fn m197_waiting_label_names_the_wait_and_the_wake_time() {
        let labelled = ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some("PR #900 合并/CI".into()),
            wake_source: Some("系统 PR 监视（≥180s）".into()),
            ..ThinState::running()
        };
        let label = waiting_wake_label(&labelled);
        assert!(label.contains("PR #900"), "{label}");
        assert!(label.contains("≥180s"), "{label}");
        assert!(!label.trim_end().ends_with('：'), "{label}");

        let empty = ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some("   ".into()),
            wake_source: None,
            ..ThinState::running()
        };
        let label = waiting_wake_label(&empty);
        assert!(label.contains("外部事项"), "{label}");
        assert!(label.contains("下一次检查"), "{label}");
        assert_ne!(label.trim(), "等唤醒：", "empty wait reason must not be projected");
    }
}
