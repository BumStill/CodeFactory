// SPDX-License-Identifier: Apache-2.0
//! M210: 模型可用性等待（所有候选模型都不可用）是一等持久事实。
//!
//! 用户看到的真相是「没有可用模型，HH:MM 恢复后自动继续」，不是「系统自动重试
//! 两次仍未推进」。所以这条等待必须有自己的一张表：它有自己的恢复时间点，不计入
//! `thin-source-repair` 的自愈次数，也不能被升级成对用户的提问。

use sqlx::SqlitePool;

/// M210/R2：没有任何「时间会修好」的原因（全是致命错误）时的重试窗口。
pub(crate) const FATAL_RETRY_MS: i64 = 5 * 60 * 1000;

/// M123/M210：没有可用模型时的下一次自动尝试间隔（有恢复时间时用恢复时间）。
pub(crate) const BLIND_RETRY_MS: i64 = 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelWait {
    /// 会话里显示的那句人话（含供应商与最早可用时间）。
    pub reason: String,
    /// `None` = 没有可预期恢复时间（全为致命错误）。
    pub resume_at_ms: Option<i64>,
    pub attempts: i64,
}

pub(crate) async fn ensure(db: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS model_availability_waits (
            session_id   TEXT PRIMARY KEY,
            reason       TEXT NOT NULL,
            resume_at_ms INTEGER,
            attempts     INTEGER NOT NULL DEFAULT 0,
            updated_at   INTEGER NOT NULL
        )",
    )
    .execute(db)
    .await?;
    Ok(())
}

pub(crate) async fn load(db: &SqlitePool, session: &str) -> sqlx::Result<Option<ModelWait>> {
    ensure(db).await?;
    let row: Option<(String, Option<i64>, i64)> = sqlx::query_as(
        "SELECT reason,resume_at_ms,attempts FROM model_availability_waits WHERE session_id=?",
    )
    .bind(session)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(reason, resume_at_ms, attempts)| ModelWait {
        reason,
        resume_at_ms,
        attempts,
    }))
}

/// 记录/续期一次模型等待。重复结算同一等待只累加 `attempts`，不产生第二行。
pub(crate) async fn record(
    db: &SqlitePool,
    session: &str,
    reason: &str,
    resume_at_ms: Option<i64>,
) -> sqlx::Result<ModelWait> {
    ensure(db).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let reason = if reason.trim().is_empty() { "当前没有可用模型" } else { reason.trim() };
    sqlx::query(
        "INSERT INTO model_availability_waits(session_id,reason,resume_at_ms,attempts,updated_at)
         VALUES (?,?,?,1,?)
         ON CONFLICT(session_id) DO UPDATE SET reason=excluded.reason,
           resume_at_ms=excluded.resume_at_ms, attempts=model_availability_waits.attempts+1,
           updated_at=excluded.updated_at",
    )
    .bind(session)
    .bind(reason)
    .bind(resume_at_ms)
    .bind(now)
    .execute(db)
    .await?;
    load(db, session).await?.ok_or(sqlx::Error::RowNotFound)
}

/// 真正的模型回合开始了（拿到了回答）——等待结束。
///
/// 同时退休还挂着的模型唤醒：模型已经可用，再叫醒它一次就是把会话从已完成的
/// 回合里拽回等待状态。
pub(crate) async fn clear(db: &SqlitePool, session: &str) -> sqlx::Result<()> {
    ensure(db).await?;
    sqlx::query("DELETE FROM model_availability_waits WHERE session_id=?")
        .bind(session)
        .execute(db)
        .await?;
    // 唤醒表可能尚未建（只读/全新库），失败不影响等待事实的清除。
    let _ = sqlx::query(
        "UPDATE wakeup_schedules SET status='cancelled'
          WHERE session_id=? AND status='pending' AND id LIKE 'model-availability-%'",
    )
    .bind(session)
    .execute(db)
    .await;
    Ok(())
}

/// 下一次该在什么时候重新检查模型可用性。
///
/// 有恢复时间且还没到点 → 就是那个时间点；已经过点（刷新过但模型仍不可用）→
/// 立刻再试一次；完全没有恢复时间 → `FATAL_RETRY_MS` 窗口后继续。
pub(crate) fn next_at(wait: &ModelWait, now_ms: i64) -> i64 {
    match wait.resume_at_ms {
        Some(at) if at > now_ms + 1_000 => at,
        Some(_) => now_ms + BLIND_RETRY_MS,
        None => now_ms + FATAL_RETRY_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> SqlitePool {
        crate::storage::db::connect("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn record_is_idempotent_and_counts_attempts() {
        let db = db().await;
        let first = record(&db, "s1", "当前没有可用模型：GPT 已达订阅用量上限，09:02 重置", Some(1_000)).await.unwrap();
        assert_eq!(first.attempts, 1);
        let second = record(&db, "s1", "当前没有可用模型：GPT 已达订阅用量上限，09:02 重置", Some(2_000)).await.unwrap();
        assert_eq!(second.attempts, 2);
        assert_eq!(second.resume_at_ms, Some(2_000));
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM model_availability_waits")
            .fetch_one(&db).await.unwrap();
        assert_eq!(rows, 1);
        clear(&db, "s1").await.unwrap();
        assert!(load(&db, "s1").await.unwrap().is_none());
        db.close().await;
    }

    #[test]
    fn next_at_follows_recovery_time_then_falls_back() {
        let far = ModelWait { reason: "x".into(), resume_at_ms: Some(10_000), attempts: 1 };
        assert_eq!(next_at(&far, 1_000), 10_000);
        // 恢复时间已过但模型仍不可用：立刻再试，不无限等到旧时间点。
        assert_eq!(next_at(&far, 20_000), 20_000 + BLIND_RETRY_MS);
        let fatal = ModelWait { reason: "x".into(), resume_at_ms: None, attempts: 1 };
        assert_eq!(next_at(&fatal, 1_000), 1_000 + FATAL_RETRY_MS);
    }
}
