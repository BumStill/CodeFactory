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
        "INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,closeout_queued) VALUES (?,?,?,0,0) ON CONFLICT(session_id) DO UPDATE SET pr_number=excluded.pr_number,ceiling=excluded.ceiling,observed_at=0,completed_reason=NULL,closeout_queued=0 WHERE thin_external_observations.pr_number<>excluded.pr_number",
    )
    .bind(session)
    .bind(number)
    .bind(&ceiling)
    .execute(db)
    .await?;
    Ok(Some(number))
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
    reason.contains("检查失败：") || reason.contains("已关闭但未合并") || reason.contains("存在合并冲突") || reason.contains("被合并队列退回")
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
        "SELECT COUNT(*) FROM thin_external_observations WHERE session_id=? AND (completed_reason LIKE '%检查失败：%' OR completed_reason LIKE '%已关闭但未合并%')",
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
async fn closeout_eligible(db: &SqlitePool, session: &str, merged_at: Option<i64>) -> sqlx::Result<bool> {
    let state = derive_state(db, session).await?;
    if !matches!(state.state, SessionState::WaitingWake | SessionState::WaitingUser) {
        return Ok(false);
    }
    if let Some(merged_at) = merged_at {
        let answered: i64 = if has_table(db, "messages").await? {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM messages WHERE session_id=? AND role='assistant' AND created_at>=? AND TRIM(content)<>'' AND (tool_calls IS NULL OR tool_calls='' OR tool_calls='[]')",
            ).bind(session).bind(merged_at).fetch_one(db).await?
        } else { 0 };
        if answered > 0 { return Ok(false); }
    }
    Ok(true)
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
        if terminal.is_some() {
            return Ok(ThinState::running());
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
                .filter(|what| super::thin::is_system_followed(what))
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
