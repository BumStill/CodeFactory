// SPDX-License-Identifier: Apache-2.0
//! R1: old rows remain legacy; new sessions explicitly select thin.
pub(crate) async fn is_thin(db: &sqlx::SqlitePool, session: &str) -> bool {
    sqlx::query_scalar::<_, String>("SELECT delivery_mode FROM sessions WHERE id=?")
        .bind(session).fetch_optional(db).await.ok().flatten().as_deref() == Some("thin")
}

pub(crate) fn resume_prompt(at: i64, summary: &str) -> String {
    let time = chrono::DateTime::from_timestamp_millis(at).map(|time| time.with_timezone(&chrono::Local).to_rfc3339()).unwrap_or_else(|| at.to_string());
    format!("你在 {time} 被中断，最后一步是 {summary}，可能已执行也可能未执行。先用 git status、git log、gh pr view 等确认现状，再决定下一步。")
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn migration_keeps_old_rows_legacy_and_accepts_explicit_thin() {
        let db = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE sessions (id TEXT PRIMARY KEY); INSERT INTO sessions VALUES ('old');")
            .execute(&db).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/0022_thin_delivery.sql"))
            .execute(&db).await.unwrap();
        assert!(!super::is_thin(&db, "old").await);
        sqlx::query("INSERT INTO sessions (id,delivery_mode) VALUES ('new','thin')").execute(&db).await.unwrap();
        assert!(super::is_thin(&db, "new").await);
        assert!(!super::is_thin(&db, "missing").await);
        assert!(sqlx::query("INSERT INTO sessions (id,delivery_mode) VALUES ('bad','other')").execute(&db).await.is_err());
    }
}
