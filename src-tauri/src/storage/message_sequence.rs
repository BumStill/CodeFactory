// SPDX-License-Identifier: Apache-2.0
//! MEM-A 阶段 A：给「会话里第几条消息」一个持久身份。
//!
//! `messages.created_at` 是秒级的，而 `messages` 的主键是 `id TEXT`（不是
//! `INTEGER PRIMARY KEY`），所以同一秒内的先后完全靠隐式 `rowid` 决定，而
//! `rowid` 在 `VACUUM` 之后不保证保持。也就是说「第 7 条消息」在今天不是一个
//! 稳定事实——这正是后续分级恢复（摘要/投影）无法自证的前提。
//!
//! 这里把排序身份物化成两列：
//! - `seq`：会话内单调递增的序号，迁移时按 `(created_at, rowid)` 回填，之后由
//!   `AFTER INSERT` 触发器在写事务内赋值，因此并发写不会撞号或乱序。
//! - `revision`：内容版本，内容相关列变化时 +1，用于回执里「基于第几版」的断言。
//!
//! 本模块只做这一件事的测试与少量读取辅助，排序语义仍归
//! [`super::load_agent_history`] 所有。

#![cfg(test)]

use super::*;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

/// 建一个文件库并按 `version` 截断迁移，用来复现「旧库」。
async fn legacy_pool(dir: &std::path::Path, version: i64) -> SqlitePool {
    let url = format!("sqlite:{}", dir.join("legacy.db").display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            url.parse::<SqliteConnectOptions>()
                .unwrap()
                .create_if_missing(true)
                .foreign_keys(false),
        )
        .await
        .unwrap();
    let all = sqlx::migrate!("./migrations");
    let prefix = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            all.iter()
                .filter(|m| m.version <= version)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    prefix.run(&pool).await.unwrap();
    pool
}

/// 用生产入口打开库：迁移 + `ensure_schema`（后者补齐 `completion_state`
/// 等新增列，`load_agent_history` 与 revision 触发器都依赖真实 schema）。
async fn prod_connect(path: &std::path::Path) -> SqlitePool {
    crate::storage::db::connect(path.to_str().unwrap())
        .await
        .unwrap()
}

async fn seed_session(pool: &SqlitePool, id: &str) {
    sqlx::query(
        "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at) VALUES (?,?,?,?,0,0)",
    )
    .bind(id)
    .bind("t")
    .bind("/tmp")
    .bind("m")
    .execute(pool)
    .await
    .unwrap();
}

async fn seqs(pool: &SqlitePool, session_id: &str) -> Vec<(String, Option<i64>)> {
    sqlx::query_as::<_, (String, Option<i64>)>(
        "SELECT id, seq FROM messages WHERE session_id = ? ORDER BY seq ASC, rowid ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn revision_of(pool: &SqlitePool, id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT revision FROM messages WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// R1 / R4：旧库（0024 及以前）里同一秒写入的多行，靠 `(created_at, rowid)` 决定
/// 先后。迁移必须把它们回填成 1..N，并且重连后仍是同一组数字。
#[tokio::test]
async fn legacy_backfill_assigns_stable_monotonic_seq() {
    let root = tempfile::tempdir().unwrap();
    let pool = legacy_pool(root.path(), 24).await;

    for (id, session, created_at) in [
        ("m1", "s1", 100),
        ("m2", "s1", 100),
        ("m3", "s1", 100),
        ("m4", "s1", 200),
        ("other", "s2", 100),
        ("anon", "anonymous", 100),
    ] {
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,?,?,?,?)",
        )
        .bind(id)
        .bind(session)
        .bind("user")
        .bind("body")
        .bind(created_at)
        .execute(&pool)
        .await
        .unwrap();
    }

    // 走生产升级路径：剩余迁移（含 0025_message_sequence）+ ensure_schema。
    pool.close().await;
    let pool = prod_connect(&root.path().join("legacy.db")).await;

    assert_eq!(
        seqs(&pool, "s1").await,
        vec![
            ("m1".to_string(), Some(1)),
            ("m2".to_string(), Some(2)),
            ("m3".to_string(), Some(3)),
            ("m4".to_string(), Some(4)),
        ],
        "seq 必须按 (created_at,rowid) 回填成与会话内实际先后一致的单调序列"
    );
    assert_eq!(
        seqs(&pool, "s2").await,
        vec![("other".to_string(), Some(1))],
        "seq 是会话内的序号，不是全局号"
    );
    assert_eq!(
        seqs(&pool, "anonymous").await,
        vec![("anon".to_string(), Some(1))],
        "匿名会话同样要有稳定序号"
    );
    for id in ["m1", "m4", "other", "anon"] {
        assert_eq!(revision_of(&pool, id).await, 1, "{id} 回填后应为首版");
    }

    // 重开同一个库：序号不能变。
    pool.close().await;
    let reopened = prod_connect(&root.path().join("legacy.db")).await;
    assert_eq!(
        seqs(&reopened, "s1").await,
        vec![
            ("m1".to_string(), Some(1)),
            ("m2".to_string(), Some(2)),
            ("m3".to_string(), Some(3)),
            ("m4".to_string(), Some(4)),
        ],
        "重开数据库后 seq 必须完全一致（这就是「稳定」的定义）"
    );
    reopened.close().await;
}

/// R1 / R5：回填是幂等的——迁移再跑一次不重排、不丢行。
#[tokio::test]
async fn backfill_is_idempotent_across_reopen() {
    let root = tempfile::tempdir().unwrap();
    let pool = legacy_pool(root.path(), 24).await;
    for id in ["a", "b", "c"] {
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,'s','user','x',0)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
    let pool = prod_connect(&root.path().join("legacy.db")).await;
    let first = seqs(&pool, "s").await;
    // 重开（等价于迁移再跑一次）：已有 seq 不得被重排。
    pool.close().await;
    let pool = prod_connect(&root.path().join("legacy.db")).await;
    assert_eq!(seqs(&pool, "s").await, first, "重复迁移不得改动已有 seq");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 3, "迁移不得删除任何消息");
    pool.close().await;
}

/// R1 / R4：并发写入不得撞号，也不得乱序——序号在写事务内分配。
#[tokio::test]
async fn concurrent_inserts_get_unique_ordered_seq() {
    let root = tempfile::tempdir().unwrap();
    let pool = prod_connect(&root.path().join("conc.db")).await;
    seed_session(&pool, "s").await;

    let mut handles = Vec::new();
    for writer in 0..4u32 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let mut mine = Vec::new();
            for index in 0..5u32 {
                let id = format!("w{writer}-{index}");
                // 刻意全部用同一个 created_at：先后只能由 seq 决定。
                sqlx::query(
                    "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,'s','user','x',1000)",
                )
                .bind(&id)
                .execute(&pool)
                .await
                .unwrap();
                let seq: i64 = sqlx::query_scalar("SELECT seq FROM messages WHERE id = ?")
                    .bind(&id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                mine.push(seq);
            }
            mine
        }));
    }

    let mut per_writer = Vec::new();
    for handle in handles {
        per_writer.push(handle.await.unwrap());
    }

    let (total, distinct, max): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(DISTINCT seq), MAX(seq) FROM messages WHERE session_id='s'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(total, 20);
    assert_eq!(distinct, 20, "并发写入不得产生重复 seq");
    assert_eq!(max, 20, "seq 必须在会话内连续递增且无空洞");
    for seqs_for_writer in &per_writer {
        assert!(
            seqs_for_writer.windows(2).all(|pair| pair[0] < pair[1]),
            "同一个写入者的插入顺序必须严格递增（写事务内分配）: {seqs_for_writer:?}"
        );
    }
    pool.close().await;
}

/// R1：`revision` 只随内容相关列变化递增，无关记账列不动它。
#[tokio::test]
async fn revision_increments_when_content_changes() {
    let root = tempfile::tempdir().unwrap();
    let pool = prod_connect(&root.path().join("rev.db")).await;
    seed_session(&pool, "s").await;
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at) VALUES ('m','s','user','v1',1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(revision_of(&pool, "m").await, 1);

    sqlx::query("UPDATE messages SET content='v2' WHERE id='m'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(revision_of(&pool, "m").await, 2, "内容变化必须升版本");

    sqlx::query("UPDATE messages SET input_tokens=42 WHERE id='m'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        revision_of(&pool, "m").await,
        2,
        "记账列（token 数）不是内容，不应升版本"
    );
    pool.close().await;
}

/// R1：`load_agent_history` 的先后由 `seq` 决定，`created_at` 不再是权威。
/// 导入会话常出现「后写入的行带更早的时间戳」，旧排序会把它们插到中间。
#[tokio::test]
async fn history_order_follows_seq_not_created_at() {
    let root = tempfile::tempdir().unwrap();
    let pool = prod_connect(&root.path().join("order.db")).await;
    seed_session(&pool, "s").await;
    for (id, created_at) in [("first", 500), ("second", 100), ("third", 300)] {
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,'s','user','x',?)",
        )
        .bind(id)
        .bind(created_at)
        .execute(&pool)
        .await
        .unwrap();
    }
    let history = load_agent_history(&pool, "s").await.unwrap();
    assert_eq!(
        history.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["first", "second", "third"],
        "排序身份是 seq（写入顺序），不是可被导入打乱的时间戳"
    );
    assert_eq!(
        history.iter().map(|m| m.seq).collect::<Vec<_>>(),
        vec![Some(1), Some(2), Some(3)],
        "历史行必须带上 seq 供回执引用"
    );
    pool.close().await;
}
