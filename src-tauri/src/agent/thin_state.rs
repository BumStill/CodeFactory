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
            state: SessionState::WaitingUser,
            waiting_for: Some(what.clone()),
            question: Some(format!(
                "需要你决定是否继续等待 {what}；目前没有唤醒源。你可以让我挂监视继续等，或结束这个等待。"
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
           updated_at=excluded.updated_at",
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
    let state = if error_text.contains("no_model_available") || super::failover::ModelUnavailable::parse(error_text).is_some() {
        after_no_model(60 * 1_000)
    } else {
        record_technical_failure(db, session_id, &first_line(error_text)).await?
    };
    save(db, session_id, &state).await?;
    project_turn(db, root_turn_id, &state).await?;
    Ok(state)
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("agent loop error").chars().take(160).collect()
}

async fn project_turn(db: &SqlitePool, root_turn_id: &str, state: &ThinState) -> sqlx::Result<()> {
    let (phase, kind, label) = match state.state {
        SessionState::Running => ("working", "thin_running", "运行中".to_string()),
        SessionState::WaitingWake => ("waiting", "thin_waiting_wake", format!("等唤醒：{}", state.waiting_for.clone().unwrap_or_default())),
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
