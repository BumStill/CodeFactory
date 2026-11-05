// SPDX-License-Identifier: Apache-2.0
#[path = "../src/specialist_observations.rs"]
mod service;
use serde_json::{json, Value};
use service::*;
use sqlx::sqlite::SqlitePoolOptions;

struct Source {
    head: String,
    actor: i64,
}
#[async_trait::async_trait]
impl ReadSource for Source {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        Ok(if path == "/user" {
            json!({"id":self.actor})
        } else if path.contains("/pulls/") {
            json!({"number":1,"state":"open","head":{"sha":self.head,"repo":{"id":7}},"base":{"repo":{"id":7}}})
        } else if path.contains("/actions/runs?") {
            json!({"total_count":1,"workflow_runs":[{"id":11,"run_attempt":1,"workflow_id":12,"head_sha":self.head,"repository":{"id":7},"head_repository":{"id":7},"status":"completed","conclusion":"success"}]})
        } else {
            json!({"id":7,"full_name":"owner/repo","permissions":{"pull":true}})
        })
    }
}
async fn fixture() -> (tempfile::TempDir, sqlx::SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect(&format!(
            "sqlite:{}?mode=rwc",
            dir.path().join("observations.db").display()
        ))
        .await
        .unwrap();
    ensure_schema(&pool).await.unwrap();
    (dir, pool)
}
fn source() -> Source {
    Source {
        head: "a".repeat(40),
        actor: 9,
    }
}
async fn binding(pool: &sqlx::SqlitePool) -> Binding {
    let epoch = project_epoch(pool, "project-a").await.unwrap();
    let snap = snapshot(&source(), "owner/repo", 1).await.unwrap();
    save_binding(pool, "project-a", "remote", epoch, &snap)
        .await
        .unwrap()
}
#[tokio::test]
async fn persists_exact_head_and_deduplicates_on_reopen() {
    let (dir, pool) = fixture().await;
    let b = binding(&pool).await;
    let data = observe(&source(), &b).await.unwrap();
    save_observation(&pool, &b, &data).await.unwrap();
    save_observation(&pool, &b, &data).await.unwrap();
    assert_eq!(
        list(&pool, "project-a").await.unwrap()[0]
            .observations
            .len(),
        1
    );
    pool.close().await;
    let pool = SqlitePoolOptions::new()
        .connect(&format!(
            "sqlite:{}",
            dir.path().join("observations.db").display()
        ))
        .await
        .unwrap();
    let rows = list(&pool, "project-a").await.unwrap();
    assert_eq!(rows[0].binding.head, "a".repeat(40));
    assert_eq!(
        rows[0].observations[0].pm_state,
        "needs_execution_provenance"
    );
    assert_eq!(rows[0].observations[0].runs[0].id, 11);
    assert!(list(&pool, "project-b").await.unwrap().is_empty());
    assert!(load_binding(&pool, "project-b", &b.id).await.is_err());
}
#[tokio::test]
async fn rejects_old_head_account_and_revocation_before_commit() {
    let (_dir, pool) = fixture().await;
    let b = binding(&pool).await;
    assert!(observe(
        &Source {
            head: "b".repeat(40),
            actor: 9
        },
        &b
    )
    .await
    .is_err());
    assert!(observe(
        &Source {
            head: b.head.clone(),
            actor: 10
        },
        &b
    )
    .await
    .is_err());
    let data = observe(&source(), &b).await.unwrap();
    stop(&pool, "project-a").await.unwrap();
    assert!(save_observation(&pool, &b, &data).await.is_err());
    let snap = snapshot(&source(), "owner/repo", 1).await.unwrap();
    assert!(save_binding(&pool, "project-a", "remote", 0, &snap)
        .await
        .is_err());
    assert!(!list(&pool, "project-a").await.unwrap()[0].active);
}
#[tokio::test]
async fn database_failure_cannot_leave_a_completed_pm_observation() {
    let (_dir, pool) = fixture().await;
    let b = binding(&pool).await;
    let data = observe(&source(), &b).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_observation BEFORE INSERT ON specialist_observations BEGIN SELECT RAISE(ABORT,'test failure'); END").execute(&pool).await.unwrap();
    assert!(save_observation(&pool, &b, &data).await.is_err());
    assert!(list(&pool, "project-a").await.unwrap()[0]
        .observations
        .is_empty());
}

struct ChangedSource {
    calls: std::sync::atomic::AtomicUsize,
    mode: &'static str,
}
#[async_trait::async_trait]
impl ReadSource for ChangedSource {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut v = source().get(path).await?;
        if self.mode == "head" && n >= 4 && path.contains("/pulls/") {
            v["head"]["sha"] = json!("b".repeat(40));
        }
        if path.contains("/actions/runs?") {
            match self.mode {
                "truncated" => v["total_count"] = json!(101),
                "wrong_run" => v["workflow_runs"][0]["head_sha"] = json!("b".repeat(40)),
                "empty" => v = json!({"total_count":0,"workflow_runs":[]}),
                "status" => {
                    v["workflow_runs"][0]["status"] = json!("in_progress");
                    v["workflow_runs"][0]["conclusion"] = Value::Null;
                }
                _ => {}
            }
        }
        Ok(v)
    }
}
fn changed(mode: &'static str) -> ChangedSource {
    ChangedSource {
        calls: std::sync::atomic::AtomicUsize::new(0),
        mode,
    }
}
#[tokio::test]
async fn rejects_head_race_partial_pages_and_wrong_run_sources() {
    let (_dir, pool) = fixture().await;
    let b = binding(&pool).await;
    for mode in ["head", "truncated", "wrong_run"] {
        assert!(observe(&changed(mode), &b).await.is_err(), "{mode}");
    }
    assert!(observe(&changed("empty"), &b).await.unwrap().is_empty());
}
#[tokio::test]
async fn concurrent_queries_deduplicate_but_changed_run_gets_new_observation() {
    let (_dir, pool) = fixture().await;
    let b = binding(&pool).await;
    let runs = observe(&source(), &b).await.unwrap();
    let (a, c) = tokio::join!(
        save_observation(&pool, &b, &runs),
        save_observation(&pool, &b, &runs)
    );
    assert_eq!(a.unwrap().id, c.unwrap().id);
    let newer = observe(&changed("status"), &b).await.unwrap();
    save_observation(&pool, &b, &newer).await.unwrap();
    assert_eq!(
        list(&pool, "project-a").await.unwrap()[0]
            .observations
            .len(),
        2
    );
}
#[tokio::test]
async fn changed_credentials_and_explicit_regrant_have_distinct_bindings() {
    let (_dir, pool) = fixture().await;
    let snap = snapshot(&source(), "owner/repo", 1).await.unwrap();
    let epoch = project_epoch(&pool, "project-a").await.unwrap();
    let a = save_binding_with_revision(&pool, "project-a", "remote", epoch, &snap, "rev1")
        .await
        .unwrap();
    let b = save_binding_with_revision(&pool, "project-a", "remote", epoch, &snap, "rev2")
        .await
        .unwrap();
    assert_ne!(a.id, b.id);
    stop(&pool, "project-a").await.unwrap();
    assert!(load_binding(&pool, "project-a", &a.id).await.is_err());
    let epoch = project_epoch(&pool, "project-a").await.unwrap();
    let c = save_binding_with_revision(&pool, "project-a", "remote", epoch, &snap, "rev2")
        .await
        .unwrap();
    assert_ne!(b.id, c.id);
    assert!(load_binding(&pool, "project-a", &c.id).await.is_ok());
}
#[tokio::test]
async fn migration_is_atomic_and_preserves_existing_app_data() {
    let (_dir, pool) = fixture().await;
    sqlx::query("CREATE TABLE existing_user_data(value TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO existing_user_data VALUES('keep')")
        .execute(&pool)
        .await
        .unwrap();
    let (a, b) = tokio::join!(ensure_schema(&pool), ensure_schema(&pool));
    a.unwrap();
    b.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM existing_user_data")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "keep"
    );
    // Force a failure after the first CREATE to prove schema setup rolls back.
    let bad = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE specialist_bindings(wrong_column TEXT)")
        .execute(&bad)
        .await
        .unwrap();
    assert!(ensure_schema(&bad).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE name='specialist_projects'"
        )
        .fetch_one(&bad)
        .await
        .unwrap(),
        0
    );
}
#[test]
fn repository_routes_reject_injection() {
    for name in [
        "../repo",
        "owner/../repo",
        "owner/repo?x=y",
        "https://evil/x",
        "owner/repo#x",
    ] {
        assert!(validate_repo(name).is_err());
    }
}
#[tokio::test]
async fn stop_overflow_is_explicit_and_does_not_corrupt_epoch() {
    let (_dir, pool) = fixture().await;
    project_epoch(&pool, "p").await.unwrap();
    sqlx::query("UPDATE specialist_projects SET epoch=? WHERE project='p'")
        .bind(i64::MAX)
        .execute(&pool)
        .await
        .unwrap();
    assert!(stop(&pool, "p").await.is_err());
    assert_eq!(project_epoch(&pool, "p").await.unwrap(), i64::MAX);
}
#[tokio::test]
async fn schema_rejects_weak_constraints_and_partial_tables() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE specialist_projects(project TEXT PRIMARY KEY NOT NULL, epoch INTEGER NOT NULL)").execute(&pool).await.unwrap();
    assert!(ensure_schema(&pool).await.is_err());
}
#[tokio::test]
async fn captured_real_provider_facts_replay_and_persist_without_claiming_live_transport() {
    struct Captured(Value);
    #[async_trait::async_trait]
    impl ReadSource for Captured {
        async fn get(&self, path: &str) -> anyhow::Result<Value> {
            self.0["responses"]
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("uncaptured route"))
        }
    }
    let captured = Captured(
        serde_json::from_str(include_str!(
            "../../docs/acceptance/specialist-provider-996.json"
        ))
        .unwrap(),
    );
    let (_dir, pool) = fixture().await;
    let snap = snapshot(&captured, "BumStill/CodeFactory", 996)
        .await
        .unwrap();
    assert_eq!(snap.head, "5639560f0603fbb97d50d4805ded6adb603a57eb");
    let epoch = project_epoch(&pool, "captured-project").await.unwrap();
    let b = save_binding_with_revision(
        &pool,
        "captured-project",
        "fixture-only",
        epoch,
        &snap,
        "not-live-auth",
    )
    .await
    .unwrap();
    let runs = observe(&captured, &b).await.unwrap();
    assert_eq!(runs.len(), 5);
    save_observation(&pool, &b, &runs).await.unwrap();
    assert_eq!(
        list(&pool, "captured-project").await.unwrap()[0].observations[0].pm_state,
        "needs_execution_provenance"
    );
}
#[tokio::test]
async fn schema_rejects_unknown_named_triggers_and_marker_only_database() {
    let (_dir, pool) = fixture().await;
    sqlx::query("CREATE TRIGGER unrelated BEFORE INSERT ON specialist_observations BEGIN SELECT RAISE(IGNORE); END").execute(&pool).await.unwrap();
    assert!(ensure_schema(&pool).await.is_err());
    let marker = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE specialist_schema(version INTEGER PRIMARY KEY CHECK(version=1))")
        .execute(&marker)
        .await
        .unwrap();
    assert!(ensure_schema(&marker).await.is_err());
}
#[tokio::test]
async fn schema_rejects_missing_version_and_unknown_index() {
    let (_dir, pool) = fixture().await;
    sqlx::query("DELETE FROM specialist_schema")
        .execute(&pool)
        .await
        .unwrap();
    assert!(ensure_schema(&pool).await.is_err());
    let (_dir, pool) = fixture().await;
    sqlx::query("CREATE INDEX unknown_name ON specialist_bindings(project,epoch)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(ensure_schema(&pool).await.is_err());
}
