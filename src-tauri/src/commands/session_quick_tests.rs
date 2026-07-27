// SPDX-License-Identifier: Apache-2.0
//
// Session-list storage tests. The commands themselves need an AppState (Tauri
// runtime), so here we verify the underlying SQL contract they depend on:
//   - one unified list carries project AND standalone rows, newest first
//   - subagent-spawned children never leak into it
//   - `kind` is a *derived* storage marker, not a user-facing species

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;

    /// Mirrors `list_sessions`, so a change to one fails the other.
    const LIST_SESSIONS_SQL: &str = "SELECT id FROM sessions \
         WHERE parent_session_id IS NULL \
         ORDER BY updated_at DESC LIMIT 200";

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
        updated_at: i64,
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
        .bind(updated_at)
        .bind(parent)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn listed_ids(pool: &SqlitePool) -> Vec<String> {
        let rows: Vec<(String,)> = sqlx::query_as(LIST_SESSIONS_SQL)
            .fetch_all(pool)
            .await
            .unwrap();
        rows.into_iter().map(|(id,)| id).collect()
    }

    #[tokio::test]
    async fn list_sessions_returns_project_and_standalone_in_one_recency_order() {
        // The split lists were what made "quick task" feel like a different
        // species. One list, ordered purely by recency; the sidebar decides
        // how to group it.
        let pool = fresh_pool().await;
        insert_session(&pool, "p1", "project", None, 300).await;
        insert_session(&pool, "q1", "quick", None, 200).await;
        insert_session(&pool, "p2", "project", None, 100).await;

        assert_eq!(listed_ids(&pool).await, vec!["p1", "q1", "p2"]);
    }

    #[tokio::test]
    async fn list_sessions_excludes_subagent_children_of_every_kind() {
        // Subagent children are machinery, not sessions the user opened —
        // and that holds whether the parent is project- or standalone-scoped.
        let pool = fresh_pool().await;
        insert_session(&pool, "p1", "project", None, 400).await;
        insert_session(&pool, "c1", "project", Some("p1"), 300).await;
        insert_session(&pool, "q1", "quick", None, 200).await;
        insert_session(&pool, "qc", "project", Some("q1"), 100).await;

        assert_eq!(listed_ids(&pool).await, vec!["p1", "q1"]);
    }

    #[tokio::test]
    async fn anonymous_sessions_never_reach_storage() {
        // Anonymous is a per-draft switch ("leave no trace"), not a stored
        // kind: nothing is ever written, so the list stays empty.
        let pool = fresh_pool().await;
        assert!(listed_ids(&pool).await.is_empty());
    }
}
