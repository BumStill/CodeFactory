// SPDX-License-Identifier: Apache-2.0
// Included inside delivery::tests to reuse the established local Git/forge fixture.
// This invokes production deliver(), not an extracted or rewritten planner.

async fn r2_assert_unproven_delivery_is_side_effect_free(case: &str) {
    let root = feature_branch_repo(&format!("r2-scope-{case}"));
    let origin = root.parent().unwrap().join("origin.git");
    assert_eq!(
        git(&root, &["remote", "get-url", "origin"]).unwrap(),
        origin.to_string_lossy()
    );
    std::fs::write(
        root.join("app.rs"),
        "fn main() { println!(\"task edit\"); }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("t1.py"),
        "# unrelated user file\nsecret = 'synthetic only'\n",
    )
    .unwrap();
    if case == "staged" {
        std::fs::create_dir_all(root.join(".codefactory")).unwrap();
        std::fs::write(
            root.join(".codefactory/unrelated.txt"),
            "synthetic private staging\n",
        )
        .unwrap();
        git(&root, &["add", "--", ".codefactory/unrelated.txt"]).unwrap();
    }
    if case == "deleted" {
        std::fs::remove_file(root.join("app.rs")).unwrap();
    }
    if case == "renamed" {
        git(&root, &["mv", "app.rs", "renamed.rs"]).unwrap();
    }
    let index_path = git2::Repository::open(&root)
        .unwrap()
        .index()
        .unwrap()
        .path()
        .unwrap()
        .to_owned();
    let before_index = std::fs::read(&index_path).unwrap();
    let before_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    let before_source = worktree_source_manifest_digest(&root).unwrap();
    let before_remote = git(&origin, &["show-ref"]).unwrap();
    let calls = stub_calls();
    let remote = StubRemote {
        ci: CiStatus::Success,
        existing_pr: None,
        merge_ok: true,
        merge_queues: true,
        caps: every_capability(),
        calls: calls.clone(),
    };
    let outcome = deliver(
        &root,
        DeliveryCeiling::PrOnly,
        MergeMethod::Squash,
        1,
        &DeliverOpts {
            title: Some("fix: verify task-owned content".into()),
            ..Default::default()
        },
        Some(&remote),
        Some("main"),
    )
    .await;
    println!(
        "R2 production acceptance case={case} final={} stage={} summary={} fixture={}",
        outcome.final_state,
        outcome.stage,
        outcome.summary,
        root.display()
    );
    assert_eq!(
        outcome.final_state, "blocked",
        "unproven task ownership must stop production delivery: {:?}",
        outcome.steps
    );
    assert_eq!(
        git(&root, &["rev-parse", "HEAD"]).unwrap(),
        before_head,
        "no local commit"
    );
    assert_eq!(
        std::fs::read(&index_path).unwrap(),
        before_index,
        "preserve exact user index bytes"
    );
    assert_eq!(
        worktree_source_manifest_digest(&root).unwrap(),
        before_source,
        "preserve source including unknown paths"
    );
    assert_eq!(
        git(&origin, &["show-ref"]).unwrap(),
        before_remote,
        "no local fake-remote push"
    );
    for count in [
        &calls.open_pr,
        &calls.update_pr_body,
        &calls.merge,
        &calls.release,
        &calls.queue_arms,
    ] {
        assert_eq!(count.load(Ordering::SeqCst), 0, "no forge mutation");
    }
    assert_eq!(
        std::fs::read_to_string(root.join("t1.py")).unwrap(),
        "# unrelated user file\nsecret = 'synthetic only'\n"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[tokio::test]
async fn r2_scope_production_rejects_unrelated_untracked_without_side_effects() {
    r2_assert_unproven_delivery_is_side_effect_free("untracked").await;
}

#[tokio::test]
async fn r2_scope_production_preserves_foreign_staged_bytes() {
    r2_assert_unproven_delivery_is_side_effect_free("staged").await;
}

#[tokio::test]
async fn r2_scope_production_rejects_unproven_tracked_deletion() {
    r2_assert_unproven_delivery_is_side_effect_free("deleted").await;
}

#[tokio::test]
async fn r2_scope_production_rejects_unproven_rename() {
    r2_assert_unproven_delivery_is_side_effect_free("renamed").await;
}

#[tokio::test]
async fn r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin() {
    let (db, temp, root, objective_id) =
        crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
    let origin = temp.path().join("origin.git");
    let scope = TaskDeliveryScope::load(&db, &objective_id, &root, &[])
        .await
        .unwrap();
    let calls = stub_calls();
    let remote = StubRemote {
        ci: CiStatus::Success,
        existing_pr: None,
        merge_ok: true,
        merge_queues: true,
        caps: every_capability(),
        calls: calls.clone(),
    };
    let result = deliver(
        &root,
        DeliveryCeiling::PrOnly,
        MergeMethod::Squash,
        1,
        &DeliverOpts {
            title: Some("fix: preserve task ownership".into()),
            task_scope: Some(scope),
            ..Default::default()
        },
        Some(&remote),
        Some("main"),
    )
    .await;
    assert_eq!(result.final_state, "delivered", "{:?}", result.steps);
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    let branch = git(&root, &["branch", "--show-current"]).unwrap();
    assert_eq!(
        git(&origin, &["rev-parse", &format!("refs/heads/{branch}")]).unwrap(),
        head
    );
    assert_eq!(
        git(&origin, &["ls-tree", "-r", "--name-only", &head]).unwrap(),
        "base.txt\nowned.txt"
    );
    assert_eq!(
        git(&origin, &["show", &format!("{head}:base.txt")]).unwrap(),
        "owned edit"
    );
    assert_eq!(
        git(&origin, &["show", &format!("{head}:owned.txt")]).unwrap(),
        "owned new content"
    );
    assert!(git(&root, &["status", "--porcelain"]).unwrap().is_empty());
    assert_eq!(calls.open_pr.load(Ordering::SeqCst), 1);
    assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
    assert_eq!(calls.release.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn r2_scope_native_proof_rejects_later_unrelated_file_without_mutations() {
    let (db, temp, root, objective_id) =
        crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
    let scope = TaskDeliveryScope::load(&db, &objective_id, &root, &[])
        .await
        .unwrap();
    std::fs::write(root.join("t1.py"), "# unrelated concurrent content\n").unwrap();
    let index_path = git2::Repository::open(&root)
        .unwrap()
        .index()
        .unwrap()
        .path()
        .unwrap()
        .to_owned();
    let index = std::fs::read(&index_path).unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    let source = worktree_source_manifest_digest(&root).unwrap();
    let origin = temp.path().join("origin.git");
    let refs = git(&origin, &["show-ref"]).unwrap();
    let calls = stub_calls();
    let remote = StubRemote {
        ci: CiStatus::Success,
        existing_pr: None,
        merge_ok: true,
        merge_queues: true,
        caps: every_capability(),
        calls: calls.clone(),
    };
    let result = deliver(
        &root,
        DeliveryCeiling::PrOnly,
        MergeMethod::Squash,
        1,
        &DeliverOpts {
            title: Some("fix: preserve task ownership".into()),
            task_scope: Some(scope),
            ..Default::default()
        },
        Some(&remote),
        Some("main"),
    )
    .await;
    assert_eq!(result.final_state, "blocked", "{:?}", result.steps);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(std::fs::read(index_path).unwrap(), index);
    assert_eq!(worktree_source_manifest_digest(&root).unwrap(), source);
    assert_eq!(git(&origin, &["show-ref"]).unwrap(), refs);
    assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn r2_scope_native_receipt_cannot_adopt_foreign_preimage() {
    let (db, _temp, root, objective_id) =
        crate::agent::tool_backend::r2_native_delivery_fixture(true).await;
    // The real edit applied to existing foreign text, but its before digest
    // cannot be reached from this Objective's trusted Git baseline.
    assert_eq!(
        std::fs::read_to_string(root.join("base.txt")).unwrap(),
        "foreign owned edit content\n"
    );
    let before = worktree_source_manifest_digest(&root).unwrap();
    let err = TaskDeliveryScope::load(&db, &objective_id, &root, &[])
        .await
        .unwrap_err();
    assert!(err.contains("base.txt"), "{err}");
    assert_eq!(worktree_source_manifest_digest(&root).unwrap(), before);
}

#[tokio::test]
async fn r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content() {
    let (db, temp, root, objective_id) =
        crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
    std::fs::create_dir_all(root.join(".codefactory")).unwrap();
    std::fs::write(root.join(".codefactory/foreign.txt"), "foreign staging\n").unwrap();
    git(&root, &["add", "--", ".codefactory/foreign.txt"]).unwrap();
    let index_path = git2::Repository::open(&root)
        .unwrap()
        .index()
        .unwrap()
        .path()
        .unwrap()
        .to_owned();
    let index = std::fs::read(&index_path).unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    let refs = git(&temp.path().join("origin.git"), &["show-ref"]).unwrap();
    assert!(TaskDeliveryScope::load(&db, &objective_id, &root, &[])
        .await
        .is_err());
    assert_eq!(std::fs::read(index_path).unwrap(), index);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(
        git(&temp.path().join("origin.git"), &["show-ref"]).unwrap(),
        refs
    );
}

#[tokio::test]
async fn r2_scope_thin_proven_dirty_tree_preserves_index_before_refusal() {
    let (db, temp, root, objective_id) =
        crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
    let scope = TaskDeliveryScope::load(&db, &objective_id, &root, &[])
        .await
        .unwrap();
    let index_path = git2::Repository::open(&root)
        .unwrap()
        .index()
        .unwrap()
        .path()
        .unwrap()
        .to_owned();
    let index = std::fs::read(&index_path).unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    let refs = git(&temp.path().join("origin.git"), &["show-ref"]).unwrap();
    let source = worktree_source_manifest_digest(&root).unwrap();
    let error = crate::tools::delivery::task_scope_thin_stage_fixture(&root, &scope).unwrap_err();
    assert!(error.contains("thin"), "{error}");
    assert_eq!(std::fs::read(index_path).unwrap(), index);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(
        git(&temp.path().join("origin.git"), &["show-ref"]).unwrap(),
        refs
    );
    assert_eq!(worktree_source_manifest_digest(&root).unwrap(), source);
}

#[tokio::test]
async fn r2_scope_thin_proven_clean_commit_pushes_only_fixed_oid() {
    let (db, temp, root, objective_id) =
        crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
    // A real fixture commit retains the native write/edit provenance; this does
    // not ask the production thin adapter to bypass normal commit hooks.
    git(&root, &["add", "--", "base.txt", "owned.txt"]).unwrap();
    git(&root, &["commit", "-qm", "fix: native receipted content"]).unwrap();
    let scope = TaskDeliveryScope::load(&db, &objective_id, &root, &[])
        .await
        .unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    let branch = git(&root, &["branch", "--show-current"]).unwrap();
    let origin = temp.path().join("origin.git");
    let refs = git(&origin, &["show-ref"]).unwrap();
    let index_path = git2::Repository::open(&root)
        .unwrap()
        .index()
        .unwrap()
        .path()
        .unwrap()
        .to_owned();
    let index = std::fs::read(&index_path).unwrap();
    assert!(
        crate::tools::delivery::task_scope_thin_stage_fixture(&root, &scope)
            .unwrap()
            .is_empty()
    );
    assert_eq!(std::fs::read(index_path).unwrap(), index);
    let mutable = format!("HEAD:refs/heads/{branch}");
    assert!(crate::tools::delivery::task_scope_thin_native_fixture(
        &root,
        &scope,
        &["push", "origin", &mutable]
    )
    .is_err());
    assert_eq!(git(&origin, &["show-ref"]).unwrap(), refs);
    let wrong_target = format!("{head}:refs/heads/unrelated");
    assert!(crate::tools::delivery::task_scope_thin_native_fixture(
        &root,
        &scope,
        &["push", "origin", &wrong_target]
    )
    .is_err());
    assert_eq!(git(&origin, &["show-ref"]).unwrap(), refs);
    let fixed = format!("{head}:refs/heads/{branch}");
    crate::tools::delivery::task_scope_thin_native_fixture(
        &root,
        &scope,
        &["push", "origin", &fixed],
    )
    .unwrap();
    assert_eq!(
        git(&origin, &["rev-parse", &format!("refs/heads/{branch}")]).unwrap(),
        head
    );
    assert_eq!(
        git(&origin, &["ls-tree", "-r", "--name-only", &head]).unwrap(),
        "base.txt\nowned.txt"
    );
}

#[tokio::test]
async fn r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch() {
    let (db, temp, root, objective_id) =
        crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
    let database = temp.path().join("native-host.sqlite");
    assert!(database.is_file(), "native host database must exist at {}", database.display());
    db.close().await;
    let reopened = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&database)
            .create_if_missing(false))
        .await
        .unwrap();
    let persisted_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM side_effect_observation_contracts WHERE objective_id=? AND state='applied'",
    ).bind(&objective_id).fetch_one(&reopened).await.unwrap();
    assert_eq!(persisted_receipts, 2, "reopen must retain both real host receipts");
    // Real persisted SQLite and a fresh connection, not a new OS process.
    let scope = TaskDeliveryScope::load(&reopened, &objective_id, &root, &[])
        .await
        .unwrap();
    scope.verify(&root).unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    git(&root, &["checkout", "-qb", "unrelated-same-head"]).unwrap();
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert!(scope.verify(&root).unwrap_err().contains("branch"));
    reopened.close().await;
}

#[tokio::test]
async fn r2_scope_new_resource_generation_under_another_objective_rejects_old_proof() {
    let (db, _temp, root, objective_id) =
        crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
    TaskDeliveryScope::load(&db, &objective_id, &root, &[])
        .await
        .unwrap();
    sqlx::query("INSERT INTO objectives (id,kind,status,decision_type,domain,requested_acceptance,created_at,updated_at)
        VALUES ('replacement-objective','local_mutation','active','continue','tool','validated_change',1,1)")
        .execute(&db).await.unwrap();
    // Only rebind the resource used by the actual host-produced receipts. The
    // workspace and old receipt bytes stay unchanged; a different Objective now
    // owns a newer resource generation.
    let inserted = sqlx::query("INSERT INTO objective_bindings
        (id,objective_id,domain,resource_kind,resource_id,resource_generation,identity_digest,created_at,updated_at)
        SELECT 'replacement:' || id,'replacement-objective',domain,resource_kind,resource_id,
            resource_generation+1,'sha256:replacement',2,2
        FROM objective_bindings WHERE id IN
            (SELECT binding_id FROM side_effect_observation_contracts WHERE objective_id=?)")
        .bind(&objective_id).execute(&db).await.unwrap();
    assert!(inserted.rows_affected() > 0);
    let before = worktree_source_manifest_digest(&root).unwrap();
    assert!(TaskDeliveryScope::load(&db, &objective_id, &root, &[])
        .await
        .is_err());
    assert_eq!(worktree_source_manifest_digest(&root).unwrap(), before);
}

async fn r2_compatibility_expect_delivery(
    db: &sqlx::SqlitePool, root: &std::path::Path, origin: &std::path::Path, objective_id: &str,
) {
    let scope = TaskDeliveryScope::load(db, objective_id, root, &[]).await
        .expect("legitimate task-owned writes should acquire delivery proof; compatibility gap must stay red");
    let calls = stub_calls();
    let remote = StubRemote { ci: CiStatus::Success, existing_pr: None, merge_ok: true,
        merge_queues: true, caps: every_capability(), calls: calls.clone() };
    let result = deliver(root, DeliveryCeiling::PrOnly, MergeMethod::Squash, 1,
        &DeliverOpts { title: Some("fix: preserve normal task delivery".into()), task_scope: Some(scope), ..Default::default() },
        Some(&remote), Some("main")).await;
    assert_eq!(result.final_state, "delivered", "legitimate task delivery must succeed: {:?}", result.steps);
    let head = git(root, &["rev-parse", "HEAD"]).unwrap();
    let branch = git(root, &["branch", "--show-current"]).unwrap();
    assert_eq!(git(origin, &["rev-parse", &format!("refs/heads/{branch}")]).unwrap(), head);
    assert_eq!(git(origin, &["ls-tree", "-r", "--name-only", &head]).unwrap(), "base.txt\nowned.txt");
    assert_eq!(git(origin, &["show", &format!("{head}:base.txt")]).unwrap(), "owned edit");
    assert_eq!(git(origin, &["show", &format!("{head}:owned.txt")]).unwrap(), "owned new content");
    assert_eq!(calls.open_pr.load(Ordering::SeqCst), 1);
    assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
    assert_eq!(calls.release.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn r2_compat_native_task_prestaged_should_deliver() {
    let (db, temp, root, objective_id) = crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
    git(&root, &["add", "--", "owned.txt"]).unwrap();
    assert_eq!(git(&root, &["diff", "--cached", "--name-only"]).unwrap(), "owned.txt");
    r2_compatibility_expect_delivery(&db, &root, &temp.path().join("origin.git"), &objective_id).await;
}

#[cfg(unix)]
#[tokio::test]
async fn r2_compat_real_bash_task_writes_should_deliver() {
    let (db, temp, root, objective_id) = crate::agent::tool_backend::r2_bash_delivery_fixture().await;
    assert_eq!(std::fs::read_to_string(root.join("owned.txt")).unwrap().trim_end(), "owned new content");
    assert_eq!(std::fs::read_to_string(root.join("base.txt")).unwrap().trim_end(), "owned edit");
    r2_compatibility_expect_delivery(&db, &root, &temp.path().join("origin.git"), &objective_id).await;
}

#[cfg(unix)]
async fn r2_shell_expect_exact_delivery(db: &sqlx::SqlitePool, root: &std::path::Path,
    origin: &std::path::Path, objective: &str, expected: &[(&str, &str)]) {
    let scope = TaskDeliveryScope::load(db, objective, root, &[]).await.expect("supported real shell provenance");
    let calls = stub_calls();
    let remote = StubRemote { ci: CiStatus::Success, existing_pr: None, merge_ok: true,
        merge_queues: true, caps: every_capability(), calls: calls.clone() };
    let outcome = deliver(root, DeliveryCeiling::PrOnly, MergeMethod::Squash, 1,
        &DeliverOpts { title: Some("fix: support proven shell task edits".into()), task_scope: Some(scope), ..Default::default() },
        Some(&remote), Some("main")).await;
    assert_eq!(outcome.final_state, "delivered", "{:?}", outcome.steps);
    let head = git(root, &["rev-parse", "HEAD"]).unwrap();
    let branch = git(root, &["branch", "--show-current"]).unwrap();
    assert_eq!(git(origin, &["rev-parse", &format!("refs/heads/{branch}")]).unwrap(), head);
    let mut paths: Vec<_> = expected.iter().map(|(path, _)| *path).collect(); paths.sort();
    assert_eq!(git(origin, &["ls-tree", "-r", "--name-only", &head]).unwrap(), paths.join("\n"));
    for (path, content) in expected {
        assert_eq!(git(origin, &["show", &format!("{head}:{path}")]).unwrap(), content.trim_end());
    }
    assert_eq!(calls.open_pr.load(Ordering::SeqCst), 1);
    assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_create_modify_append_and_prestage_deliver() {
    let (db, temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(&[], &[
        "printf 'first\\n' > owned.txt && printf 'second\\n' >> owned.txt",
        "printf 'tail\\n' >> base.txt",
    ]).await;
    git(&root, &["add", "--", "owned.txt"]).unwrap();
    r2_shell_expect_exact_delivery(&db, &root, &temp.path().join("origin.git"), &objective,
        &[("base.txt", "base\ntail\n"), ("owned.txt", "first\nsecond\n")]).await;
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_copy_move_delete_deliver_exact_result() {
    let (db, temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(&[], &[
        "cp base.txt copied.txt", "mv copied.txt moved.txt", "rm base.txt",
    ]).await;
    assert!(!root.join("base.txt").exists()); assert!(!root.join("copied.txt").exists());
    r2_shell_expect_exact_delivery(&db, &root, &temp.path().join("origin.git"), &objective,
        &[("moved.txt", "base\n")]).await;
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_cat_and_echo_have_real_read_dependency_proof() {
    let (db, temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(&[], &[
        "cat base.txt > copied.txt && echo extra >> copied.txt",
    ]).await;
    r2_shell_expect_exact_delivery(&db, &root, &temp.path().join("origin.git"), &objective,
        &[("base.txt", "base\n"), ("copied.txt", "base\nextra\n")]).await;
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_foreign_staging_is_preserved_and_not_adopted() {
    let (db, temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(
        &[("foreign.txt", "private fixture\n")], &["printf 'owned\\n' > owned.txt"]).await;
    git(&root, &["add", "--", "foreign.txt"]).unwrap();
    let index_path = git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap().to_owned();
    let index = std::fs::read(&index_path).unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    let refs = git(&temp.path().join("origin.git"), &["show-ref"]).unwrap();
    assert!(TaskDeliveryScope::load(&db, &objective, &root, &[]).await.is_err());
    assert_eq!(std::fs::read(index_path).unwrap(), index);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(git(&temp.path().join("origin.git"), &["show-ref"]).unwrap(), refs);
    assert_eq!(std::fs::read_to_string(root.join("foreign.txt")).unwrap(), "private fixture\n");
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_foreign_preimage_cannot_be_adopted_by_overwrite() {
    let (db, _temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(
        &[("base.txt", "foreign preimage\n")], &["printf 'task result\\n' > base.txt"]).await;
    assert_eq!(std::fs::read_to_string(root.join("base.txt")).unwrap(), "task result\n");
    assert!(TaskDeliveryScope::load(&db, &objective, &root, &[]).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_foreign_excluded_read_dependency_cannot_be_laundered() {
    for command in ["cp .codefactory/user.txt owned.txt", "cat .codefactory/user.txt > owned.txt"] {
        let (db, _temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(
            &[(".codefactory/user.txt", "foreign read dependency\n")], &[command]).await;
        assert_eq!(std::fs::read_to_string(root.join("owned.txt")).unwrap(), "foreign read dependency\n");
        assert!(TaskDeliveryScope::load(&db, &objective, &root, &[]).await.is_err(),
            "excluding the foreign source must not authenticate its derived output: {command}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_rename_cannot_adopt_foreign_destination() {
    let (db, _temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(
        &[("foreign.txt", "foreign destination\n")], &["mv base.txt foreign.txt"]).await;
    assert!(!root.join("base.txt").exists());
    assert_eq!(std::fs::read_to_string(root.join("foreign.txt")).unwrap(), "base\n");
    assert!(TaskDeliveryScope::load(&db, &objective, &root, &[]).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_mixed_unknown_dynamic_effect_has_no_delivery_proof() {
    let (db, _temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(&[], &[
        "printf 'known\\n' > owned.txt",
        "f=unknown.txt; printf 'dynamic\\n' > \"$f\"",
    ]).await;
    assert_eq!(std::fs::read_to_string(root.join("unknown.txt")).unwrap(), "dynamic\n");
    assert!(TaskDeliveryScope::load(&db, &objective, &root, &[]).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn r2_shell_reopen_and_late_foreign_write_invalidates_proof() {
    let (db, temp, root, objective) = crate::agent::tool_backend::r2_shell_case_fixture(&[], &[
        "printf 'owned\\n' > owned.txt", "printf 'tail\\n' >> base.txt",
    ]).await;
    db.close().await;
    let reopened = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(temp.path().join("native-host.sqlite"))
            .create_if_missing(false)).await.unwrap();
    let scope = TaskDeliveryScope::load(&reopened, &objective, &root, &[]).await.unwrap();
    scope.verify(&root).unwrap();
    std::fs::write(root.join("owned.txt"), "late foreign write\n").unwrap();
    assert!(scope.verify(&root).is_err());
    assert!(TaskDeliveryScope::load(&reopened, &objective, &root, &[]).await.is_err());
    reopened.close().await;
}

#[tokio::test]
async fn r2_fresh_thin_native_write_without_legacy_binding_should_have_proof() {
    let (db, _temp, root, objective) = crate::agent::tool_backend::r2_fresh_thin_fixture().await;
    let scope = TaskDeliveryScope::load(&db, &objective, &root, &[]).await
        .expect("fresh thin actual writes need their own content provenance, not legacy receipts");
    scope.verify(&root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn r2_fresh_thin_first_bash_writes_have_proof_and_exact_delivery() {
    let (db, temp, root, objective) = crate::agent::tool_backend::r2_fresh_thin_shell_fixture(&[], &[
        "printf 'owned new content\\n' > owned.txt", "printf 'owned edit\\n' > base.txt",
    ]).await;
    let (intents, applied): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM workspace_content_intents), (SELECT COUNT(*) FROM workspace_content_results WHERE state='applied' AND actual_success=1)")
        .fetch_one(&db).await.unwrap();
    assert_eq!((intents, applied), (2, 2));
    // Real thin writes and journal -> production scope -> real local Git/bare
    // origin. The forge remains synthetic; this is not a Tauri/provider test.
    r2_shell_expect_exact_delivery(&db, &root, &temp.path().join("origin.git"), &objective,
        &[("base.txt", "owned edit\n"), ("owned.txt", "owned new content\n")]).await;
}

#[cfg(unix)]
#[tokio::test]
async fn r2_fresh_thin_repeated_cp_uses_distinct_dispatch_and_changed_inputs() {
    let (db, temp, root, objective) = crate::agent::tool_backend::r2_fresh_thin_shell_fixture(&[], &[
        "printf 'first\\n' > owned.txt", "cp owned.txt copied.txt",
        "printf 'second\\n' > owned.txt", "cp owned.txt copied.txt",
    ]).await;
    let (intents, applied): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(DISTINCT dispatch_id) FROM workspace_content_intents), (SELECT COUNT(*) FROM workspace_content_results WHERE state='applied' AND actual_success=1)")
        .fetch_one(&db).await.unwrap();
    assert_eq!((intents, applied), (4, 4), "every actual dispatch must retain independent evidence");
    let (copies, manifests): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),COUNT(DISTINCT i.manifest_json) FROM workspace_content_intents i JOIN tool_calls t ON t.id=i.tool_call_id WHERE t.arguments=?")
        .bind(serde_json::json!({"command":"cp owned.txt copied.txt"}).to_string())
        .fetch_one(&db).await.unwrap();
    assert_eq!((copies, manifests), (2, 2), "same command's changed inputs cannot overwrite or replay old evidence");
    assert_eq!(std::fs::read_to_string(root.join("copied.txt")).unwrap(), "second\n");
    r2_shell_expect_exact_delivery(&db, &root, &temp.path().join("origin.git"), &objective,
        &[("base.txt", "base\n"), ("copied.txt", "second\n"), ("owned.txt", "second\n")]).await;
}

#[cfg(unix)]
#[tokio::test]
async fn r2_fresh_thin_journal_cannot_launder_foreign_excluded_read() {
    let (db, temp, root, objective) = crate::agent::tool_backend::r2_fresh_thin_shell_fixture(
        &[(".codefactory/foreign.txt", "foreign input\n")], &["cp .codefactory/foreign.txt owned.txt"]).await;
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_content_results WHERE state='applied'")
        .fetch_one(&db).await.unwrap();
    assert_eq!(applied, 1, "actual execution evidence alone is not task-input ownership");
    let index_path = git2::Repository::open(&root).unwrap().index().unwrap().path().unwrap().to_owned();
    let index = std::fs::read(&index_path).unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
    let refs = git(&temp.path().join("origin.git"), &["show-ref"]).unwrap();
    assert!(TaskDeliveryScope::load(&db, &objective, &root, &[]).await.is_err());
    assert_eq!(std::fs::read(index_path).unwrap(), index);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(git(&temp.path().join("origin.git"), &["show-ref"]).unwrap(), refs);
    assert_eq!(std::fs::read_to_string(root.join(".codefactory/foreign.txt")).unwrap(), "foreign input\n");
}
