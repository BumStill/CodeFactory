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
            attempt_count   INTEGER NOT NULL DEFAULT 0,
            task_context_json TEXT
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

    // Per-message git checkpoints: every user message that kicks off
    // agent work captures the working tree state so the user can revert
    // any wrong-direction change with one click. See agent/checkpoint.rs
    // for the snapshot mechanics.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS checkpoints (
            id          TEXT PRIMARY KEY,
            session_id  TEXT NOT NULL,
            message_id  TEXT,
            cwd         TEXT NOT NULL,
            git_sha     TEXT NOT NULL,
            label       TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            reverted    INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_checkpoints_session ON checkpoints(session_id)")
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
    ensure_column(pool, "task_runs", "task_context_json", "TEXT").await?;

    // ── learning_events: post-task observations the AI surfaces for
    //    user approval. status: pending | accepted | rejected.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS learning_events (
            id           TEXT PRIMARY KEY,
            session_id   TEXT NOT NULL,
            cwd          TEXT NOT NULL,
            observation  TEXT NOT NULL,
            suggestion   TEXT NOT NULL,
            status       TEXT NOT NULL DEFAULT 'pending',
            created_at   TEXT NOT NULL,
            decided_at   TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_learning_events_cwd_status \
         ON learning_events(cwd, status)",
    )
    .execute(pool)
    .await?;
    // ── learning_events.kind: distinguishes free-form memory entries
    //    from structured preference proposals. Existing rows default to
    //    'memory' (their original semantics). For 'preference' rows,
    //    pref_key / pref_value carry the structured payload — the
    //    suggestion column still holds a human-readable rendering for UI.
    ensure_column(pool, "learning_events", "kind", "TEXT NOT NULL DEFAULT 'memory'").await?;
    ensure_column(pool, "learning_events", "pref_key", "TEXT").await?;
    ensure_column(pool, "learning_events", "pref_value", "TEXT").await?;
    // 'project' (default) for full software-factory sessions, 'quick' for
    // ephemeral one-off chats launched from the home page's Quick Task entry.
    // List-sessions excludes 'quick' from the Recent Projects card.
    ensure_column(pool, "sessions", "kind", "TEXT NOT NULL DEFAULT 'project'").await?;
    // Per-session reasoning effort override (NULL → use the global default).
    ensure_column(pool, "sessions", "reasoning_effort", "TEXT").await?;

    // task_runs.acceptance_criteria_json: JSON Vec<String> set by the
    // decompose commands at task creation time. The autonomous subagent
    // reads it, must verify each criterion, and the scheduler post-task
    // hook respawns the agent if any criterion isn't evidenced in the
    // result. NULL on legacy rows = no-criteria back-compat.
    ensure_column(pool, "task_runs", "acceptance_criteria_json", "TEXT").await?;

    // task_runs spec link: which spec this task was decomposed from, so the
    // workspace task tree can show "来自规范《X》" and close the spec→task loop.
    ensure_column(pool, "task_runs", "spec_req_id", "TEXT").await?;
    ensure_column(pool, "task_runs", "spec_title", "TEXT").await?;

    // ── user_preferences: structured key→value the AI reads at
    //    decomposition / execution time. Scoped per cwd so different
    //    projects can have different defaults. `source` is one of:
    //      'user'    — manually set in the Profile UI
    //      'ai'      — proposed by post-mortem, user accepted
    //      'default' — seeded by the app on first run
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_preferences (
            cwd         TEXT NOT NULL,
            key         TEXT NOT NULL,
            value       TEXT NOT NULL,
            source      TEXT NOT NULL DEFAULT 'user',
            updated_at  TEXT NOT NULL,
            PRIMARY KEY (cwd, key)
        )",
    )
    .execute(pool)
    .await?;

    crate::knowledge::ensure_schema(pool).await?;

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
    // PRAGMA returns 0 rows when the table doesn't exist. Don't try to
    // ALTER a missing table — it would error. In production the table is
    // created by the migration before ensure_schema runs; in tests the
    // fixture may only create a subset of tables.
    if rows.is_empty() {
        return Ok(());
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// Synthesise the exact "old user DB" shape from the v0.3.7 regression:
    /// `messages` table predates the `reasoning_content` column, but the
    /// Message struct expects it. This test reproduces the bug and asserts
    /// that `ensure_schema` repairs the schema in place.
    #[tokio::test]
    async fn ensure_schema_recovers_pre_reasoning_messages_table() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");

        // Old schema: messages without tool_calls or reasoning_content columns.
        sqlx::query(
            "CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                role TEXT,
                content TEXT,
                model_id TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                created_at INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Seed a row so the post-sync SELECT * has something to find.
        sqlx::query("INSERT INTO messages (id, session_id, role, content, created_at) VALUES ('m1','s1','user','hi',1)")
            .execute(&pool)
            .await
            .unwrap();

        ensure_schema(&pool).await.expect("ensure_schema OK");

        // The repaired schema must expose both the columns the app code
        // requires today — otherwise SELECT * → struct FromRow blows up
        // exactly like in production.
        let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('messages')")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(cols.contains(&"tool_calls".to_string()),
            "ensure_schema must add tool_calls column. Got: {cols:?}");
        assert!(cols.contains(&"reasoning_content".to_string()),
            "ensure_schema must add reasoning_content column. Got: {cols:?}");

        // The seeded row's data must survive the ALTER TABLE.
        let role: String = sqlx::query_scalar("SELECT role FROM messages WHERE id='m1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(role, "user");
    }

    /// ensure_schema must be safe to run twice — startup may execute it on
    /// every launch.
    #[tokio::test]
    async fn ensure_schema_is_idempotent() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)").execute(&pool).await.unwrap();

        ensure_schema(&pool).await.unwrap();
        ensure_schema(&pool).await.unwrap();
        ensure_schema(&pool).await.unwrap();

        let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('messages')")
            .fetch_all(&pool)
            .await
            .unwrap();
        // No duplicate columns (sqlite would have errored if we double-added).
        assert_eq!(
            cols.iter().filter(|c| *c == "reasoning_content").count(),
            1
        );
    }

    /// Fresh-install path: ensure_schema must also create the satellite
    /// tables the application uses but that aren't in 0001_init.sql any
    /// more (cost_entries, task_runs, task_dependencies).
    #[tokio::test]
    async fn ensure_schema_creates_satellite_tables_on_fresh_db() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // No tables at all — pretend this is a brand-new DB created after
        // 0001_init.sql ran.
        sqlx::query("CREATE TABLE messages (id TEXT)").execute(&pool).await.unwrap();

        ensure_schema(&pool).await.unwrap();

        for table in [
            "cost_entries",
            "task_runs",
            "task_dependencies",
            "knowledge_libraries",
            "knowledge_documents",
            "knowledge_chunks",
            "retrieval_events",
        ] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(exists, 1, "ensure_schema must create {table}");
        }

        let task_cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('task_runs')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            task_cols.contains(&"task_context_json".to_string()),
            "task_runs must persist connector context for task execution evidence. Got: {task_cols:?}"
        );
    }
}
