// SPDX-License-Identifier: Apache-2.0
//
// Quick-task session storage tests. The actual `get_or_create_quick_session`
// command needs an AppState (Tauri runtime), so here we verify the underlying
// SQL contract that command depends on:
//   - sessions table accepts kind='quick' rows
//   - list_sessions WHERE clause excludes quick rows
//   - subsequent inserts don't create duplicates if the caller is well-behaved

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        // Mirror the production schema slice that matters here.
        sqlx::query(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                cwd TEXT NOT NULL,
                model_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                parent_session_id TEXT,
                kind TEXT NOT NULL DEFAULT 'project'
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_session(
        pool: &SqlitePool,
        id: &str,
        kind: &str,
        parent: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at, parent_session_id, kind) \
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(id)
        .bind(format!("session {id}"))
        .bind("/tmp/proj")
        .bind("m")
        .bind(0_i64)
        .bind(0_i64)
        .bind(parent)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_sessions_excludes_quick_kind() {
        let pool = fresh_pool().await;
        insert_session(&pool, "p1", "project", None).await;
        insert_session(&pool, "p2", "project", None).await;
        insert_session(&pool, "q1", "quick",   None).await;

        let ids: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM sessions \
             WHERE parent_session_id IS NULL AND kind != 'quick' \
             ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let ids: Vec<String> = ids.into_iter().map(|(s,)| s).collect();
        assert_eq!(ids, vec!["p1", "p2"]);
    }

    #[tokio::test]
    async fn list_sessions_excludes_subagent_children_independently_of_kind() {
        // Defence-in-depth: quick + subagent are independent filters.
        // A subagent-spawned child of a quick session shouldn't leak into
        // the list either (rare in practice but cheap to verify).
        let pool = fresh_pool().await;
        insert_session(&pool, "p1", "project", None).await;
        insert_session(&pool, "c1", "project", Some("p1")).await; // child
        insert_session(&pool, "q1", "quick",   None).await;
        insert_session(&pool, "qc", "project", Some("q1")).await; // child of quick

        let ids: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM sessions \
             WHERE parent_session_id IS NULL AND kind != 'quick' \
             ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            ids.into_iter().map(|(s,)| s).collect::<Vec<_>>(),
            vec!["p1"]
        );
    }

    #[tokio::test]
    async fn quick_session_lookup_returns_most_recent() {
        // The command picks ORDER BY updated_at DESC LIMIT 1 — if the user
        // somehow has two quick sessions (e.g. from a bug or a restore),
        // we should return the newer one, not error.
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at, parent_session_id, kind) \
             VALUES ('old','q','/tmp','m',1,1,NULL,'quick'), \
                    ('new','q','/tmp','m',2,2,NULL,'quick')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let id: (String,) = sqlx::query_as(
            "SELECT id FROM sessions WHERE kind = 'quick' ORDER BY updated_at DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(id.0, "new");
    }
}
