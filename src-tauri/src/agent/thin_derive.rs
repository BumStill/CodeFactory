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
    sqlx::query("CREATE TABLE IF NOT EXISTS thin_external_observations(session_id TEXT PRIMARY KEY,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL,observed_at INTEGER NOT NULL DEFAULT 0,completed_reason TEXT,closeout_queued INTEGER NOT NULL DEFAULT 0,merged_at INTEGER)")
        .execute(db).await?;
    // M146：历史库没有 merged_at；用它判定「合并之后是否已经收尾过」。
    if !has_column(db, "thin_external_observations", "merged_at").await? {
        sqlx::query("ALTER TABLE thin_external_observations ADD COLUMN merged_at INTEGER")
            .execute(db).await?;
    }
    Ok(())
}

async fn has_column(db: &SqlitePool, table: &str, column: &str) -> sqlx::Result<bool> {
    let rows: Vec<(String,)> = sqlx::query_as(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .fetch_all(db).await?;
    Ok(rows.iter().any(|row| row.0 == column))
}

/// M158/R3：用户可见的文案里不得出现内部状态词（唤醒源 / 外部事项 / 状态名）。
fn has_internal_wording(text: &str) -> bool {
    ["唤醒源", "外部事项", "waiting_user", "waiting_wake"]
        .iter()
        .any(|token| text.contains(token))
}

/// M158/R2：系统自己盯着的等待。外部对象之外，后台任务 / 定时唤醒这类系统
/// 占位等待同样不属于用户的决定——它只能由系统心跳跟进或结束，不能问用户。
fn is_system_followed(what: &str) -> bool {
    super::thin::is_external_object(what) || what.contains("后台任务") || what.contains("定时唤醒")
}

/// 从一段会话事实里认最后一个 PR 号：`/pull/<n>` 链接，或「PR #<n>」叙述。
fn last_pr_number(text: &str) -> Option<i64> {
    let mut number = None;
    let mut rest = text;
    while let Some(at) = rest.find("/pull/") {
        let tail = &rest[at + "/pull/".len()..];
        let digits: String = tail.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if let Ok(found) = digits.parse::<i64>() {
            number = Some(found);
        }
        rest = tail;
    }
    if number.is_some() {
        return number;
    }
    let has_pr_word = text.to_lowercase().contains("pr");
    if !has_pr_word {
        return None;
    }
    let mut rest = text;
    while let Some(at) = rest.find('#') {
        let tail = &rest[at + 1..];
        let digits: String = tail.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if let Ok(found) = digits.parse::<i64>() {
            number = Some(found);
        }
        rest = tail;
    }
    number
}

/// 最近会话事实里认领的 PR 号；认不出就是没有（不猜）。
async fn discovered_pr(db: &SqlitePool, session: &str) -> sqlx::Result<Option<i64>> {
    if !has_table(db, "messages").await? {
        return Ok(None);
    }
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT content FROM messages WHERE session_id=? AND (content LIKE '%/pull/%' OR content LIKE '%PR #%') ORDER BY created_at DESC LIMIT 20",
    )
    .bind(session)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().find_map(|(content,)| last_pr_number(content)))
}

/// M158/R1+R2：授权交付会话开出 PR 后，系统**自己**登记 PR 监视。
/// 先认已有的交付引用；没有时从会话事实里认领 PR 链接，登记默认交付引用
/// （ceiling: merged）并建立观测行，让既有心跳（≥180 秒、遵守 GitHub 配额
/// 策略）立刻接管。这件事绝不再升级成「请选择继续监视或结束任务」。
pub(crate) async fn ensure_pr_monitor(db: &SqlitePool, session: &str) -> sqlx::Result<Option<i64>> {
    if !has_table(db, "thin_delivery_refs").await? {
        return Ok(None);
    }
    let delivered: Option<(i64, String, String)> = if has_table(db, "tool_calls").await? {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT tool.result FROM tool_calls tool JOIN messages message ON message.id=tool.message_id WHERE message.session_id=? AND tool.tool_name='deliver_changes' AND tool.result IS NOT NULL ORDER BY tool.created_at DESC LIMIT 20")
            .bind(session).fetch_all(db).await?;
        rows.iter().find_map(|(raw,)| {
            let value: serde_json::Value = serde_json::from_str(raw).ok()?;
            let number = value["pr_number"].as_i64()?;
            let branch = value["branch"].as_str()?.to_string();
            let ceiling = value["effective_ceiling"].as_str().unwrap_or("ThroughMerge").to_string();
            Some((number, branch, ceiling))
        })
    } else { None };
    if let Some((number, branch, ceiling)) = delivered {
        // Actual delivery evidence wins over the session's registered worktree
        // branch (including taking over another session's PR).
        sqlx::query("INSERT INTO thin_delivery_refs(session_id,branch,pr_number,ceiling) VALUES (?,?,?,?) ON CONFLICT(session_id) DO UPDATE SET branch=excluded.branch,pr_number=excluded.pr_number,ceiling=excluded.ceiling WHERE thin_delivery_refs.pr_number<>excluded.pr_number OR thin_delivery_refs.branch<>excluded.branch OR thin_delivery_refs.ceiling<>excluded.ceiling")
            .bind(session).bind(branch).bind(number).bind(ceiling).execute(db).await?;
    }
    let existing: Option<(i64, String)> =
        sqlx::query_as("SELECT pr_number,ceiling FROM thin_delivery_refs WHERE session_id=?")
            .bind(session)
            .fetch_optional(db)
            .await?;
    let (number, ceiling) = match existing {
        Some(found) => found,
        None => match discovered_pr(db, session).await? {
            Some(number) => {
                sqlx::query(
                    "INSERT INTO thin_delivery_refs (session_id,branch,pr_number,ceiling) VALUES (?,?,?,'ThroughMerge') ON CONFLICT(session_id) DO NOTHING",
                )
                .bind(session)
                .bind(format!("pr-{number}"))
                .bind(number)
                .execute(db)
                .await?;
                (number, "ThroughMerge".to_string())
            }
            None => return Ok(None),
        },
    };
    ensure_external(db).await?;
    sqlx::query(
        "INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,closeout_queued) VALUES (?,?,?,0,0) ON CONFLICT(session_id) DO UPDATE SET pr_number=excluded.pr_number,ceiling=excluded.ceiling,observed_at=0,completed_reason=NULL,closeout_queued=0 WHERE thin_external_observations.pr_number<>excluded.pr_number OR thin_external_observations.ceiling<>excluded.ceiling",
    )
    .bind(session)
    .bind(number)
    .bind(&ceiling)
    .execute(db)
    .await?;
    Ok(Some(number))
}

fn completion_reason(pr: &serde_json::Value, ceiling: &str, released: bool, number: i64) -> Option<String> {
    if pr["state"] == "OPEN" {
        let green = pr["statusCheckRollup"].as_array().is_some_and(|checks| {
            !checks.is_empty() && checks.iter().all(|check| {
                check["state"] == "SUCCESS" || (check["status"] == "COMPLETED"
                    && matches!(check["conclusion"].as_str(), Some("SUCCESS" | "NEUTRAL" | "SKIPPED")))
            })
        });
        return green.then(|| format!("PR #{number} 检查已全绿"));
    }
    let terminal = pr["state"] == "MERGED";
    if !terminal || (matches!(ceiling, "ThroughRelease" | "through_release") && !released) {
        return None;
    }
    Some(format!("PR #{number} 已合并"))
}

fn failure_reason(pr: &serde_json::Value, number: i64) -> Option<String> {
    if pr["state"] == "CLOSED" { return Some(format!("PR #{number} 已关闭但未合并")); }
    if pr["state"] != "OPEN" { return None; }
    // M146/R4：冲突和被合并队列退回同样是「需要会话处理」的事实，必须带原因唤醒。
    if matches!(pr["mergeable"].as_str(), Some("CONFLICTING"))
        || matches!(pr["mergeStateStatus"].as_str(), Some("DIRTY")) {
        return Some(format!("PR #{number} 存在合并冲突（mergeable=CONFLICTING），需要解决冲突后重新交付"));
    }
    let labels: Vec<&str> = pr["labels"].as_array().map(|labels| {
        labels.iter().filter_map(|label| label["name"].as_str()).collect()
    }).unwrap_or_default();
    if labels.iter().any(|name| *name == "merge-queue: blocked") {
        return Some(format!("PR #{number} 被合并队列退回（merge-queue: blocked），需要按退回原因修复后重新排队"));
    }
    let failed: Vec<&str> = pr["statusCheckRollup"].as_array()?.iter().filter(|check| {
        matches!(check["conclusion"].as_str(), Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" | "STALE"))
            || matches!(check["state"].as_str(), Some("FAILURE" | "ERROR"))
    }).map(|check| check["name"].as_str().or(check["context"].as_str()).unwrap_or("未知检查")).collect();
    (!failed.is_empty()).then(|| format!("PR #{number} 检查失败：{}", failed.join("、")))
}

fn is_external_failure(reason: &str) -> bool {
    reason.contains("检查已全绿") || reason.contains("检查失败：") || reason.contains("已关闭但未合并") || reason.contains("存在合并冲突") || reason.contains("被合并队列退回")
}

/// M150：把一次观测结果落库。
/// 关键约束：失败原因**只是当下事实的投影**，不是历史资产。当最新观测不再包含
/// 任何失败（典型：PR 已合并，或 PR 从未打开过），旧失败原因必须清除；否则
/// `COALESCE` 会让它永久存活，同时 `closeout_queued` 归零，于是每轮心跳都重排
/// 一次修复唤醒，反复消耗模型与 API 额度（现场：PR #829 合并后仍每 60s 唤醒）。
/// 非失败原因（合并完成）不清除，它代表已经发生的事实。
async fn apply_observation(
    db: &SqlitePool,
    session: &str,
    number: i64,
    ceiling: &str,
    reason: Option<&str>,
    merged_at: Option<i64>,
) -> sqlx::Result<()> {
    let stale_failure = reason.is_none() && sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM thin_external_observations WHERE session_id=? AND (completed_reason NOT LIKE '%已合并%')",
    ).bind(session).fetch_one(db).await? == 1;
    if stale_failure {
        sqlx::query("UPDATE thin_external_observations SET completed_reason=NULL,merged_at=COALESCE(?,merged_at) WHERE session_id=? AND pr_number=? AND ceiling=?")
            .bind(merged_at).bind(session).bind(number).bind(ceiling).execute(db).await?;
        return Ok(());
    }
    sqlx::query("UPDATE thin_external_observations SET closeout_queued=CASE WHEN completed_reason IS ? THEN closeout_queued ELSE 0 END,completed_reason=COALESCE(?,completed_reason),merged_at=COALESCE(?,merged_at) WHERE session_id=? AND pr_number=? AND ceiling=?")
        .bind(reason).bind(reason).bind(merged_at).bind(session).bind(number).bind(ceiling).execute(db).await?;
    Ok(())
}

/// 合并时间（gh `mergedAt`，RFC3339）→ 毫秒；未知即 None（不伪造事实）。
fn merged_at_millis(pr: &serde_json::Value) -> Option<i64> {
    let raw = pr["mergedAt"].as_str()?;
    chrono::DateTime::parse_from_rfc3339(raw).ok().map(|at| at.timestamp_millis())
}

/// M146/R3：同一时刻最多两个收尾/修复回合，避免升级后成批占用模型。
const CLOSEOUT_CONCURRENCY: i64 = 2;
/// 已投递的收尾回合在这个窗口内仍算占用额度（回合本身可能还在跑）。
const CLOSEOUT_WINDOW_SECS: i64 = 600;

async fn closeout_slot_free(db: &SqlitePool, at_ms: i64) -> sqlx::Result<bool> {
    if !has_table(db, "wakeup_schedules").await? { return Ok(true); }
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wakeup_schedules WHERE (id LIKE 'thin-closeout-%' OR id LIKE 'thin-repair-%') AND (status='pending' OR (status='done' AND COALESCE(last_fired_at,0)>=?))",
    ).bind(at_ms / 1000 - CLOSEOUT_WINDOW_SECS).fetch_one(db).await?;
    Ok(active < CLOSEOUT_CONCURRENCY)
}

/// M146/R1：收尾回合只给「交付后仍在等待」的会话。已 ended、已 close_objective、
/// 或合并之后已经给过收尾回答的会话不唤醒，只把外部事项标记完成。
async fn closeout_eligible(db: &SqlitePool, session: &str, _merged_at: Option<i64>) -> sqlx::Result<bool> {
    let state = derive_state(db, session).await?;
    // Only the durable closeout wake and its final answer prove §8 was run.
    // An arbitrary post-merge answer may still be the stale waiting message.
    Ok(matches!(state.state, SessionState::WaitingWake | SessionState::WaitingUser))
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

/// M174: reconcile every persisted waiting session once per process startup.
/// Reuse the bounded, durable observer; no parallel unbudgeted GitHub poller.
pub(crate) async fn reconcile_waiting_on_startup(db: &SqlitePool) -> sqlx::Result<usize> {
    Ok(reconcile_waiting_wake_sources(db).await?.scanned)
}

/// M197/R3：`waiting_wake` 的周期对账报告（启动一次 + 之后每 180 秒）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeReconcileReport {
    pub scanned: usize,
    pub swept: usize,
    pub still_waiting: usize,
}

/// M197/R1+R3：对账全部 `waiting_wake` 会话，把「无源等待」当场变成
/// 有源等待 / 人话提问 / 已结束；最后跑一次失效守卫 sweep。
/// 全程只读本地持久事实（GH 观测由既有 ≥180s 心跳与配额策略负责），幂等。
pub async fn reconcile_waiting_wake_sources(db: &SqlitePool) -> sqlx::Result<WakeReconcileReport> {
    reconcile_orphan_turns(db).await?;
    super::thin_state::ensure(db).await?;
    let sessions: Vec<String> = sqlx::query_scalar("SELECT state.session_id FROM thin_session_state state JOIN sessions session ON session.id=state.session_id WHERE state.state='waiting_wake' AND session.delivery_mode='thin'")
        .fetch_all(db).await?;
    for session in &sessions {
        ensure_pr_monitor(db, session).await?;
        heartbeat_external(db, session).await?;
        repair_waiting_wake_source(db, session).await?;
        refresh(db, session).await?;
    }
    let swept = super::thin_state::sweep(db, chrono::Utc::now().timestamp_millis()).await?;
    let still_waiting: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM thin_session_state WHERE state='waiting_wake'",
    ).fetch_one(db).await.unwrap_or(0);
    Ok(WakeReconcileReport { scanned: sessions.len(), swept, still_waiting: still_waiting.max(0) as usize })
}

/// M183/R3：等待投影不是唤醒源。持久补建只发生在没有执行者和真实来源时。
///
/// M197/R1：这是「`waiting_wake` 必须有具体唤醒源」的唯一落地点，也是 fail-closed
/// 的：没有源时不允许继续停在 `waiting_wake`。系统自己能推进的（PR/CI/后台任务/
/// 定时唤醒）立即补一个真实来源并写明来源（系统自动重试/自愈）；补不出来且自愈
/// 次数用尽的，转成对用户的人话提问；已经无事可做的直接结束。
async fn repair_waiting_wake_source(db: &SqlitePool, session: &str) -> sqlx::Result<()> {
    if crate::session_executor_alive(session) != Some(false) { return Ok(()); }
    let Some(state) = super::thin_state::load(db, session).await? else { return Ok(()) };
    if state.state != SessionState::WaitingWake { return Ok(()); }
    if super::background_tasks::wake_source_cover(db, session).await.is_covered() { return Ok(()); }
    // 未决 PR 由已有持久监视登记负责；已完成的 PR 则必须有真实收尾唤醒。
    if has_table(db, "thin_external_observations").await? {
        let monitor: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM thin_external_observations WHERE session_id=? AND completed_reason IS NULL")
            .bind(session).fetch_one(db).await?;
        if monitor > 0 { return Ok(()); }
    }
    let what = state.waiting_for.clone().unwrap_or_default().trim().to_string();
    // R1：没有等待对象 = 没有待办，不能停在「等唤醒」。
    if what.is_empty() {
        let ended = ThinState::ended("没有待办事项，任务已结束", "本会话记录");
        super::thin_state::save(db, session, &ended).await?;
        audit_wait_guard(db, session, "ended_no_wait", "", 0).await;
        tracing::warn!(session, "waiting_wake without a wait target ended");
        return Ok(());
    }
    let completed = completed_external(db, session).await?;
    if completed.is_some() && derive_state(db, session).await?.state == SessionState::Ended { return Ok(()); }
    if let Some(reason) = completed {
        // R2：外部事项已完成 → 明确的收尾唤醒（幂等，重复补建不叠加）。
        arm_durable_wake(db, session, "thin-closeout-source", &format!("{reason}。系统自动重试/自愈：核对持久事实并继续原任务；若 PR 已合并，执行任务模板 §8 收尾后给出最终回答，不重复交付。")).await?;
        audit_wait_guard(db, session, "closeout_wake", &what, 0).await;
        tracing::warn!(session, "waiting_wake source missing; durable automatic repair queued");
        return Ok(());
    }
    let fired = self_continue_fires(db, session).await?;
    if is_system_followed(&what) && fired < MAX_SELF_CONTINUE {
        // R1/R5：系统自己能推进 —— 补真实来源（系统自动重试/自愈），不是问用户。
        arm_durable_wake(db, session, "thin-source-repair", &format!("存量等待仍无推进：{what}。系统自动重试/自愈：核对持久事实并继续原任务；确实无事可做时给出最终回答。")).await?;
        audit_wait_guard(db, session, "self_continue", &what, fired).await;
        tracing::warn!(session, attempts = fired, "waiting_wake source missing; durable automatic repair queued");
        return Ok(());
    }
    // R1：需要人，或来源彻底消失且自愈次数用尽 —— 必须让人看到人话原因。
    let question = if is_system_followed(&what) {
        format!("我原本在等「{what}」，但这件事已经没有东西会在之后把它推进了；系统自动重试 {fired} 次后仍然停在这里。请你决定：要我换个做法继续，还是就此结束？")
    } else {
        format!("我停在「{what}」上，需要你一句话才能继续：是继续做，还是先结束？")
    };
    super::thin_state::ask_user(db, session, &question).await?;
    audit_wait_guard(db, session, "asked_user", &what, fired).await;
    tracing::warn!(session, attempts = fired, "waiting_wake source gone; asked the user");
    Ok(())
}

/// R5：同一会话最多自动补两次来源；再多就不是「系统自愈」而是空转。
const MAX_SELF_CONTINUE: i64 = 2;

async fn arm_durable_wake(db: &SqlitePool, session: &str, prefix: &str, prompt: &str) -> sqlx::Result<()> {
    let at = chrono::Utc::now().timestamp();
    sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,created_at) VALUES (?,?,?,'agent_decided',?,'pending',?) ON CONFLICT(id) DO UPDATE SET prompt=excluded.prompt,next_at=excluded.next_at,status='pending' WHERE wakeup_schedules.status<>'pending'")
        .bind(format!("{prefix}-{session}")).bind(session)
        .bind(prompt)
        .bind(at).bind(at).execute(db).await?;
    Ok(())
}

async fn self_continue_fires(db: &SqlitePool, session: &str) -> sqlx::Result<i64> {
    if !has_table(db, "wakeup_schedules").await? { return Ok(0); }
    let fired: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(fire_count),0) FROM wakeup_schedules WHERE session_id=? AND id LIKE 'thin-source-repair-%'",
    ).bind(session).fetch_one(db).await?;
    Ok(fired.max(0))
}

async fn audit_wait_guard(db: &SqlitePool, session: &str, action: &str, waiting_for: &str, fired: i64) {
    if !has_table(db, "session_audit_events").await.unwrap_or(false) { return; }
    let detail = serde_json::json!({ "action": action, "waiting_for": waiting_for, "self_continue_fires": fired }).to_string();
    if let Err(error) = sqlx::query(
        "INSERT INTO session_audit_events (id, session_id, kind, actor, detail_json, created_at)
         VALUES (?, ?, 'thin_wait_guard', 'system', ?, CAST(strftime('%s','now') AS INTEGER) * 1000)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(session)
    .bind(&detail)
    .execute(db)
    .await
    {
        tracing::warn!(session, %error, "thin wait guard audit not recorded");
    }
}

/// 心跳先刷新事实，再由持久唤醒入口投递一次 §8 收尾。
/// 查询失败/额度不足不伪造完成；observed_at 在请求前 CAS，重启也遵守 180s。
pub(crate) async fn heartbeat_external(db: &SqlitePool, session: &str) -> sqlx::Result<()> {
    if !super::thin::is_thin(db, session).await { return Ok(()); }
    if crate::session_executor_alive(session) != Some(false) { return Ok(()); }
    // M146/R1：用事实判定「还值得观测吗」，不信任可能过期的缓存投影；
    // 已经 ended 的历史会话不再观测，也不再有收尾回合。
    let waiting = derive_state(db, session).await?.state;
    let retry_pending: i64 = if has_table(db, "thin_external_observations").await? {
        sqlx::query_scalar("SELECT COUNT(*) FROM thin_external_observations WHERE session_id=? AND closeout_queued=0 AND completed_reason IS NOT NULL")
            .bind(session).fetch_one(db).await?
    } else { 0 };
    if !matches!(waiting, SessionState::WaitingWake | SessionState::WaitingUser) && retry_pending == 0 {
        return Ok(());
    }
    ensure_external(db).await?;
    super::background_tasks::wake_source_cover(db, session).await;
    // Repair the old silent-closeout receipt: queued=1 without an actual wake
    // is not delivery. An explicit historical §8 final remains terminal.
    if derive_state(db, session).await?.state == SessionState::WaitingWake {
        sqlx::query("UPDATE thin_external_observations SET closeout_queued=0 WHERE session_id=? AND closeout_queued=1 AND completed_reason LIKE '%已合并%' AND NOT EXISTS (SELECT 1 FROM wakeup_schedules wake WHERE wake.session_id=? AND wake.id LIKE 'thin-closeout-%' AND wake.prompt LIKE thin_external_observations.completed_reason || '%')")
            .bind(session).bind(session).execute(db).await?;
    }
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
    let claimed = sqlx::query("UPDATE thin_external_observations SET observed_at=? WHERE session_id=? AND (completed_reason IS NULL OR completed_reason NOT LIKE '%已合并%') AND observed_at<=?")
        .bind(at).bind(session).bind(at - 180_000).execute(db).await?.rows_affected() == 1;
    if claimed {
        let observe_ceiling = ceiling.clone();
        let observed = tokio::task::spawn_blocking(move || {
            let raw = observe_command(&cwd, &["pr", "view", &number.to_string(), "--json", "state,mergeCommit,mergedAt,mergeable,mergeStateStatus,labels,statusCheckRollup"])?;
            let pr: serde_json::Value = serde_json::from_str(&raw).ok()?;
            let merged_at = merged_at_millis(&pr);
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
            Some((failure_reason(&pr, number).or_else(|| completion_reason(&pr, &observe_ceiling, released, number)), merged_at))
        }).await.ok().flatten();
        if let Some((reason, merged_at)) = observed {
            apply_observation(db, session, number, &ceiling, reason.as_deref(), merged_at).await?;
        }
    }
    let pending: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT completed_reason,merged_at FROM thin_external_observations WHERE session_id=? AND closeout_queued=0",
    ).bind(session).fetch_optional(db).await?;
    let Some((Some(reason), merged_at)) = pending else { return Ok(()) };
    {
        let failed = is_external_failure(&reason);
        if failed {
            // M146/R1：失败的会话已经结束（停止/收尾完成）时同样不再起回合。
            if derive_state(db, session).await?.state == SessionState::Ended {
                mark_closeout_silent(db, session).await?;
                return Ok(());
            }
        } else {
            // M146/R1：只有在还等待着的会话才值得一次 §8 收尾回合。
            if !closeout_eligible(db, session, merged_at).await? {
                mark_closeout_silent(db, session).await?;
                return Ok(());
            }
            // M146/R3：收尾回合限流；让出额度时不标记完成，留待下一轮心跳。
            if !closeout_slot_free(db, at).await? { return Ok(()); }
        }
    }
    {
        super::background_tasks::wake_source_cover(db, session).await;
        // 同一事务写唤醒与去重事实，不留下“标记成功但没排唤醒”的崩溃窗口。
        let mut tx = crate::storage::db::begin_write(db).await?;
        let queued = sqlx::query("UPDATE thin_external_observations SET closeout_queued=1 WHERE session_id=? AND closeout_queued=0")
            .bind(session).execute(&mut *tx).await?.rows_affected() == 1;
        if queued {
            let failed = is_external_failure(&reason);
            let prefix = if failed { "thin-repair" } else { "thin-closeout" };
            let prompt = if reason.contains("检查已全绿") {
                format!("{reason}。系统自动重试/自愈：核对必需检查与交付授权，满足条件后标记 merge-queue: armed，由合并队列合并并继续监视；不要手工合并。")
            } else if failed {
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

/// M146/R2：最新用户消息是一条「定时唤醒」文本，且它之后没有任何助手回答，
/// 说明这次唤醒没有真正开始回合（上一回合已结算），不能当作 running。
async fn wake_without_a_turn(db: &SqlitePool, session: &str, latest_user: i64) -> sqlx::Result<bool> {
    if !has_table(db, "messages").await? { return Ok(false); }
    let body: Option<String> = sqlx::query_scalar(
        "SELECT content FROM messages WHERE session_id=? AND role='user' AND created_at=? ORDER BY id DESC LIMIT 1",
    ).bind(session).bind(latest_user).fetch_optional(db).await?;
    let Some(body) = body else { return Ok(false) };
    let shaped = body.trim_start().starts_with("定时唤醒 ") && body.contains("节奏：");
    if !shaped { return Ok(false); }
    let answered: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE session_id=? AND role='assistant' AND created_at>=?",
    ).bind(session).bind(latest_user).fetch_one(db).await?;
    Ok(answered == 0)
}

/// M146/R1：标记外部事项已完成，但不起任何模型回合。
async fn mark_closeout_silent(db: &SqlitePool, session: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE thin_external_observations SET closeout_queued=1 WHERE session_id=? AND closeout_queued=0")
        .bind(session).execute(db).await?;
    Ok(())
}

async fn completed_external(db: &SqlitePool, session: &str) -> sqlx::Result<Option<String>> {
    if !has_table(db, "thin_external_observations").await? { return Ok(None); }
    sqlx::query_scalar("SELECT observation.completed_reason FROM thin_external_observations observation JOIN thin_delivery_refs ref ON ref.session_id=observation.session_id AND ref.pr_number=observation.pr_number AND ref.ceiling=observation.ceiling WHERE observation.session_id=? AND observation.closeout_queued=1")
        .bind(session).fetch_optional(db).await.map(Option::flatten)
}

/// 唯一四态判定。缓存中的 state/question/reason 不参与事实选择。
/// M148/R3：未完成的已交付 PR —— 有引用、且没有被成功关闭（closeout 未入队或
/// 观测结果是故障）—— 就是「外部进行中事项」。只要它还在，会话就不能被判 ended，
/// 否则 M142 的系统 PR 监视随 waiting 一起停摆，PR 失败无人处理。
async fn unfinished_delivery(db: &SqlitePool, session: &str) -> sqlx::Result<Option<i64>> {
    if !has_table(db, "thin_delivery_refs").await? {
        return Ok(None);
    }
    let number: Option<i64> = sqlx::query_scalar("SELECT pr_number FROM thin_delivery_refs WHERE session_id=?")
        .bind(session).fetch_optional(db).await?;
    let Some(number) = number else { return Ok(None) };
    if !has_table(db, "thin_external_observations").await? {
        return Ok(Some(number));
    }
    let row: Option<(Option<String>, i64)> = sqlx::query_as(
        "SELECT completed_reason, COALESCE(closeout_queued,0) FROM thin_external_observations WHERE session_id=? AND pr_number=?",
    ).bind(session).bind(number).fetch_optional(db).await?;
    Ok(match row {
        None => Some(number),
        Some((reason, queued)) => {
            let finished = queued == 1
                && reason.as_deref().is_some_and(|reason| !is_external_failure(reason));
            (!finished).then_some(number)
        }
    })
}

/// M188: only process-local ownership is execution evidence. Persisted rows
/// survive a restart and must never manufacture an executor.
async fn has_active_work(_db: &SqlitePool, session: &str) -> sqlx::Result<bool> {
    // A retained cancellation handle is not an executor: callers can keep it
    // after unregistering a completed run (including the real liveness smoke).
    Ok(crate::session_executor_alive(session) == Some(true))
}

async fn live_background_count(db: &SqlitePool, session: &str) -> sqlx::Result<i64> {
    if !has_table(db, "background_tasks").await?
        || !has_column(db, "background_tasks", "pid").await?
        || !has_column(db, "background_tasks", "process_started_at").await? { return Ok(0); }
    let processes: Vec<(Option<i64>, Option<String>)> = sqlx::query_as("SELECT pid,process_started_at FROM background_tasks WHERE session_id=? AND status='running'")
        .bind(session).fetch_all(db).await?;
    Ok(processes.into_iter().filter(|(pid, marker)| {
        pid.and_then(|pid| u32::try_from(pid).ok())
            .and_then(super::background_tasks::process_start_marker)
            .zip(marker.as_ref()).is_some_and(|(current, recorded)| &current == recorded)
    }).count() as i64)
}

/// Atomically retire abandoned turns before deriving any session projection.
/// Snapshot exact roots so concurrent admission cannot sweep a newly created turn.
pub(crate) async fn reconcile_orphan_turns(db: &SqlitePool) -> sqlx::Result<u64> {
    if !has_table(db, "chat_turn_state").await? { return Ok(0); }
    let rows: Vec<(String, String, i64)> = sqlx::query_as("SELECT root_turn_id,session_id,revision FROM chat_turn_state WHERE completed_at IS NULL AND terminal_reason IS NULL")
        .fetch_all(db).await?;
    let mut retired = 0;
    let mut orphaned = Vec::new();
    let has_interruptions = has_table(db, "thin_restart_interruptions").await?;
    for (root, session, revision) in rows {
        if has_interruptions {
            let interrupted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM thin_restart_interruptions WHERE root_turn_id=?")
                .bind(&root).fetch_one(db).await?;
            if interrupted > 0 { continue; }
        }
        if crate::session_executor_alive(&session) != Some(false)
            || has_active_work(db, &session).await? { continue; }
        orphaned.push((root, revision));
    }
    let mut tx = db.begin().await?;
    for (root, revision) in orphaned {
        let now = chrono::Utc::now().timestamp_millis();
        let result = sqlx::query("UPDATE chat_turn_state SET phase='settled',status='completed',revision=revision+1,terminal_reason='orphaned_by_restart',completed_at=?,updated_at=? WHERE root_turn_id=? AND revision=? AND completed_at IS NULL AND terminal_reason IS NULL")
            .bind(now).bind(now).bind(&root).bind(revision).execute(&mut *tx).await?;
        retired += result.rows_affected();
    }
    tx.commit().await?;
    Ok(retired)
}

/// M188: repair every phantom running projection on startup and once per minute.
pub(crate) async fn reconcile_running_liveness(db: &SqlitePool) -> sqlx::Result<()> {
    reconcile_orphan_turns(db).await?;
    super::thin_state::ensure(db).await?;
    let sessions: Vec<String> = sqlx::query_scalar("SELECT session_id FROM thin_session_state WHERE state='running'").fetch_all(db).await?;
    for session in sessions {
        if crate::session_executor_alive(&session) != Some(false) { continue; }
        tracing::warn!(session, "running without executor; automatic recovery rederives projection");
        refresh(db, &session).await?;
    }
    Ok(())
}

pub async fn derive_state(db: &SqlitePool, session: &str) -> sqlx::Result<ThinState> {
    super::workspace_recovery_budget::ensure(db).await?;
    let settled: Option<String> = sqlx::query_scalar("SELECT settled_reason FROM workspace_recovery_budget WHERE session_id=?")
        .bind(session).fetch_optional(db).await?.flatten();
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
    if has_active_work(db, session).await? {
        tracing::warn!(session, "active work protects session from terminal projection");
        return Ok(ThinState::running());
    }
    if let Some(reason) = settled { return Ok(ThinState::ended(&reason, "本会话记录")); }
    match crate::session_executor_alive(session) {
        Some(true) => return Ok(ThinState::running()),
        None => return Ok(ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some("系统自动重试/自愈：重新核对执行者".into()),
            wake_source: Some("系统周期活性检查".into()),
            ..ThinState::running()
        }),
        Some(false) => {}
    }
    let pending_background: i64 = if has_table(db, "background_tasks").await? {
        sqlx::query_scalar("SELECT COUNT(*) FROM background_tasks WHERE session_id=? AND wake_state='pending'")
            .bind(session).fetch_one(db).await?
    } else { 0 };
    let due_wakes: i64 = if has_table(db, "wakeup_schedules").await? {
        sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id=? AND status='pending' AND next_at<=?")
            .bind(session).bind(at / 1000).fetch_one(db).await?
    } else { 0 };
    let tasks = live_background_count(db, session).await?;
    if tasks > 0 {
        return Ok(super::thin_state::after_final(super::thin_state::WaitNeed::External {
            what: "后台任务".into(), covered: true,
        }, "本会话记录"));
    }
    let wakes: i64 = if has_table(db, "wakeup_schedules").await? {
        sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id=? AND status='pending' AND next_at>?")
            .bind(session).bind(at / 1000).fetch_one(db).await?
    } else { 0 };
    let completed = completed_external(db, session).await?;
    // M174: terminal PR facts outrank a cached waiting_wake projection. Keep
    // waiting for the durable closeout/closed-PR turn, never for another merge.
    let terminal_pr: Option<(String, i64)> = if has_table(db, "thin_external_observations").await? {
        sqlx::query_as("SELECT observation.completed_reason,observation.closeout_queued FROM thin_external_observations observation JOIN thin_delivery_refs ref ON ref.session_id=observation.session_id AND ref.pr_number=observation.pr_number AND ref.ceiling=observation.ceiling WHERE observation.session_id=? AND (observation.completed_reason LIKE '%已合并%' OR observation.completed_reason LIKE '%已关闭但未合并%')")
            .bind(session).fetch_optional(db).await?
    } else { None };
    if let Some((reason, queued)) = terminal_pr {
        let prefix = if reason.contains("已关闭但未合并") { "thin-repair-%" } else { "thin-closeout-%" };
        let answered: i64 = if has_table(db, "wakeup_schedules").await? {
            sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules wake JOIN messages answer ON answer.session_id=wake.session_id AND answer.role='assistant' AND answer.created_at>=wake.last_fired_at*1000 AND TRIM(answer.content)<>'' AND (answer.tool_calls IS NULL OR answer.tool_calls='' OR answer.tool_calls='[]') AND (answer.completion_state IS NULL OR answer.completion_state='' OR answer.completion_state='final') WHERE wake.session_id=? AND wake.id LIKE ? AND wake.status='done' AND wake.prompt LIKE ? AND NOT EXISTS (SELECT 1 FROM messages user WHERE user.session_id=answer.session_id AND user.role='user' AND user.created_at>answer.created_at)")
                .bind(session).bind(prefix).bind(format!("{reason}%")).fetch_one(db).await?
        } else { 0 };
        let legacy_closed_out: i64 = if reason.contains("已合并") && has_table(db, "messages").await? {
            sqlx::query_scalar("SELECT COUNT(*) FROM messages answer JOIN thin_external_observations observation ON observation.session_id=answer.session_id WHERE answer.session_id=? AND observation.merged_at IS NOT NULL AND answer.created_at>=observation.merged_at AND answer.role='assistant' AND answer.content LIKE '%§8 收尾完成%' AND (answer.tool_calls IS NULL OR answer.tool_calls='' OR answer.tool_calls='[]') AND (answer.completion_state IS NULL OR answer.completion_state='' OR answer.completion_state='final') AND NOT EXISTS (SELECT 1 FROM messages user WHERE user.session_id=answer.session_id AND user.role='user' AND user.created_at>answer.created_at)")
                .bind(session).fetch_one(db).await?
        } else { 0 };
        if legacy_closed_out > 0 || (queued == 1 && answered > 0) {
            return Ok(ThinState::ended(&reason, "本会话记录"));
        }
        return Ok(ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some(format!("{reason}，等待 §8 收尾")),
            wake_source: Some("持久收尾投递器：系统自动重试/自愈".into()),
            reason: Some(format!("{reason}；系统自动重试/自愈：接续收尾")),
            ..ThinState::running()
        });
    }
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
    if terminal.as_ref().is_some_and(|(_, reason)| reason == "delivery_pending") {
        if let Some(state) = super::thin_state::load(db, session).await?.filter(|state| state.state == SessionState::WaitingUser) {
            return Ok(state);
        }
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
        if repaired == 0 && pending_question_at == 0 {
            return Ok(ThinState {
                state: SessionState::WaitingWake,
                waiting_for: Some("系统自动重试/自愈：接续 PR 检查结果".into()),
                wake_source: Some("持久投递器自动重试".into()),
                ..ThinState::running()
            });
        }
    }
    // M148/R3：未完成的已交付 PR 是外部进行中事项——只要它还在，下面的终态推导
    // 不得结束会话。v1.91.x 的现场：等待 PR 的会话被写成
    // `ended/superseded_by_user_reprompt`，M142 的系统 PR 监视随之停摆，PR 失败
    // 无人处理。真实提问（pending_question_at）仍然优先，停止/审批已在最前面处理。
    if pending_question_at == 0 {
        if let Some(number) = unfinished_delivery(db, session).await? {
            return Ok(ThinState {
                state: SessionState::WaitingWake,
                waiting_for: Some(format!("等 PR #{number} 合并/CI")),
                wake_source: Some("系统 PR 监视（心跳，间隔不少于 180 秒）".into()),
                reason: Some("PR 未合并前不结束会话".into()),
                ..ThinState::running()
            });
        }
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
        // M146/R2：定时唤醒写入的用户消息如果没有回合真正开始（执行者已死、也没有
        // 助手回答），不能把会话永久投影成 running。v1.91.15 现场（d632e17a）就是这样
        // 唤醒后 running 30 分钟、零助手回复。
        if latest_user.is_some_and(|time| time > cutoff)
            && wake_without_a_turn(db, session, latest_user.unwrap_or_default()).await? {
            return Ok(ThinState::ended("定时唤醒未开始回合（上一回合已结算）", "本会话记录"));
        }
        // A newly admitted message is a fact even before the transport is registered.
        if terminal.is_some() && pending_background + due_wakes == 0 {
            return Ok(executor_missing_state());
        }
    } else if let Some((_, reason)) = &terminal {
        let previous = super::thin_state::load(db, session).await?;
        // A final answer may deliberately settle to waiting_wake. A stop never does.
        // M158/R3：历史上写进库里的「没有活着的唤醒源」问题（waiting_user +
        // 内部用词）同样不算终态，交给下面的派生重新登记监视。
        let still_waiting = previous.as_ref().is_some_and(|state| {
            state.state == SessionState::WaitingWake
                || (state.state == SessionState::WaitingUser
                    && state.question.as_deref().is_some_and(has_internal_wording))
        });
        if stopped_at > 0 || !still_waiting {
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
    if pending_background + due_wakes > 0 {
        return Ok(ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some("系统自动重试/自愈：接续后台结果".into()),
            wake_source: Some("持久投递器自动重试".into()),
            ..ThinState::running()
        });
    }
    if let Some(previous) = super::thin_state::load(db, session).await? {
        // M158/R1+R2+R3：还在等外部事项（PR / CI / 发版）时，系统自己登记监视，
        // 状态是 waiting_wake；只有真的等用户决定时才问用户，而且用他能回答的话。
        // 历史上那条「没有活着的唤醒源，请选择继续监视或结束任务」的内部问题
        // 也在这里被重新派生掉，不让它永久粘在界面上。
        let internal_limbo = previous.state == SessionState::WaitingUser
            && previous
                .question
                .as_deref()
                .is_some_and(has_internal_wording);
        if previous.state == SessionState::WaitingWake || internal_limbo {
            if let Some(number) = ensure_pr_monitor(db, session).await? {
                return Ok(ThinState {
                    state: SessionState::WaitingWake,
                    waiting_for: Some(format!("等 PR #{number} 的 CI 结果与合并")),
                    wake_source: Some("系统自动监视（心跳，间隔不少于 180 秒）".into()),
                    reason: Some(format!("PR #{number} 还没合并，系统会自动跟进，不需要你回复")),
                    ..ThinState::running()
                });
            }
            if let Some(what) = previous
                .waiting_for
                .clone()
                .filter(|what| is_system_followed(what))
            {
                return Ok(ThinState {
                    state: SessionState::WaitingWake,
                    waiting_for: Some(what),
                    wake_source: Some("系统自动监视（心跳，间隔不少于 180 秒）".into()),
                    reason: Some("外部事项仍未完成，系统会自动跟进".into()),
                    ..ThinState::running()
                });
            }
            let what = previous
                .waiting_for
                .clone()
                .unwrap_or_else(|| "下一步".to_string());
            return Ok(ThinState {
                state: SessionState::WaitingUser,
                waiting_for: Some(what.clone()),
                question: Some(format!("还在等「{what}」。要我继续等，还是先结束这个任务？")),
                ..ThinState::running()
            });
        }
        // M151/R2：缓存说 running，但这里已经没有任何活着的执行者。回合结束会
        // 注销执行者，进程重启/崩溃也会让执行者消失；此时把 running 原样返回，
        // 界面就永远显示「正在做」，用户发不出下一条消息，也不知道依赖是什么。
        // 终态之外一律降级成可解释的「已结束」，而不是让缓存自持。
        if previous.state == SessionState::Running
            && super::objective_diagnostics::dead_process_run_control(db, session).await?
        {
            return Ok(ThinState::ended(
                "上一回合的执行者已随进程退出（遗留运行控制行），可重置会话状态后重新开始",
                "本会话记录",
            ));
        }
        return Ok(if previous.state == SessionState::Running { executor_missing_state() } else { previous });
    }
    if latest_user.is_some() {
        return Ok(executor_missing_state());
    }
    Ok(ThinState::ended(terminal.as_ref().map_or("无活跃执行者或未决等待", |end| end.1.as_str()), "本会话记录"))
}

fn executor_missing_state() -> ThinState {
    ThinState {
        state: SessionState::WaitingWake,
        waiting_for: Some("系统自动重试/自愈：接续中断回合".into()),
        wake_source: Some("系统周期恢复扫描".into()),
        ..ThinState::running()
    }
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
    if state.state == SessionState::WaitingWake
        && state.wake_source.as_deref() == Some("系统周期恢复扫描") {
        repair_waiting_wake_source(db, session).await?;
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    // ── M197：`waiting_wake` 必须有具体唤醒源 ──────────────────────────────────
    async fn m197_wait(session: &str, db: &SqlitePool, what: &str) {
        super::super::thin_state::save(db, session, &ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some(what.to_string()),
            wake_source: Some(format!("已登记的唤醒源：{what}")),
            ..ThinState::running()
        }).await.unwrap();
    }

    async fn m197_fired_repair(db: &SqlitePool, session: &str, fire_count: i64) {
        super::super::background_tasks::ensure_wake_schema(db).await.unwrap();
        sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,fire_count,created_at) VALUES (?,?,'repair','agent_decided',1,'done',?,1) ON CONFLICT(id) DO UPDATE SET status='done',fire_count=excluded.fire_count")
            .bind(format!("thin-source-repair-{session}")).bind(session).bind(fire_count)
            .execute(db).await.unwrap();
    }

    /// R1/R5：唤醒已经触发完、又没挂新唤醒的会话，不会永远停在 `waiting_wake`：
    /// 系统自己能推进的立即补一个真实来源（系统自动重试/自愈），不是问用户。
    #[tokio::test]
    async fn m197_wake_without_source_is_never_left_waiting() {
        let db = fixture().await;
        m197_wait("derive", &db, "PR #77 合并/CI").await;
        m197_fired_repair(&db, "derive", 1).await;
        let report = reconcile_waiting_wake_sources(&db).await.unwrap();
        assert_eq!(report.scanned, 1, "{report:?}");
        let state = super::super::thin_state::load(&db, "derive").await.unwrap().unwrap();
        assert_eq!(state.state, SessionState::WaitingWake, "{state:?}");
        let cover = super::super::background_tasks::wake_source_cover(&db, "derive").await;
        assert!(cover.is_covered(), "no real wake source after reconcile: {cover:?}");
        assert!(state.question.is_none(), "system-followable wait must not ask the user: {state:?}");
        let prompt: String = sqlx::query_scalar(
            "SELECT prompt FROM wakeup_schedules WHERE session_id='derive' AND status='pending'",
        ).fetch_one(&db).await.unwrap();
        assert!(prompt.contains("系统自动重试"), "{prompt}");
    }

    /// R1/R5：自愈有界。同一会话自动补过两次仍然没有推进对象时，必须变成人话提问，
    /// 不允许无限补来源、也不允许继续停在 `waiting_wake`。
    #[tokio::test]
    async fn m197_self_continue_is_bounded_then_asks_user() {
        let db = fixture().await;
        m197_wait("derive", &db, "PR #77 合并/CI").await;
        m197_fired_repair(&db, "derive", MAX_SELF_CONTINUE).await;
        reconcile_waiting_wake_sources(&db).await.unwrap();
        let state = super::super::thin_state::load(&db, "derive").await.unwrap().unwrap();
        assert_eq!(state.state, SessionState::WaitingUser, "{state:?}");
        let question = state.question.expect("escalation must carry a question");
        assert!(question.contains("PR #77"), "{question}");
        for internal in ["唤醒源", "外部事项", "waiting_wake", "waiting_user"] {
            assert!(!question.contains(internal), "internal wording in {question}");
        }
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wakeup_schedules WHERE session_id='derive' AND status='pending'",
        ).fetch_one(&db).await.unwrap();
        assert_eq!(pending, 0, "bounded self-continue must not keep arming");
    }

    /// R2：PR 已合并 → 立刻挂出收尾唤醒，而不是没有源的等待。
    #[tokio::test]
    async fn m197_merged_pr_gets_closeout_wake() {
        let db = fixture().await;
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',73,'ThroughMerge')")
            .execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,closeout_queued) VALUES ('derive',73,'ThroughMerge',1,'PR #73 已合并',0)")
            .execute(&db).await.unwrap();
        m197_wait("derive", &db, "PR #73 合并/CI").await;
        reconcile_waiting_wake_sources(&db).await.unwrap();
        let cover = super::super::background_tasks::wake_source_cover(&db, "derive").await;
        assert!(cover.is_covered(), "{cover:?}");
        let prompt: String = sqlx::query_scalar(
            "SELECT prompt FROM wakeup_schedules WHERE session_id='derive' AND status='pending' ORDER BY created_at DESC LIMIT 1",
        ).fetch_one(&db).await.unwrap();
        assert!(prompt.contains("已合并"), "{prompt}");
        assert!(prompt.contains("§8 收尾"), "{prompt}");
    }

    /// R2：活着的外部观测本身就是唤醒源；收尾入队之后再算账。
    #[tokio::test]
    async fn m197_active_observation_counts_as_wake_source() {
        let db = fixture().await;
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,closeout_queued) VALUES ('derive',73,'ThroughMerge',0,0)")
            .execute(&db).await.unwrap();
        let live = super::super::background_tasks::wake_source_cover(&db, "derive").await;
        assert_eq!(live.external_observations, 1, "{live:?}");
        assert!(live.is_covered(), "{live:?}");
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',73,'ThroughMerge')")
            .execute(&db).await.unwrap();
        sqlx::query("UPDATE thin_external_observations SET completed_reason='PR #73 已合并', closeout_queued=1 WHERE session_id='derive'")
            .execute(&db).await.unwrap();
        let done = super::super::background_tasks::wake_source_cover(&db, "derive").await;
        assert_eq!(done.external_observations, 0, "{done:?}");
    }

    /// R3/R5：重启对账把存量脏状态一次修完，而且幂等。
    #[tokio::test]
    async fn m197_stock_dirty_states_repaired_and_idempotent() {
        let db = fixture().await;
        for (id, title) in [("a", "a"), ("b", "b")] {
            sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES (?,'synthetic','/synthetic','model',1,1,'thin')")
                .bind(id).bind(title).execute(&db).await.unwrap();
        }
        // a：系统能推进（PR 已合并的收尾事实缺失）→ 补来源
        m197_wait("a", &db, "PR #91 合并/CI").await;
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('a','synthetic',91,'ThroughMerge')")
            .execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,closeout_queued) VALUES ('a',91,'ThroughMerge',1,'PR #91 已合并',0)")
            .execute(&db).await.unwrap();
        // b：等的是一个人的决定 → 必须变成人话提问
        m197_wait("b", &db, "是否继续这次重构").await;

        let first = reconcile_waiting_wake_sources(&db).await.unwrap();
        assert_eq!(first.scanned, 2, "{first:?}");
        let mut uncovered = Vec::new();
        for session in ["a", "b"] {
            let state = super::super::thin_state::load(&db, session).await.unwrap().unwrap();
            let covered = super::super::background_tasks::wake_source_cover(&db, session).await.is_covered();
            if state.state == SessionState::WaitingWake && !covered { uncovered.push(session); }
        }
        assert!(uncovered.is_empty(), "uncovered waiting_wake survived reconcile: {uncovered:?}");

        let b = super::super::thin_state::load(&db, "b").await.unwrap().unwrap();
        assert_eq!(b.state, SessionState::WaitingUser, "{b:?}");
        assert!(!b.question.unwrap_or_default().contains("唤醒源"));

        let pending_first: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='pending'")
            .fetch_one(&db).await.unwrap();
        let second = reconcile_waiting_wake_sources(&db).await.unwrap();
        let pending_second: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='pending'")
            .fetch_one(&db).await.unwrap();
        assert_eq!(pending_first, pending_second, "reconcile must be idempotent");
        assert_eq!(second.still_waiting, second.scanned, "no silent uncovered leftover: {second:?}");
    }

    #[tokio::test]
    async fn m183_startup_repairs_waiting_wake_without_a_source() {
        let db = fixture().await;
        // M197/R1 之后，「系统自己盯着」的等待才允许自动补来源；要人决定的等待转人话提问
        // （见 m197_self_continue_is_bounded_then_asks_user）。这里仍验证补来源路径。
        super::super::thin_state::save(&db, "derive", &ThinState {
            state: SessionState::WaitingWake,
            waiting_for: Some("等待后台任务完成".into()),
            wake_source: Some("持久收尾投递器：系统自动重试/自愈".into()),
            ..ThinState::running()
        }).await.unwrap();
        reconcile_waiting_on_startup(&db).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id='derive' AND status='pending'")
            .fetch_one(&db).await.unwrap();
        assert_eq!(count, 1, "waiting_wake must have a real durable wake");
        reconcile_waiting_on_startup(&db).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id='derive' AND status='pending'")
            .fetch_one(&db).await.unwrap();
        assert_eq!(count, 1, "startup repair must be idempotent");
    }

    #[tokio::test]
    async fn m188_running_requires_executor_not_cached_or_durable_activity() {
        let db = fixture().await;
        super::super::thin_state::save(&db, "derive", &ThinState::running()).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m188-user','derive','user','synthetic task',100)").execute(&db).await.unwrap();
        let state = refresh(&db, "derive").await.unwrap();
        assert_ne!(state.state, SessionState::Running, "without executor: {state:?}");
        assert!(state.state != SessionState::WaitingUser || state.question.as_ref().is_some_and(|text| !text.is_empty()));
    }

    #[tokio::test]
    async fn m188_cross_process_orphans_settle_and_live_bash_survives() {
        use crate::util::no_window::NoWindow;
        const CHILD: &str = "CODEFACTORY_M188_SYNTHETIC_DB";
        if let Ok(url) = std::env::var(CHILD) {
            let db = crate::storage::db::connect(&url).await.unwrap();
            let started = std::time::Instant::now();
            reconcile_waiting_on_startup(&db).await.unwrap();
            merged_settlement::settle_merged_sessions(&db).await.unwrap();
            assert!(started.elapsed() < std::time::Duration::from_secs(60));
            assert_eq!(super::super::thin_state::load(&db, "derive").await.unwrap().unwrap().state, SessionState::Ended);
            let reasons: Vec<String> = sqlx::query_scalar("SELECT terminal_reason FROM chat_turn_state WHERE session_id='derive'").fetch_all(&db).await.unwrap();
            assert_eq!(reasons, vec!["orphaned_by_restart"; 18]);
            reconcile_waiting_on_startup(&db).await.unwrap();
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_turn_state WHERE completed_at IS NULL AND session_id='derive'").fetch_one(&db).await.unwrap();
            assert_eq!(count, 0);
            let control = crate::ChatRunControl::pending();
            crate::register_session_executor("derive", &control);
            #[cfg(unix)]
            let mut bash = tokio::process::Command::new("bash").no_window().args(["-c", "sleep 2; printf M188-LIVE"]).stdout(std::process::Stdio::piped()).spawn().unwrap();
            #[cfg(windows)]
            let mut bash = tokio::process::Command::new("powershell").no_window().args(["-NoProfile", "-Command", "Start-Sleep -Seconds 2; Write-Output M188-LIVE"]).stdout(std::process::Stdio::piped()).spawn().unwrap();
            assert!(bash.try_wait().unwrap().is_none());
            sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at) VALUES ('m188-live','derive','working','active',1,1)").execute(&db).await.unwrap();
            reconcile_waiting_on_startup(&db).await.unwrap();
            merged_settlement::settle_ended_and_cancel(&db, "derive", "old PR 已合并", "synthetic").await.unwrap();
            assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::Running);
            let reason: Option<String> = sqlx::query_scalar("SELECT terminal_reason FROM chat_turn_state WHERE root_turn_id='m188-live'").fetch_one(&db).await.unwrap();
            assert!(reason.is_none());
            let output = bash.wait_with_output().await.unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "M188-LIVE");
            crate::unregister_session_executor("derive", &control);
            drop(control);
            merged_settlement::settle_merged_sessions(&db).await.unwrap();
            assert_eq!(super::super::thin_state::load(&db, "derive").await.unwrap().unwrap().state, SessionState::Ended);
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("m188.sqlite").display());
        let db = crate::storage::db::connect(&url).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('derive','synthetic','/synthetic','model',1,1,'thin')").execute(&db).await.unwrap();
        super::super::thin_state::save(&db, "derive", &ThinState::running()).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,completed_reason) VALUES ('derive',188,'ThroughMerge','PR #188 已合并')").execute(&db).await.unwrap();
        for index in 0..18 {
            sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at) VALUES (?,'derive','working','active',1,1)").bind(format!("orphan-{index}")).execute(&db).await.unwrap();
        }
        db.close().await;
        let output = std::process::Command::new(std::env::current_exe().unwrap()).env(CHILD, &url).args(["m188_cross_process_orphans_settle_and_live_bash_survives", "--nocapture"]).output().unwrap();
        assert!(output.status.success(), "M188 worker failed: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }

    /// M187/R2/R4: durable rows alone are not live; current executor owns work.
    #[tokio::test]
    async fn m187_cross_process_active_facts_prevent_terminal_projection() {
        const CHILD: &str = "CODEFACTORY_M187_SYNTHETIC_DB";
        if let Ok(url) = std::env::var(CHILD) {
            let db = crate::storage::db::connect(&url).await.unwrap();
            sqlx::query("INSERT INTO thin_delivery_refs VALUES ('new-delivery','synthetic',188,'ThroughMerge')")
                .execute(&db).await.unwrap();
            for index in 0..26 {
                let session = format!("merged-idle-{index}");
                sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES (?,'synthetic','/synthetic','model',1,1,'thin')")
                    .bind(&session).execute(&db).await.unwrap();
                let mut waiting = ThinState::running();
                waiting.state = SessionState::WaitingWake;
                super::super::thin_state::save(&db, &session, &waiting).await.unwrap();
                sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,completed_reason) VALUES (?,187,'ThroughMerge','PR #187 已合并')")
                    .bind(&session).execute(&db).await.unwrap();
            }
            let controls: Vec<_> = ["tool-pending", "tool-running", "thinking", "background"].into_iter().map(|session| {
                let control = crate::ChatRunControl::pending();
                crate::register_session_executor(session, &control);
                (session, control)
            }).collect();
            let batch = merged_settlement::settle_merged_sessions(&db).await.unwrap();
            assert_eq!(batch.settled, 26, "M187 idle merged backlog must settle: {batch:?}");
            for index in 0..26 {
                let state = super::super::thin_state::load(&db, &format!("merged-idle-{index}")).await.unwrap().unwrap();
                assert_eq!(state.state, SessionState::Ended);
            }
            for (session, control) in controls {
                let state = refresh(&db, session).await.unwrap();
                assert_ne!(state.state, SessionState::Ended, "M187 active {session}: {state:?}");
                merged_settlement::settle_merged_sessions(&db).await.unwrap();
                let state = super::super::thin_state::load(&db, session).await.unwrap().unwrap();
                assert_ne!(state.state, SessionState::Ended, "M187 sweep ended {session}");
                merged_settlement::settle_ended_and_cancel(&db, session, "stale closeout", "synthetic").await.unwrap();
                let state = super::super::thin_state::load(&db, session).await.unwrap().unwrap();
                assert_ne!(state.state, SessionState::Ended, "M187 direct settlement ended {session}");
                super::super::workspace_recovery_budget::ensure(&db).await.unwrap();
                sqlx::query("INSERT INTO workspace_recovery_budget(session_id,settled_reason) VALUES (?,'stale missing workspace')")
                    .bind(session).execute(&db).await.unwrap();
                let state = refresh(&db, session).await.unwrap();
                assert_ne!(state.state, SessionState::Ended, "M187 stale workspace receipt ended {session}");
                crate::unregister_session_executor(session, &control);
            }
            // A new delivery must never be settled by an older PR receipt.
            let mut waiting = ThinState::running();
            waiting.state = SessionState::WaitingWake;
            super::super::thin_state::save(&db, "new-delivery", &waiting).await.unwrap();
            merged_settlement::settle_merged_sessions(&db).await.unwrap();
            let state = super::super::thin_state::load(&db, "new-delivery").await.unwrap().unwrap();
            assert_ne!(state.state, SessionState::Ended, "M187 stale PR settled new delivery");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("m187.sqlite").display());
        let db = crate::storage::db::connect(&url).await.unwrap();
        ensure_external(&db).await.unwrap();
        for session in ["tool-pending", "tool-running", "thinking", "background", "new-delivery"] {
            sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES (?, 'synthetic','/synthetic','model',1,1,'thin')")
                .bind(session).execute(&db).await.unwrap();
            sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES (?,?,'assistant','working',1)")
                .bind(session).bind(session).execute(&db).await.unwrap();
            let mut waiting = ThinState::running();
            waiting.state = SessionState::WaitingWake;
            super::super::thin_state::save(&db, session, &waiting).await.unwrap();
            sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,completed_reason) VALUES (?,187,'ThroughMerge','PR #187 已合并')")
                .bind(session).execute(&db).await.unwrap();
        }
        for (session, status) in [("tool-pending", "pending"), ("tool-running", "running")] {
            sqlx::query("INSERT INTO tool_calls(id,message_id,tool_name,arguments,status,created_at) VALUES (?,?,'bash','{}',?,1)")
                .bind(session).bind(session).bind(status).execute(&db).await.unwrap();
        }
        sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,revision,phase,status,started_at,updated_at) VALUES ('thinking','thinking',1,'model_request','active',1,1)")
            .execute(&db).await.unwrap();
        // A minimal historical schema is sufficient for the running-task fact.
        sqlx::query("CREATE TABLE IF NOT EXISTS background_tasks(id TEXT PRIMARY KEY,session_id TEXT,status TEXT,wake_state TEXT)")
            .execute(&db).await.unwrap();
        sqlx::query("INSERT INTO background_tasks(id,session_id,status,wake_state) VALUES ('background','background','running','none')")
            .execute(&db).await.unwrap();
        db.close().await;
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, &url).args(["m187_cross_process_active_facts_prevent_terminal_projection", "--nocapture"])
            .output().unwrap();
        assert!(output.status.success(), "M187 worker failed: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }

    #[tokio::test]
    async fn m174_cross_process_merged_wait_is_closed_out() {
        use super::super::background_tasks::{WakeSink, WakeFuture, drain_due_wakeups_at};
        const CHILD: &str = "CODEFACTORY_M174_SYNTHETIC_DB";
        if let Ok(url) = std::env::var(CHILD) {
            let db = crate::storage::db::connect(&url).await.unwrap();
            super::super::background_tasks::wake_source_cover(&db, "derive").await;
            assert_eq!(reconcile_waiting_on_startup(&db).await.unwrap(), 2);
            for session in ["derive", "adopted"] {
                heartbeat_external(&db, session).await.unwrap();
                let waiting = refresh(&db, session).await.unwrap();
                assert_eq!(waiting.state, SessionState::WaitingWake);
                assert!(!waiting.reason.as_deref().unwrap_or("").contains("还没合并"), "{waiting:?}");
            }
            struct Sink;
            impl WakeSink for Sink {
                fn enqueue(&self, _: String, text: String) -> WakeFuture {
                    assert!(text.contains("§8 收尾"));
                    assert!(text.contains("已合并"));
                    Box::pin(async { true })
                }
            }
            let due: i64 = sqlx::query_scalar("SELECT MAX(next_at) FROM wakeup_schedules WHERE status='pending'")
                .fetch_one(&db).await.unwrap();
            assert_eq!(drain_due_wakeups_at(&db, &Sink, due).await, 2);
            for session in ["derive", "adopted"] {
                sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES (?,?, 'assistant','§8 收尾完成',?)")
                    .bind(format!("final-{session}")).bind(session).bind(due * 1000 + 1).execute(&db).await.unwrap();
                let ended = refresh(&db, session).await.unwrap();
                assert_eq!(ended.state, SessionState::Ended, "{ended:?}");
                assert!(ended.reason.unwrap().contains("已合并"));
                heartbeat_external(&db, session).await.unwrap();
            }
            assert_eq!(drain_due_wakeups_at(&db, &Sink, due + 1).await, 0);
            db.close().await;
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("fixture.sqlite").display());
        let db = crate::storage::db::connect(&url).await.unwrap();
        for (session, branch, number) in [("derive", "own-branch", 174), ("adopted", "another-delivered-branch", 175)] {
            sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES (?, 'synthetic','/synthetic','model',1,1,'thin')")
                .bind(session).execute(&db).await.unwrap();
            super::super::thin_state::save(&db, session, &super::super::thin_state::after_final(
                super::super::thin_state::WaitNeed::External { what: "PR / CI".into(), covered: false }, "")).await.unwrap();
            sqlx::query("INSERT INTO thin_delivery_refs VALUES (?,?,?,'ThroughMerge')")
                .bind(session).bind(branch).bind(number).execute(&db).await.unwrap();
            ensure_pr_monitor(&db, session).await.unwrap();
            if session == "adopted" {
                sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('delivery-owner','adopted','assistant','',1)").execute(&db).await.unwrap();
                sqlx::query("INSERT INTO tool_calls(id,message_id,tool_name,arguments,result,status,created_at) VALUES ('delivery-result','delivery-owner','deliver_changes','{}',?,'done',1)")
                    .bind(serde_json::json!({"pr_number":number,"branch":branch,"effective_ceiling":"ThroughMerge"}).to_string()).execute(&db).await.unwrap();
                sqlx::query("UPDATE thin_delivery_refs SET branch='registered-worktree',pr_number=999,ceiling='pr_only' WHERE session_id='adopted'").execute(&db).await.unwrap();
                // Even when branch/number were repaired earlier, the actual delivery
                // ceiling must replace the stale registered-worktree ceiling.
                sqlx::query("UPDATE thin_delivery_refs SET branch=?,pr_number=? WHERE session_id='adopted'")
                    .bind(branch).bind(number).execute(&db).await.unwrap();
                assert_eq!(ensure_pr_monitor(&db, session).await.unwrap(), Some(number));
                let ceiling: String = sqlx::query_scalar("SELECT ceiling FROM thin_delivery_refs WHERE session_id='adopted'")
                    .fetch_one(&db).await.unwrap();
                assert_eq!(ceiling, "ThroughMerge");
            }
            apply_observation(&db, session, number, "ThroughMerge", Some(&format!("PR #{number} 已合并")), Some(100)).await.unwrap();
            sqlx::query("UPDATE thin_external_observations SET closeout_queued=1 WHERE session_id=?")
                .bind(session).execute(&db).await.unwrap();
            // A final answer after merge is not proof that §8 was executed.
            sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES (?,?, 'assistant','继续等待系统自动跟进',101)")
                .bind(format!("stale-{session}")).bind(session).execute(&db).await.unwrap();
        }
        db.close().await;
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["m174_cross_process_merged_wait_is_closed_out", "--nocapture"])
            .env(CHILD, &url).output().await.unwrap();
        assert!(output.status.success(), "child failed: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }

    #[tokio::test]
    async fn m174_merge_during_wait_crosses_process_and_closes_out() {
        const CHILD: &str = "CODEFACTORY_M174_LIVE_WAIT_DB";
        if let Ok(url) = std::env::var(CHILD) {
            let db = crate::storage::db::connect(&url).await.unwrap();
            let stage = std::env::var("CODEFACTORY_M174_STAGE").unwrap();
            if stage == "wait" {
                ensure_pr_monitor(&db, "derive").await.unwrap();
                assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
            } else if stage == "merge" {
                // An independent observer process writes the external merge fact
                // while the persisted session is waiting, without an app restart.
                let pr = serde_json::json!({"state":"MERGED","mergedAt":"2026-01-01T00:00:00Z"});
                let reason = completion_reason(&pr, "through_merge", false, 314).unwrap();
                apply_observation(&db, "derive", 314, "through_merge", Some(&reason), merged_at_millis(&pr)).await.unwrap();
            } else {
                use super::super::background_tasks::{WakeSink, WakeFuture, drain_due_wakeups_at};
                heartbeat_external(&db, "derive").await.unwrap();
                let state = refresh(&db, "derive").await.unwrap();
                assert!(state.reason.as_deref().unwrap().contains("已合并"));
                struct Sink;
                impl WakeSink for Sink {
                    fn enqueue(&self, _: String, text: String) -> WakeFuture {
                        assert!(text.contains("§8 收尾"));
                        assert!(text.contains("清理远端分支"));
                        Box::pin(async { true })
                    }
                }
                let due: i64 = sqlx::query_scalar("SELECT next_at FROM wakeup_schedules WHERE id LIKE 'thin-closeout-%'")
                    .fetch_one(&db).await.unwrap();
                assert_eq!(drain_due_wakeups_at(&db, &Sink, due).await, 1);
                sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('closeout-final','derive','assistant','核查及清理完成',?)")
                    .bind(due * 1000 + 1).execute(&db).await.unwrap();
                assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::Ended);
                heartbeat_external(&db, "derive").await.unwrap();
                assert_eq!(drain_due_wakeups_at(&db, &Sink, due + 1).await, 0);
            }
            db.close().await;
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("live.sqlite").display());
        let db = crate::storage::db::connect(&url).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('derive','synthetic','/synthetic','model',1,1,'thin')")
            .execute(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','adopted-synthetic',314,'through_merge')")
            .execute(&db).await.unwrap();
        db.close().await;
        for stage in ["wait", "merge", "closeout"] {
            let output = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["m174_merge_during_wait_crosses_process_and_closes_out", "--nocapture"])
                .env(CHILD, &url).env("CODEFACTORY_M174_STAGE", stage).output().await.unwrap();
            assert!(output.status.success(), "{stage}: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
    }

    #[tokio::test]
    async fn m174_closed_pr_final_settles_instead_of_waiting_forever() {
        let db = fixture().await;
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','adopted',176,'ThroughMerge')").execute(&db).await.unwrap();
        ensure_pr_monitor(&db, "derive").await.unwrap();
        super::super::background_tasks::wake_source_cover(&db, "derive").await;
        apply_observation(&db, "derive", 176, "ThroughMerge", Some("PR #176 已关闭但未合并"), None).await.unwrap();
        sqlx::query("UPDATE thin_external_observations SET closeout_queued=1").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,after_sec,next_at,status,created_at,last_fired_at) VALUES ('thin-repair-closed','derive','PR #176 已关闭但未合并','agent_decided',60,1,'done',1,1)").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('closed-final','derive','assistant','PR 已关闭，核查完成，结束任务',1001)").execute(&db).await.unwrap();
        let state = refresh(&db, "derive").await.unwrap();
        assert_eq!(state.state, SessionState::Ended, "{state:?}");
        assert!(state.reason.unwrap().contains("已关闭但未合并"));
    }

    #[tokio::test]
    async fn m168_pending_pr_wake_without_executor_is_not_running() {
        let db = fixture().await;
        super::super::background_tasks::wake_source_cover(&db, "derive").await;
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',73,'ThroughMerge')").execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason) VALUES ('derive',73,'ThroughMerge',?,'PR #73 检查失败：synthetic-ci')")
            .bind(chrono::Utc::now().timestamp_millis() + 3_600_000).execute(&db).await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
    }

    #[tokio::test]
    async fn m168_cross_process_pr_ci_failure_is_delivered_after_restart() {
        use super::super::background_tasks::{WakeSink, WakeFuture, drain_due_wakeups_at};
        const CHILD: &str = "CODEFACTORY_M168_PR_FIXTURE";
        if let Ok(url) = std::env::var(CHILD) {
            let db = crate::storage::db::connect(&url).await.unwrap();
            heartbeat_external(&db, "derive").await.unwrap();
            struct Sink;
            impl WakeSink for Sink {
                fn enqueue(&self, session: String, text: String) -> WakeFuture {
                    assert_eq!(session, "derive");
                    assert!(text.contains("synthetic-ci"));
                    assert!(text.contains("系统自动重试/自愈"));
                    Box::pin(async { true })
                }
            }
            let due: i64 = sqlx::query_scalar("SELECT next_at FROM wakeup_schedules WHERE status='pending'")
                .fetch_one(&db).await.unwrap();
            assert_eq!(drain_due_wakeups_at(&db, &Sink, due).await, 1);
            assert_eq!(drain_due_wakeups_at(&db, &Sink, due + 1).await, 0);
            db.close().await;
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("fixture.sqlite").display());
        let db = crate::storage::db::connect(&url).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('derive','synthetic','/synthetic','model',1,1,'thin')")
            .execute(&db).await.unwrap();
        super::super::thin_state::save(&db, "derive", &super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "PR / CI".into(), covered: false }, "")).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',73,'ThroughMerge')").execute(&db).await.unwrap();
        ensure_pr_monitor(&db, "derive").await.unwrap();
        let reason = failure_reason(&serde_json::json!({"state":"OPEN", "statusCheckRollup":[{"name":"synthetic-ci","conclusion":"FAILURE"}]}), 73).unwrap();
        apply_observation(&db, "derive", 73, "ThroughMerge", Some(&reason), None).await.unwrap();
        // The observation was persisted, but the app died before queuing its wake.
        sqlx::query("UPDATE thin_external_observations SET observed_at=?")
            .bind(chrono::Utc::now().timestamp_millis() + 3_600_000).execute(&db).await.unwrap();
        db.close().await;
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["m168_cross_process_pr_ci_failure_is_delivered_after_restart", "--nocapture"])
            .env(CHILD, &url).output().await.unwrap();
        assert!(output.status.success(), "child failed: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let reopened = crate::storage::db::connect(&url).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='done' AND fire_count=1")
            .fetch_one(&reopened).await.unwrap();
        assert_eq!(count, 1);
        heartbeat_external(&reopened, "derive").await.unwrap();
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules").fetch_one(&reopened).await.unwrap();
        assert_eq!(total, 1);
        reopened.close().await;
    }

    #[test]
    fn m168_all_green_ci_is_a_wake_condition() {
        let pr = serde_json::json!({"state":"OPEN", "statusCheckRollup":[
            {"name":"synthetic-test", "status":"COMPLETED", "conclusion":"SUCCESS"},
            {"context":"synthetic-status", "state":"SUCCESS"}
        ]});
        assert_eq!(completion_reason(&pr, "ThroughMerge", false, 168).as_deref(), Some("PR #168 检查已全绿"));
        let pending = serde_json::json!({"state":"OPEN", "statusCheckRollup":[
            {"name":"synthetic-test", "status":"IN_PROGRESS", "conclusion":""}
        ]});
        assert!(completion_reason(&pending, "ThroughMerge", false, 168).is_none());
        assert!(completion_reason(&serde_json::json!({"state":"OPEN", "statusCheckRollup":[]}), "ThroughMerge", false, 168).is_none());
    }

    #[tokio::test]
    async fn m168_pending_wake_without_executor_is_not_running() {
        let db = fixture().await;
        super::super::background_tasks::schedule_wakeup(&db,
            super::super::background_tasks::WakeupRequest {
                session_id: "derive".into(), root_turn_id: None, prompt: "synthetic".into(),
                source: super::super::background_tasks::TaskSource::AgentDecided,
                after_sec: Some(60), every_sec: None,
            }).await.unwrap();
        sqlx::query("UPDATE wakeup_schedules SET next_at=1").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('m168-user','derive','user','synthetic',1)")
            .execute(&db).await.unwrap();
        let state = derive_state(&db, "derive").await.unwrap();
        assert_eq!(state.state, SessionState::WaitingWake);
    }

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
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
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

    /// M158 取代 M140 的「外部等待立刻抛给用户」：等 PR / CI 是系统自己能监视的
    /// 对象，没有活着的监视源时由系统登记默认心跳，绝不变成用户的问题
    /// （现场 816bc573 / 9161c74a）。
    #[tokio::test]
    async fn m158_external_wait_stays_a_system_wait_with_or_without_a_live_monitor() {
        let db = fixture().await;
        let waiting = super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "交付 PR / CI".into(), covered: false }, "");
        super::super::thin_state::save(&db, "derive", &waiting).await.unwrap();
        let unwatched = refresh(&db, "derive").await.unwrap();
        assert_eq!(unwatched.state, SessionState::WaitingWake, "{unwatched:?}");
        assert!(unwatched.question.is_none(), "外部等待不得变成用户问题：{:?}", unwatched.question);
        assert!(unwatched.wake_source.unwrap_or_default().contains("系统自动监视"));
        super::super::thin_state::save(&db, "derive", &waiting).await.unwrap();
        super::super::background_tasks::wake_source_cover(&db, "derive").await;
        sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,created_at) VALUES ('monitor','derive','synthetic','agent_decided',9223372036854775807,'pending',1)")
            .execute(&db).await.unwrap();
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
        sqlx::query("UPDATE wakeup_schedules SET next_at=1").execute(&db).await.unwrap();
        let expired = refresh(&db, "derive").await.unwrap();
        assert_eq!(expired.state, SessionState::WaitingWake, "监视到期的外部等待仍由系统跟进：{expired:?}");
        assert!(expired.question.is_none());
    }

    /// M158/R3：历史上写进库里的内部问题（waiting_user + 内部用词）不得永久粘在
    /// 界面上；派生层用用户能回答的话重写它，且不再出现内部状态词。
    #[tokio::test]
    async fn m158_legacy_internal_question_is_rewritten_without_internal_wording() {
        let db = fixture().await;
        super::super::thin_state::save(&db, "derive", &ThinState {
            state: SessionState::WaitingUser,
            waiting_for: Some("目标目录确认".into()),
            question: Some("没有活着的唤醒源，请选择继续监视或结束任务".into()),
            ..ThinState::running()
        })
        .await
        .unwrap();
        let state = refresh(&db, "derive").await.unwrap();
        assert_eq!(state.state, SessionState::WaitingUser, "{state:?}");
        let question = state.question.clone().unwrap_or_default();
        assert!(question.contains("目标目录确认"), "{question}");
        assert!(!has_internal_wording(&question), "{question}");
    }

    /// M158/R2：后台任务 / 定时唤醒这类系统占位等待也是系统自己跟进的，
    /// 到期没有进展时不许把它变成用户的问题。
    #[tokio::test]
    async fn m158_system_placeholder_wait_never_becomes_a_user_question() {
        for what in ["后台任务或未来定时唤醒", "定时唤醒（十分钟后）", "PR / CI 监视"] {
            let db = fixture().await;
            super::super::thin_state::save(&db, "derive", &super::super::thin_state::after_final(
                super::super::thin_state::WaitNeed::External { what: what.into(), covered: false }, "")).await.unwrap();
            let state = refresh(&db, "derive").await.unwrap();
            assert_eq!(state.state, SessionState::WaitingWake, "{what}: {state:?}");
            assert!(state.question.is_none(), "{what}: {:?}", state.question);
        }
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
        let control = crate::ChatRunControl::pending();
        crate::register_session_executor("derive-live", &control);
        assert_eq!(derive_state(&db, "derive-live").await.unwrap().state, SessionState::Running);
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
        assert_eq!(derive_state(&db, "derive-live").await.unwrap().state, SessionState::WaitingWake);
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
        assert_eq!(derive_state(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
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
                SessionState::WaitingWake,
                "{reason} 无 executor 时应等待恢复而非 ended/running（reason={:?}）",
                derived.reason
            );
        }
    }

    /// M148/R3：等 PR 的会话不得因为「回合被取代 / 最终回答已完成」而结束——那会让
    /// M142 的系统 PR 监视停摆，PR 失败无人处理。这里用「已合并但 closeout 未入队」
    /// 的观测：前面的 M142 分支不覆盖它，只有 unfinished_delivery 守卫能拦住。
    #[tokio::test]
    async fn unfinished_delivery_keeps_a_delivered_session_waiting() {
        let db = fixture().await;
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS thin_external_observations(session_id TEXT PRIMARY KEY,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL,observed_at INTEGER NOT NULL DEFAULT 0,completed_reason TEXT,closeout_queued INTEGER NOT NULL DEFAULT 0,merged_at INTEGER);
             CREATE TABLE IF NOT EXISTS wakeup_schedules(id TEXT PRIMARY KEY,session_id TEXT,status TEXT,next_at INTEGER,last_fired_at INTEGER,prompt TEXT);
             DELETE FROM chat_turn_state",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at,completed_at,terminal_reason) VALUES ('turn','derive','settled','completed',1000,2000,2000,'superseded_by_user_reprompt')")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs(session_id,branch,pr_number,ceiling) VALUES ('derive','feat/m148',305,'merged')")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,closeout_queued) VALUES ('derive',305,'merged',2500,'merged',0)")
            .execute(&db)
            .await
            .unwrap();

        let derived = derive_state(&db, "derive").await.unwrap();
        assert_eq!(
            derived.state,
            SessionState::WaitingWake,
            "未关闭的 PR 必须让会话保持等待（reason={:?}）",
            derived.reason
        );
        assert_eq!(
            derived.wake_source.as_deref(),
            Some("系统 PR 监视（心跳，间隔不少于 180 秒）")
        );

        // 边界：closeout 已入队（PR 真正收口）后，同一个终态事实照旧结束会话。
        sqlx::query("UPDATE thin_external_observations SET closeout_queued=1")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(
            derive_state(&db, "derive").await.unwrap().state,
            SessionState::Ended
        );
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
        assert_eq!(refresh(&db, "derive").await.unwrap().state, SessionState::WaitingWake);
    }

    #[tokio::test]
    async fn uncertain_running_projection_is_not_ended_or_retimestamped() {
        let db = fixture().await;
        let mut state = ThinState::running();
        state.updated_at = 123;
        super::super::thin_state::save(&db, "derive", &state).await.unwrap();
        let mut repaired_at = None;
        for _ in 0..3 {
            let derived = refresh(&db, "derive").await.unwrap();
            assert_eq!(derived.state, SessionState::WaitingWake);
            assert_eq!(*repaired_at.get_or_insert(derived.updated_at), derived.updated_at);
        }
    }

    async fn ensure_wake_table(db: &SqlitePool) {
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS wakeup_schedules(id TEXT PRIMARY KEY,session_id TEXT,root_turn_id TEXT,prompt TEXT NOT NULL,source TEXT NOT NULL,after_sec INTEGER,every_sec INTEGER,next_at INTEGER NOT NULL,status TEXT NOT NULL,fire_count INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL,last_fired_at INTEGER);")
            .execute(db).await.unwrap();
    }

    /// M146/R1：升级回填不得给早已收尾的历史会话起模型回合。
    /// 现场 v1.91.14：升级后 01:12:35–01:13:20 一次性唤醒 23 个已 ended 的旧会话。
    #[tokio::test]
    async fn m146_historical_closed_out_session_is_marked_without_a_turn() {
        let db = fixture().await;
        ensure_wake_table(&db).await;
        super::super::thin_state::save(&db, "derive", &super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "PR / CI".into(), covered: true }, "")).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',776,'ThroughMerge')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('closeout','derive','assistant','§8 收尾完成：分支已清理',2000)")
            .execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,merged_at) VALUES ('derive',776,'ThroughMerge',1,'PR #776 已合并',1000)")
            .execute(&db).await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        let wakes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules").fetch_one(&db).await.unwrap();
        assert_eq!(wakes, 0, "已收尾的历史会话不得再被收尾唤醒");
        let queued: i64 = sqlx::query_scalar("SELECT closeout_queued FROM thin_external_observations").fetch_one(&db).await.unwrap();
        assert_eq!(queued, 1, "外部事项仍要标记完成，只是不起回合");
        assert_ne!(refresh(&db, "derive").await.unwrap().state, SessionState::Running);
    }

    /// M146/R1 的反面：合并之后还没有任何回答的等待会话，仍要唤醒收尾一次。
    #[tokio::test]
    async fn m146_waiting_session_gets_closeout_wake_when_answer_predates_merge() {
        let db = fixture().await;
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',776,'ThroughMerge')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('delivered','derive','assistant','PR 已交付，等待合并监视',100)")
            .execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,merged_at) VALUES ('derive',776,'ThroughMerge',1,'PR #776 已合并',5000)")
            .execute(&db).await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        let wakes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE id LIKE 'thin-closeout-%'").fetch_one(&db).await.unwrap();
        assert_eq!(wakes, 1);
    }

    /// M146/R2：定时唤醒写入的用户消息如果没有回合真正开始，不能把会话永久投影成
    /// running。现场 d632e17a：01:12:51 唤醒后 running 30 分钟、零助手回复。
    #[tokio::test]
    async fn m146_wake_message_without_a_turn_is_not_running() {
        let db = fixture().await;
        ensure_wake_table(&db).await;
        sqlx::raw_sql("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('answer','derive','assistant','最终回答',100);
            INSERT INTO wakeup_schedules(id,session_id,prompt,source,after_sec,next_at,status,created_at,last_fired_at) VALUES ('thin-closeout-synthetic','derive','PR #776 已合并，交付外部事项已完成。','agent_decided',60,150,'done',150,150);")
            .execute(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',776,'ThroughMerge')").execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,closeout_queued) VALUES ('derive',776,'ThroughMerge',1,'PR #776 已合并',1)")
            .execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('wake','derive','user','定时唤醒 thin-closeout-synthetic：PR #776 已合并\n节奏：一次性 after_sec=60',200)")
            .execute(&db).await.unwrap();
        let state = refresh(&db, "derive").await.unwrap();
        assert_ne!(state.state, SessionState::Running, "没有回合在跑就不能显示 running（{:?}）", state.reason);
    }

    /// M146/R4：PR 冲突或被合并队列退回复活会话也要带原因唤醒。
    #[test]
    fn m146_conflicting_and_queue_blocked_prs_wake_with_reason() {
        let conflict = failure_reason(&serde_json::json!({"state":"OPEN","mergeable":"CONFLICTING","statusCheckRollup":[]}), 91).unwrap();
        assert!(conflict.contains("合并冲突") && conflict.contains("#91"));
        assert!(is_external_failure(&conflict));
        let dirty = failure_reason(&serde_json::json!({"state":"OPEN","mergeable":"UNKNOWN","mergeStateStatus":"DIRTY"}), 91).unwrap();
        assert!(dirty.contains("合并冲突"));
        let blocked = failure_reason(&serde_json::json!({"state":"OPEN","mergeable":"MERGEABLE","labels":[{"name":"merge-queue: blocked"}]}), 91).unwrap();
        assert!(blocked.contains("合并队列") && blocked.contains("blocked"));
        assert!(is_external_failure(&blocked));
        assert!(failure_reason(&serde_json::json!({"state":"OPEN","mergeable":"MERGEABLE","labels":[{"name":"merge-queue: armed"}]}), 91).is_none());
    }

    /// M146/R3：同一时刻最多 2 个收尾回合，升级后不再成批占用模型。
    #[tokio::test]
    async fn m146_closeout_turns_are_throttled_to_two() {
        let db = fixture().await;
        ensure_wake_table(&db).await;
        for id in ["derive-b", "derive-c"] {
            sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES (?,'synthetic','/synthetic','model',1,1,'thin')")
                .bind(id).execute(&db).await.unwrap();
        }
        sqlx::raw_sql("INSERT INTO wakeup_schedules(id,session_id,prompt,source,after_sec,next_at,status,created_at) VALUES ('thin-closeout-running-1','derive-b','synthetic','agent_decided',60,99999999999,'pending',1);
            INSERT INTO wakeup_schedules(id,session_id,prompt,source,after_sec,next_at,status,created_at) VALUES ('thin-closeout-running-2','derive-c','synthetic','agent_decided',60,99999999999,'pending',1);")
            .execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',776,'ThroughMerge')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,merged_at) VALUES ('derive',776,'ThroughMerge',1,'PR #776 已合并',5000)")
            .execute(&db).await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        let mine: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id='derive'").fetch_one(&db).await.unwrap();
        assert_eq!(mine, 0, "已有 2 个收尾回合在排队时必须让出额度");
        let queued: i64 = sqlx::query_scalar("SELECT closeout_queued FROM thin_external_observations WHERE session_id='derive'").fetch_one(&db).await.unwrap();
        assert_eq!(queued, 0, "被限流的会话要留待下一轮，不得静默标记完成");
    }

    /// M150：PR 已合并、但 ceiling 是 ThroughRelease 且还没发布时，新观测既不是失败
    /// 也不是完成。旧失败原因必须被清除——否则 COALESCE 会永久保留它、同时把
    /// closeout_queued 重置为 0，于是每轮心跳都重排一次 thin-repair 唤醒
    /// （现场：PR #829 合并后仍每 60s 收到同一条失败唤醒）。
    #[tokio::test]
    async fn m150_terminal_pr_clears_stale_failure_instead_of_requeueing_repair() {
        let db = fixture().await;
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',829,'ThroughRelease')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,closeout_queued) VALUES ('derive',829,'ThroughRelease',1,'PR #829 检查失败：scenario-gate-pr',1)")
            .execute(&db).await.unwrap();
        apply_observation(&db, "derive", 829, "ThroughRelease", None, Some(1_792_622_995_000)).await.unwrap();
        let row: (Option<String>, i64, Option<i64>) = sqlx::query_as(
            "SELECT completed_reason,closeout_queued,merged_at FROM thin_external_observations WHERE session_id='derive'",
        ).fetch_one(&db).await.unwrap();
        assert_eq!(row.0, None, "合并后旧失败原因必须清除");
        assert_eq!(row.1, 1, "清除失败不得把 closeout_queued 归零，否则会再排一次修复唤醒");
        assert_eq!(row.2, Some(1_792_622_995_000), "合并时间仍要落库");
    }

    /// M150 的反向保护：PR 仍开着且出现新失败时，必须照旧重新排队一次修复回合。
    #[tokio::test]
    async fn m150_live_pr_new_failure_still_requeues() {
        let db = fixture().await;
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',829,'ThroughMerge')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,closeout_queued) VALUES ('derive',829,'ThroughMerge',1,'PR #829 检查失败：unit-tests',1)")
            .execute(&db).await.unwrap();
        apply_observation(&db, "derive", 829, "ThroughMerge", Some("PR #829 检查失败：scenario-gate-pr"), None).await.unwrap();
        let row: (Option<String>, i64) = sqlx::query_as(
            "SELECT completed_reason,closeout_queued FROM thin_external_observations WHERE session_id='derive'",
        ).fetch_one(&db).await.unwrap();
        assert_eq!(row.0.as_deref(), Some("PR #829 检查失败：scenario-gate-pr"));
        assert_eq!(row.1, 0, "新失败必须重新排队一次修复回合");
    }

    /// M146/R4+R5 的唤醒侧：此前的用例只断言 `failure_reason` 的文案，没有断言冲突
    /// 事实真的排出了一次唤醒。冲突必须带原因排一次修复唤醒（thin-repair）。
    #[tokio::test]
    async fn m146_conflicting_pr_queues_one_repair_wake_with_reason() {
        let db = fixture().await;
        ensure_wake_table(&db).await;
        super::super::thin_state::save(&db, "derive", &super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "PR / CI".into(), covered: true }, ""))
            .await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',776,'ThroughMerge')").execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason) VALUES ('derive',776,'ThroughMerge',1,'PR #776 存在合并冲突（mergeable=CONFLICTING），需要解决冲突后重新交付')")
            .execute(&db).await.unwrap();
        heartbeat_external(&db, "derive").await.unwrap();
        let wakes: Vec<(String, String)> = sqlx::query_as(
            "SELECT id,prompt FROM wakeup_schedules WHERE session_id='derive' ORDER BY id",
        ).fetch_all(&db).await.unwrap();
        assert_eq!(wakes.len(), 1, "冲突必须唤醒会话一次：{wakes:?}");
        assert!(wakes[0].0.starts_with("thin-repair-"), "冲突是外部失败，走修复唤醒：{}", wakes[0].0);
        assert!(wakes[0].1.contains("合并冲突"), "唤醒必须带原因：{}", wakes[0].1);
    }

    /// M146/R1+R5 的「一次」：合并后的同一份观测重复心跳，收尾唤醒只排一次。
    #[tokio::test]
    async fn m146_closeout_wake_is_queued_only_once() {
        let db = fixture().await;
        ensure_wake_table(&db).await;
        sqlx::query("INSERT INTO thin_delivery_refs VALUES ('derive','synthetic',776,'ThroughMerge')").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('delivered','derive','assistant','PR 已交付，等待合并监视',100)")
            .execute(&db).await.unwrap();
        ensure_external(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,merged_at) VALUES ('derive',776,'ThroughMerge',1,'PR #776 已合并',5000)")
            .execute(&db).await.unwrap();
        for round in 1..=3 {
            heartbeat_external(&db, "derive").await.unwrap();
            let wakes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id='derive' AND id LIKE 'thin-closeout-%'")
                .fetch_one(&db).await.unwrap();
            assert_eq!(wakes, 1, "第 {round} 次心跳重复唤醒了收尾回合");
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

    /// M158/R1+R2+R3（现场：会话 816bc573 / 9161c74a 开完 PR 后停在
    /// 「外部事项尚无法确认完成，且没有活着的唤醒源。请选择继续监视或结束任务。」）：
    /// 交付回合结束时，系统必须自己登记 PR 监视，派生 waiting_wake，
    /// 并且绝不把内部状态词变成给用户的问题。
    #[tokio::test]
    async fn m158_opened_pr_registers_system_monitor_instead_of_asking_the_user() {
        let db = fixture().await;
        super::super::thin_state::save(&db, "derive", &super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "PR / CI".into(), covered: false }, ""))
            .await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('created','derive','assistant','已开 PR：https://github.com/BumStill/CodeFactory/pull/846',100)")
            .execute(&db).await.unwrap();
        let state = refresh(&db, "derive").await.unwrap();
        assert_eq!(state.state, SessionState::WaitingWake, "开完 PR 不得停在 waiting_user：{state:?}");
        assert!(state.question.is_none(), "不得把内部决策变成给用户的问题：{:?}", state.question);
        assert!(state.waiting_for.as_deref().unwrap_or_default().contains("#846"), "{:?}", state.waiting_for);
        let reference: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM thin_delivery_refs WHERE session_id='derive' AND pr_number=846",
        ).fetch_one(&db).await.unwrap();
        assert_eq!(reference, 1, "PR 监视必须真的登记成交付引用");
        let observed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM thin_external_observations WHERE session_id='derive' AND pr_number=846",
        ).fetch_one(&db).await.unwrap();
        assert_eq!(observed, 1, "登记之后必须有可被心跳观测的行");
        for text in [state.waiting_for, state.question, state.reason, state.wake_source].into_iter().flatten() {
            assert!(!has_internal_wording(&text), "用户可见文案不得含内部词：{text}");
        }
    }

    /// M158/R3：已经落库的「没有活着的唤醒源」这一行会被重新派生掉。
    #[tokio::test]
    async fn m158_legacy_limbo_row_is_rederived_into_a_system_monitor() {
        let db = fixture().await;
        let mut limbo = super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "PR / CI".into(), covered: false }, "");
        limbo.state = SessionState::WaitingUser;
        limbo.question = Some("外部事项尚无法确认完成，且没有活着的唤醒源。请选择继续监视或结束任务。".into());
        super::super::thin_state::save(&db, "derive", &limbo).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('created','derive','assistant','PR #846 已开，等 CI',100)")
            .execute(&db).await.unwrap();
        let state = refresh(&db, "derive").await.unwrap();
        assert_eq!(state.state, SessionState::WaitingWake, "{state:?}");
        assert!(state.question.is_none(), "内部问题必须消失：{:?}", state.question);
        assert!(state.waiting_for.as_deref().unwrap_or_default().contains("#846"));
    }

    /// M158/R3：真的等用户时，问题写成用户能回答的一句，且不含内部词。
    #[tokio::test]
    async fn m158_user_wait_question_uses_plain_wording() {
        let db = fixture().await;
        super::super::thin_state::save(&db, "derive", &super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::Unwatched { what: "客户的回复".into() }, ""))
            .await.unwrap();
        let state = refresh(&db, "derive").await.unwrap();
        assert_eq!(state.state, SessionState::WaitingUser, "{state:?}");
        let question = state.question.clone().unwrap_or_default();
        assert!(question.contains("客户的回复"), "{question}");
        assert!(!has_internal_wording(&question), "内部词不得出现在问题里：{question}");
        assert!(state.wake_source.is_none() || !has_internal_wording(&state.wake_source.clone().unwrap_or_default()));
    }
}

/// M186：已合并/已关闭的等待会话必须被结算，绝不能留下「唤醒已取消 + waiting_wake」
/// 这种既不会醒也无人负责的组合。逻辑放在这个已登记的场景文件里（HLT-004），
/// 因为新增独立模块需要同时修改场景登记表——那是信任根，普通 PR 不得自我修改。
pub(crate) mod merged_settlement {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicI64, Ordering};

    use anyhow::Result;
    use sqlx::SqlitePool;

    use crate::agent::thin_state;
    use crate::util::no_window::NoWindow;

    /// M186/R3：终端状态与「取消该会话全部 pending 唤醒」在同一事务内提交。
    /// 缺陷正是这两件事分开写造成的：cancelled 唤醒旁边留着 waiting_wake，
    /// 既不会醒，也再没有任何路径会来结算它。
    pub(crate) async fn settle_ended_and_cancel(
        db: &SqlitePool,
        session_id: &str,
        reason: &str,
        location: &str,
    ) -> Result<()> {
        write_state_and_cancel(
            db,
            session_id,
            &thin_state::ThinState::ended(reason, location),
        )
        .await
    }

    /// M186/R3：问用户与取消唤醒同样原子，问题落定后不留陈旧唤醒。
    pub(crate) async fn ask_user_and_cancel(
        db: &SqlitePool,
        session_id: &str,
        question: &str,
    ) -> Result<()> {
        let next = thin_state::ThinState {
            state: thin_state::SessionState::WaitingUser,
            waiting_for: None,
            wake_source: None,
            question: Some(question.to_string()),
            ..thin_state::ThinState::running()
        };
        write_state_and_cancel(db, session_id, &next).await
    }

    async fn write_state_and_cancel(
        db: &SqlitePool,
        session_id: &str,
        state: &thin_state::ThinState,
    ) -> Result<()> {
        if state.state == thin_state::SessionState::Ended
            && (super::has_active_work(db, session_id).await?
                || super::live_background_count(db, session_id).await? > 0) {
            tracing::warn!(session_id, "terminal settlement rejected: live work; automatic recovery retains ownership");
            let next = if super::has_active_work(db, session_id).await? {
                thin_state::ThinState::running()
            } else {
                thin_state::after_final(thin_state::WaitNeed::External {
                    what: "后台任务".into(), covered: true,
                }, "本会话记录")
            };
            thin_state::save(db, session_id, &next).await?;
            return Ok(());
        }
        thin_state::ensure(db).await?;
        crate::agent::background_tasks::ensure_wake_schema(db).await?;
        let mut tx = db.begin().await?;
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
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE wakeup_schedules SET status='cancelled'
             WHERE session_id=? AND status='pending'",
        )
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// R4: the periodic sweep may run at most this often; the supervisor polls much
    /// faster, so this is a budget gate rather than the cadence.
    pub const SETTLE_TICK_INTERVAL_MS: i64 = 60_000;

    static LAST_TICK_MS: AtomicI64 = AtomicI64::new(0);

    #[derive(Debug, Default, PartialEq, Eq)]
    pub struct SettleSummary {
        pub settled: usize,
        pub asked_user: usize,
        pub skipped: usize,
    }

    #[derive(Debug, Clone)]
    struct TerminalWorkspace {
        worktree_path: PathBuf,
        repo_root: PathBuf,
        branch_name: String,
    }

    /// R3/R4: budgeted safety tick for the supervisor poll loop.
    pub async fn safety_tick(pool: &SqlitePool) -> Result<SettleSummary> {
        if !tick_due(chrono::Utc::now().timestamp_millis()) {
            return Ok(SettleSummary::default());
        }
        let summary = settle_merged_sessions(pool).await?;
        super::reconcile_running_liveness(pool).await?;
        Ok(summary)
    }

    fn tick_due(now: i64) -> bool {
        let last = LAST_TICK_MS.load(Ordering::SeqCst);
        if last != 0 && now.saturating_sub(last) < SETTLE_TICK_INTERVAL_MS {
            return false;
        }
        LAST_TICK_MS
            .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// R1/R2: settle every `waiting_wake` session whose external work concluded.
    /// Deterministic — this never asks a model to finish the task.
    pub async fn settle_merged_sessions(pool: &SqlitePool) -> Result<SettleSummary> {
        thin_state::ensure(pool).await?;
        super::reconcile_orphan_turns(pool).await?;
        let sessions: Vec<String> =
            sqlx::query_scalar("SELECT session_id FROM thin_session_state WHERE state IN ('waiting_wake','running')")
                .fetch_all(pool)
                .await?;

        let mut summary = SettleSummary::default();
        for session in sessions {
            // A live executor owns its own settlement; never race it.
            if crate::session_executor_alive(&session) != Some(false)
                || super::live_background_count(pool, &session).await? > 0 {
                summary.skipped += 1;
                continue;
            }
            let Some(reason) = terminal_reason(pool, &session).await? else {
                summary.skipped += 1;
                continue;
            };
            let workspaces = terminal_workspaces(pool, &session).await?;
            match closeout_all(&workspaces) {
                Ok(note) => {
                    let text = format!("{reason}。系统已自动完成收尾：{note}。任务已结束。");
                    settle_ended_and_cancel(pool, &session, &text, &note).await?;
                    close_workspace_rows(pool, &session).await?;
                    tracing::info!(session, %note, "merged/closed session settled after restart");
                    summary.settled += 1;
                }
                Err(failure) => {
                    let question = format!(
                        "{reason}，但系统自动收尾没有完成：{failure}。需要你决定如何处理这个工作区。"
                    );
                    ask_user_and_cancel(pool, &session, &question).await?;
                    tracing::warn!(session, %failure, "merged session closeout deferred to the user");
                    summary.asked_user += 1;
                }
            }
        }
        Ok(summary)
    }

    /// R4 invariant probe: `waiting_wake` rows no wake source can ever cover.
    pub async fn waiting_wake_without_wake_source(pool: &SqlitePool) -> Vec<String> {
        let Ok(sessions) = sqlx::query_scalar::<_, String>(
            "SELECT session_id FROM thin_session_state WHERE state='waiting_wake'",
        )
        .fetch_all(pool)
        .await
        else {
            return Vec::new();
        };
        let mut orphaned = Vec::new();
        for session in sessions {
            if crate::agent::background_tasks::wake_source_cover(pool, &session)
                .await
                .is_covered()
            {
                continue;
            }
            orphaned.push(session);
        }
        orphaned
    }

    async fn terminal_reason(pool: &SqlitePool, session: &str) -> Result<Option<String>> {
        if !has_table(pool, "thin_external_observations").await? {
            return Ok(None);
        }
        if has_table(pool, "thin_delivery_refs").await? {
            let mismatched: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM thin_delivery_refs ref WHERE ref.session_id=?
                 AND NOT EXISTS (SELECT 1 FROM thin_external_observations observation
                     WHERE observation.session_id=ref.session_id
                       AND observation.pr_number=ref.pr_number
                       AND observation.ceiling=ref.ceiling)",
            ).bind(session).fetch_one(pool).await?;
            if mismatched > 0 {
                tracing::warn!(session, "stale terminal PR observation cannot settle current delivery");
                return Ok(None);
            }
        }
        let reason: Option<String> = sqlx::query_scalar(
            "SELECT completed_reason FROM thin_external_observations
             WHERE session_id=? AND completed_reason IS NOT NULL
               AND (completed_reason LIKE '%已合并%' OR completed_reason LIKE '%已关闭%')
             ORDER BY rowid DESC LIMIT 1",
        )
        .bind(session)
        .fetch_optional(pool)
        .await?;
        Ok(reason)
    }

    async fn terminal_workspaces(pool: &SqlitePool, session: &str) -> Result<Vec<TerminalWorkspace>> {
        if !has_table(pool, "execution_workspaces").await? {
            return Ok(Vec::new());
        }
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT worktree_path, repo_root, branch_name FROM execution_workspaces
             WHERE session_id=? AND state<>'closed'",
        )
        .bind(session)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(worktree_path, repo_root, branch_name)| TerminalWorkspace {
                    worktree_path: PathBuf::from(worktree_path),
                    repo_root: PathBuf::from(repo_root),
                    branch_name,
                },
            )
            .collect())
    }

    async fn close_workspace_rows(pool: &SqlitePool, session: &str) -> Result<()> {
        if !has_table(pool, "execution_workspaces").await? {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE execution_workspaces
             SET state='closed', failure_code=NULL, failure_detail=NULL,
                 lease_owner=NULL, lease_expires_at=NULL, closed_at=?, updated_at=?
             WHERE session_id=? AND state<>'closed'",
        )
        .bind(now)
        .bind(now)
        .bind(session)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn has_table(pool: &SqlitePool, name: &str) -> Result<bool> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(name)
                .fetch_one(pool)
                .await?;
        Ok(count > 0)
    }

    /// R1: no workspace left → nothing to close out. R2: workspace still there →
    /// deterministic §8 closeout, or a plain reason for the user.
    fn closeout_all(workspaces: &[TerminalWorkspace]) -> Result<String, String> {
        if workspaces.is_empty() {
            return Ok("本地工作区已不存在".to_string());
        }
        let mut notes = Vec::new();
        for workspace in workspaces {
            notes.push(closeout_workspace(workspace)?);
        }
        Ok(notes.join("；"))
    }

    fn closeout_workspace(workspace: &TerminalWorkspace) -> Result<String, String> {
        // Destructive local Git surgery: refuse anything outside the reserved
        // namespace or pointing at the repository itself.
        if !workspace.branch_name.starts_with("codefactory/objective-") {
            return Err(format!(
                "分支 {} 不在 CodeFactory 保留命名空间内，系统不会删除",
                workspace.branch_name
            ));
        }
        let canonical_repo = workspace
            .repo_root
            .canonicalize()
            .map_err(|error| format!("仓库根目录不可用：{error}"))?;
        if let Ok(canonical_worktree) = workspace.worktree_path.canonicalize() {
            if canonical_worktree == canonical_repo {
                return Err("工作区路径指向仓库根目录，系统不会删除".to_string());
            }
        }

        let mut notes = Vec::new();
        if workspace.worktree_path.exists() {
            let dirty = git(&workspace.worktree_path, &["status", "--porcelain"]).map_err(|error| {
                format!(
                    "无法确认工作区 {} 是否干净：{error}",
                    workspace.worktree_path.display()
                )
            })?;
            if !dirty.is_empty() {
                let count = dirty.lines().count();
                return Err(format!(
                    "本地工作区 {} 里还有 {count} 项未提交改动",
                    workspace.worktree_path.display()
                ));
            }
            // M187/R3: clean does not imply published. Preserve commits that
            // have no surviving local branch or remote-tracking reference.
            let unpublished = git(&workspace.worktree_path, &[
                "rev-list", "--count", "HEAD", "--not", "--remotes",
                &format!("--exclude={}", workspace.branch_name), "--branches",
            ])?;
            if unpublished.trim() != "0" {
                return Err(format!("工作区还有 {unpublished} 个未推送且未被其他分支保存的提交，系统自动重试/自愈：保留工作区"));
            }
            let path = workspace.worktree_path.to_string_lossy().to_string();
            git(
                &workspace.repo_root,
                &["worktree", "remove", "--force", &path],
            )
            .or_else(|_| git(&workspace.repo_root, &["worktree", "remove", &path]))
            .map_err(|error| {
                format!(
                    "删除本地工作区 {} 失败：{error}",
                    workspace.worktree_path.display()
                )
            })?;
            notes.push("已删除本地工作区".to_string());
        } else {
            notes.push("本地工作区已不存在".to_string());
        }
        let _ = git(&workspace.repo_root, &["worktree", "prune"]);

        match git(
            &workspace.repo_root,
            &["branch", "-D", &workspace.branch_name],
        ) {
            Ok(_) => notes.push("已删除本地分支".to_string()),
            Err(error) if error.contains("not found") => notes.push("本地分支已不存在".to_string()),
            Err(error) => {
                return Err(format!(
                    "删除本地分支 {} 失败：{error}",
                    workspace.branch_name
                ))
            }
        }

        match remote_branch_state(&workspace.repo_root, &workspace.branch_name) {
            Some(true) => {
                git(
                    &workspace.repo_root,
                    &["push", "origin", "--delete", &workspace.branch_name],
                )
                .map_err(|error| {
                    format!(
                        "删除已合并的远端分支 {} 失败：{error}",
                        workspace.branch_name
                    )
                })?;
                notes.push("已删除已合并远端分支".to_string());
            }
            Some(false) => notes.push("远端分支已不存在".to_string()),
            // Offline or no `origin`: a network read must never decide a session's
            // fate. The merge is already recorded and the local closeout is done.
            None => notes.push("远端分支状态未知，未做远端删除".to_string()),
        }
        Ok(notes.join("，"))
    }

    fn remote_branch_state(repo: &Path, branch: &str) -> Option<bool> {
        let output = Command::new("git")
            .no_window()
            .current_dir(repo)
            .args(["ls-remote", "--heads", "origin", branch])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    }

    fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .no_window()
            .current_dir(cwd)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }


    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{SystemTime, UNIX_EPOCH};

        fn scratch_dir(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "cf-m186-{tag}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn git_ok(cwd: &Path, args: &[&str]) -> String {
            git(cwd, args).expect("git")
        }

        /// A real repository with one worktree per synthetic session. The child
        /// process builds it so the parent only sees durable state written by a
        /// previous process — exactly the restart the defect needs.
        fn build_fixture(dir: &Path) -> (PathBuf, String, PathBuf, String) {
            let repo = dir.join("repo");
            std::fs::create_dir_all(&repo).unwrap();
            git_ok(&repo, &["init", "-q"]);
            git_ok(&repo, &["config", "user.name", "Synthetic"]);
            git_ok(
                &repo,
                &["config", "user.email", "synthetic@example.invalid"],
            );
            std::fs::write(repo.join("tracked.txt"), "base").unwrap();
            git_ok(&repo, &["add", "."]);
            git_ok(&repo, &["commit", "-qm", "synthetic base"]);

            let missing = dir.join("ws-missing");
            git_ok(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "codefactory/objective-m186a",
                    &missing.to_string_lossy(),
                    "HEAD",
                ],
            );
            git_ok(
                &repo,
                &["worktree", "remove", "--force", &missing.to_string_lossy()],
            );
            let _ = std::fs::remove_dir_all(&missing);
            assert!(!missing.exists());

            let alive = dir.join("ws-alive");
            git_ok(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "codefactory/objective-m186b",
                    &alive.to_string_lossy(),
                    "HEAD",
                ],
            );
            (
                repo,
                "codefactory/objective-m186a".into(),
                alive,
                "codefactory/objective-m186b".into(),
            )
        }

        async fn open_db(dir: &Path) -> SqlitePool {
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(dir.join("app.db"))
                .create_if_missing(true);
            SqlitePool::connect_with(options).await.unwrap()
        }

        async fn seed_state(dir: &Path) -> SqlitePool {
            let pool = open_db(dir).await;
            sqlx::query("CREATE TABLE IF NOT EXISTS objectives (id TEXT PRIMARY KEY, status TEXT)")
                .execute(&pool)
                .await
                .unwrap();
            thin_state::ensure(&pool).await.unwrap();
            crate::agent::background_tasks::ensure_wake_schema(&pool)
                .await
                .unwrap();
            crate::agent::execution_workspace::ensure_schema(&pool)
                .await
                .unwrap();
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS thin_external_observations (
                    session_id TEXT NOT NULL,
                    completed_reason TEXT,
                    observed_at INTEGER)",
            )
            .execute(&pool)
            .await
            .unwrap();
            pool
        }

        /// Child-process re-entry: writes the durable restart fixture and exits.
        #[test]
        fn fixture_writer() {
            let Ok(dir) = std::env::var("M186_FIXTURE_DIR") else {
                return;
            };
            let dir = PathBuf::from(dir);
            let marker = dir.join("app.db");
            let (repo, branch_a, alive, branch_b) = build_fixture(&dir);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let pool = seed_state(&dir).await;
                for (session, objective, path, branch) in [
                    ("sess-a", "obj-a", dir.join("ws-missing"), branch_a.as_str()),
                    ("sess-b", "obj-b", alive.clone(), branch_b.as_str()),
                ] {
                    let mut state = thin_state::ThinState::running();
                    state.state = crate::agent::thin_state::SessionState::WaitingWake;
                    state.waiting_for = Some("PR #186 合并".into());
                    state.wake_source = Some("已登记的唤醒源：PR #186 合并".into());
                    thin_state::save(&pool, session, &state).await.unwrap();
                    sqlx::query(
                        "INSERT INTO thin_external_observations(session_id, completed_reason, observed_at)
                         VALUES (?, 'PR #186 已合并', 1)",
                    )
                    .bind(session)
                    .execute(&pool)
                    .await
                    .unwrap();
                    // The defect signature: the wake was cancelled, `fire_count=0`.
                    sqlx::query(
                        "INSERT INTO wakeup_schedules(id, session_id, prompt, source, next_at, status, created_at)
                         VALUES (?, ?, 'repair', 'agent_decided', 1, 'cancelled', 1)",
                    )
                    .bind(format!("thin-source-repair-{session}"))
                    .bind(session)
                    .execute(&pool)
                    .await
                    .unwrap();
                    sqlx::query("INSERT INTO objectives(id, status) VALUES (?, 'completed')")
                        .bind(objective)
                        .execute(&pool)
                        .await
                        .unwrap();
                    sqlx::query(
                        "INSERT INTO execution_workspaces
                         (id, objective_id, session_id, repo_identity, repo_root, git_common_dir,
                          worktree_path, branch_name, base_ref, base_sha, state, created_at, updated_at)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'HEAD', 'deadbeef', 'active', 1, 1)",
                    )
                    .bind(format!("ws-{session}"))
                    .bind(objective)
                    .bind(session)
                    .bind(repo.to_string_lossy().to_string())
                    .bind(repo.to_string_lossy().to_string())
                    .bind(repo.join(".git").to_string_lossy().to_string())
                    .bind(path.to_string_lossy().to_string())
                    .bind(branch)
                    .execute(&pool)
                    .await
                    .unwrap();
                }
                pool.close().await;
            });
            assert!(marker.exists());
        }

        /// R4: two states built by another process, then a fresh start settles both
        /// within the first tick (no waiting for a 30-minute heartbeat).
        #[tokio::test]
        async fn r1_r2_merged_sessions_settle_on_restart() {
            let dir = scratch_dir("restart");
            let child = Command::new(std::env::current_exe().unwrap())
                .env("M186_FIXTURE_DIR", &dir)
                .args(["--test-threads", "1", "fixture_writer"])
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "fixture child failed: {}",
                String::from_utf8_lossy(&child.stderr)
            );
            assert!(dir.join("app.db").exists(), "child wrote no fixture db");

            // Parent process: only durable state from the previous process exists.
            let pool = open_db(&dir).await;
            let summary = settle_merged_sessions(&pool).await.unwrap();
            assert_eq!(summary.settled, 2, "{summary:?}");
            assert_eq!(summary.asked_user, 0, "{summary:?}");

            // R1: workspace already gone → ended, no model round.
            let a = thin_state::load(&pool, "sess-a").await.unwrap().unwrap();
            assert_eq!(a.state, crate::agent::thin_state::SessionState::Ended);
            assert!(a.reason.unwrap().contains("已合并"));

            // R2: workspace still there → system closeout, then ended.
            assert!(!dir.join("ws-alive").exists(), "worktree not removed");
            assert!(
                git(
                    &dir.join("repo"),
                    &["rev-parse", "--verify", "codefactory/objective-m186b"]
                )
                .is_err(),
                "local branch not deleted"
            );
            let b = thin_state::load(&pool, "sess-b").await.unwrap().unwrap();
            assert_eq!(b.state, crate::agent::thin_state::SessionState::Ended);
            assert!(b.reason.unwrap().contains("已删除本地工作区"));

            // R3: no orphaned wait, no leftover pending wakeup, rows closed.
            assert!(waiting_wake_without_wake_source(&pool).await.is_empty());
            let pending: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='pending'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(pending, 0);
            let open: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM execution_workspaces WHERE state<>'closed'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(open, 0);
        }

        #[test]
        fn m187_unpushed_commit_prevents_workspace_deletion() {
            let dir = scratch_dir("m187-unpushed");
            let (repo, _, alive, branch) = build_fixture(&dir);
            std::fs::write(alive.join("local-only.txt"), "synthetic unpublished work").unwrap();
            git_ok(&alive, &["add", "local-only.txt"]);
            git_ok(&alive, &["commit", "-qm", "synthetic local-only commit"]);
            let result = closeout_workspace(&TerminalWorkspace {
                worktree_path: alive.clone(), repo_root: repo, branch_name: branch,
            });
            assert!(result.is_err(), "M187: unpublished commit must block closeout: {result:?}");
            assert!(alive.exists(), "M187: unpublished worktree was removed");
        }

        /// R2: a dirty workspace is not silently deleted and not silently waited on.
        #[tokio::test]
        async fn r2_dirty_workspace_asks_user_with_a_reason() {
            let dir = scratch_dir("dirty");
            let (repo, _branch_a, alive, branch_b) = build_fixture(&dir);
            std::fs::write(alive.join("scratch.txt"), "uncommitted").unwrap();
            let pool = seed_state(&dir).await;
            thin_state::save(
                &pool,
                "sess-b",
                &thin_state::ThinState {
                    state: crate::agent::thin_state::SessionState::WaitingWake,
                    waiting_for: Some("PR #186 合并".into()),
                    ..thin_state::ThinState::running()
                },
            )
            .await
            .unwrap();
            sqlx::query("INSERT INTO thin_external_observations(session_id, completed_reason, observed_at) VALUES ('sess-b','PR #186 已合并',1)")
                .execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO wakeup_schedules(id, session_id, prompt, source, next_at, status, created_at) VALUES ('w-sess-b','sess-b','p','agent_decided',1,'pending',1)")
                .execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO objectives(id, status) VALUES ('obj-b','completed')")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO execution_workspaces
                 (id, objective_id, session_id, repo_identity, repo_root, git_common_dir,
                  worktree_path, branch_name, base_ref, base_sha, state, created_at, updated_at)
                 VALUES ('ws-b','obj-b','sess-b',?,?,?,?,?,'HEAD','deadbeef','active',1,1)",
            )
            .bind(repo.to_string_lossy().to_string())
            .bind(repo.to_string_lossy().to_string())
            .bind(repo.join(".git").to_string_lossy().to_string())
            .bind(alive.to_string_lossy().to_string())
            .bind(&branch_b)
            .execute(&pool)
            .await
            .unwrap();

            let summary = settle_merged_sessions(&pool).await.unwrap();
            assert_eq!(summary.asked_user, 1, "{summary:?}");
            assert!(alive.exists(), "dirty worktree must be preserved");
            let state = thin_state::load(&pool, "sess-b").await.unwrap().unwrap();
            assert_eq!(state.state, crate::agent::thin_state::SessionState::WaitingUser);
            assert!(state.question.unwrap().contains("未提交"));
            // R3: the pending wake was cancelled in the same write — and the row is
            // no longer `waiting_wake`, so it is not an orphan either.
            let pending: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='pending'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(pending, 0);
            assert!(waiting_wake_without_wake_source(&pool).await.is_empty());
        }

        /// R3: state write and wake cancellation are one transaction.
        #[tokio::test]
        async fn r3_settlement_cancels_wakes_atomically() {
            let dir = scratch_dir("atomic");
            let pool = seed_state(&dir).await;
            thin_state::save(
                &pool,
                "sess-c",
                &thin_state::ThinState {
                    state: crate::agent::thin_state::SessionState::WaitingWake,
                    waiting_for: Some("PR #1".into()),
                    ..thin_state::ThinState::running()
                },
            )
            .await
            .unwrap();
            sqlx::query("INSERT INTO wakeup_schedules(id, session_id, prompt, source, next_at, status, created_at) VALUES ('w-c','sess-c','p','agent_decided',1,'pending',1)")
                .execute(&pool).await.unwrap();
            assert_eq!(
                waiting_wake_without_wake_source(&pool).await,
                Vec::<String>::new()
            );

            settle_ended_and_cancel(&pool, "sess-c", "PR 已合并，任务结束", "note")
                .await
                .unwrap();
            let state = thin_state::load(&pool, "sess-c").await.unwrap().unwrap();
            assert_eq!(state.state, crate::agent::thin_state::SessionState::Ended);
            let pending: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM wakeup_schedules WHERE session_id='sess-c' AND status='pending'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(pending, 0, "settlement must cancel its wake source");
            assert!(waiting_wake_without_wake_source(&pool).await.is_empty());
        }

        /// The probe really detects an orphaned wait (otherwise R4 proves nothing).
        #[tokio::test]
        async fn invariant_probe_reports_waiting_wake_without_source() {
            let dir = scratch_dir("probe");
            let pool = seed_state(&dir).await;
            thin_state::save(
                &pool,
                "sess-d",
                &thin_state::ThinState {
                    state: crate::agent::thin_state::SessionState::WaitingWake,
                    waiting_for: Some("PR #1".into()),
                    ..thin_state::ThinState::running()
                },
            )
            .await
            .unwrap();
            assert_eq!(
                waiting_wake_without_wake_source(&pool).await,
                vec!["sess-d".to_string()]
            );
        }
    }
}
pub(crate) use merged_settlement::{
    ask_user_and_cancel, safety_tick, settle_ended_and_cancel, settle_merged_sessions,
    waiting_wake_without_wake_source,
};
