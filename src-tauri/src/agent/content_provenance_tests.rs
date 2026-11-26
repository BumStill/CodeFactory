// SPDX-License-Identifier: Apache-2.0
// Split production prepare → actual native executor → finish to inject races.
// These tests are observer-boundary tests, not a complete UI/permission run.
async fn r2_content_fixture() -> (
    DesktopToolBackend,
    tempfile::TempDir,
    std::path::PathBuf,
    ToolCtx,
) {
    let (db, temp, root, _) = r2_fresh_thin_fixture_inner().await;
    let mut host = backend().await;
    host.db = db;
    let ctx = objective_ctx(&root);
    (host, temp, root, ctx)
}
async fn r2_content_prepare(
    host: &DesktopToolBackend,
    ctx: &ToolCtx,
    id: &str,
    name: &str,
    args: &serde_json::Value,
) -> (content_provenance::Prepared, String, ToolCall) {
    let call = call_with_args(id, name, args);
    register_tool_call(host, &call, args).await;
    let prepared = content_provenance::prepare(&host.db, &call, args, ctx)
        .await
        .unwrap()
        .expect("actual active foreground scope");
    let dispatch: String = sqlx::query_scalar(
        "SELECT dispatch_id FROM workspace_content_intents WHERE tool_call_id=?",
    )
    .bind(crate::trajectory::trace_record_id(TEST_SESSION_ID, id))
    .fetch_one(&host.db)
    .await
    .unwrap();
    (prepared, dispatch, call)
}
async fn r2_content_execute(
    host: &DesktopToolBackend,
    ctx: &ToolCtx,
    call: &ToolCall,
    args: &serde_json::Value,
) -> ToolInvocationResult {
    let mut exec = crate::tools::ExecCtx::new(ctx.working_directory.clone(), Some(host.db.clone()));
    exec.session_id = ctx.session_id.clone();
    exec.root_turn_id = ctx.root_turn_id.clone();
    let output = crate::tools::dispatch(&call.function.name, args.clone(), &exec)
        .await
        .unwrap();
    let (command, kind) = host.classify(call, args);
    invocation_from_output(output, command, kind)
}
async fn r2_content_result_state(db: &sqlx::SqlitePool, dispatch: &str) -> String {
    sqlx::query_scalar("SELECT state FROM workspace_content_results WHERE dispatch_id=?")
        .bind(dispatch)
        .fetch_one(db)
        .await
        .unwrap()
}
#[tokio::test]
async fn r2_content_epoch_aba_invalidates_inflight_but_preserves_completed_history() {
    let (host, _temp, root, ctx) = r2_content_fixture().await;
    let args = serde_json::json!({"path":"race.txt","content":"planned"});
    let (prepared, dispatch, call) =
        r2_content_prepare(&host, &ctx, "epoch-race", "write_file", &args).await;
    sqlx::query("UPDATE sessions SET permission_mode='standard' WHERE id=?")
        .bind(TEST_SESSION_ID)
        .execute(&host.db)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET permission_mode='trusted' WHERE id=?")
        .bind(TEST_SESSION_ID)
        .execute(&host.db)
        .await
        .unwrap();
    let epoch: i64 =
        sqlx::query_scalar("SELECT epoch FROM session_content_epochs WHERE session_id=?")
            .bind(TEST_SESSION_ID)
            .fetch_one(&host.db)
            .await
            .unwrap();
    assert_eq!(epoch, 2, "mode ABA must be monotonic, not timestamps");
    let actual = r2_content_execute(&host, &ctx, &call, &args).await;
    assert!(!actual.is_error);
    content_provenance::finish(&host.db, prepared, &actual, &ctx)
        .await
        .unwrap();
    assert_eq!(
        r2_content_result_state(&host.db, &dispatch).await,
        "unknown"
    );
    let workspace = super::super::execution_workspace::verify_objective_workspace(
        &host.db,
        TEST_OBJECTIVE_ID,
        &root,
    )
    .await
    .unwrap();
    assert_eq!(
        content_provenance::completed(&host.db, &workspace)
            .await
            .unwrap()
            .len(),
        1,
        "completed historical native write remains owned after later mode changes"
    );
    assert!(super::super::delivery::TaskDeliveryScope::load(
        &host.db,
        TEST_OBJECTIVE_ID,
        &root,
        &[]
    )
    .await
    .is_err());
}
#[tokio::test]
async fn r2_content_stop_and_replacement_run_cannot_settle_old_dispatch() {
    for cancellation in ["stop", "replacement", "internal_fence"] {
        let (host, _temp, _root, ctx) = r2_content_fixture().await;
        let args = serde_json::json!({"path":"late.txt","content":"planned"});
        let (prepared, dispatch, call) =
            r2_content_prepare(&host, &ctx, "run-race", "write_file", &args).await;
        if cancellation == "replacement" {
            sqlx::query(
                "UPDATE chat_run_controls SET status='completed',settled_at=2 WHERE session_id=?",
            )
            .bind(TEST_SESSION_ID)
            .execute(&host.db)
            .await
            .unwrap();
            sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,objective_id,objective_revision,status,created_process_instance,created_at,updated_at) VALUES ('replacement',?,?,?,1,'active','replacement-test',2,2)")
                .bind(TEST_SESSION_ID).bind(TEST_ROOT_TURN_ID).bind(TEST_OBJECTIVE_ID).execute(&host.db).await.unwrap();
        } else if cancellation == "internal_fence" {
            // Internal stop fences this active run without cancelling the Objective.
            sqlx::query("UPDATE chat_run_controls SET cancel_requested_at=2 WHERE session_id=?")
                .bind(TEST_SESSION_ID)
                .execute(&host.db)
                .await
                .unwrap();
            let status: String =
                sqlx::query_scalar("SELECT status FROM chat_run_controls WHERE session_id=?")
                    .bind(TEST_SESSION_ID)
                    .fetch_one(&host.db)
                    .await
                    .unwrap();
            assert_eq!(status, "active");
            let objective_status: String =
                sqlx::query_scalar("SELECT status FROM objectives WHERE id=?")
                    .bind(TEST_OBJECTIVE_ID)
                    .fetch_one(&host.db)
                    .await
                    .unwrap();
            assert_eq!(objective_status, "active");
        } else {
            sqlx::query("UPDATE chat_run_controls SET status='cancel_requested',cancel_requested_at=2 WHERE session_id=?").bind(TEST_SESSION_ID).execute(&host.db).await.unwrap();
        }
        let actual = r2_content_execute(&host, &ctx, &call, &args).await;
        assert!(!actual.is_error);
        content_provenance::finish(&host.db, prepared, &actual, &ctx)
            .await
            .unwrap();
        assert_eq!(
            r2_content_result_state(&host.db, &dispatch).await,
            "unknown",
            "cancellation={cancellation}"
        );
    }
}
#[tokio::test]
async fn r2_content_crash_after_real_effect_has_no_completed_proof() {
    let (host, _temp, root, ctx) = r2_content_fixture().await;
    let args = serde_json::json!({"path":"pending.txt","content":"real effect"});
    let (_prepared, dispatch, call) =
        r2_content_prepare(&host, &ctx, "pending-effect", "write_file", &args).await;
    let actual = r2_content_execute(&host, &ctx, &call, &args).await;
    assert!(!actual.is_error);
    assert_eq!(
        std::fs::read_to_string(root.join("pending.txt")).unwrap(),
        "real effect"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_content_results WHERE dispatch_id=?")
            .bind(dispatch)
            .fetch_one(&host.db)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "a crash must not synthesize terminal content evidence"
    );
    assert!(super::super::delivery::TaskDeliveryScope::load(
        &host.db,
        TEST_OBJECTIVE_ID,
        &root,
        &[]
    )
    .await
    .is_err());
}
#[cfg(unix)]
#[tokio::test]
async fn r2_content_failed_real_shell_cannot_be_applied() {
    let (host, _temp, root, ctx) = r2_content_fixture().await;
    let args = serde_json::json!({"command":"printf '%s' planned > output"});
    let (prepared, dispatch, call) =
        r2_content_prepare(&host, &ctx, "failed-command", "bash", &args).await;
    std::fs::create_dir(root.join("output")).unwrap();
    let actual = r2_content_execute(&host, &ctx, &call, &args).await;
    assert!(
        actual.is_error,
        "actual redirection into directory must fail"
    );
    content_provenance::finish(&host.db, prepared, &actual, &ctx)
        .await
        .unwrap();
    assert_eq!(
        r2_content_result_state(&host.db, &dispatch).await,
        "unknown"
    );
}
#[tokio::test]
async fn r2_content_schema_is_idempotent_and_wrong_root_or_trigger_cannot_mint() {
    let (host, _temp, _root, ctx) = r2_content_fixture().await;
    content_provenance::ensure(&host.db).await.unwrap();
    content_provenance::ensure(&host.db).await.unwrap();
    assert!(
        sqlx::query("UPDATE workspace_content_intents SET expected_digest='changed'")
            .execute(&host.db)
            .await
            .is_err(),
        "intent history is immutable"
    );
    assert!(
        sqlx::query("UPDATE workspace_content_results SET state='unknown'")
            .execute(&host.db)
            .await
            .is_err(),
        "observations are immutable"
    );
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_content_intents")
        .fetch_one(&host.db)
        .await
        .unwrap();
    let args = serde_json::json!({"path":"wrong.txt","content":"unproven"});
    let call = call_with_args("wrong-root", "write_file", &args);
    register_tool_call(&host, &call, &args).await;
    let mut wrong = ctx.clone();
    wrong.root_turn_id = Some("unbound-root".into());
    assert!(content_provenance::prepare(&host.db, &call, &args, &wrong)
        .await
        .is_err());
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_content_intents")
        .fetch_one(&host.db)
        .await
        .unwrap();
    assert_eq!(before, after);
    sqlx::raw_sql("DROP TRIGGER trg_session_content_permission_epoch; CREATE TRIGGER trg_session_content_permission_epoch AFTER UPDATE OF permission_mode ON sessions BEGIN SELECT 1; END;").execute(&host.db).await.unwrap();
    assert!(
        content_provenance::ensure(&host.db).await.is_err(),
        "replaced ABA trigger cannot silently pass schema verification"
    );
}

#[tokio::test]
async fn r2_content_previous_schema_migrates_and_reopens_without_adopting_history() {
    let (host, temp, _workspace) = r2_production_host_workspace(true).await;
    // Remove only this migration's objects/metadata in a synthetic database:
    // this is the production v26 schema presented to the real migrator.
    sqlx::raw_sql(
        "DROP TABLE workspace_content_results; DROP TABLE workspace_content_intents;
        DROP TRIGGER trg_session_content_permission_epoch; DROP TABLE session_content_epochs;
        DELETE FROM _sqlx_migrations WHERE version=27;",
    )
    .execute(&host.db)
    .await
    .unwrap();
    host.db.close().await;
    let url = format!(
        "sqlite://{}",
        temp.path().join("native-host.sqlite").display()
    );
    for _ in 0..2 {
        let db = crate::storage::db::connect(&url).await.unwrap();
        content_provenance::ensure(&db).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_content_intents")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "schema upgrade cannot invent ownership of existing files"
        );
        let check: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(check, "ok");
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&db)
            .await
            .unwrap()
            .is_empty());
        db.close().await;
    }
}
