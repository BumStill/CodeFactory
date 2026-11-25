// SPDX-License-Identifier: Apache-2.0
//! M285 regression tests colocated with the private delivery modules.

use crate::config::settings::DeliveryCeiling;
use super::delivery_m285_policy::{effective_delivery_ceiling, existing_pr_merge_refusal, preserve_existing_pr_content};

#[test]
fn effective_ceiling_is_the_lower_authorized_and_requested_value() {
    assert_eq!(effective_delivery_ceiling(DeliveryCeiling::ThroughRelease, Some(DeliveryCeiling::PrOnly)), DeliveryCeiling::PrOnly);
    assert_eq!(effective_delivery_ceiling(DeliveryCeiling::PrOnly, Some(DeliveryCeiling::ThroughRelease)), DeliveryCeiling::PrOnly);
}
#[test]
fn draft_and_removed_arm_are_readable_revocations() {
    let draft = existing_pr_merge_refusal(true, true, false, DeliveryCeiling::ThroughMerge).unwrap(); assert!(draft.contains("草稿") && draft.contains("撤回"));
    let removed = existing_pr_merge_refusal(false, true, false, DeliveryCeiling::ThroughMerge).unwrap(); assert!(removed.contains("摘掉") && removed.contains("明确授权"));
}
#[test]
fn recovery_preserves_remote_title_body_and_requirement_table() {
    let body = "Summary\n\nR | 是否完成 | 证据\nR1 | 是 | synthetic"; let (title, kept) = preserve_existing_pr_content("fix: maintainer title", body); assert_eq!(title, "fix: maintainer title"); assert_eq!(kept, body);
}

#[cfg(test)]
mod integration {
    use super::DeliveryCeiling;
    use serde_json::json;
    use std::fs;
    #[cfg(unix)] use std::os::unix::fs::PermissionsExt;

    fn fake_gh(state: &std::path::Path) -> String { format!(r#"#!/bin/sh
set -eu
BASE="{s}"; A="$BASE.armed"; D="$BASE.draft"; L="$BASE.log"
[ -f "$A" ] || echo false > "$A"; [ -f "$D" ] || echo false > "$D"
ARMED=$(cat "$A"); DRAFT=$(cat "$D")
if [ "${{CF_TEST_GH_LIST_FAIL:-0}}" = 1 ] && [ "$1 $2" = "pr list" ] && [ ! -f "$BASE.failed_once" ]; then touch "$BASE.failed_once"; exit 2; fi
case "$1 $2" in
 "pr list") [ "$ARMED" = true ] && LABELS='[{{"name":"merge-queue: armed"}}]' || LABELS='[]'; echo "[{{\"number\":101,\"url\":\"https://forge.invalid/pr/101\",\"title\":\"fix: remote\",\"body\":\"R | 是否完成 | 证据\",\"state\":\"OPEN\",\"headRefOid\":\"unused\",\"isDraft\":$DRAFT,\"labels\":$LABELS}}]";;
 "pr view") if echo "$@" | grep -q labels; then [ "$ARMED" = true ] && LABELS='[{{"name":"merge-queue: armed"}}]' || LABELS='[]'; echo "{{\"labels\":$LABELS}}"; elif echo "$@" | grep -q number,url; then echo '{{"number":101,"url":"https://forge.invalid/pr/101"}}'; else echo '{{"state":"OPEN","statusCheckRollup":[{{"status":"IN_PROGRESS"}}]}}'; fi;;
 "pr edit") echo "$@" >> "$L"; echo "$@" | grep -q -- --add-label && echo true > "$A" || :;;
 "pr create") echo "$@" >> "$L";;
 "workflow run") echo "$@" >> "$L";;
 *) echo "unexpected $*" >&2; exit 3;;
esac
"#, s=state.display()) }

    async fn setup() -> (tempfile::TempDir, tempfile::TempDir, sqlx::SqlitePool, std::path::PathBuf) {
        use std::process::Command;
        let dir=tempfile::tempdir().unwrap(); let bare=tempfile::tempdir().unwrap();
        for a in [vec!["init"],vec!["config","user.name","Synthetic"],vec!["config","user.email","synthetic@example.invalid"]] { assert!(Command::new("git").current_dir(dir.path()).args(a).status().unwrap().success()); }
        fs::write(dir.path().join("README.md"),"x").unwrap(); for a in [vec!["add","."],vec!["commit","-m","init"]] { assert!(Command::new("git").current_dir(dir.path()).args(a).status().unwrap().success()); }
        assert!(Command::new("git").current_dir(bare.path()).args(["init","--bare"]).status().unwrap().success()); assert!(Command::new("git").current_dir(dir.path()).args(["remote","add","origin",bare.path().to_str().unwrap()]).status().unwrap().success());
        let head=String::from_utf8(Command::new("git").current_dir(dir.path()).args(["rev-parse","HEAD"]).output().unwrap().stdout).unwrap(); assert!(Command::new("git").current_dir(dir.path()).args(["update-ref","refs/remotes/origin/main",head.trim()]).status().unwrap().success()); assert!(Command::new("git").current_dir(dir.path()).args(["checkout","-b","feature"]).status().unwrap().success()); fs::write(dir.path().join("file.txt"),"changed").unwrap();
        let state=dir.path().join("gh-state"); let gh=dir.path().join("gh"); fs::write(&gh,fake_gh(&state)).unwrap(); #[cfg(unix)] { let mut p=fs::metadata(&gh).unwrap().permissions(); p.set_mode(0o755); fs::set_permissions(&gh,p).unwrap(); } std::env::set_var("CODEFACTORY_TEST_GH_BIN",gh);
        let db=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE sessions(id TEXT PRIMARY KEY,delivery_authorized INTEGER NOT NULL,release_urgency TEXT NOT NULL,delivery_mode TEXT NOT NULL)").execute(&db).await.unwrap(); sqlx::query("INSERT INTO sessions VALUES('s',1,'default','thin')").execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE thin_delivery_refs(session_id TEXT PRIMARY KEY,branch TEXT,pr_number INTEGER,ceiling TEXT)").execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE task_runs(id TEXT PRIMARY KEY,objective_id TEXT)").execute(&db).await.unwrap(); sqlx::query("INSERT INTO task_runs VALUES('task','objective')").execute(&db).await.unwrap();
        (dir,bare,db,state)
    }
    fn context(dir:&std::path::Path,db:sqlx::SqlitePool)->crate::tools::ExecCtx { let mut c=crate::tools::ExecCtx::new(dir.to_path_buf(),Some(db)); c.session_id=Some("s".into()); c.task_id=Some("task".into()); let mut s=crate::config::settings::Settings::default(); s.delivery_ceiling=DeliveryCeiling::ThroughMerge; c.settings=Some(s); c }

    #[tokio::test]
    async fn query_failure_downgrades_to_pr_only_and_never_arms(){let(d,_b,db,state)=setup().await;std::env::set_var("CF_TEST_GH_LIST_FAIL","1");let o=crate::tools::dispatch("deliver_changes",json!({"title":"fix: t","body":"b","ceiling":"through_merge"}),&context(d.path(),db)).await.unwrap();std::env::remove_var("CF_TEST_GH_LIST_FAIL");assert_eq!(o.status,crate::tools::ToolExecutionStatus::Waiting,"{}",o.content);let v:serde_json::Value=serde_json::from_str(&o.content).unwrap();assert_eq!(v["effective_ceiling"],"pr_only");assert!(!fs::read_to_string(state.with_extension("log")).unwrap_or_default().contains("--add-label"));}
    #[tokio::test]
    async fn draft_revocation_persists_across_restart_until_new_authorization(){let(d,_b,db,state)=setup().await;fs::write(state.with_extension("draft"),"true").unwrap();sqlx::query("INSERT INTO thin_delivery_refs VALUES('s','feature',101,'ThroughMerge')").execute(&db).await.unwrap();let a=crate::tools::dispatch("deliver_changes",json!({"title":"fix: t","body":"b","ceiling":"through_merge"}),&context(d.path(),db.clone())).await.unwrap();assert!(a.is_error&&a.content.contains("草稿"),"{}",a.content);let b=crate::tools::dispatch("deliver_changes",json!({"title":"fix: t","body":"b","ceiling":"through_merge"}),&context(d.path(),db.clone())).await.unwrap();assert!(b.is_error&&b.content.contains("撤回"),"{}",b.content);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT delivery_authorized FROM sessions WHERE id='s'").fetch_one(&db).await.unwrap(),0);}
    #[tokio::test]
    async fn remote_title_and_body_are_preserved_without_rewrite(){let(d,_b,db,state)=setup().await;let o=crate::tools::dispatch("deliver_changes",json!({"title":"fix: local","body":"local","ceiling":"through_ci_green"}),&context(d.path(),db)).await.unwrap();assert!(!o.is_error,"{}",o.content);let l=fs::read_to_string(state.with_extension("log")).unwrap_or_default();assert!(!l.contains("--title")&&!l.contains("--body"),"{l}");}
    #[tokio::test]
    async fn authorized_not_revoked_arms_normally(){let(d,_b,db,state)=setup().await;let o=crate::tools::dispatch("deliver_changes",json!({"title":"fix: t","body":"b","ceiling":"through_merge"}),&context(d.path(),db)).await.unwrap();assert_eq!(o.status,crate::tools::ToolExecutionStatus::Waiting,"{}",o.content);assert!(fs::read_to_string(state.with_extension("log")).unwrap_or_default().contains("--add-label"));}
}
