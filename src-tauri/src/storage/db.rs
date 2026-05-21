// SPDX-License-Identifier: Apache-2.0
use sqlx::{migrate::MigrateDatabase, sqlite::SqlitePoolOptions, Row, SqlitePool};

pub async fn connect(db_path: &str) -> crate::errors::Result<SqlitePool> {
    if !sqlx::Sqlite::database_exists(db_path)
        .await
        .unwrap_or(false)
    {
        sqlx::Sqlite::create_database(db_path).await?;
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(db_path)
        .await?;

    // sqlx file-based migrations first — they handle the cases where a
    // version slot is unclaimed in the user's `_sqlx_migrations` table.
    // We ignore the result on a checksum/version mismatch so that the
    // ensure_schema pass below can still run; the row-level damage from
    // a half-applied migration is what we're working around here.
    if let Err(e) = sqlx::migrate!("../migrations").run(&pool).await {
        tracing::warn!(
            "sqlx::migrate reported {e}; falling back to idempotent ensure_schema"
        );
    }

    // Idempotent schema sync — see ensure_schema doc-comment for the why.
    ensure_schema(&pool).await?;

    // Enable FK enforcement (SQLite disables it by default).
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    Ok(pool)
}

/// Make sure every column the application code expects actually exists on
/// the live DB, regardless of which historical mix of migrations the user
/// has had applied.
///
/// Background: early versions of CodeFactory shipped a series of small
/// migration files (`0002_tool_calls`, `0003_tasks`, `0004_task_verification`,
/// `0005_cost_entries`, …) that were later consolidated/dropped from the
/// repo. Users' DBs still carry their effects, including entries in
/// `_sqlx_migrations` claiming versions 2-5 are applied. When a new
/// migration file gets dropped in at version 2 with different content
/// (as happened with `0002_reasoning_content.sql`), sqlx sees the slot
/// already taken and either errors out or silently skips — leaving the
/// expected column missing. The visible symptom was sessions failing to
/// load because `SELECT * FROM messages` returned rows that couldn't
/// deserialize into a Message struct expecting `reasoning_content`.
///
/// This function bypasses all of that: it reads `pragma_table_info` and
/// conditionally ALTERs missing columns. Safe to run on every startup,
/// works for fresh installs and any historical state alike.
async fn ensure_schema(pool: &SqlitePool) -> crate::errors::Result<()> {
    // ── Tables that historic ad-hoc migrations created — fresh installs
    //    miss them and the corresponding command modules would crash on
    //    first use. CREATE IF NOT EXISTS is a no-op on existing DBs.

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cost_entries (
            id            TEXT PRIMARY KEY,
            session_id    TEXT NOT NULL,
            model         TEXT NOT NULL,
            endpoint      TEXT NOT NULL,
            input_tokens  INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            cost_usd      REAL    NOT NULL,
            created_at    TEXT    NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_cost_entries_session ON cost_entries(session_id)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_cost_entries_created ON cost_entries(created_at)")
        .execute(pool)
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS task_runs (
            id              TEXT PRIMARY KEY,
            session_id      TEXT NOT NULL,
            title           TEXT NOT NULL,
            description     TEXT NOT NULL,
            status          TEXT NOT NULL,
            cwd             TEXT NOT NULL,
            parent_task_id  TEXT,
            sub_session_id  TEXT,
            created_at      TEXT NOT NULL,
            started_at      TEXT,
            completed_at    TEXT,
            result          TEXT,
            error           TEXT,
            attempt_count   INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_task_runs_session ON task_runs(session_id)")
        .execute(pool)
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS task_dependencies (
            task_id            TEXT NOT NULL,
            depends_on_task_id TEXT NOT NULL,
            PRIMARY KEY (task_id, depends_on_task_id)
        )",
    )
    .execute(pool)
    .await?;

    // ── Per-column adds for messages — covers the v0.3.7 regression and
    //    any prior divergence between code and old DBs.
    ensure_column(pool, "messages", "tool_calls", "TEXT").await?;
    ensure_column(pool, "messages", "reasoning_content", "TEXT").await?;

    // ── task_runs has a verification_results JSON column referenced by
    //    the verification engine. Some older DBs and all fresh installs
    //    miss it.
    ensure_column(pool, "task_runs", "verification_results", "TEXT").await?;

    Ok(())
}

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    col_type: &str,
) -> crate::errors::Result<()> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await?;
    let exists = rows.iter().any(|r| {
        r.try_get::<String, _>("name")
            .map(|n| n == column)
            .unwrap_or(false)
    });
    if !exists {
        tracing::info!("schema sync: adding column {table}.{column} {col_type}");
        sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN {column} {col_type}"))
            .execute(pool)
            .await?;
    }
    Ok(())
}
