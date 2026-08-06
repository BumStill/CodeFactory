// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row, SqlitePool,
};

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    // Signal 0 performs a liveness/permission check without delivering a
    // signal. EPERM still proves that the process exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_ACCESS_DENIED: u32 = 5;
    const ERROR_INVALID_PARAMETER: u32 = 87;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, exit_code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return match GetLastError() {
                ERROR_INVALID_PARAMETER => false,
                ERROR_ACCESS_DENIED => true,
                // Unknown inspection failures must not cause a live owner to
                // be killed. A later startup can retry the liveness check.
                _ => true,
            };
        }
        let mut exit_code = 0;
        let alive = GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE;
        CloseHandle(handle);
        alive
    }
}

#[cfg(not(any(unix, windows)))]
fn is_process_alive(_pid: u32) -> bool {
    // Conservative fallback: never close an owner we cannot inspect.
    true
}

#[cfg(target_os = "macos")]
fn process_start_token(pid: u32) -> Option<String> {
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (read == size).then(|| format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec))
}

#[cfg(target_os = "linux")]
fn process_start_token(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(')')?.1.trim();
    // /proc/<pid>/stat field 22 is the process start time. `after_name`
    // begins at field 3 (state), so the zero-based offset is 19.
    after_name.split_whitespace().nth(19).map(str::to_string)
}

#[cfg(windows)]
fn process_start_token(pid: u32) -> Option<String> {
    use std::ffi::c_void;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation = FileTime { low: 0, high: 0 };
        let mut exit = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        ok.then(|| (((creation.high as u64) << 32) | creation.low as u64).to_string())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn process_start_token(_pid: u32) -> Option<String> {
    None
}

pub(crate) fn current_process_start_token() -> Option<String> {
    process_start_token(std::process::id())
}

pub(crate) fn process_identity_is_live(pid: u32, expected_start_token: Option<&str>) -> bool {
    if !is_process_alive(pid) {
        return false;
    }
    match expected_start_token {
        Some(expected) => process_start_token(pid)
            .map(|actual| actual == expected)
            // Inspection failure is not proof that an owner is dead.
            .unwrap_or(true),
        // Legacy jobs predate process identity tokens. Preserve their original
        // conservative PID-only behavior instead of killing a possibly live job.
        None => true,
    }
}

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
        tracing::warn!("sqlx::migrate reported {e}; falling back to idempotent ensure_schema");
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
async fn table_exists(pool: &SqlitePool, table: &str) -> crate::errors::Result<bool> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
            .bind(table)
            .fetch_one(pool)
            .await?;
    Ok(count > 0)
}

async fn ensure_schema(pool: &SqlitePool) -> crate::errors::Result<()> {
    // Session → PR is durable product intent: the workspace may return to main
    // after merge, while the conversation still needs to show its own delivery.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS session_delivery_refs (
            session_id TEXT PRIMARY KEY,
            branch TEXT NOT NULL,
            pr_number INTEGER NOT NULL,
            pr_url TEXT NOT NULL,
            commit_sha TEXT,
            updated_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

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

    // Request-level model usage is the accounting truth source. The legacy
    // cost_entries table only captured the final tool-free round and applied a
    // single guessed price to every provider, so it remains for rollback but
    // is no longer suitable for user-facing totals.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS model_usage_events (
            id                 TEXT PRIMARY KEY,
            request_id         TEXT NOT NULL UNIQUE,
            attempt_id         TEXT,
            session_id         TEXT NOT NULL,
            task_id            TEXT,
            surface            TEXT NOT NULL,
            provider           TEXT NOT NULL,
            endpoint           TEXT NOT NULL,
            model              TEXT NOT NULL,
            input_tokens       INTEGER NOT NULL CHECK(input_tokens >= 0),
            output_tokens      INTEGER NOT NULL CHECK(output_tokens >= 0),
            reasoning_tokens   INTEGER NOT NULL DEFAULT 0 CHECK(reasoning_tokens >= 0),
            cached_tokens      INTEGER NOT NULL DEFAULT 0 CHECK(cached_tokens >= 0),
            actual_cost_usd    REAL,
            estimated_cost_usd REAL,
            cost_source        TEXT NOT NULL,
            source             TEXT NOT NULL DEFAULT 'provider_usage',
            created_at         TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    ensure_column(pool, "model_usage_events", "attempt_id", "TEXT").await?;
    sqlx::query("UPDATE model_usage_events SET attempt_id = request_id WHERE attempt_id IS NULL")
        .execute(pool)
        .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_model_usage_attempt ON model_usage_events(attempt_id)",
    )
    .execute(pool)
    .await?;
    for index_sql in [
        "CREATE INDEX IF NOT EXISTS idx_model_usage_created ON model_usage_events(created_at)",
        "CREATE INDEX IF NOT EXISTS idx_model_usage_session ON model_usage_events(session_id, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_model_usage_surface ON model_usage_events(surface, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_model_usage_model ON model_usage_events(model, created_at)",
    ] {
        sqlx::query(index_sql).execute(pool).await?;
    }

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS usage_tracking_metadata (
            singleton  INTEGER PRIMARY KEY CHECK(singleton = 1),
            started_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS usage_budget_receipts (
            id            TEXT PRIMARY KEY,
            period_kind   TEXT NOT NULL,
            period_key    TEXT NOT NULL,
            threshold     REAL NOT NULL,
            usage_tokens  INTEGER NOT NULL,
            limit_tokens  INTEGER NOT NULL,
            triggered_at  TEXT NOT NULL,
            UNIQUE(period_kind, period_key, threshold)
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS usage_migration_receipts (
            version      TEXT PRIMARY KEY,
            scanned      INTEGER NOT NULL,
            inserted     INTEGER NOT NULL,
            skipped      INTEGER NOT NULL,
            conflicted   INTEGER NOT NULL,
            completed_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_usage_budget_period
         ON usage_budget_receipts(period_kind, period_key)",
    )
    .execute(pool)
    .await?;
    let tracking_started = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    sqlx::query(
        "INSERT OR IGNORE INTO usage_tracking_metadata(singleton, started_at) VALUES (1, ?)",
    )
    .bind(&tracking_started)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE usage_tracking_metadata
         SET started_at = strftime('%Y-%m-%dT%H:%M:%fZ', started_at)
         WHERE strftime('%Y-%m-%dT%H:%M:%fZ', started_at) IS NOT NULL",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE model_usage_events
         SET created_at = strftime('%Y-%m-%dT%H:%M:%fZ', created_at)
         WHERE strftime('%Y-%m-%dT%H:%M:%fZ', created_at) IS NOT NULL",
    )
    .execute(pool)
    .await?;

    // Link the assistant transcript row to the exact provider attempt. This
    // lets startup repair distinguish a live-recorded round from a genuinely
    // historical message without comparing token values or timestamps.
    ensure_column(pool, "messages", "usage_request_id", "TEXT").await?;
    if table_exists(pool, "messages").await? {
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_usage_request ON messages(usage_request_id)",
        )
        .execute(pool)
        .await?;
    }

    let mut migration_scanned = 0_i64;
    let mut migration_inserted = 0_i64;
    let mut migration_skipped = 0_i64;
    let mut migration_conflicted = 0_i64;

    // Backfill per-message provider usage first. Assistant messages already
    // preserve one usage object per provider round, including tool rounds.
    // The deterministic request id makes startup repair safe to rerun.
    if table_exists(pool, "messages").await? && table_exists(pool, "sessions").await? {
        let message_scanned: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages m
             WHERE m.role = 'assistant'
               AND (COALESCE(m.input_tokens, 0) > 0 OR COALESCE(m.output_tokens, 0) > 0)
               AND m.created_at < CAST(strftime('%s', (
                   SELECT started_at FROM usage_tracking_metadata WHERE singleton = 1
               )) AS INTEGER) * 1000",
        )
        .fetch_one(pool)
        .await?;
        let message_conflicted: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages m
             WHERE m.role = 'assistant'
               AND (COALESCE(m.input_tokens, 0) > 0 OR COALESCE(m.output_tokens, 0) > 0)
               AND m.created_at < CAST(strftime('%s', (
                   SELECT started_at FROM usage_tracking_metadata WHERE singleton = 1
               )) AS INTEGER) * 1000
               AND (EXISTS (
                   SELECT 1 FROM model_usage_events existing
                   WHERE existing.request_id = 'message:' || m.id
               ) OR (m.usage_request_id IS NOT NULL AND EXISTS (
                   SELECT 1 FROM model_usage_events live
                   WHERE live.request_id = m.usage_request_id
               )))",
        )
        .fetch_one(pool)
        .await?;
        let message_result = sqlx::query(
            "INSERT OR IGNORE INTO model_usage_events (
                id, request_id, attempt_id, session_id, task_id, surface, provider, endpoint, model,
                input_tokens, output_tokens, reasoning_tokens, cached_tokens,
                actual_cost_usd, estimated_cost_usd, cost_source, source, created_at
             )
             SELECT 'message:' || m.id, 'message:' || m.id, 'message:' || m.id, m.session_id, NULL,
                    'interactive', 'historical', 'historical-message',
                    COALESCE(m.model_id, s.model_id, 'unknown'),
                    COALESCE(m.input_tokens, 0), COALESCE(m.output_tokens, 0), 0, 0,
                    NULL, NULL, 'unknown', 'backfill_message',
                    strftime('%Y-%m-%dT%H:%M:%fZ', m.created_at / 1000.0, 'unixepoch')
             FROM messages m
             LEFT JOIN sessions s ON s.id = m.session_id
             WHERE m.role = 'assistant'
               AND (COALESCE(m.input_tokens, 0) > 0 OR COALESCE(m.output_tokens, 0) > 0)
               AND m.created_at < CAST(strftime('%s', (
                   SELECT started_at FROM usage_tracking_metadata WHERE singleton = 1
               )) AS INTEGER) * 1000
               AND (m.usage_request_id IS NULL OR NOT EXISTS (
                   SELECT 1 FROM model_usage_events live
                   WHERE live.request_id = m.usage_request_id
               ))",
        )
        .execute(pool)
        .await?;
        migration_scanned += message_scanned;
        migration_inserted += message_result.rows_affected() as i64;
        migration_conflicted += message_conflicted;
        migration_skipped +=
            (message_scanned - message_result.rows_affected() as i64 - message_conflicted).max(0);
    }

    // Legacy cost rows are incomplete estimates. Import only rows that do not
    // match a message backfill, and never promote their guessed dollars to
    // provider-actual spend.
    let legacy_scanned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cost_entries")
        .fetch_one(pool)
        .await?;
    let legacy_skipped: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cost_entries c
         WHERE EXISTS (
             SELECT 1 FROM model_usage_events u
             WHERE u.source = 'backfill_message'
               AND u.session_id = c.session_id
               AND u.input_tokens = c.input_tokens
               AND u.output_tokens = c.output_tokens
         )",
    )
    .fetch_one(pool)
    .await?;
    let legacy_conflicted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cost_entries c
         WHERE NOT EXISTS (
             SELECT 1 FROM model_usage_events u
             WHERE u.source = 'backfill_message'
               AND u.session_id = c.session_id
               AND u.input_tokens = c.input_tokens
               AND u.output_tokens = c.output_tokens
         ) AND EXISTS (
             SELECT 1 FROM model_usage_events existing
             WHERE existing.request_id = 'legacy:' || c.id
         )",
    )
    .fetch_one(pool)
    .await?;
    let legacy_result = sqlx::query(
        "INSERT OR IGNORE INTO model_usage_events (
            id, request_id, attempt_id, session_id, task_id, surface, provider, endpoint, model,
            input_tokens, output_tokens, reasoning_tokens, cached_tokens,
            actual_cost_usd, estimated_cost_usd, cost_source, source, created_at
         )
         SELECT 'legacy:' || c.id, 'legacy:' || c.id, 'legacy:' || c.id, c.session_id, NULL,
                'interactive', c.endpoint, c.endpoint, c.model,
                c.input_tokens, c.output_tokens, 0, 0,
                NULL, c.cost_usd, 'legacy_estimate', 'legacy_cost_entry',
                COALESCE(strftime('%Y-%m-%dT%H:%M:%fZ', c.created_at), c.created_at)
         FROM cost_entries c
         WHERE NOT EXISTS (
             SELECT 1 FROM model_usage_events u
             WHERE u.source = 'backfill_message'
               AND u.session_id = c.session_id
               AND u.input_tokens = c.input_tokens
               AND u.output_tokens = c.output_tokens
         )",
    )
    .execute(pool)
    .await?;
    migration_scanned += legacy_scanned;
    migration_inserted += legacy_result.rows_affected() as i64;
    migration_skipped += legacy_skipped;
    migration_conflicted += legacy_conflicted;

    sqlx::query(
        "INSERT OR IGNORE INTO usage_migration_receipts
         (version, scanned, inserted, skipped, conflicted, completed_at)
         VALUES ('usage-v1', ?, ?, ?, ?, ?)",
    )
    .bind(migration_scanned)
    .bind(migration_inserted)
    .bind(migration_skipped)
    .bind(migration_conflicted)
    .bind(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
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

    // CF-EVO-R1: normalized tool lifecycle is the observation truth source.
    // Historic databases may only have messages.tool_calls JSON, so create
    // the table idempotently instead of trusting a migration version slot.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tool_calls (
            id          TEXT PRIMARY KEY,
            message_id  TEXT NOT NULL,
            tool_name   TEXT NOT NULL,
            arguments   TEXT NOT NULL DEFAULT '{}',
            result      TEXT,
            metadata    TEXT,
            status      TEXT NOT NULL DEFAULT 'pending',
            error       TEXT,
            duration_ms INTEGER,
            created_at  INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_calls_message ON tool_calls(message_id)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_calls_status ON tool_calls(status)")
        .execute(pool)
        .await?;
    ensure_column(pool, "tool_calls", "metadata", "TEXT").await?;

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

    // Completion-gate control events. The gate injects prompts to make the
    // loop keep working; those are framework instructions, never conversation.
    // Keeping them in `messages` made them role=user rows that had to be
    // filtered out of replay, out of the transcript, out of user-turn paging
    // and out of intent extraction — and one missed filter once turned an
    // internal notice into the next turn's completion instruction. They live
    // here instead, where nothing has to defend against them.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS gate_events (
            id          TEXT PRIMARY KEY,
            session_id  TEXT NOT NULL,
            kind        TEXT NOT NULL,
            content     TEXT NOT NULL DEFAULT '',
            message_id  TEXT,
            created_at  INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    // `message_id` post-dates the table by one release slot — DBs created by
    // the first gate_events build need it added rather than recreated.
    ensure_column(pool, "gate_events", "message_id", "TEXT").await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_gate_events_session ON gate_events(session_id, kind)",
    )
    .execute(pool)
    .await?;
    // `load_agent_history` anti-joins on this for every agent turn.
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_gate_events_message ON gate_events(message_id, kind)",
    )
    .execute(pool)
    .await?;

    // ── Per-column adds for messages — covers the v0.3.7 regression and
    //    any prior divergence between code and old DBs.
    ensure_column(pool, "messages", "tool_calls", "TEXT").await?;
    ensure_column(pool, "messages", "reasoning_content", "TEXT").await?;
    ensure_column(pool, "messages", "completion_state", "TEXT").await?;
    ensure_column(pool, "messages", "endpoint_id", "TEXT").await?;

    // The turn-notice dedup check filters by (session_id, completion_state)
    // before its LIKE, so this turns a full conversation scan per turn into an
    // index seek over the handful of notice rows.
    //
    // Two ordering constraints: it must follow the ensure_column above
    // (`completion_state` is not in 0001_init.sql), and it must tolerate a
    // `messages` table that predates `session_id` — ensure_schema's whole job
    // is surviving whatever historical schema the user actually has, so it
    // checks rather than assumes.
    let message_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('messages')")
            .fetch_all(pool)
            .await?;
    if message_columns.iter().any(|c| c == "session_id") {
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_session_completion_state \
             ON messages(session_id, completion_state)",
        )
        .execute(pool)
        .await?;
    }

    // Append-only structured execution-route snapshots. They remain beside
    // the transcript, so old builds can ignore them without losing messages.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS chat_plan_events (
            id             TEXT PRIMARY KEY,
            session_id     TEXT NOT NULL,
            root_turn_id   TEXT NOT NULL,
            revision       INTEGER NOT NULL,
            plan_json      TEXT NOT NULL,
            explanation    TEXT,
            waiting_reason TEXT,
            change_reason  TEXT,
            created_at     INTEGER NOT NULL,
            UNIQUE(root_turn_id, revision),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;
    let plan_session_cascade: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM pragma_foreign_key_list('chat_plan_events')
         WHERE \"table\" = 'sessions' AND on_delete = 'CASCADE'",
    )
    .fetch_one(pool)
    .await?;
    if plan_session_cascade == 0 {
        // Early development builds created this table before the ownership
        // cascade was specified. Rebuild it transactionally so those databases
        // do not retain orphaned plan history after a session is deleted.
        let sessions_exist: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'sessions'",
        )
        .fetch_one(pool)
        .await?;
        let mut migration = pool.begin().await?;
        sqlx::query("DROP TABLE IF EXISTS chat_plan_events_with_session_cascade")
            .execute(&mut *migration)
            .await?;
        sqlx::query(
            "CREATE TABLE chat_plan_events_with_session_cascade (
                id             TEXT PRIMARY KEY,
                session_id     TEXT NOT NULL,
                root_turn_id   TEXT NOT NULL,
                revision       INTEGER NOT NULL,
                plan_json      TEXT NOT NULL,
                explanation    TEXT,
                waiting_reason TEXT,
                change_reason  TEXT,
                created_at     INTEGER NOT NULL,
                UNIQUE(root_turn_id, revision),
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )",
        )
        .execute(&mut *migration)
        .await?;
        let copy_sql = if sessions_exist == 0 {
            "INSERT INTO chat_plan_events_with_session_cascade
             SELECT * FROM chat_plan_events"
        } else {
            "INSERT INTO chat_plan_events_with_session_cascade
             SELECT event.* FROM chat_plan_events event
             WHERE EXISTS (
                 SELECT 1 FROM sessions session WHERE session.id = event.session_id
             )"
        };
        sqlx::query(copy_sql).execute(&mut *migration).await?;
        sqlx::query("DROP TABLE chat_plan_events")
            .execute(&mut *migration)
            .await?;
        sqlx::query(
            "ALTER TABLE chat_plan_events_with_session_cascade
             RENAME TO chat_plan_events",
        )
        .execute(&mut *migration)
        .await?;
        migration.commit().await?;
    }
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_chat_plan_events_session_turn
         ON chat_plan_events(session_id, root_turn_id, revision DESC)",
    )
    .execute(pool)
    .await?;

    // P1 session execution governance: additive, bounded state beside the
    // append-only transcript. Old builds ignore these tables safely.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS chat_task_segments (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            goal_root_turn_id TEXT NOT NULL,
            previous_segment_id TEXT,
            checkpoint_json TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            UNIQUE(session_id, ordinal),
            UNIQUE(session_id, goal_root_turn_id),
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS chat_turn_state (
            root_turn_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            task_segment_id TEXT,
            revision INTEGER NOT NULL DEFAULT 1,
            phase TEXT NOT NULL,
            status TEXT NOT NULL,
            recovery_attempt INTEGER NOT NULL DEFAULT 0,
            current_step_id TEXT,
            next_step TEXT,
            recent_activity_kind TEXT,
            recent_activity_label TEXT,
            waiting_reason TEXT,
            started_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            completed_at INTEGER,
            terminal_reason TEXT,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            FOREIGN KEY(task_segment_id) REFERENCES chat_task_segments(id) ON DELETE SET NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS task_attempts (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            attempt_index INTEGER NOT NULL,
            sub_session_id TEXT,
            status TEXT NOT NULL,
            failure_code TEXT,
            started_at TEXT NOT NULL,
            completed_at TEXT,
            error TEXT,
            result TEXT,
            verification_results TEXT,
            UNIQUE(task_id, attempt_index),
            UNIQUE(sub_session_id),
            FOREIGN KEY(task_id) REFERENCES task_runs(id) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;
    for statement in [
        "CREATE INDEX IF NOT EXISTS idx_chat_task_segments_session ON chat_task_segments(session_id, ordinal)",
        "CREATE INDEX IF NOT EXISTS idx_chat_turn_state_session ON chat_turn_state(session_id, updated_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_task_attempts_task ON task_attempts(task_id, attempt_index)",
    ] {
        sqlx::query(statement).execute(pool).await?;
    }

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
    ensure_column(
        pool,
        "learning_events",
        "kind",
        "TEXT NOT NULL DEFAULT 'memory'",
    )
    .await?;
    ensure_column(pool, "learning_events", "pref_key", "TEXT").await?;
    ensure_column(pool, "learning_events", "pref_value", "TEXT").await?;
    // ── Self-evolution P1: cross-session mined insights carry an evidence
    //    count (how many sessions back the pattern) + the raw metrics.
    //    Per-session post-mortem rows leave support_count = 0.
    ensure_column(
        pool,
        "learning_events",
        "support_count",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    ensure_column(
        pool,
        "learning_events",
        "evidence_json",
        "TEXT NOT NULL DEFAULT '{}'",
    )
    .await?;
    // Evolution workbench: keep learning_events as the backward-compatible
    // candidate source of truth and attach newly mined candidates to the job
    // that produced them. Legacy rows intentionally remain NULL.
    ensure_column(pool, "learning_events", "job_id", "TEXT").await?;

    // A deliberately small local job ledger. It records real analysis and
    // review/materialization executions without introducing a workflow engine.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS evolution_jobs (
            id                  TEXT PRIMARY KEY,
            cwd                 TEXT NOT NULL,
            trigger             TEXT NOT NULL,
            candidate_id        TEXT,
            status              TEXT NOT NULL,
            idempotency_key     TEXT,
            input_session_count INTEGER NOT NULL DEFAULT 0,
            input_trace_count   INTEGER NOT NULL DEFAULT 0,
            candidate_count     INTEGER NOT NULL DEFAULT 0,
            started_at          TEXT NOT NULL,
            completed_at        TEXT,
            error               TEXT,
            owner_pid           INTEGER,
            owner_start_token   TEXT
        )",
    )
    .execute(pool)
    .await?;
    ensure_column(pool, "evolution_jobs", "owner_pid", "INTEGER").await?;
    ensure_column(pool, "evolution_jobs", "owner_start_token", "TEXT").await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_evolution_jobs_cwd_started \
         ON evolution_jobs(cwd, started_at DESC)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_evolution_jobs_candidate_started \
         ON evolution_jobs(candidate_id, started_at DESC)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_evolution_jobs_idempotency \
         ON evolution_jobs(idempotency_key) WHERE idempotency_key IS NOT NULL",
    )
    .execute(pool)
    .await?;

    // Append-only structured nodes. detail_json is constrained by command code
    // to redacted aggregate metadata; raw prompts, reasoning and traces do not
    // belong in this table.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS evolution_job_events (
            id           TEXT PRIMARY KEY,
            cwd          TEXT NOT NULL,
            job_id       TEXT NOT NULL,
            candidate_id TEXT,
            stage        TEXT NOT NULL,
            status       TEXT NOT NULL,
            title        TEXT NOT NULL,
            detail_json  TEXT NOT NULL DEFAULT '{}',
            created_at   TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_evolution_job_events_cwd_job_created \
         ON evolution_job_events(cwd, job_id, created_at)",
    )
    .execute(pool)
    .await?;

    // Evolution Phase 4 keeps approval, evaluation and activation separate
    // from legacy learning_events.status. Existing accepted rows stay exactly
    // as they are and never receive fabricated Eval/activation records.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS improvement_candidates (
            id                       TEXT PRIMARY KEY,
            cwd                      TEXT NOT NULL,
            kind                     TEXT NOT NULL,
            source_learning_event_id TEXT UNIQUE,
            current_revision         INTEGER NOT NULL,
            current_state            TEXT NOT NULL,
            state_version            INTEGER NOT NULL DEFAULT 1,
            created_at               TEXT NOT NULL,
            updated_at               TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_improvement_candidates_cwd_updated
         ON improvement_candidates(cwd, updated_at DESC)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS candidate_revisions (
            candidate_id TEXT NOT NULL,
            revision     INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            payload_hash TEXT NOT NULL,
            evidence_json TEXT NOT NULL DEFAULT '{}',
            risk_class   TEXT NOT NULL DEFAULT 'low',
            created_at   TEXT NOT NULL,
            PRIMARY KEY (candidate_id, revision)
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS candidate_reviews (
            id            TEXT PRIMARY KEY,
            candidate_id  TEXT NOT NULL,
            revision      INTEGER NOT NULL,
            decision      TEXT NOT NULL,
            actor         TEXT NOT NULL,
            auto_activate INTEGER NOT NULL DEFAULT 0,
            reason        TEXT,
            created_at    TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_candidate_reviews_candidate_created
         ON candidate_reviews(candidate_id, revision, created_at DESC)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS evolution_eval_runs (
            id                 TEXT PRIMARY KEY,
            job_id             TEXT NOT NULL,
            cwd                TEXT NOT NULL,
            candidate_id       TEXT NOT NULL,
            revision           INTEGER NOT NULL,
            status             TEXT NOT NULL,
            manifest_hash      TEXT NOT NULL,
            runner_version     TEXT NOT NULL,
            baseline_hash      TEXT NOT NULL,
            treatment_hash     TEXT NOT NULL,
            target_fingerprint TEXT NOT NULL,
            required_count     INTEGER NOT NULL DEFAULT 0,
            passed_count       INTEGER NOT NULL DEFAULT 0,
            failed_count       INTEGER NOT NULL DEFAULT 0,
            idempotency_key    TEXT NOT NULL UNIQUE,
            owner_pid          INTEGER,
            owner_start_token  TEXT,
            started_at         TEXT NOT NULL,
            completed_at       TEXT,
            error              TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_evolution_eval_runs_cwd_started
         ON evolution_eval_runs(cwd, started_at DESC)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_evolution_eval_running_candidate
         ON evolution_eval_runs(candidate_id, revision) WHERE status='running'",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS evolution_eval_case_results (
            id            TEXT PRIMARY KEY,
            run_id        TEXT NOT NULL,
            case_id       TEXT NOT NULL,
            title         TEXT NOT NULL,
            status        TEXT NOT NULL,
            hard_gate     INTEGER NOT NULL DEFAULT 1,
            detail_json   TEXT NOT NULL DEFAULT '{}',
            created_at    TEXT NOT NULL,
            UNIQUE (run_id, case_id)
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS evolution_activation_receipts (
            id              TEXT PRIMARY KEY,
            job_id          TEXT NOT NULL,
            cwd             TEXT NOT NULL,
            candidate_id    TEXT NOT NULL,
            revision        INTEGER NOT NULL,
            eval_run_id     TEXT NOT NULL,
            target_kind     TEXT NOT NULL,
            target_key      TEXT,
            status          TEXT NOT NULL,
            payload_hash    TEXT NOT NULL,
            before_hash     TEXT NOT NULL,
            after_hash      TEXT NOT NULL,
            before_json     TEXT NOT NULL DEFAULT '{}',
            idempotency_key TEXT NOT NULL UNIQUE,
            activated_at    TEXT,
            rolled_back_at  TEXT,
            error           TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_evolution_activation_cwd_activated
         ON evolution_activation_receipts(cwd, activated_at DESC)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS evolution_active_memory (
            candidate_id  TEXT PRIMARY KEY,
            cwd           TEXT NOT NULL,
            revision      INTEGER NOT NULL,
            activation_id TEXT NOT NULL UNIQUE,
            content       TEXT NOT NULL,
            content_hash  TEXT NOT NULL,
            active        INTEGER NOT NULL DEFAULT 1,
            activated_at  TEXT NOT NULL,
            rolled_back_at TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_evolution_active_memory_cwd_active
         ON evolution_active_memory(cwd, active, activated_at DESC)",
    )
    .execute(pool)
    .await?;

    // Jobs execute in-process today. PID plus process-start token is the owner
    // identity: the token prevents PID reuse from making an interrupted job
    // look live. Another CodeFactory process may share this SQLite database.
    let running_jobs: Vec<(String, String, Option<String>, Option<i64>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, cwd, candidate_id, owner_pid, owner_start_token FROM evolution_jobs
         WHERE status IN ('queued', 'running')",
        )
        .fetch_all(pool)
        .await?;
    let interrupted_jobs: Vec<_> = running_jobs
        .into_iter()
        .filter(|(_, _, _, owner_pid, owner_start_token)| {
            match owner_pid.and_then(|pid| u32::try_from(pid).ok()) {
                Some(pid) => !process_identity_is_live(pid, owner_start_token.as_deref()),
                None => true,
            }
        })
        .collect();
    let interrupted_at = Utc::now().to_rfc3339();
    let mut recovery = pool.begin().await?;
    for (job_id, cwd, candidate_id, _, _) in interrupted_jobs {
        sqlx::query(
            "INSERT INTO evolution_job_events
             (id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at)
             VALUES (?, ?, ?, ?, 'job', 'failed', '应用重启，作业未完成',
                     '{\"schema_version\":1,\"reason\":\"process_restart\"}', ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&cwd)
        .bind(&job_id)
        .bind(candidate_id)
        .bind(&interrupted_at)
        .execute(&mut *recovery)
        .await?;
        sqlx::query(
            "UPDATE evolution_jobs
             SET status='failed', completed_at=?, error='应用在作业完成前中断，请重新运行'
             WHERE id=? AND status IN ('queued', 'running')",
        )
        .bind(&interrupted_at)
        .bind(&job_id)
        .execute(&mut *recovery)
        .await?;
    }
    recovery.commit().await?;
    // Only one accept/reject command may own a candidate at a time, including
    // across two desktop processes sharing the same local SQLite database.
    // Recovery runs first so stale owners from a previous process do not block
    // creation of the partial unique index.
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_evolution_jobs_candidate_running \
         ON evolution_jobs(candidate_id) \
         WHERE candidate_id IS NOT NULL AND status = 'running'",
    )
    .execute(pool)
    .await?;
    // A project may have one active local analysis at a time. This prevents
    // two app processes from mining the same evolving scope concurrently;
    // it is not a claim of analysis-window idempotency.
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_evolution_jobs_scope_analysis_running \
         ON evolution_jobs(cwd) \
         WHERE trigger = 'cross_session' AND status = 'running'",
    )
    .execute(pool)
    .await?;

    // 'project' (default) for full software-factory sessions, 'quick' for
    // ephemeral one-off chats launched from the home page's Quick Task entry.
    // List-sessions excludes 'quick' from the Recent Projects card.
    ensure_column(pool, "sessions", "kind", "TEXT NOT NULL DEFAULT 'project'").await?;
    // Per-session reasoning effort override (NULL → use the global default).
    ensure_column(pool, "sessions", "reasoning_effort", "TEXT").await?;
    // Per-session delivery authorization (0/1). Survives app restarts so a
    // user who said "提交上线" is not asked to re-confirm after relaunching.
    ensure_column(pool, "sessions", "delivery_authorized", "INTEGER NOT NULL DEFAULT 0").await?;
    // Per-session permission preset. Existing sessions get standard so old
    // global allow/ask/deny details no longer leak into the primary UX.
    ensure_column(
        pool,
        "sessions",
        "permission_mode",
        "TEXT NOT NULL DEFAULT 'standard'",
    )
    .await?;
    if table_exists(pool, "sessions").await? {
        sqlx::query(
            "UPDATE sessions SET permission_mode = 'standard'
             WHERE permission_mode IS NULL OR permission_mode NOT IN ('safe','standard','trusted')",
        )
        .execute(pool)
        .await?;
    }
    // Model runtime ownership moved from global Settings to the session.
    // Existing rows stay fixed and unresolved rather than silently crossing
    // providers after upgrade.
    ensure_column(pool, "sessions", "endpoint_id", "TEXT").await?;
    ensure_column(
        pool,
        "sessions",
        "model_policy",
        "TEXT NOT NULL DEFAULT 'fixed'",
    )
    .await?;
    if table_exists(pool, "sessions").await? {
        sqlx::query(
            "UPDATE sessions SET model_policy = 'fixed'
             WHERE model_policy IS NULL OR model_policy NOT IN ('fixed','prefer','auto')",
        )
        .execute(pool)
        .await?;
    }

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS model_route_attempts (
            id                  TEXT PRIMARY KEY,
            root_turn_id        TEXT NOT NULL,
            session_id          TEXT NOT NULL,
            endpoint            TEXT NOT NULL,
            model               TEXT NOT NULL,
            policy              TEXT NOT NULL,
            status              TEXT NOT NULL,
            failure_code        TEXT,
            output_started      INTEGER NOT NULL DEFAULT 0,
            side_effect_started INTEGER NOT NULL DEFAULT 0,
            created_at          TEXT NOT NULL,
            completed_at        TEXT
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_model_route_attempts_turn
         ON model_route_attempts(session_id, root_turn_id, created_at)",
    )
    .execute(pool)
    .await?;

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
    ensure_column(pool, "user_preferences", "activation_id", "TEXT").await?;

    // ── task_journal: durable last-known-good record of a completed task's
    //    dispatch, kept SEPARATE from live task_runs state. task_runs.result is
    //    NULLed by retry_failed_tasks + resume invalidation to re-run a row, so
    //    it cannot be the durable cache; task_journal.result_json is. Keyed by
    //    task_id (the task's stable position in the session tree). Powers the
    //    content-addressed resume journal: a completed task is replayed from
    //    cache only if its recomputed dispatch_key still matches AND its output
    //    is still materialized on disk.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS task_journal (
            task_id             TEXT PRIMARY KEY,
            session_id          TEXT NOT NULL,
            hash_version        INTEGER NOT NULL DEFAULT 1,
            local_digest        TEXT NOT NULL,
            dispatch_key        TEXT NOT NULL,
            dep_keys_json       TEXT NOT NULL DEFAULT '[]',
            resolved_model      TEXT NOT NULL,
            resolved_tools_json TEXT NOT NULL DEFAULT '[]',
            isolation_mode      TEXT NOT NULL,
            state               TEXT NOT NULL,
            merge_applied       INTEGER NOT NULL DEFAULT 0,
            materialization     TEXT NOT NULL,
            checkpoint_id       TEXT,
            base_sha            TEXT,
            patch_path          TEXT,
            repo_root           TEXT,
            result_json         TEXT,
            completed_at        TEXT NOT NULL,
            updated_at          TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_task_journal_session ON task_journal(session_id)")
        .execute(pool)
        .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_task_journal_checkpoint ON task_journal(checkpoint_id)",
    )
    .execute(pool)
    .await?;

    // Owner identity on task_runs, mirroring evolution_jobs: PID + process-start
    // token so a crash-orphaned 'running' row can be distinguished from one a
    // live sibling process still owns. Default NULL — TaskRun's SELECT * FromRow
    // is untouched; these are read by targeted tuple queries in journal.rs.
    ensure_column(pool, "task_runs", "owner_pid", "INTEGER").await?;
    ensure_column(pool, "task_runs", "owner_start_token", "TEXT").await?;

    // GAP 1 boot recovery: turn every crash-orphaned 'running' task (dead owner)
    // into 'completed' (worktree merge that already applied) or 'pending', before
    // any scheduler runs. Session-agnostic; mirrors the evolution_jobs recovery
    // above. Best-effort: a recovery hiccup must never block app startup.
    if let Err(e) =
        crate::agent::journal::recover_orphaned_tasks(pool, crate::agent::journal::OrphanScope::All)
            .await
    {
        tracing::warn!("task orphan recovery at boot failed (non-fatal): {e}");
    }

    // Existing rows all carry a frozen activity time; pull them up to their
    // newest message so the sidebar's order is right on first launch after the
    // upgrade. Best-effort: a stale session list must never block startup.
    match backfill_session_activity_time(pool).await {
        Ok(0) => {}
        Ok(rows) => tracing::info!("backfilled session activity time for {rows} session(s)"),
        Err(e) => tracing::warn!("session activity backfill failed (non-fatal): {e}"),
    }

    crate::knowledge::ensure_schema(pool).await?;
    crate::benchmark::ensure_schema(pool).await?;

    Ok(())
}

/// Pull each session's activity time up to its newest message.
///
/// Until this shipped, nothing advanced `sessions.updated_at` when messages
/// were written — only changing the model or naming the session did, and the
/// name is generated once at the start. Every existing row therefore carries a
/// timestamp frozen near creation, which the sidebar shows as the session's
/// time and sorts by. Without this backfill the fix would be invisible on
/// existing data: old busy sessions would keep sorting below idle ones forever.
///
/// **Only ever moves forward.** A rename can legitimately set `updated_at`
/// later than the last message, and dragging that backwards would be a second
/// bug. Moving forward only also makes this idempotent: after the first pass
/// the WHERE clause matches nothing, so it stays a cheap no-op on every boot
/// instead of needing a one-shot migration slot.
pub(crate) async fn backfill_session_activity_time(
    pool: &SqlitePool,
) -> crate::errors::Result<u64> {
    let result = sqlx::query(
        "UPDATE sessions
         SET updated_at = (
             SELECT MAX(m.created_at) FROM messages m WHERE m.session_id = sessions.id
         )
         WHERE EXISTS (SELECT 1 FROM messages m WHERE m.session_id = sessions.id)
           AND updated_at < (
             SELECT MAX(m.created_at) FROM messages m WHERE m.session_id = sessions.id
           )",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
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
        sqlx::query(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {col_type}"
        ))
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Close `pool` and make SQLite release *every* file the database owns —
/// including the `-wal` and `-shm` sidecars — so the containing directory can
/// be deleted immediately afterwards.
///
/// [`connect`] does not set a journal mode, so sqlx's default applies and every
/// database is opened in WAL mode. WAL creates two sidecars next to the `.db`
/// file, and the `-shm` one is *memory-mapped*. SQLite only checkpoints and
/// unlinks those sidecars when the **last** connection closes, and only if that
/// connection can take an exclusive lock — with a multi-connection pool the
/// closes race and the unlink is skipped. Measured on a 5-connection pool:
/// roughly two thirds of plain `pool.close()` calls leave `-wal`/`-shm` on disk.
///
/// That is invisible on Unix, where an unlinked-but-mapped file just goes away.
/// On Windows a file with a live section object cannot be deleted until the
/// kernel tears the mapping down, and that teardown is asynchronous with handle
/// close — so `remove_dir_all` fails with `ERROR_SHARING_VIOLATION` (os error
/// 32) even though no process visibly holds the file.
///
/// Reopening the database alone with `journal_mode = DELETE` forces that
/// checkpoint-and-unlink to actually happen.
///
/// The retry is load-bearing, and it was learned from Windows CI rather than
/// from local runs: a single-shot version of this function still left the
/// sidecars behind in **3 of 20 Windows attempts** while being perfect locally.
/// `pool.close()` returning does not guarantee Windows has finished releasing
/// the old handles, so the exclusive access this needs can lose a race. Both
/// macOS measurements — plain reopen and DELETE-mode reopen — cleared the
/// sidecars 100% of the time, which is exactly why the local signal was useless
/// here and the loop condition is the *filesystem*, not an error code: it keeps
/// retrying until the sidecars are actually gone, whatever the reason was.
///
/// Setting the journal mode through [`SqliteConnectOptions`] rather than as a
/// follow-up `PRAGMA` is a smaller point: it folds a failed mode switch into the
/// connect error, so it is retried rather than swallowed.
///
/// Callers that are about to delete the database's directory should use this
/// instead of a bare `pool.close()`. Everything here is best-effort: the
/// database is being torn down regardless, so failures are logged, not raised.
pub async fn close_and_release_files(pool: SqlitePool) {
    /// Long enough to outlast a contended handoff, short enough that a database
    /// genuinely still in use does not stall a teardown for long.
    const BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
    const FIRST_BACKOFF: std::time::Duration = std::time::Duration::from_millis(20);
    const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_millis(400);

    let filename = pool.connect_options().get_filename().to_path_buf();
    pool.close().await;
    drop(pool);

    // An in-memory database owns no files to release.
    if filename.as_os_str() == ":memory:" {
        return;
    }

    let wal = sidecar_path(&filename, "-wal");
    let shm = sidecar_path(&filename, "-shm");
    let settled = |wal: &std::path::Path, shm: &std::path::Path| !wal.exists() && !shm.exists();

    let deadline = std::time::Instant::now() + BUDGET;
    let mut backoff = FIRST_BACKOFF;
    let mut last_error: Option<String> = None;

    loop {
        // A database the caller already deleted, or one whose sidecars are
        // already gone, needs nothing further.
        if !filename.exists() || settled(&wal, &shm) {
            return;
        }

        // `create_if_missing(false)` keeps this from resurrecting a database a
        // caller has already deleted.
        match SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&filename)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Delete),
            )
            .await
        {
            Ok(solo) => {
                solo.close().await;
                if settled(&wal, &shm) {
                    return;
                }
                last_error = Some("sidecars survived a successful DELETE-mode open".into());
            }
            Err(error) => last_error = Some(error.to_string()),
        }

        if std::time::Instant::now() + backoff >= deadline {
            tracing::warn!(
                "could not release the WAL sidecars of {} within {:?}: {}",
                filename.display(),
                BUDGET,
                last_error.as_deref().unwrap_or("unknown"),
            );
            return;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// `foo.db` + `-wal` → `foo.db-wal`, matching how SQLite names its sidecars.
/// Deliberately appends to the whole filename rather than replacing an
/// extension, which would produce `foo-wal` and silently check the wrong path.
fn sidecar_path(db: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::no_window::NoWindow;
    use sqlx::sqlite::SqlitePoolOptions;

    /// The WAL sidecars must be gone once the pool is closed, every single time.
    ///
    /// This is the deterministic form of a Windows-only CI flake: the evidence
    /// pack test failed in fixture cleanup with `os error 32` because a
    /// memory-mapped `-shm` file outlived the pool. Asserting the invariant here
    /// catches a teardown regression on every platform, rather than waiting for
    /// a sharing violation to surface on windows-latest.
    #[tokio::test]
    async fn close_and_release_files_unlinks_wal_sidecars() {
        let mut survivors = Vec::new();

        for attempt in 0..20 {
            let root = std::env::temp_dir()
                .join(format!("codefactory-db-sidecar-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let db_path = root.join("test.db");
            let db_url = format!("sqlite:{}", db_path.display());

            let pool = connect(&db_url).await.unwrap();
            sqlx::query(
                "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at) \
                 VALUES ('s', 't', 'c', 'm', 1, 1)",
            )
            .execute(&pool)
            .await
            .unwrap();

            close_and_release_files(pool).await;

            let wal = root.join("test.db-wal").exists();
            let shm = root.join("test.db-shm").exists();
            if wal || shm {
                survivors.push(format!("attempt {attempt}: -wal={wal} -shm={shm}"));
            }

            let _ = std::fs::remove_dir_all(&root);
        }

        assert!(
            survivors.is_empty(),
            "WAL sidecars outlived close_and_release_files in {}/20 attempts \
             (on Windows each survivor is a potential ERROR_SHARING_VIOLATION \
             during cleanup):\n  {}",
            survivors.len(),
            survivors.join("\n  ")
        );
    }

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
        let cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('messages')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            cols.contains(&"tool_calls".to_string()),
            "ensure_schema must add tool_calls column. Got: {cols:?}"
        );
        assert!(
            cols.contains(&"reasoning_content".to_string()),
            "ensure_schema must add reasoning_content column. Got: {cols:?}"
        );
        assert!(
            cols.contains(&"completion_state".to_string()),
            "ensure_schema must add completion_state column. Got: {cols:?}"
        );
        assert!(
            cols.contains(&"endpoint_id".to_string()),
            "ensure_schema must add endpoint_id column for actual route provenance. Got: {cols:?}"
        );

        // Gate control events get their own table, and the index over
        // messages(session_id, completion_state) must survive the fact that
        // completion_state is added by ALTER, not by 0001_init.sql.
        let objects: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE name IN \
             ('gate_events','idx_gate_events_session','idx_messages_session_completion_state')",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            objects.len(),
            3,
            "gate_events + both indexes. Got: {objects:?}"
        );

        let plan_foreign_keys: Vec<(String, String)> = sqlx::query_as(
            "SELECT \"table\", on_delete
             FROM pragma_foreign_key_list('chat_plan_events')",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            plan_foreign_keys,
            vec![("sessions".to_string(), "CASCADE".to_string())],
            "chat plan history must be deleted with its owning session"
        );

        // The seeded row's data must survive the ALTER TABLE.
        let role: String = sqlx::query_scalar("SELECT role FROM messages WHERE id='m1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(role, "user");
    }

    #[tokio::test]
    async fn ensure_schema_repairs_plan_history_without_session_cascade() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::query(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                cwd TEXT NOT NULL,
                model_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
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
        sqlx::query(
            "CREATE TABLE chat_plan_events (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                root_turn_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                plan_json TEXT NOT NULL,
                explanation TEXT,
                waiting_reason TEXT,
                change_reason TEXT,
                created_at INTEGER NOT NULL,
                UNIQUE(root_turn_id, revision)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions
             (id, title, cwd, model_id, created_at, updated_at)
             VALUES ('owned-session', 'owned', '/tmp', 'model', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (id, session_id, root_turn_id) in [
            ("owned-event", "owned-session", "owned-root"),
            ("orphan-event", "missing-session", "orphan-root"),
        ] {
            sqlx::query(
                "INSERT INTO chat_plan_events
                 (id, session_id, root_turn_id, revision, plan_json, created_at)
                 VALUES (?, ?, ?, 1, '[]', 1)",
            )
            .bind(id)
            .bind(session_id)
            .bind(root_turn_id)
            .execute(&pool)
            .await
            .unwrap();
        }

        ensure_schema(&pool).await.expect("repair schema");

        let retained_ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM chat_plan_events ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(retained_ids, vec!["owned-event"]);
        let plan_foreign_keys: Vec<(String, String)> = sqlx::query_as(
            "SELECT \"table\", on_delete
             FROM pragma_foreign_key_list('chat_plan_events')",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            plan_foreign_keys,
            vec![("sessions".to_string(), "CASCADE".to_string())]
        );
    }

    /// ensure_schema must be safe to run twice — startup may execute it on
    /// every launch.
    #[tokio::test]
    async fn ensure_schema_is_idempotent() {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();

        ensure_schema(&pool).await.unwrap();
        ensure_schema(&pool).await.unwrap();
        ensure_schema(&pool).await.unwrap();

        let cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('messages')")
                .fetch_all(&pool)
                .await
                .unwrap();
        // No duplicate columns (sqlite would have errored if we double-added).
        assert_eq!(cols.iter().filter(|c| *c == "reasoning_content").count(), 1);
    }

    #[tokio::test]
    async fn legacy_sessions_gain_fixed_model_runtime_config_and_route_journal() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                cwd TEXT NOT NULL,
                model_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions
             (id, title, cwd, model_id, created_at, updated_at)
             VALUES ('legacy', 'legacy', '/tmp', 'gpt-5.5', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        ensure_schema(&pool).await.unwrap();
        ensure_schema(&pool).await.unwrap();

        let cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('sessions')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(cols.contains(&"endpoint_id".to_string()), "{cols:?}");
        assert!(cols.contains(&"model_policy".to_string()), "{cols:?}");
        assert!(cols.contains(&"permission_mode".to_string()), "{cols:?}");
        let policy: String =
            sqlx::query_scalar("SELECT model_policy FROM sessions WHERE id='legacy'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(policy, "fixed");
        let permission_mode: String =
            sqlx::query_scalar("SELECT permission_mode FROM sessions WHERE id='legacy'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(permission_mode, "standard");

        let attempts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table' AND name='model_route_attempts'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(attempts, 1);
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
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();

        ensure_schema(&pool).await.unwrap();

        for table in [
            "cost_entries",
            "task_runs",
            "task_dependencies",
            "knowledge_libraries",
            "knowledge_documents",
            "knowledge_chunks",
            "retrieval_events",
            "tool_calls",
            "benchmark_runs",
            "benchmark_trials",
            "learning_events",
            "evolution_jobs",
            "evolution_job_events",
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

        let tool_cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('tool_calls')")
                .fetch_all(&pool)
                .await
                .unwrap();
        for expected in [
            "arguments",
            "result",
            "metadata",
            "status",
            "error",
            "duration_ms",
        ] {
            assert!(
                tool_cols.contains(&expected.to_string()),
                "tool_calls must expose {expected}. Got: {tool_cols:?}"
            );
        }
        let learning_cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('learning_events')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(learning_cols.contains(&"job_id".to_string()));
    }

    #[tokio::test]
    async fn ensure_schema_adds_evolution_jobs_without_rewriting_legacy_learnings() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE learning_events (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, cwd TEXT NOT NULL,
                observation TEXT NOT NULL, suggestion TEXT NOT NULL, status TEXT NOT NULL,
                created_at TEXT NOT NULL, decided_at TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO learning_events
             (id, session_id, cwd, observation, suggestion, status, created_at)
             VALUES ('legacy', 's1', '/proj', 'obs', 'sug', 'accepted', '2026-07-15')",
        )
        .execute(&pool)
        .await
        .unwrap();

        ensure_schema(&pool).await.unwrap();

        let learning_cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('learning_events')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            learning_cols.contains(&"job_id".to_string()),
            "legacy learning_events must gain nullable job_id: {learning_cols:?}"
        );
        for table in ["evolution_jobs", "evolution_job_events"] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(exists, 1, "ensure_schema must create {table}");
        }
        let legacy: (String, Option<String>) =
            sqlx::query_as("SELECT status, job_id FROM learning_events WHERE id='legacy'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(legacy, ("accepted".into(), None));
    }

    #[tokio::test]
    async fn ensure_schema_closes_in_process_jobs_interrupted_by_restart() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO evolution_jobs (id, cwd, trigger, status, started_at)
             VALUES ('interrupted', '/proj', 'cross_session', 'running', '2026-07-15')",
        )
        .execute(&pool)
        .await
        .unwrap();

        ensure_schema(&pool).await.unwrap();

        let job: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT status, completed_at, error FROM evolution_jobs WHERE id='interrupted'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(job.0, "failed");
        assert!(job.1.is_some());
        assert_eq!(job.2.as_deref(), Some("应用在作业完成前中断，请重新运行"));
        let event: (String, String, String) = sqlx::query_as(
            "SELECT stage, status, detail_json FROM evolution_job_events
             WHERE job_id='interrupted' ORDER BY created_at DESC, rowid DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((event.0.as_str(), event.1.as_str()), ("job", "failed"));
        assert!(event.2.contains("process_restart"));
    }

    #[tokio::test]
    async fn ensure_schema_does_not_interrupt_a_job_owned_by_this_live_process() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, status, owner_pid, owner_start_token, started_at)
             VALUES ('live-owner', '/proj', 'cross_session', 'running', ?, ?, '2026-07-15')",
        )
        .bind(std::process::id() as i64)
        .bind(current_process_start_token())
        .execute(&pool)
        .await
        .unwrap();

        ensure_schema(&pool).await.unwrap();

        let status: String =
            sqlx::query_scalar("SELECT status FROM evolution_jobs WHERE id='live-owner'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "running");
        let recovery_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_job_events
             WHERE job_id='live-owner' AND detail_json LIKE '%process_restart%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(recovery_events, 0);
    }

    #[tokio::test]
    async fn ensure_schema_closes_a_job_when_pid_was_reused() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, status, owner_pid, owner_start_token, started_at)
             VALUES ('reused-pid', '/proj', 'cross_session', 'running', ?, 'not-the-current-process', '2026-07-15')",
        )
        .bind(std::process::id() as i64)
        .execute(&pool)
        .await
        .unwrap();

        ensure_schema(&pool).await.unwrap();

        let status: String =
            sqlx::query_scalar("SELECT status FROM evolution_jobs WHERE id='reused-pid'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn ensure_schema_closes_a_job_owned_by_a_real_dead_process() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();

        #[cfg(unix)]
        let mut child = std::process::Command::new("sh")
            .no_window()
            .args(["-c", "sleep 0.1"])
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .no_window()
            .args(["/C", "ping -n 2 127.0.0.1 >NUL"])
            .spawn()
            .unwrap();
        let child_pid = child.id();
        let child_start_token = process_start_token(child_pid)
            .expect("supported platforms must expose a process start token");
        child.wait().unwrap();

        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, status, owner_pid, owner_start_token, started_at)
             VALUES ('dead-owner', '/proj', 'cross_session', 'running', ?, ?, '2026-07-15')",
        )
        .bind(child_pid as i64)
        .bind(child_start_token)
        .execute(&pool)
        .await
        .unwrap();

        ensure_schema(&pool).await.unwrap();

        let status: String =
            sqlx::query_scalar("SELECT status FROM evolution_jobs WHERE id='dead-owner'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
    }

    #[tokio::test]
    async fn ensure_schema_adds_phase4_evals_activation_without_backfilling_legacy_accepts() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO learning_events
             (id, session_id, cwd, observation, suggestion, status, created_at)
             VALUES ('legacy-phase4', 's1', '/proj', 'obs', 'legacy active', 'accepted', '2026-07-15')",
        )
        .execute(&pool)
        .await
        .unwrap();

        ensure_schema(&pool).await.unwrap();

        for table in [
            "improvement_candidates",
            "candidate_revisions",
            "candidate_reviews",
            "evolution_eval_runs",
            "evolution_eval_case_results",
            "evolution_activation_receipts",
            "evolution_active_memory",
        ] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(exists, 1, "Phase 4 schema must create {table}");
        }
        let legacy_state: String =
            sqlx::query_scalar("SELECT status FROM learning_events WHERE id='legacy-phase4'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(legacy_state, "accepted");
        let fake_candidates: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM improvement_candidates")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            fake_candidates, 0,
            "legacy accepts must not receive fabricated Eval state"
        );
    }

    #[tokio::test]
    async fn ensure_schema_adds_session_execution_governance_tables_idempotently() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT)")
            .execute(&pool)
            .await
            .unwrap();

        ensure_schema(&pool).await.unwrap();
        ensure_schema(&pool).await.unwrap();

        for table in ["chat_task_segments", "chat_turn_state", "task_attempts"] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(exists, 1, "missing additive table {table}");
        }
    }

    /// The backfill is what makes the fix visible on existing data — and it
    /// must never drag a timestamp backwards, because a rename can legitimately
    /// set `updated_at` past the last message.
    #[tokio::test]
    async fn backfill_moves_stale_sessions_forward_only() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY, updated_at INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT, created_at INTEGER)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // frozen: the bug's signature — busy session stuck at creation time.
        sqlx::query("INSERT INTO sessions VALUES ('frozen', 100)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages VALUES ('m1','frozen',100),('m2','frozen',900)")
            .execute(&pool)
            .await
            .unwrap();
        // renamed later than its last message — must NOT be pulled back.
        sqlx::query("INSERT INTO sessions VALUES ('renamed', 5000)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages VALUES ('m3','renamed',900)")
            .execute(&pool)
            .await
            .unwrap();
        // empty session — nothing to derive from.
        sqlx::query("INSERT INTO sessions VALUES ('empty', 42)")
            .execute(&pool)
            .await
            .unwrap();

        let rows = backfill_session_activity_time(&pool).await.unwrap();
        assert_eq!(rows, 1, "only the frozen session needed moving");

        let at = |id: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT updated_at FROM sessions WHERE id = ?")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(at("frozen").await, 900, "pulled up to its newest message");
        assert_eq!(at("renamed").await, 5000, "a later rename must not be undone");
        assert_eq!(at("empty").await, 42, "no messages, nothing to derive");

        // Idempotent: a second pass is a no-op, so it can run on every boot.
        assert_eq!(backfill_session_activity_time(&pool).await.unwrap(), 0);
    }
}
