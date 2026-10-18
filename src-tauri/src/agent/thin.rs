// SPDX-License-Identifier: Apache-2.0
//! R1: old rows remain legacy; new sessions explicitly select thin.
pub(crate) async fn is_thin(db: &sqlx::SqlitePool, session: &str) -> bool {
    sqlx::query_scalar::<_, String>("SELECT delivery_mode FROM sessions WHERE id=?")
        .bind(session).fetch_optional(db).await.ok().flatten().as_deref() == Some("thin")
}

pub(crate) fn external_pending(pr: &serde_json::Value) -> bool {
    pr["state"] == "OPEN" || pr["statusCheckRollup"].as_array().is_some_and(|checks| checks.iter().any(|check| {
        check["status"].as_str().is_some_and(|status| status != "COMPLETED")
            || check["state"].as_str().is_some_and(|state| matches!(state, "PENDING" | "EXPECTED"))
    }))
}

/// Observe GitHub once, never persist a second source of delivery truth.
pub(crate) async fn external_wait_hint(db: &sqlx::SqlitePool, session: &str) -> Option<String> {
    let reference: Option<(String, i64)> = sqlx::query_as("SELECT branch,pr_number FROM thin_delivery_refs WHERE session_id=?")
        .bind(session).fetch_optional(db).await.ok().flatten();
    let Some((_branch, number)) = reference else { return None; };
    let cwd: String = sqlx::query_scalar("SELECT cwd FROM sessions WHERE id=?")
        .bind(session).fetch_one(db).await.ok()?;
    let raw = tokio::task::spawn_blocking(move || {
        super::delivery::bounded_delivery_output(super::delivery::dev_command("gh")
            .current_dir(cwd).args(["pr", "view", &number.to_string(), "--json", "state,statusCheckRollup"]))
    }).await;
    tracing::info!(session_id=session, gh_calls=1, step="completion_observe", "thin_delivery");
    let pending = match raw {
        Ok(Ok(raw)) => serde_json::from_str::<serde_json::Value>(&raw).map(|pr| external_pending(&pr)).unwrap_or(true),
        _ => true,
    };
    super::background_tasks::external_wait_completion_hint(db, session, pending).await
}

pub(crate) fn resume_prompt(at: i64, summary: &str) -> String {
    let time = chrono::DateTime::from_timestamp_millis(at).map(|time| time.with_timezone(&chrono::Local).to_rfc3339()).unwrap_or_else(|| at.to_string());
    format!("你在 {time} 被中断，最后一步是 {summary}，可能已执行也可能未执行。先用 git status、git log、gh pr view 等确认现状，再决定下一步。")
}

#[cfg(test)]
mod tests {
    #[test]
    fn resumed_bash_always_has_system_prompt_for_success_failure_unknown() {
        for status in ["done", "error", "pending"] {
            let prompt = super::resume_prompt(0, &format!("bash ({status}): synthetic-command"));
            assert!(prompt.contains("可能已执行也可能未执行"));
            assert!(prompt.contains("git status、git log、gh pr view"));
            assert!(prompt.contains(status));
        }
    }
    #[test]
    fn external_wait_tracks_forge_not_mapping_existence() {
        assert!(!super::external_pending(&serde_json::json!({"state":"MERGED","statusCheckRollup":[]})));
        assert!(!super::external_pending(&serde_json::json!({"state":"CLOSED","statusCheckRollup":[]})));
        assert!(super::external_pending(&serde_json::json!({"state":"OPEN","statusCheckRollup":[]})));
        assert!(super::external_pending(&serde_json::json!({"state":"MERGED","statusCheckRollup":[{"status":"IN_PROGRESS"}]})));
    }
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
