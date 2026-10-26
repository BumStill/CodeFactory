// SPDX-License-Identifier: Apache-2.0
//! The shared write-transaction entry point for the local SQLite store, plus
//! the vocabulary for recognising lock contention.
//!
//! This module is deliberately free of `crate::` dependencies. The integration
//! tests reuse production modules directly through `#[path]`, and a helper they
//! cannot include would force those tests onto a weaker adapter than the one
//! that actually ships.

use sqlx::{Sqlite, SqlitePool, Transaction};
use std::time::Duration;

/// How long a single SQLite connection waits for a contended lock before the
/// driver returns `SQLITE_BUSY` (code 5).
///
/// Why 10s and not the driver default of 5s: the *normal* contenders here are
/// not other app writers but short external readers (a user's `sqlite3` CLI, a
/// monitoring script). Production evidence (2026-10-08) recorded real
/// `database is locked` code-5 failures against the 5s default, so a read
/// snapshot that overlaps a batch migration must not surface as an agent error.
/// It stays bounded — 10s is short enough that a genuine deadlock still reaches
/// the retry layer below instead of hanging a turn indefinitely.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

/// Number of times [`begin_write`] re-attempts the write lock after the busy
/// timeout expires, with exponential backoff between attempts.
pub const CONTENTION_ATTEMPTS: u32 = 3;

/// Raw SQLite busy codes that mean "another connection holds or invalidated the
/// lock" — never a defect in the statement itself.
const SQLITE_BUSY: i32 = 5;
const SQLITE_BUSY_SNAPSHOT: i32 = 517;

/// The distinguishable failure code for a local write that kept colliding.
///
/// It exists so a lock collision is never persisted as `agent_loop_error`: that
/// code enters the recovery ladder, burns recovery budget, and can mark the
/// first turn failed (which is what made auto-naming give up permanently).
pub const LOCAL_STORE_CONTENDED: &str = "local_store_contended";

/// True when a driver error is SQLite lock contention that survived the busy
/// timeout. Covers both code 5 (`SQLITE_BUSY`) and code 517
/// (`SQLITE_BUSY_SNAPSHOT`), plus the text form for errors that arrive already
/// wrapped into a plain message by a store layer.
pub fn is_lock_contention(error: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db) = error {
        // sqlx surfaces the SQLite extended result code as the driver code.
        if let Some(code) = db.code().and_then(|code| code.parse::<i32>().ok()) {
            if code == SQLITE_BUSY || code == SQLITE_BUSY_SNAPSHOT {
                return true;
            }
        }
    }
    is_lock_contention_message(&error.to_string())
}

/// The text half of [`is_lock_contention`]. A failure classification point that
/// only has the rendered message (the chat runner) uses this, so a wrapped
/// `provider output checkpoint failed: … database is locked` is still
/// recognised as local contention rather than an agent failure.
pub fn is_lock_contention_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if lower.contains("database is locked")
        || lower.contains("database table is locked")
        || lower.contains("sqlite_busy")
    {
        return true;
    }
    // Some layers render the SQLite result-code name without separators
    // (`DatabaseTableIsLocked`, `SQLITEBUSY`). Normalise to alphanumerics so a
    // wrapped message still classifies, while still requiring the whole phrase
    // so unrelated text cannot match.
    let compact: String = lower.chars().filter(char::is_ascii_alphanumeric).collect();
    compact.contains("databaseislocked")
        || compact.contains("databasetableislocked")
        || compact.contains("sqlitebusy")
}

/// Take SQLite's write lock *before* the transaction reads anything.
///
/// In WAL mode a deferred transaction that reads first and writes later holds a
/// read snapshot; if any other connection commits in between, the write upgrade
/// fails immediately with `SQLITE_BUSY_SNAPSHOT` (code 517) — and `busy_timeout`
/// deliberately does not apply to that case. `BEGIN IMMEDIATE` takes the write
/// lock up front, so concurrent writers queue in SQLite's own FIFO and wait
/// under the normal busy timeout instead of colliding.
///
/// Contention that survives the busy timeout is retried here, in place, with
/// bounded backoff before it is ever surfaced. Nothing has been written when
/// the `BEGIN` itself fails, so retrying the acquisition retries the whole
/// operation. Only after [`CONTENTION_ATTEMPTS`] is the error returned — at
/// which point it is classified as [`LOCAL_STORE_CONTENDED`], which is not an
/// agent failure.
pub async fn begin_write(pool: &SqlitePool) -> sqlx::Result<Transaction<'static, Sqlite>> {
    /// Log a warning when acquiring the write lock took long enough to matter.
    /// Both numbers are measured from the platform clock, not guessed.
    const SLOW_ACQUIRE_WARN: Duration = Duration::from_millis(250);
    const FIRST_BACKOFF: Duration = Duration::from_millis(50);
    const MAX_BACKOFF: Duration = Duration::from_millis(400);

    let mut backoff = FIRST_BACKOFF;
    let mut attempt = 0_u32;
    loop {
        let started = std::time::Instant::now();
        match pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => {
                let waited = started.elapsed();
                if waited >= SLOW_ACQUIRE_WARN {
                    tracing::warn!(
                        "write transaction waited {}ms for the SQLite write lock",
                        waited.as_millis()
                    );
                }
                return Ok(tx);
            }
            Err(error) if is_lock_contention(&error) && attempt + 1 < CONTENTION_ATTEMPTS => {
                attempt += 1;
                tracing::warn!(
                    "SQLite write lock contended (attempt {attempt}/{CONTENTION_ATTEMPTS}, \
                     retrying in {backoff:?}): {error}"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            Err(error) => {
                // Self-recovery: a leaked transaction on an idle pooled
                // connection keeps code 5 forever. Release it and give the
                // caller one final chance instead of requiring a restart.
                let recovered = rollback_idle_transactions(pool).await;
                if recovered > 0 {
                    tracing::warn!(
                        recovered,
                        "released leaked SQLite write transactions; retrying the write once"
                    );
                    return pool.begin_with("BEGIN IMMEDIATE").await;
                }
                return Err(error);
            }
        }
    }
}

/// Roll back any transaction still open on an *idle* pooled connection.
///
/// [`begin_write`] calls this only after bounded contention retries are
/// exhausted. A connection that was returned with a bare, untracked
/// `BEGIN IMMEDIATE` still owns the database write lock; releasing it here is
/// what turns "restart the app" into "retry the same send". Connections that
/// are currently in use are left alone, so an ordinary long write is never
/// interrupted by another caller's recovery attempt.
pub async fn rollback_idle_transactions(pool: &SqlitePool) -> usize {
    extern "C" {
        fn sqlite3_get_autocommit(db: *mut std::ffi::c_void) -> i32;
    }
    let mut recovered = 0;
    for _ in 0..8 {
        let Ok(Ok(mut connection)) =
            tokio::time::timeout(Duration::from_millis(100), pool.acquire()).await
        else {
            break;
        };
        let open = {
            let Ok(mut handle) = connection.lock_handle().await else {
                continue;
            };
            let raw = handle.as_raw_handle().as_ptr().cast();
            // SAFETY: the pool connection is exclusively ours and the driver
            // thread is paused for the lifetime of the locked handle.
            unsafe { sqlite3_get_autocommit(raw) == 0 }
        };
        if open {
            tracing::warn!("rolling back a transaction leaked on an idle pooled connection");
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            recovered += 1;
        }
        drop(connection);
    }
    recovered
}

/// Re-run a whole write operation when it fails on lock contention.
///
/// [`begin_write`] already retries the lock acquisition, which is where
/// contention actually lands once writers take the lock up front. This wrapper
/// exists for the remaining single-statement writes: the caller's closure must
/// be free of side effects outside the database so replaying it is safe.
pub async fn retry_write<T, F, Fut>(mut operation: F) -> sqlx::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = sqlx::Result<T>>,
{
    const FIRST_BACKOFF: Duration = Duration::from_millis(50);
    const MAX_BACKOFF: Duration = Duration::from_millis(400);

    let mut backoff = FIRST_BACKOFF;
    let mut attempt = 0_u32;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if is_lock_contention(&error) && attempt + 1 < CONTENTION_ATTEMPTS => {
                attempt += 1;
                tracing::warn!(
                    "SQLite write contended (attempt {attempt}/{CONTENTION_ATTEMPTS}, \
                     retrying in {backoff:?}): {error}"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            Err(error) => return Err(error),
        }
    }
}

/// The backoff for retry `attempt` (1-based), shared by the retry loops so the
/// ladder is identical wherever the write happens.
pub fn contention_backoff(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(3);
    (Duration::from_millis(50) * (1 << shift)).min(Duration::from_millis(400))
}
