// SPDX-License-Identifier: Apache-2.0
//! CF-STOP — 「停止」真的停下：会话级交付停止栅栏 + PR 侧的如实收尾。
//!
//! ## 为什么需要它（M47, 2026-10-09）
//!
//! 用户按了菜单里的「停止当前执行」，会话的 objective 也确实变成 cancelled，
//! 但十几分钟后那次交付在 CI 变绿时照样自动合并了 #584。原因是"停止"只落在了
//! 会话/objective 这一层，而交付是一条**自己的长时间线**：它当时正卡在等 CI，
//! objective 的终态没人在它下一个动作前读一次。
//!
//! 所以停止必须落成一个**交付能读到的事实**，而不是一句只在某条路径上生效的
//! 状态转换。这个模块给出三样东西：
//!
//! 1. `delivery_stop_fences`：按 `session_id` 定位的持久栅栏。停止时写入，
//!    只有显式的"继续交付"才清除；后台恢复回路只读不写。
//! 2. `DeliveryStopGate`：交付阶梯在每个"还没发生"的动作前读一次栅栏的抽象；
//!    读得到就停在那一步，不会再有推送 / 开 PR / 合并 / 发版。
//! 3. `stop_pending_pr_actions`：对已经开出来的 PR 做幂等收尾——
//!    清掉仓库合并队列的授权标签，关掉 GitHub 的 auto-merge。真正的漏网场景
//!    （app 在等 CI 时被杀、进程重启）靠这一步兜住，而且它只关"尚未发生的合并"，
//!    不回滚任何已经发生的事。

use std::fmt::Debug;
use std::sync::Arc;

use sqlx::SqlitePool;

/// Table that carries a session's stop fence.
pub const STOP_FENCE_TABLE: &str = "delivery_stop_fences";

/// One durable stop fence. Absent row = delivery for this session is not stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopFence {
    pub session_id: String,
    /// Why the session was stopped (user stop, orchestrator stop, ...).
    pub reason: String,
    /// The PR the delivery had already opened when the stop arrived. `None`
    /// means nothing had reached the remote yet — nothing to disarm, and
    /// nothing that may still happen either.
    pub pr_number: Option<i64>,
    pub stopped_at_ms: i64,
}

pub async fn ensure_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {STOP_FENCE_TABLE} (
            session_id    TEXT PRIMARY KEY,
            reason        TEXT NOT NULL,
            pr_number     INTEGER,
            stopped_at_ms INTEGER NOT NULL
        )"
    ))
    .execute(pool)
    .await?;
    Ok(())
}

/// Record (or refresh) the stop fence for a session. Idempotent: stopping twice
/// keeps the latest reason/PR and never clears an earlier fence.
pub async fn record_stop(
    pool: &SqlitePool,
    session_id: &str,
    reason: &str,
    pr_number: Option<i64>,
) -> Result<StopFence, sqlx::Error> {
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(&format!(
        "INSERT INTO {STOP_FENCE_TABLE} (session_id, reason, pr_number, stopped_at_ms)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(session_id) DO UPDATE SET
           reason=excluded.reason,
           pr_number=COALESCE(excluded.pr_number, {STOP_FENCE_TABLE}.pr_number),
           stopped_at_ms=excluded.stopped_at_ms"
    ))
    .bind(session_id)
    .bind(reason)
    .bind(pr_number)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(StopFence {
        session_id: session_id.to_string(),
        reason: reason.to_string(),
        pr_number,
        stopped_at_ms: now,
    })
}

/// Read a session's stop fence. `None` means delivery may proceed.
pub async fn stop_fence(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<StopFence>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String, Option<i64>, i64)>(&format!(
        "SELECT session_id, reason, pr_number, stopped_at_ms FROM {STOP_FENCE_TABLE} WHERE session_id=?"
    ))
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(session_id, reason, pr_number, stopped_at_ms)| StopFence {
        session_id,
        reason,
        pr_number,
        stopped_at_ms,
    }))
}

/// Clear a session's stop fence.
///
/// Deliberately **not** called by recovery: CF-STOP-R3 says continuing after a
/// stop is the user's or the orchestrator's explicit act, so the only callers
/// are a user "continue delivery" intent and an orchestrator resume request.
pub async fn resume_delivery(pool: &SqlitePool, session_id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(&format!(
        "DELETE FROM {STOP_FENCE_TABLE} WHERE session_id=?"
    ))
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// The delivery ladder's view of a session's stop state.
///
/// Checked immediately before every not-yet-happened action, so the answer is
/// allowed to change mid-run (that is the whole point of M47: the stop arrives
/// while the ladder is parked waiting for CI).
#[async_trait::async_trait]
pub trait DeliveryStopGate: Debug + Send + Sync {
    /// `Some(reason)` = this delivery must not take another not-yet-happened
    /// action. `None` = no stop is recorded for this session.
    async fn stop_reason(&self) -> Option<String>;
}

/// Database-backed gate used by the real delivery tool.
#[derive(Debug)]
pub struct SessionStopGate {
    pool: SqlitePool,
    session_id: String,
}

impl SessionStopGate {
    pub fn new(pool: SqlitePool, session_id: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            session_id: session_id.into(),
        })
    }
}

#[async_trait::async_trait]
impl DeliveryStopGate for SessionStopGate {
    async fn stop_reason(&self) -> Option<String> {
        match stop_fence(&self.pool, &self.session_id).await {
            Ok(Some(fence)) => Some(fence.reason),
            // A fence read failure must not silently unfence a stopped session:
            // an unreadable stop state is treated as "stop", because taking a
            // not-yet-happened delivery action is the irreversible direction.
            Err(error) => Some(format!("停止栅栏读取失败，按已停止处理: {error}")),
            Ok(None) => None,
        }
    }
}

/// In-memory gate for callers without a database (tests, non-durable runs).
#[derive(Debug)]
pub struct FlagStopGate {
    stopped: std::sync::atomic::AtomicBool,
    reason: String,
}

impl FlagStopGate {
    pub fn new(reason: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            stopped: std::sync::atomic::AtomicBool::new(false),
            reason: reason.into(),
        })
    }

    pub fn stop(&self) {
        self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl DeliveryStopGate for FlagStopGate {
    async fn stop_reason(&self) -> Option<String> {
        self.stopped
            .load(std::sync::atomic::Ordering::SeqCst)
            .then(|| self.reason.clone())
    }
}

/// R1 的文案：停止之后会话里必须说清"停在哪、什么没发生、已经发生的怎么办"。
pub fn stopped_delivery_notice(pr_number: Option<u64>) -> String {
    match pr_number {
        Some(number) => format!(
            "已停止：PR #{number} 保持打开，没有合并。停止之后不再执行还没发生的交付动作\
（推送、开 PR、合并、发版）；已经发生的动作不回滚。要接着交付，需要你或编排方明确发起。"
        ),
        None => "已停止：远端还没有本次交付的 PR，停止之后不再执行还没发生的交付动作\
（推送、开 PR、合并、发版）；已经发生的本地提交不回滚。要接着交付，需要你或编排方明确发起。"
            .to_string(),
    }
}

/// What a stop is allowed to do to an already-open PR.
///
/// Both operations are idempotent and best-effort: a failure is recorded and
/// reported, never used as a reason to pretend the stop did not happen.
pub trait PrStopDisarm {
    fn clear_merge_queue_arm(&mut self, number: u64) -> Result<(), String>;
    fn disable_auto_merge(&mut self, number: u64) -> Result<(), String>;
}

/// `gh` args that close GitHub's own auto-merge registration on a PR.
pub fn gh_disable_auto_merge_args(number: u64) -> Vec<String> {
    vec![
        "pr".into(),
        "merge".into(),
        number.to_string(),
        "--disable-auto".into(),
    ]
}

/// The auditable result of the PR-side stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopDisarmReport {
    pub pr_number: Option<u64>,
    pub cleared_merge_queue_arm: bool,
    pub disabled_auto_merge: bool,
    /// Per-step truth, so a partial stop is visible instead of assumed.
    pub notes: Vec<String>,
    /// What the session must be told.
    pub notice: String,
}

impl StopDisarmReport {
    /// A stop that touched no remote object (no PR yet) is still a complete stop.
    pub fn is_complete(&self) -> bool {
        self.pr_number.is_none() || (self.cleared_merge_queue_arm && self.disabled_auto_merge)
    }
}

/// R2: 停止时一并关掉这个 PR 的自动合并（仓库合并队列授权 + GitHub auto-merge）。
///
/// Ordering matters: the merge-queue arm is cleared first because that is the
/// label the repository's own queue acts on. Even if the second call fails, the
/// queue can no longer pick the PR up, and the failure is reported.
pub fn stop_pending_pr_actions(
    remote: &mut dyn PrStopDisarm,
    pr_number: Option<u64>,
) -> StopDisarmReport {
    let Some(number) = pr_number else {
        return StopDisarmReport {
            pr_number: None,
            cleared_merge_queue_arm: false,
            disabled_auto_merge: false,
            notes: vec!["停止时远端还没有本次交付的 PR，没有需要关闭的自动合并".into()],
            notice: stopped_delivery_notice(None),
        };
    };
    let mut notes = Vec::new();
    let cleared_merge_queue_arm = match remote.clear_merge_queue_arm(number) {
        Ok(()) => {
            notes.push(format!("已清除 PR #{number} 的仓库合并队列授权标签"));
            true
        }
        Err(error) => {
            notes.push(format!("清除 PR #{number} 的合并队列授权标签失败: {error}"));
            false
        }
    };
    let disabled_auto_merge = match remote.disable_auto_merge(number) {
        Ok(()) => {
            notes.push(format!("已关闭 PR #{number} 的 GitHub auto-merge"));
            true
        }
        Err(error) => {
            notes.push(format!("关闭 PR #{number} 的 auto-merge 失败: {error}"));
            false
        }
    };
    StopDisarmReport {
        pr_number: Some(number),
        cleared_merge_queue_arm,
        disabled_auto_merge,
        notes,
        notice: stopped_delivery_notice(Some(number)),
    }
}

/// The PR this session most recently delivered, if any.
///
/// `session_delivery_refs` is the durable session → PR link, so a stop can name
/// (and disarm) exactly the object it is talking about instead of guessing.
pub async fn latest_session_pr(pool: &SqlitePool, session_id: &str) -> Option<u64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT pr_number FROM session_delivery_refs WHERE session_id=?",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|number| number as u64)
}

/// `gh`-backed [`PrStopDisarm`]. The stop path runs in the user's own machine
/// with the same `gh` the delivery used, so it needs no separate token.
#[derive(Debug)]
pub struct GhPrStopDisarm {
    pub cwd: std::path::PathBuf,
}

fn run_gh(cwd: &std::path::Path, args: &[String]) -> Result<(), String> {
    let output = std::process::Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("无法运行 gh: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // Nothing to close is a success for a stop: the requirement is that the
    // registration is gone, not that it once existed.
    if stderr.contains("auto-merge is not enabled") || stderr.contains("no auto-merge") {
        return Ok(());
    }
    Err(if stderr.is_empty() {
        format!("gh {} 退出码 {:?}", args.join(" "), output.status.code())
    } else {
        stderr
    })
}

impl PrStopDisarm for GhPrStopDisarm {
    fn clear_merge_queue_arm(&mut self, number: u64) -> Result<(), String> {
        let args = vec![
            "pr".to_string(),
            "edit".to_string(),
            number.to_string(),
            "--remove-label".to_string(),
            crate::agent::delivery::MERGE_QUEUE_ARM_LABEL.to_string(),
        ];
        run_gh(&self.cwd, &args)
    }

    fn disable_auto_merge(&mut self, number: u64) -> Result<(), String> {
        run_gh(&self.cwd, &gh_disable_auto_merge_args(number))
    }
}

/// The workspace the session's delivery was running in — the directory `gh`
/// must be invoked from so the stop touches the same repository.
pub async fn latest_session_workspace(
    pool: &SqlitePool,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    sqlx::query_scalar::<_, String>(
        "SELECT workspace_path FROM delivery_runs
         WHERE session_id=? AND workspace_path IS NOT NULL AND workspace_path <> ''
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(std::path::PathBuf::from)
}

/// The whole stop, in one call: fence the session durably, then close the
/// not-yet-happened merge on the PR it was delivering.
///
/// The fence is written **first** on purpose. Even if the remote close-out
/// fails or the app dies mid-way, the delivery ladder and every recovery path
/// already see the stop and refuse to take another action (CF-STOP-R1/R3).
pub async fn stop_session_delivery(
    pool: &SqlitePool,
    session_id: &str,
    reason: &str,
    workspace_hint: Option<&std::path::Path>,
) -> Result<StopDisarmReport, sqlx::Error> {
    let pr_number = latest_session_pr(pool, session_id).await;
    record_stop(pool, session_id, reason, pr_number.map(|number| number as i64)).await?;
    let workspace = match workspace_hint {
        Some(path) => path.to_path_buf(),
        None => latest_session_workspace(pool, session_id)
            .await
            .unwrap_or_else(|| std::path::PathBuf::from(".")),
    };
    let mut remote = GhPrStopDisarm { cwd: workspace };
    Ok(stop_pending_pr_actions(&mut remote, pr_number))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic PR-side client: records what a stop asked of the provider.
    #[derive(Debug, Default)]
    struct RecordingDisarm {
        calls: Vec<String>,
        fail_arm_clear: bool,
        fail_disable_auto: bool,
    }

    impl PrStopDisarm for RecordingDisarm {
        fn clear_merge_queue_arm(&mut self, number: u64) -> Result<(), String> {
            self.calls.push(format!("clear-arm:{number}"));
            if self.fail_arm_clear {
                return Err("no such label".into());
            }
            Ok(())
        }

        fn disable_auto_merge(&mut self, number: u64) -> Result<(), String> {
            self.calls.push(format!("disable-auto:{number}"));
            if self.fail_disable_auto {
                return Err("gh: not authenticated".into());
            }
            Ok(())
        }
    }

    /// CF-STOP-R2 最低证据：用合成的 GitHub 客户端断言关闭调用。
    #[test]
    fn r2_a_stop_closes_the_arms_on_the_sessions_pr() {
        let mut remote = RecordingDisarm::default();
        let report = stop_pending_pr_actions(&mut remote, Some(584));
        assert_eq!(
            remote.calls,
            vec!["clear-arm:584".to_string(), "disable-auto:584".to_string()],
            "停止必须同时清掉仓库合并队列授权和 GitHub auto-merge"
        );
        assert!(report.cleared_merge_queue_arm && report.disabled_auto_merge);
        assert!(report.is_complete());
        assert!(report.notice.contains("PR #584 保持打开，没有合并"));
    }

    /// 停止是幂等的：重复停止不会变成另一种动作，也不会漏掉任何一次关闭。
    #[test]
    fn r2_stopping_twice_issues_the_same_two_closures() {
        let mut remote = RecordingDisarm::default();
        let _ = stop_pending_pr_actions(&mut remote, Some(42));
        let second = stop_pending_pr_actions(&mut remote, Some(42));
        assert_eq!(
            remote.calls,
            vec![
                "clear-arm:42".to_string(),
                "disable-auto:42".to_string(),
                "clear-arm:42".to_string(),
                "disable-auto:42".to_string(),
            ]
        );
        assert!(second.is_complete());
    }

    /// 部分失败必须如实写出来，而不是把"停止"报成完整。
    #[test]
    fn r2_a_failed_closure_is_reported_not_hidden() {
        let mut remote = RecordingDisarm {
            fail_disable_auto: true,
            ..Default::default()
        };
        let report = stop_pending_pr_actions(&mut remote, Some(7));
        assert!(!report.is_complete());
        assert!(report.cleared_merge_queue_arm);
        assert!(!report.disabled_auto_merge);
        assert!(report
            .notes
            .iter()
            .any(|note| note.contains("auto-merge 失败")));
    }

    #[test]
    fn r2_a_stop_without_a_pr_still_says_what_did_not_happen() {
        let mut remote = RecordingDisarm::default();
        let report = stop_pending_pr_actions(&mut remote, None);
        assert!(remote.calls.is_empty());
        assert!(report.is_complete());
        assert!(report.notice.contains("不再执行还没发生的交付动作"));
    }

    #[test]
    fn r1_notice_names_the_pr_and_says_it_was_not_merged() {
        let notice = stopped_delivery_notice(Some(584));
        assert!(notice.contains("PR #584 保持打开，没有合并"), "{notice}");
        assert!(notice.contains("已经发生的动作不回滚"), "{notice}");
        assert!(notice.contains("明确发起"), "{notice}");
    }

    #[test]
    fn gh_disable_auto_merge_args_close_the_registration_on_the_named_pr() {
        assert_eq!(
            gh_disable_auto_merge_args(584),
            vec!["pr", "merge", "584", "--disable-auto"]
        );
    }

    /// R3 的持久层：栅栏只由显式继续清除。
    #[tokio::test]
    async fn r3_a_fence_survives_until_an_explicit_resume() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        ensure_schema(&pool).await.unwrap();
        assert!(stop_fence(&pool, "s1").await.unwrap().is_none());

        record_stop(&pool, "s1", "user_stop", Some(584)).await.unwrap();
        let fence = stop_fence(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(fence.reason, "user_stop");
        assert_eq!(fence.pr_number, Some(584));

        // 后台恢复回路只读栅栏：它不能靠"读到栅栏"就把交付放回去。
        let gate = SessionStopGate::new(pool.clone(), "s1");
        assert_eq!(gate.stop_reason().await.as_deref(), Some("user_stop"));

        assert!(resume_delivery(&pool, "s1").await.unwrap());
        assert!(stop_fence(&pool, "s1").await.unwrap().is_none());
        assert!(!resume_delivery(&pool, "s1").await.unwrap());
        assert_eq!(gate.stop_reason().await, None);
    }

    /// 重复停止不会丢掉先前记下的 PR：第二次并不知道 PR 号时也要留着。
    #[tokio::test]
    async fn r3_a_later_stop_keeps_the_pr_the_earlier_stop_recorded() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        ensure_schema(&pool).await.unwrap();
        record_stop(&pool, "s1", "user_stop", Some(584)).await.unwrap();
        record_stop(&pool, "s1", "orchestrator_stop", None).await.unwrap();
        let fence = stop_fence(&pool, "s1").await.unwrap().unwrap();
        assert_eq!(fence.reason, "orchestrator_stop");
        assert_eq!(fence.pr_number, Some(584));
    }

    /// 栅栏读不出来时按已停止处理：动作是不可逆的那一侧，不能靠"读失败"放行。
    #[tokio::test]
    async fn an_unreadable_fence_never_unstops_a_session() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // 表不存在 = 读失败。
        let gate = SessionStopGate::new(pool, "s1");
        assert!(gate.stop_reason().await.is_some());
    }
}
