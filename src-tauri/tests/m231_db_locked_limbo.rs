// SPDX-License-Identifier: Apache-2.0
//! M231: production lock helpers against real file SQLite and independent writers.
#[path = "../src/storage/write_lock.rs"]
mod write_lock;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::time::Duration;

async fn pool(path: &std::path::Path) -> sqlx::SqlitePool {
    SqlitePoolOptions::new().max_connections(3).connect_with(
        SqliteConnectOptions::new().filename(path).create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal).busy_timeout(Duration::from_millis(20))
    ).await.unwrap()
}

#[test]
fn m231_locked_extended_codes_and_names_are_contention() {
    for message in ["SQLITE_LOCKED", "SQLITE_LOCKED_SHAREDCACHE", "Database error (code: 6)", "Database error (code: 262)"] {
        assert!(write_lock::is_lock_contention_message(message), "missed {message}");
    }
    assert!(!write_lock::is_lock_contention_message("syntax error near locked_column"));
}

#[tokio::test]
async fn m231_result_retry_does_not_reexecute_side_effect() {
    let dir = tempfile::tempdir().unwrap();
    let db = pool(&dir.path().join("result.db")).await;
    sqlx::query("CREATE TABLE receipts(value INTEGER)").execute(&db).await.unwrap();
    let mut holder = db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query("INSERT INTO receipts VALUES (0)").execute(&mut *holder).await.unwrap();
    let side_effects = std::sync::atomic::AtomicUsize::new(0);
    side_effects.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let writer = db.clone();
    let result = tokio::spawn(async move {
        write_lock::retry_write(|| async {
            sqlx::query("INSERT INTO receipts VALUES (1)").execute(&writer).await
        }).await
    });
    tokio::time::sleep(Duration::from_millis(65)).await;
    holder.commit().await.unwrap();
    result.await.unwrap().unwrap();
    assert_eq!(side_effects.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT SUM(value) FROM receipts").fetch_one(&db).await.unwrap(), 1);
    db.close().await;
}

#[tokio::test]
async fn m231_immediate_writer_avoids_deferred_snapshot_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let db = pool(&dir.path().join("snapshot.db")).await;
    sqlx::query("CREATE TABLE revisions(value INTEGER)").execute(&db).await.unwrap();
    let mut old = db.begin().await.unwrap();
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM revisions").fetch_one(&mut *old).await.unwrap();
    sqlx::query("INSERT INTO revisions VALUES (1)").execute(&db).await.unwrap();
    let error = sqlx::query("INSERT INTO revisions VALUES (2)").execute(&mut *old).await.unwrap_err();
    assert!(write_lock::is_lock_contention(&error));
    old.rollback().await.unwrap();
    let mut tx = write_lock::begin_write(&db).await.unwrap();
    let previous = sqlx::query_scalar::<_, i64>("SELECT MAX(value) FROM revisions").fetch_one(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO revisions VALUES (?)").bind(previous + 1).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT MAX(value) FROM revisions").fetch_one(&db).await.unwrap(), 2);
    db.close().await;
}

#[tokio::test]
async fn m231_persistent_lock_exhausts_bounded_database_only_retries() {
    let dir = tempfile::tempdir().unwrap();
    let db = pool(&dir.path().join("exhausted.db")).await;
    sqlx::query("CREATE TABLE receipts(value INTEGER)").execute(&db).await.unwrap();
    let holder = db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let start = std::time::Instant::now();
    let error = write_lock::retry_write(|| async {
        sqlx::query("INSERT INTO receipts VALUES (1)").execute(&db).await
    }).await.unwrap_err();
    assert!(write_lock::is_lock_contention(&error));
    assert!(start.elapsed() < Duration::from_secs(2));
    holder.rollback().await.unwrap();
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM receipts").fetch_one(&db).await.unwrap(), 0);
    db.close().await;
}
