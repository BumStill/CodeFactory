// SPDX-License-Identifier: Apache-2.0
// Test-only fixture: real managed workspace, SQLite, and native tool execution.
// No manually inserted observation/receipt can stand in for host provenance.
async fn r2_delivery_fixture_inner(
    foreign_preimage: bool,
    use_bash: bool,
) -> (
    sqlx::SqlitePool,
    tempfile::TempDir,
    std::path::PathBuf,
    String,
) {
    let (backend, temp, source, workspace) = r2_host_workspace_fixture().await;
    if foreign_preimage {
        std::fs::write(
            workspace.worktree_path.join("base.txt"),
            "foreign base content\n",
        )
        .unwrap();
    }
    let operations = if use_bash {
        let command = if cfg!(windows) {
            "Set-Content -Path owned.txt -Value 'owned new content'; Set-Content -Path base.txt -Value 'owned edit'"
        } else {
            "printf 'owned new content\\n' > owned.txt; printf 'owned edit\\n' > base.txt"
        };
        vec![("r2-bash-create-edit", "bash", serde_json::json!({"command":command}))]
    } else { vec![
        (
            "r2-native-create",
            "write_file",
            serde_json::json!({"path":"owned.txt", "content":"owned new content\n"}),
        ),
        (
            "r2-native-edit",
            "edit_file",
            serde_json::json!({"path":"base.txt", "old_string":"base", "new_string":"owned edit"}),
        ),
    ] };
    for (id, name, args) in operations {
        let call = call_with_args(id, name, &args);
        register_tool_call(&backend, &call, &args).await;
        let result = backend
            .execute(&call, &args, &objective_ctx(&workspace.worktree_path))
            .await
            .unwrap();
        println!("compatibility native tool={name} status={:?} error={}", result.status, result.is_error);
        assert_eq!(
            result.status,
            ToolExecutionStatus::Done,
            "real native {name}: {result:?}"
        );
        assert!(!result.is_error, "real native operation failed: {result:?}");
        crate::trajectory::record_tool_call_finished(
            &backend.db,
            TEST_SESSION_ID,
            id,
            "done",
            Some("native acceptance operation completed"),
            None,
            0,
        )
        .await
        .unwrap();
    }
    let applied: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM side_effect_observation_contracts WHERE state='applied' AND observer_kind='file_content_sha256_v1'",
    )
    .fetch_one(&backend.db)
    .await
    .unwrap();
    println!("compatibility use_bash={use_bash} exact_file_applied_receipts={applied}");
    assert_eq!(
        applied, if use_bash { 1 } else { 2 },
        "both proofs must originate in production host execution"
    );
    if use_bash {
        let (kind, state, locator, expected, observed): (String, String, String, String, Option<String>) =
            sqlx::query_as("SELECT observer_kind,state,safe_locator_json,expected_postcondition_digest,observed_digest FROM side_effect_observation_contracts WHERE observer_kind='file_content_sha256_v1' AND state='applied'")
                .fetch_one(&backend.db).await.unwrap();
        assert_eq!(kind, "file_content_sha256_v1");
        assert_eq!(state, "applied");
        assert_eq!(observed.as_deref(), Some(expected.as_str()));
        let locator: serde_json::Value = serde_json::from_str(&locator).unwrap();
        let entries = locator["shell_intent_v1"]["entries"].as_array().expect("real shell manifest");
        let mut paths: Vec<_> = entries.iter().map(|entry| entry["path"].as_str().unwrap()).collect();
        paths.sort();
        assert_eq!(paths, ["base.txt", "owned.txt"]);
    }
    assert_eq!(
        std::fs::read_to_string(source.join("base.txt")).unwrap(),
        "base\n"
    );
    assert!(!source.join("owned.txt").exists());
    (
        backend.db.clone(),
        temp,
        workspace.worktree_path,
        TEST_OBJECTIVE_ID.into(),
    )
}


pub(super) async fn r2_native_delivery_fixture_inner(foreign_preimage: bool) -> (sqlx::SqlitePool, tempfile::TempDir, std::path::PathBuf, String) {
    r2_delivery_fixture_inner(foreign_preimage, false).await
}

pub(super) async fn r2_bash_delivery_fixture_inner() -> (sqlx::SqlitePool, tempfile::TempDir, std::path::PathBuf, String) {
    r2_delivery_fixture_inner(false, true).await
}


async fn r2_host_workspace_fixture() -> (DesktopToolBackend, tempfile::TempDir, std::path::PathBuf, super::super::execution_workspace::ExecutionWorkspace) {
    let temp = tempfile::tempdir().unwrap();
    let mut native_backend = backend().await;
    native_backend.db = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new()
            .filename(temp.path().join("native-host.sqlite"))
            .create_if_missing(true))
        .await.unwrap();
    let backend = objective_backend_from_backend(false, native_backend).await;
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    init_local_git_repo(&source);
    let origin = temp.path().join("origin.git");
    let git = super::super::delivery::git;
    git(
        temp.path(),
        &["init", "--bare", "-q", origin.to_str().unwrap()],
    )
    .unwrap();
    git(
        &source,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    )
    .unwrap();
    git(&source, &["push", "-q", "origin", "main"]).unwrap();
    let workspace = super::super::execution_workspace::allocate_or_attach(
        &backend.db,
        super::super::execution_workspace::ExecutionWorkspaceRequest {
            objective_id: TEST_OBJECTIVE_ID.into(),
            session_id: Some(TEST_SESSION_ID.into()),
            source_cwd: source.clone(),
            workspace_container: temp.path().join("managed"),
            process_instance: "r2-native-acceptance".into(),
        },
    )
    .await
    .unwrap();
    (backend, temp, source, workspace)
}

pub(super) async fn r2_shell_case_fixture_inner(
    preexisting: &[(&str, &str)], commands: &[&str],
) -> (sqlx::SqlitePool, tempfile::TempDir, std::path::PathBuf, String) {
    let (backend, temp, source, workspace) = r2_host_workspace_fixture().await;
    for (path, content) in preexisting {
        let path = workspace.worktree_path.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    for (index, command) in commands.iter().enumerate() {
        let id = format!("r2-shell-case-{index}");
        let args = serde_json::json!({"command":command});
        let call = call_with_args(&id, "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let result = backend.execute(&call, &args, &objective_ctx(&workspace.worktree_path)).await.unwrap();
        println!("real shell case={index} status={:?} error={}", result.status, result.is_error);
        assert_eq!(result.status, ToolExecutionStatus::Done, "{result:?}");
        assert!(!result.is_error, "{result:?}");
        crate::trajectory::record_tool_call_finished(&backend.db, TEST_SESSION_ID, &id,
            "done", Some("real shell acceptance completed"), None, 0).await.unwrap();
    }
    assert_eq!(std::fs::read_to_string(source.join("base.txt")).unwrap(), "base\n");
    (backend.db.clone(), temp, workspace.worktree_path, TEST_OBJECTIVE_ID.into())
}

pub(super) async fn r2_fresh_thin_fixture_inner() -> (sqlx::SqlitePool, tempfile::TempDir, std::path::PathBuf, String) {
    let (backend, temp, workspace) = r2_production_host_workspace(true).await;
    for table in ["objective_bindings", "side_effect_receipts"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&backend.db).await.unwrap();
        assert_eq!(count, 0, "fresh thin must start without legacy {table}");
    }
    let args = serde_json::json!({"path":"owned.txt","content":"fresh thin content\n"});
    let call = call_with_args("r2-fresh-thin-native", "write_file", &args);
    register_tool_call(&backend, &call, &args).await;
    let result = backend.execute(&call, &args, &objective_ctx(&workspace.worktree_path)).await.unwrap();
    assert_eq!(result.status, ToolExecutionStatus::Done, "{result:?}");
    assert!(!result.is_error, "{result:?}");
    crate::trajectory::record_tool_call_finished(&backend.db, TEST_SESSION_ID, &call.id,
        "done", Some("fresh thin native write completed"), None, 0).await.unwrap();
    for table in ["objective_bindings", "side_effect_receipts"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&backend.db).await.unwrap();
        assert_eq!(count, 0, "thin execution must not acquire legacy {table}");
    }
    assert_eq!(std::fs::read_to_string(workspace.worktree_path.join("owned.txt")).unwrap(), "fresh thin content\n");
    (backend.db.clone(), temp, workspace.worktree_path, TEST_OBJECTIVE_ID.into())
}


async fn r2_production_host_workspace(thin: bool) -> (DesktopToolBackend, tempfile::TempDir, super::super::execution_workspace::ExecutionWorkspace) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    init_local_git_repo(&source);
    let origin = temp.path().join("origin.git");
    let git = super::super::delivery::git;
    git(temp.path(), &["init", "--bare", "-q", origin.to_str().unwrap()]).unwrap();
    git(&source, &["remote", "add", "origin", origin.to_str().unwrap()]).unwrap();
    git(&source, &["push", "-q", "origin", "main"]).unwrap();
    let mut executor = backend().await;
    executor.db = crate::storage::db::connect(&format!("sqlite://{}", temp.path().join("native-host.sqlite").display())).await.unwrap();
    let db = &executor.db;
    sqlx::query("INSERT INTO sessions (id,title,cwd,model_id,created_at,updated_at,delivery_mode,permission_mode)
        VALUES (?,'Production-schema acceptance',?,'synthetic-no-provider',1,1,?,'trusted')")
        .bind(TEST_SESSION_ID).bind(source.to_str().unwrap()).bind(if thin {"thin"} else {"legacy"})
        .execute(db).await.unwrap();
    for (id, role) in [(TEST_ROOT_TURN_ID, "user"), ("assistant-message", "assistant")] {
        sqlx::query("INSERT INTO messages (id,session_id,role,content,created_at) VALUES (?,?,?,'synthetic',1)")
            .bind(id).bind(TEST_SESSION_ID).bind(role).execute(db).await.unwrap();
    }
    sqlx::query("INSERT INTO objectives (id,revision,kind,session_id,root_turn_id,status,decision_type,
        domain,autonomous_completion,requested_acceptance,requires_user_action,recovery_owner,created_surface,created_at,updated_at)
        VALUES (?,1,'local_mutation',?,?,'active','continue','tool',1,'validated_change',0,'objective-supervisor','test',1,1)")
        .bind(TEST_OBJECTIVE_ID).bind(TEST_SESSION_ID).bind(TEST_ROOT_TURN_ID).execute(db).await.unwrap();
    if !thin {
        sqlx::query("INSERT INTO objective_bindings (id,objective_id,domain,resource_kind,resource_id,resource_generation,identity_digest,created_at,updated_at)
            VALUES (?,?,'chat','chat_root_turn',?,1,'sha256:test-binding',1,1)")
            .bind(TEST_BINDING_ID).bind(TEST_OBJECTIVE_ID).bind(TEST_ROOT_TURN_ID).execute(db).await.unwrap();
    }
    sqlx::query("INSERT INTO chat_turn_state (root_turn_id,session_id,objective_id,revision,phase,status,started_at,updated_at)
        VALUES (?,?,?,1,'running','running',1,1)")
        .bind(TEST_ROOT_TURN_ID).bind(TEST_SESSION_ID).bind(TEST_OBJECTIVE_ID).execute(db).await.unwrap();
    sqlx::query("INSERT INTO chat_run_controls (run_instance_id,session_id,root_turn_id,objective_id,objective_revision,status,created_process_instance,created_at,updated_at)
        VALUES ('r2-production-run',?,?,?,1,'active','r2-production-fixture',1,1)")
        .bind(TEST_SESSION_ID).bind(TEST_ROOT_TURN_ID).bind(TEST_OBJECTIVE_ID).execute(db).await.unwrap();
    let workspace = super::super::execution_workspace::allocate_or_attach(db,
        super::super::execution_workspace::ExecutionWorkspaceRequest {
            objective_id: TEST_OBJECTIVE_ID.into(), session_id: Some(TEST_SESSION_ID.into()),
            source_cwd: source, workspace_container: temp.path().join("managed"),
            process_instance: "r2-production-fixture".into(),
        }).await.unwrap();
    sqlx::query("UPDATE sessions SET cwd=? WHERE id=?").bind(workspace.worktree_path.to_str().unwrap())
        .bind(TEST_SESSION_ID).execute(db).await.unwrap();
    (executor, temp, workspace)
}

pub(super) async fn r2_production_native_fixture_inner() -> (sqlx::SqlitePool, tempfile::TempDir, std::path::PathBuf, String) {
    let (backend, temp, workspace) = r2_production_host_workspace(false).await;
    for (id, name, args) in [
        ("r2-production-create", "write_file", serde_json::json!({"path":"owned.txt", "content":"owned new content\n"})),
        ("r2-production-edit", "edit_file", serde_json::json!({"path":"base.txt", "old_string":"base", "new_string":"owned edit"})),
    ] {
        let call = call_with_args(id, name, &args);
        register_tool_call(&backend, &call, &args).await;
        let result = backend.execute(&call, &args, &objective_ctx(&workspace.worktree_path)).await.unwrap();
        assert_eq!(result.status, ToolExecutionStatus::Done, "{result:?}");
        assert!(!result.is_error, "{result:?}");
        crate::trajectory::record_tool_call_finished(&backend.db, TEST_SESSION_ID, id,
            "done", Some("production-schema native operation completed"), None, 0).await.unwrap();
    }
    (backend.db.clone(), temp, workspace.worktree_path, TEST_OBJECTIVE_ID.into())
}

#[cfg(unix)]
pub(super) async fn r2_fresh_thin_shell_fixture_inner(
    preexisting: &[(&str, &str)], commands: &[&str],
) -> (sqlx::SqlitePool, tempfile::TempDir, std::path::PathBuf, String) {
    let (executor, temp, workspace) = r2_production_host_workspace(true).await;
    for (name, content) in preexisting {
        let path = workspace.worktree_path.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    for table in ["objective_bindings", "side_effect_receipts"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&executor.db).await.unwrap();
        assert_eq!(count, 0, "fresh thin starts with no {table}");
    }
    for (index, command) in commands.iter().enumerate() {
        let id = format!("fresh-thin-shell-{index}");
        let args = serde_json::json!({"command": command});
        let call = call_with_args(&id, "bash", &args);
        register_tool_call(&executor, &call, &args).await;
        let result = executor.execute(&call, &args, &objective_ctx(&workspace.worktree_path)).await.unwrap();
        assert_eq!(result.status, ToolExecutionStatus::Done, "{result:?}");
        assert!(!result.is_error, "{result:?}");
        crate::trajectory::record_tool_call_finished(&executor.db, TEST_SESSION_ID, &id,
            "done", Some("fresh thin actual bash completed"), None, 0).await.unwrap();
        for table in ["objective_bindings", "side_effect_receipts"] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&executor.db).await.unwrap();
            assert_eq!(count, 0, "actual thin bash must not mint legacy {table}");
        }
    }
    (executor.db.clone(), temp, workspace.worktree_path, TEST_OBJECTIVE_ID.into())
}
