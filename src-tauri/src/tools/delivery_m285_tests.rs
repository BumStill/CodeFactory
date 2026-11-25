// SPDX-License-Identifier: Apache-2.0
//! M285 regression tests colocated with the private delivery modules.

use crate::config::settings::DeliveryCeiling;
use super::delivery_m285_policy::{
    effective_delivery_ceiling, existing_pr_merge_refusal, preserve_existing_pr_content,
};

#[test]
fn effective_ceiling_is_the_lower_authorized_and_requested_value() {
    assert_eq!(effective_delivery_ceiling(DeliveryCeiling::ThroughRelease, Some(DeliveryCeiling::PrOnly)), DeliveryCeiling::PrOnly);
    assert_eq!(effective_delivery_ceiling(DeliveryCeiling::PrOnly, Some(DeliveryCeiling::ThroughRelease)), DeliveryCeiling::PrOnly);
}

#[test]
fn draft_and_removed_arm_are_readable_revocations() {
    let draft = existing_pr_merge_refusal(true, true, false, DeliveryCeiling::ThroughMerge)
        .expect("draft PR must refuse merge authorization");
    assert!(draft.contains("草稿") && draft.contains("撤回"), "{draft}");
    let removed = existing_pr_merge_refusal(false, true, false, DeliveryCeiling::ThroughMerge)
        .expect("removed arm must refuse after restart");
    assert!(removed.contains("摘掉") && removed.contains("明确授权"), "{removed}");
    assert!(existing_pr_merge_refusal(false, false, false, DeliveryCeiling::PrOnly).is_none());
}

#[test]
fn recovery_preserves_remote_title_body_and_requirement_table() {
    let remote_title = "fix: maintainer title";
    let remote_body = "Summary\n\nR | 是否完成 | 证据\nR1 | 是 | synthetic";
    let (title, body) = preserve_existing_pr_content(remote_title, remote_body);
    assert_eq!(title, remote_title);
    assert_eq!(body, remote_body);
    assert!(body.contains("R | 是否完成 | 证据"));
}

// ---------------- M285b integration (fake gh + sqlite hitting deliver_changes) ----------------
#[cfg(test)]
mod integration {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn write_executable(path: &std::path::Path, content: &str) {
        fs::write(path, content).unwrap();
        #[cfg(unix)] {
            let mut perm = fs::metadata(path).unwrap().permissions();
            perm.set_mode(0o755);
            fs::set_permissions(path, perm).unwrap();
        }
    }

    fn fake_gh_script(state_path: &std::path::Path) -> String {
        // Implement a tiny state without jq: three files alongside $STATE base.
        // $STATE.number (default 101), $STATE.armed (true/false), $STATE.draft (true/false)
        format!(r#"#!/bin/sh
set -eu
BASE="{state}"
NUM_FILE="$BASE.number"; ARMED_FILE="$BASE.armed"; DRAFT_FILE="$BASE.draft"
log() {{ echo "$@" >> "{state}.log"; }}
[ -f "$NUM_FILE" ] || echo 101 > "$NUM_FILE"
[ -f "$ARMED_FILE" ] || echo false > "$ARMED_FILE"
[ -f "$DRAFT_FILE" ] || echo false > "$DRAFT_FILE"
NUMBER=$(cat "$NUM_FILE")
ARMED=$(cat "$ARMED_FILE")
DRAFT=$(cat "$DRAFT_FILE")
if [ "${{CF_TEST_GH_LIST_FAIL:-0}}" = 1 ] && [ "$1 $2" = "pr list" ]; then exit 2; fi
case "$1 $2" in
  "pr list")
    if [ "$DRAFT" = true ]; then ISDRAFT=true; else ISDRAFT=false; fi
    if [ "$ARMED" = true ]; then LABELS='[{"name":"merge-queue: armed"}]'; else LABELS='[]'; fi
    echo "[{\"number\":$NUMBER,\"title\":\"remote title\",\"body\":\"remote body\",\"isDraft\":$ISDRAFT,\"labels\":$LABELS}]";
    ;;
  "pr create")
    log pr_create "$@";
    ;;
  "pr view")
    if echo "$@" | grep -q labels; then
      if [ "$ARMED" = true ]; then LABELS='[{"name":"merge-queue: armed"}]'; else LABELS='[]'; fi
      echo "{\"labels\":$LABELS}";
    elif echo "$@" | grep -q number,url; then
      echo "{\"number\":$NUMBER,\"url\":\"https://forge.invalid/pr/$NUMBER\"}";
    else
      echo '{"state":"OPEN","statusCheckRollup":[{"status":"IN_PROGRESS"}]}'
    fi
    ;;
  "pr edit")
    if echo "$@" | grep -q -- "--add-label"; then
      echo true > "$ARMED_FILE"; log pr_edit "$@";
    elif echo "$@" | grep -q -- "--title"; then
      log pr_edit "$@";
    elif echo "$@" | grep -q -- "--remove-label"; then
      echo false > "$ARMED_FILE"; log pr_edit "$@";
    fi
    ;;
  "workflow run") ;;
  *) echo 1>&2 "unexpected gh args: $@"; exit 3;;
 esac
"#,
        state = state_path.display())
    }

    async fn mk_pool() -> sqlx::SqlitePool { sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap() }

    fn init_repo(root: &std::path::Path, bare: &std::path::Path) {
        crate::tools::delivery::git(root, &["init"]).unwrap();
        crate::tools::delivery::git(root, &["config","user.name","Synthetic"]).unwrap();
        crate::tools::delivery::git(root, &["config","user.email","synthetic@example.invalid"]).unwrap();
        fs::write(root.join("README.md"), "x").unwrap();
        crate::tools::delivery::git(root, &["add","."]).unwrap();
        crate::tools::delivery::git(root, &["commit","-m","init"]).unwrap();
        crate::tools::delivery::git(bare, &["init","--bare"]).unwrap();
        crate::tools::delivery::git(root, &["remote","add","origin", bare.to_str().unwrap()]).unwrap();
        let head = crate::tools::delivery::git(root, &["rev-parse","HEAD"]).unwrap();
        crate::tools::delivery::git(root, &["update-ref","refs/remotes/origin/main", &head]).unwrap();
        crate::tools::delivery::git(root, &["checkout","-b","feature"]).unwrap();
        fs::write(root.join("file.txt"), "changed").unwrap();
    }

    #[tokio::test]
    async fn f3_query_failure_downgrades_to_pr_only_and_never_arms() {
        let dir = tempfile::tempdir().unwrap();
        let bare = tempfile::tempdir().unwrap();
        init_repo(dir.path(), bare.path());
        let bin = dir.path().join("bin"); fs::create_dir_all(&bin).unwrap();
        let state = dir.path().join("gh-state.json");
        write_executable(&bin.join("gh"), &fake_gh_script(&state));
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
        std::env::set_var("CF_TEST_GH_LIST_FAIL", "1");
        let db = mk_pool().await;
        sqlx::query("CREATE TABLE sessions(id TEXT PRIMARY KEY, delivery_authorized INTEGER NOT NULL DEFAULT 1)").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO sessions VALUES('s',1)").execute(&db).await.unwrap();
        let mut ctx = crate::tools::ExecCtx::new(dir.path().to_path_buf(), Some(db));
        ctx.session_id = Some("s".into());
        let out = crate::tools::dispatch("deliver_changes", json!({"title":"fix: t","body":"b","ceiling":"through_merge"}), &ctx).await.unwrap();
        assert_eq!(out.status, crate::tools::ToolExecutionStatus::Waiting);
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["effective_ceiling"], "pr_only");
        let log = fs::read_to_string(state.with_extension("json.log")).unwrap_or_default();
        assert!(!log.contains("--add-label"), "must not try to arm: {log}");
    }

    #[tokio::test]
    async fn f2_f5_draft_or_removed_arm_records_revocation_and_refuses_merge() {
        let dir = tempfile::tempdir().unwrap();
        let bare = tempfile::tempdir().unwrap();
        init_repo(dir.path(), bare.path());
        let bin = dir.path().join("bin"); fs::create_dir_all(&bin).unwrap();
        let state = dir.path().join("gh-state.json");
        write_executable(&bin.join("gh"), &fake_gh_script(&state));
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
        let db = mk_pool().await;
        sqlx::query("CREATE TABLE sessions(id TEXT PRIMARY KEY, delivery_authorized INTEGER NOT NULL DEFAULT 1)").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO sessions VALUES('s',1)").execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE thin_delivery_refs(session_id TEXT PRIMARY KEY,branch TEXT,pr_number INTEGER,ceiling TEXT)").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs VALUES('s','feature',101,'ThroughMerge')").execute(&db).await.unwrap();
        fs::write(&state, "{\"number\":101,\"armed\":false,\"draft\":true}").unwrap();
        let mut ctx = crate::tools::ExecCtx::new(dir.path().to_path_buf(), Some(db.clone()));
        ctx.session_id = Some("s".into());
        let out = crate::tools::dispatch("deliver_changes", json!({"title":"fix: t","body":"b","ceiling":"through_merge"}), &ctx).await.unwrap();
        assert!(out.is_error && out.content.contains("草稿"), "{:#?}", out.content);
        sqlx::query("CREATE TABLE IF NOT EXISTS thin_delivery_revocations(session_id TEXT PRIMARY KEY, pr_number INTEGER NOT NULL, revoked_at INTEGER NOT NULL, kind TEXT NOT NULL)").execute(&db).await.unwrap();
        let val: i64 = sqlx::query_scalar("SELECT delivery_authorized FROM sessions WHERE id='s'").fetch_one(&db).await.unwrap();
        assert_eq!(val, 0);
    }

    #[tokio::test]
    async fn f6_remote_title_body_are_preserved_no_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let bare = tempfile::tempdir().unwrap();
        init_repo(dir.path(), bare.path());
        let bin = dir.path().join("bin"); fs::create_dir_all(&bin).unwrap();
        let state = dir.path().join("gh-state.json");
        fs::write(&state, "{\"number\":101,\"armed\":false,\"draft\":false}").unwrap();
        write_executable(&bin.join("gh"), &fake_gh_script(&state));
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
        let db = mk_pool().await;
        sqlx::query("CREATE TABLE sessions(id TEXT PRIMARY KEY, delivery_authorized INTEGER NOT NULL DEFAULT 1)").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO sessions VALUES('s',1)").execute(&db).await.unwrap();
        let mut ctx = crate::tools::ExecCtx::new(dir.path().to_path_buf(), Some(db));
        ctx.session_id = Some("s".into());
        let out = crate::tools::dispatch("deliver_changes", json!({"title":"fix: local","body":"local body","ceiling":"through_ci_green"}), &ctx).await.unwrap();
        assert!(!out.is_error);
        let log = fs::read_to_string(state.with_extension("json.log")).unwrap_or_default();
        assert!(!log.contains("--title"), "must not rewrite remote title/body: {log}");
    }

    #[tokio::test]
    async fn counterexample_authorized_and_not_revoked_arms_normally() {
        let dir = tempfile::tempdir().unwrap();
        let bare = tempfile::tempdir().unwrap();
        init_repo(dir.path(), bare.path());
        let bin = dir.path().join("bin"); fs::create_dir_all(&bin).unwrap();
        let state = dir.path().join("gh-state.json");
        fs::write(&state, "{\"number\":101,\"armed\":false,\"draft\":false}").unwrap();
        write_executable(&bin.join("gh"), &fake_gh_script(&state));
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
        let db = mk_pool().await;
        sqlx::query("CREATE TABLE sessions(id TEXT PRIMARY KEY, delivery_authorized INTEGER NOT NULL DEFAULT 1)").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO sessions VALUES('s',1)").execute(&db).await.unwrap();
        let mut ctx = crate::tools::ExecCtx::new(dir.path().to_path_buf(), Some(db));
        ctx.session_id = Some("s".into());
        let out = crate::tools::dispatch("deliver_changes", json!({"title":"fix: t","body":"b","ceiling":"through_merge"}), &ctx).await.unwrap();
        assert_eq!(out.status, crate::tools::ToolExecutionStatus::Waiting);
        let log = fs::read_to_string(state.with_extension("json.log")).unwrap_or_default();
        assert!(log.contains("--add-label"), "must arm when authorized and not revoked: {log}");
    }
}
