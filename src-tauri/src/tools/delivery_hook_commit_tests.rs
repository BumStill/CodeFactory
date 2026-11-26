// Included in delivery_thin::tests; real native provenance, SQLite and Git.
#[cfg(unix)]
async fn hook_failure_fixture(script: &str) -> (sqlx::SqlitePool, tempfile::TempDir, std::path::PathBuf, String, delivery::TaskDeliveryScope, Vec<u8>, String) {
    use std::os::unix::fs::PermissionsExt;
    let (db, temp, root, objective) = delivery::r2_production_native_fixture().await;
    let common = delivery::git(&root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).unwrap();
    let path = std::path::PathBuf::from(common).join("hooks/pre-commit");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, script.replace("SOURCE_ROOT", &format!("'{}'", root.to_string_lossy().replace('\'', "'\"'\"'")))).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let scope = delivery::TaskDeliveryScope::load(&db, &objective, &root, &[]).await.unwrap();
    let repo = git2::Repository::open(&root).unwrap();
    let index = std::fs::read(repo.index().unwrap().path().unwrap()).unwrap();
    let head = delivery::git(&root, &["rev-parse", "HEAD"]).unwrap();
    (db, temp, root, objective, scope, index, head)
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_hook_rejection_preserves_source_index_and_records_unknown() {
    let (db, _temp, root, objective, scope, index, head) = hook_failure_fixture("#!/bin/sh\nexit 7\n").await;
    let error = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap_err();
    assert!(error.to_string().contains("retained hook operation"), "{error}");
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(std::fs::read(git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap()).unwrap(), index);
    assert_eq!(std::fs::read_to_string(root.join("base.txt")).unwrap(), "owned edit\n");
    let statuses: Vec<String> = sqlx::query_scalar("SELECT status FROM delivery_mutation_intents WHERE rung='git_hook_commit_prepare'").fetch_all(&db).await.unwrap();
    assert_eq!(statuses, vec!["unknown"]);
    let local: i64 = sqlx::query_scalar("SELECT count(*) FROM delivery_mutation_intents WHERE rung='git_local_commit'").fetch_one(&db).await.unwrap();
    assert_eq!(local, 0);
    // An adversarial later row must not hide the genuinely dispatched unknown
    // operation. It is deliberately not a valid success proof.
    sqlx::query("INSERT INTO delivery_mutation_intents (intent_id,run_id,claim_epoch,rung,operation_key,status,process_instance,evidence_json,started_at,updated_at) SELECT intent_id||'-later',run_id,claim_epoch,rung,operation_key||'-later','committed',process_instance,evidence_json,started_at+1,updated_at+1 FROM delivery_mutation_intents WHERE rung='git_hook_commit_prepare'")
        .execute(&db).await.unwrap();
    let recovery = super::super::delivery::task_scope_thin_recover_fixture(&db).await.unwrap_err();
    assert!(recovery.to_string().contains("hook completion is unknown"), "{recovery}");
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    let authorized: i64 = sqlx::query_scalar("SELECT next_action_authorized FROM delivery_runs WHERE run_kind='thin_local_commit'").fetch_one(&db).await.unwrap();
    assert_eq!(authorized, 0, "unknown hooks cannot be dispatched by recovery");
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_two_local_generations_keep_one_business_objective() {
    let (db, _temp, root, objective, scope, _, _) = hook_failure_fixture("#!/bin/sh\nexit 0\n").await;
    let first = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap();
    delivery::r2_native_write_existing_fixture(&db, &root, "second-owned.txt", "second generation\n").await;
    let second_scope = delivery::TaskDeliveryScope::load(&db, &objective, &root, &[]).await.unwrap();
    let second = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &second_scope).await.unwrap();
    assert_ne!(first, second);
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD^"]).unwrap(), first);
    assert_eq!(delivery::git(&root, &["show", "HEAD:second-owned.txt"]).unwrap(), "second generation");
    let states: Vec<(String, String)> = sqlx::query_as("SELECT id, status FROM delivery_runs WHERE run_kind='thin_local_commit'").fetch_all(&db).await.unwrap();
    assert_eq!(states.len(), 2);
    assert!(states.iter().all(|(_, status)| status == "completed"));
    let objective_state: (String, i64, Option<String>) = sqlx::query_as("SELECT status, revision, delivery_run_id FROM objectives WHERE id=?").bind(&objective).fetch_one(&db).await.unwrap();
    assert_eq!(objective_state, ("active".into(), 1, None));
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_oversized_hook_fails_before_hook_dispatch() {
    let script = format!("#!/bin/sh\n#{}\n", "x".repeat(1024 * 1024));
    let (db, _temp, root, objective, scope, index, head) = hook_failure_fixture(&script).await;
    let error = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap_err();
    assert!(error.to_string().contains("1 MiB"), "{error}");
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(std::fs::read(git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap()).unwrap(), index);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_mutation_intents").fetch_one(&db).await.unwrap();
    assert_eq!(count, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_wrong_branch_preflight_never_runs_hooks_or_commits() {
    let (db, _temp, root, objective, scope, index, head) = hook_failure_fixture("#!/bin/sh\nprintf unexpected > \"$(git rev-parse --git-common-dir)/unexpected-hook\"\n").await;
    // Empty is intentionally included: the durable parser trims it away, while
    // thin's exact branch guard has always rejected it.
    for expected in ["wrong-branch", ""] {
        let error = super::super::delivery::task_scope_thin_commit_args_fixture(&db, &objective, &root, &scope,
            &json!({"title":"fix: exact native task content", "expect_branch":expected})).await.unwrap_err();
        assert!(error.to_string().contains("expect_branch"), "{error}");
    }
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(std::fs::read(git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap()).unwrap(), index);
    let common = delivery::git(&root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).unwrap();
    assert!(!std::path::PathBuf::from(common).join("unexpected-hook").exists());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM delivery_mutation_intents").fetch_one(&db).await.unwrap();
    assert_eq!(count, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_post_checkout_failure_preserves_source_and_stops() {
    use std::os::unix::fs::PermissionsExt;
    let (db, _temp, root, objective, scope, index, head) = hook_failure_fixture("#!/bin/sh\nexit 0\n").await;
    let common = std::path::PathBuf::from(delivery::git(&root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).unwrap());
    let path = common.join("hooks/post-checkout");
    std::fs::write(&path, "#!/bin/sh\nprintf checkout-ran > \"$(git rev-parse --git-common-dir)/checkout-ran\"\nexit 9\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap_err();
    assert!(error.to_string().contains("hook preparation may have created"), "{error}");
    assert!(common.join("checkout-ran").exists());
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(std::fs::read(git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap()).unwrap(), index);
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_parent_checkout_hook_must_match_before_worktree_add() {
    let (db, _temp, root, objective, _, index, head) = hook_failure_fixture("#!/bin/sh\nexit 0\n").await;
    delivery::r2_native_write_existing_fixture(&db, &root, ".githooks/post-checkout", "#!/bin/sh\nexit 0\n").await;
    // Fixture configuration only: production code never changes hooksPath.
    delivery::git(&root, &["config", "core.hooksPath", ".githooks"]).unwrap();
    let scope = delivery::TaskDeliveryScope::load(&db, &objective, &root, &[]).await.unwrap();
    let before = delivery::git(&root, &["worktree", "list", "--porcelain"]).unwrap();
    let error = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap_err();
    assert!(error.to_string().contains("parent-tree post-checkout differs"), "{error}");
    assert_eq!(delivery::git(&root, &["worktree", "list", "--porcelain"]).unwrap(), before);
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(std::fs::read(git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap()).unwrap(), index);
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_fresh_lane_content_commits_without_legacy_binding() {
    let (db, _temp, root, objective) = delivery::r2_fresh_thin_fixture().await;
    let scope = delivery::TaskDeliveryScope::load(&db, &objective, &root, &[]).await.unwrap();
    let parent = delivery::git(&root, &["rev-parse", "HEAD"]).unwrap();
    let head = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap();
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD^"]).unwrap(), parent);
    assert_eq!(delivery::git(&root, &["show", "HEAD:owned.txt"]).unwrap(), "fresh thin content");
    for table in ["objective_bindings", "side_effect_receipts", "objective_remediations"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}")).fetch_one(&db).await.unwrap();
        assert_eq!(count, 0, "local thin delivery must not manufacture legacy {table}");
    }
    let state: (String, Option<String>) = sqlx::query_as("SELECT status,delivery_run_id FROM objectives WHERE id=?").bind(&objective).fetch_one(&db).await.unwrap();
    assert_eq!(state, ("active".into(), None));
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_hooks_path_spaces_are_part_of_the_exact_identity() {
    use std::os::unix::fs::PermissionsExt;
    let (db, temp, root, objective, _, _, _) = hook_failure_fixture("#!/bin/sh\nexit 0\n").await;
    let hooks = temp.path().join(" hooks ");
    std::fs::create_dir(&hooks).unwrap();
    let path = hooks.join("pre-commit");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    delivery::git(&root, &["config", "core.hooksPath", hooks.to_str().unwrap()]).unwrap();
    let scope = delivery::TaskDeliveryScope::load(&db, &objective, &root, &[]).await.unwrap();
    let plan = delivery::HookCommitPlan::new(&root, &scope, "fix: exact hooks path").unwrap();
    std::fs::write(&path, "#!/bin/sh\nexit 11\n").unwrap();
    assert!(delivery::verify_hook_policy(&root, &plan.hook_policy_digest).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_hook_container_symlink_cannot_redirect_checkout() {
    use std::os::unix::fs::symlink;
    let (db, temp, root, objective, scope, index, head) = hook_failure_fixture("#!/bin/sh\nexit 0\n").await;
    let outside = temp.path().join("unrelated-directory");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), "keep").unwrap();
    let common = std::path::PathBuf::from(delivery::git(&root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).unwrap());
    symlink(&outside, common.join("codefactory-hook-worktrees")).unwrap();
    let error = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap_err();
    assert!(error.to_string().contains("not a plain owned directory"), "{error}");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    assert_eq!(std::fs::read_to_string(outside.join("keep.txt")).unwrap(), "keep");
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(std::fs::read(git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap()).unwrap(), index);
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_hook_tree_rewrite_is_not_implicitly_adopted() {
    let (db, _temp, root, objective, scope, index, head) = hook_failure_fixture("#!/bin/sh\nset -eu\nprintf foreign > owned.txt\ngit add owned.txt\n").await;
    let error = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap_err();
    assert!(error.to_string().contains("hooks changed"), "{error}");
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(std::fs::read(git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap()).unwrap(), index);
    assert_eq!(std::fs::read_to_string(root.join("owned.txt")).unwrap(), "owned new content\n");
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_hook_concurrent_user_index_is_never_restored_over() {
    let (db, _temp, root, objective, scope, _index, head) = hook_failure_fixture("#!/bin/sh\nset -eu\nblob=$(printf foreign | git hash-object -w --stdin)\nenv -u GIT_INDEX_FILE -u GIT_DIR -u GIT_WORK_TREE git -C SOURCE_ROOT update-index --add --cacheinfo 100644,$blob,foreign.txt\n").await;
    let error = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap_err();
    assert!(error.to_string().contains("index changed") || error.to_string().contains("source changed"), "{error}");
    assert_eq!(delivery::git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(delivery::git(&root, &["show", ":foreign.txt"]).unwrap(), "foreign");
    assert!(!root.join("foreign.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_stop_after_commit_blocks_bare_push_before_dispatch() {
    let (db, temp, root, objective, scope, _, _) = hook_failure_fixture("#!/bin/sh\nexit 0\n").await;
    let installed = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap();
    let session: String = sqlx::query_scalar("SELECT session_id FROM objectives WHERE id=?").bind(&objective).fetch_one(&db).await.unwrap();
    crate::agent::stop_fence::record_stop(&db, &session, "r2 stop after local commit", None).await.unwrap();
    let origin = temp.path().join("origin.git");
    let before = delivery::git(&origin, &["show-ref"]).unwrap();
    let gate = super::super::delivery::thin_stop_for_identity(db.clone(), session, objective, None);
    let runtime = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || {
        let refspec = format!("{installed}:refs/heads/{}", scope.proven_branch());
        let mut local = Local { root: &root, scope: Some(&scope), staged: true, installed_head: Some(installed), stop: Some((runtime, gate)) };
        local.git(&["push", "origin", &refspec])
    }).await.unwrap();
    assert_eq!(delivery::git(&origin, &["show-ref"]).unwrap(), before, "Stop must prevent the actual bare-origin write");
    assert!(outcome.unwrap_err().contains("STOPPED"));
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_cancel_intent_and_exact_objective_gate_forge_writes_not_reads() {
    let (db, _temp, root, objective, scope, _, _) = hook_failure_fixture("#!/bin/sh\nexit 0\n").await;
    let installed = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective, &root, &scope).await.unwrap();
    let (session, turn): (String, String) = sqlx::query_as("SELECT session_id, root_turn_id FROM objectives WHERE id=?").bind(&objective).fetch_one(&db).await.unwrap();
    let gate = super::super::delivery::thin_stop_for_identity(db.clone(), session.clone(), objective.clone(), Some(turn.clone()));
    assert!(gate.stop_reason().await.is_none());
    // The Stop click's first durable write precedes delivery_stop_fences.
    sqlx::query("INSERT INTO chat_session_cancel_intents(session_id,status,requested_at,updated_at) VALUES (?,'requested',1,1)").bind(&session).execute(&db).await.unwrap();
    assert!(gate.stop_reason().await.unwrap().contains("cancellation requested"));
    let fence_count: i64 = sqlx::query_scalar("SELECT count(*) FROM delivery_stop_fences").fetch_one(&db).await.unwrap();
    assert_eq!(fence_count, 0);
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let local = Local { root: &root, scope: Some(&scope), staged: true, installed_head: Some(installed), stop: Some((runtime, gate)) };
        let calls = std::sync::atomic::AtomicUsize::new(0);
        for args in [vec!["pr", "create"], vec!["pr", "edit"], vec!["pr", "merge"], vec!["workflow", "run"]] {
            let result = local.dispatch_forge(&args, || { calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst); Ok("unexpected write".into()) });
            assert!(result.unwrap_err().contains("DELIVERY_STOPPED"));
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(local.dispatch_forge(&["pr", "view"], || Ok("read-only diagnostic".into())).unwrap(), "read-only diagnostic");
    }).await.unwrap();
    // A settled old request is not a blanket ban on a legitimate live task.
    sqlx::query("UPDATE chat_session_cancel_intents SET status='settled',settled_at=2 WHERE session_id=?").bind(&session).execute(&db).await.unwrap();
    let gate = super::super::delivery::thin_stop_for_identity(db.clone(), session.clone(), objective.clone(), Some(turn.clone()));
    assert!(gate.stop_reason().await.is_none());
    sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,status,created_process_instance,cancel_requested_at,settled_at,created_at,updated_at) VALUES ('old-cancel',?,'different-old-root','cancelled','test',1,2,1,2)").bind(&session).execute(&db).await.unwrap();
    assert!(gate.stop_reason().await.is_none(), "historical root must not cancel the new one");
    let foreground = super::super::delivery::thin_foreground_stop_for_identity(db.clone(), session.clone(), objective.clone(), Some(turn.clone())).await;
    assert!(foreground.stop_reason().await.is_none());
    sqlx::query("UPDATE chat_run_controls SET status='cancel_requested',cancel_requested_at=3 WHERE session_id=? AND status='active'").bind(&session).execute(&db).await.unwrap();
    assert!(foreground.stop_reason().await.unwrap().contains("chat run cancelled"));
    sqlx::query("UPDATE chat_run_controls SET status='cancelled',settled_at=4 WHERE session_id=? AND status='cancel_requested'").bind(&session).execute(&db).await.unwrap();
    sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,objective_id,objective_revision,status,created_process_instance,created_at,updated_at) VALUES ('same-root-resumed',?,?,?,1,'active','test',5,5)").bind(&session).bind(&turn).bind(&objective).execute(&db).await.unwrap();
    let resumed = super::super::delivery::thin_foreground_stop_for_identity(db.clone(), session.clone(), objective.clone(), Some(turn.clone())).await;
    assert!(resumed.stop_reason().await.is_none(), "new gate binds the explicitly admitted replacement run");
    assert!(foreground.stop_reason().await.is_some(), "old worker cannot borrow the new run's authority");
    sqlx::query("UPDATE chat_run_controls SET cancel_requested_at=6 WHERE run_instance_id='same-root-resumed'").execute(&db).await.unwrap();
    assert!(resumed.stop_reason().await.is_some(), "internal active run fence must stop this worker without cancelling the Objective");
    assert!(gate.stop_reason().await.is_none(), "local receipt reconciliation is not abandoned by a historical internal run fence");
    crate::agent::objective::ObjectiveStore::new(db.clone()).cancel_chat_exact(&session, &turn, &objective).await.unwrap();
    assert!(gate.stop_reason().await.unwrap().contains("Objective cancelled"));
    db.close().await;
    assert!(gate.stop_reason().await.is_some(), "unreadable cancellation state must refuse mutations");
}

#[cfg(unix)]
#[tokio::test]
async fn r2_thin_shared_gate_cannot_rebind_between_local_and_remote_phases() {
    let (db, temp, root, objective, scope, _, _) = hook_failure_fixture("#!/bin/sh\nexit 0\n").await;
    let (session, turn): (String, String) = sqlx::query_as("SELECT session_id,root_turn_id FROM objectives WHERE id=?").bind(&objective).fetch_one(&db).await.unwrap();
    let gate = super::super::delivery::thin_foreground_stop_for_identity(db.clone(), session.clone(), objective.clone(), Some(turn.clone())).await;
    assert!(gate.stop_reason().await.is_none());
    let installed = super::super::delivery::task_scope_thin_commit_bound_fixture(&db, &objective, &root, &scope, &json!({"title":"fix: bound invocation"}), Some(gate.clone())).await.unwrap();
    // A new explicit foreground run starts after the original invocation has
    // completed its local phase. The original Arc must still identify A.
    sqlx::query("UPDATE chat_run_controls SET status='cancelled',cancel_requested_at=1,settled_at=2 WHERE session_id=? AND status='active'").bind(&session).execute(&db).await.unwrap();
    sqlx::query("INSERT INTO chat_run_controls(run_instance_id,session_id,root_turn_id,objective_id,objective_revision,status,created_process_instance,created_at,updated_at) VALUES ('phase-replacement',?,?,?,1,'active','test',3,3)").bind(&session).bind(&turn).bind(&objective).execute(&db).await.unwrap();
    let new_gate = super::super::delivery::thin_foreground_stop_for_identity(db.clone(), session, objective, Some(turn)).await;
    assert!(new_gate.stop_reason().await.is_none());
    assert!(gate.stop_reason().await.is_some());
    let origin = temp.path().join("origin.git");
    let before = delivery::git(&origin, &["show-ref"]).unwrap();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let refspec = format!("{installed}:refs/heads/{}", scope.proven_branch());
        let mut local = Local { root: &root, scope: Some(&scope), staged: true, installed_head: Some(installed), stop: Some((runtime, gate)) };
        assert!(local.git(&["push", "origin", &refspec]).unwrap_err().contains("DELIVERY_STOPPED"));
        let calls = std::sync::atomic::AtomicUsize::new(0);
        assert!(local.dispatch_forge(&["pr", "create"], || { calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst); Ok("unexpected".into()) }).unwrap_err().contains("DELIVERY_STOPPED"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }).await.unwrap();
    assert_eq!(delivery::git(&origin, &["show-ref"]).unwrap(), before);
}
