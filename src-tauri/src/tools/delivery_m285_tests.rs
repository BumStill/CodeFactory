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
if [ -f "$BASE.fail_list_once" ] && [ "$1 $2" = "pr list" ] && [ ! -f "$BASE.failed_once" ]; then touch "$BASE.failed_once"; exit 2; fi
case "$1 $2" in
 "pr list") [ "$ARMED" = true ] && LABELS='[{{"name":"merge-queue: armed"}}]' || LABELS='[]'; echo "[{{\"number\":101,\"url\":\"https://forge.invalid/pr/101\",\"title\":\"fix: remote\",\"body\":\"R | 是否完成 | 证据\",\"state\":\"OPEN\",\"headRefOid\":\"unused\",\"isDraft\":$DRAFT,\"labels\":$LABELS}}]";;
 "pr view") if echo "$@" | grep -q labels; then [ "$ARMED" = true ] && LABELS='[{{"name":"merge-queue: armed"}}]' || LABELS='[]'; echo "{{\"labels\":$LABELS}}"; elif echo "$@" | grep -q number,url; then echo '{{"number":101,"url":"https://forge.invalid/pr/101"}}'; else echo '{{"state":"OPEN","statusCheckRollup":[{{"status":"IN_PROGRESS"}}]}}'; fi;;
 "pr edit") echo "$@" >> "$L"; echo "$@" | grep -q -- --add-label && echo true > "$A" || :;;
 "pr create") echo "$@" >> "$L";;
 "workflow run") echo "$@" >> "$L";;
 *) echo "unexpected $*" >&2; exit 3;;
esac
"#, s=state.display()) }

    fn fake_gh_windows(state_path: &std::path::Path) -> String {
        let base = state_path.display().to_string().replace('\\', "\\\\");
        format!(r#"@echo off
setlocal EnableDelayedExpansion
set "BASE={base}"
set "A=!BASE!.armed"
set "D=!BASE!.draft"
set "L=!BASE!.log"
if not exist "!A!" echo false>"!A!"
if not exist "!D!" echo false>"!D!"
set /p ARMED=<"!A!"
set /p DRAFT=<"!D!"
if exist "!BASE!.fail_list_once" if "%1 %2"=="pr list" if not exist "!BASE!.failed_once" (type nul>"!BASE!.failed_once" & exit /b 2)
if "%1 %2"=="pr list" (
  set "LABELS=[]"
  if "!ARMED!"=="true" set "LABELS=[{""name"":""merge-queue: armed""}]"
  echo [{"number":101,"url":"https://forge.invalid/pr/101","title":"fix: remote","body":"R table","state":"OPEN","headRefOid":"unused","isDraft":!DRAFT!,"labels":!LABELS!}]
  exit /b 0
)
if "%1 %2"=="pr view" (
  echo %*| findstr /C:"labels" >nul && (set "LABELS=[]" & if "!ARMED!"=="true" set "LABELS=[{""name"":""merge-queue: armed""}]" & echo {"labels":!LABELS!} & exit /b 0)
  echo %*| findstr /C:"number,url" >nul && (echo {"number":101,"url":"https://forge.invalid/pr/101"} & exit /b 0)
  echo {"state":"OPEN","statusCheckRollup":[{"status":"IN_PROGRESS"}]}
  exit /b 0
)
if "%1 %2"=="pr edit" (
  echo %*>>"!L!"
  echo %*| findstr /C:"--add-label" >nul && echo true>"!A!"
  exit /b 0
)
if "%1 %2"=="pr create" (echo %*>>"!L!" & exit /b 0)
if "%1 %2"=="workflow run" (echo %*>>"!L!" & exit /b 0)
exit /b 3
"#)
    }

    async fn setup() -> (tempfile::TempDir, tempfile::TempDir, sqlx::SqlitePool, std::path::PathBuf) {
        let dir=tempfile::tempdir().unwrap(); let bare=tempfile::tempdir().unwrap();
        let git = |cwd: &std::path::Path, args: &[&str]| crate::agent::delivery::git(cwd,args).unwrap();
        for a in [vec!["init"],vec!["config","user.name","Synthetic"],vec!["config","user.email","synthetic@example.invalid"]] { git(dir.path(), &a); }
        fs::write(dir.path().join("README.md"),"x").unwrap(); for a in [vec!["add","."],vec!["commit","-m","init"]] { git(dir.path(), &a); }
        git(bare.path(), &["init","--bare"]); git(dir.path(), &["remote","add","origin",bare.path().to_str().unwrap()]);
        let head=git(dir.path(), &["rev-parse","HEAD"]); git(dir.path(), &["update-ref","refs/remotes/origin/main",head.trim()]); git(dir.path(), &["checkout","-b","feature"]); fs::write(dir.path().join("file.txt"),"changed").unwrap();
        let state=dir.path().join("gh-state");
        let gh=if cfg!(windows) { dir.path().join("gh.cmd") } else { dir.path().join("gh") };
        fs::write(&gh,if cfg!(windows) { fake_gh_windows(&state) } else { fake_gh(&state) }).unwrap();
        #[cfg(unix)] { let mut p=fs::metadata(&gh).unwrap().permissions(); p.set_mode(0o755); fs::set_permissions(&gh,p).unwrap(); }
        crate::agent::delivery::register_test_gh(dir.path(), gh);
        let db=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE sessions(id TEXT PRIMARY KEY,delivery_authorized INTEGER NOT NULL,release_urgency TEXT NOT NULL,delivery_mode TEXT NOT NULL)").execute(&db).await.unwrap(); sqlx::query("INSERT INTO sessions VALUES('s',1,'default','thin')").execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE thin_delivery_refs(session_id TEXT PRIMARY KEY,branch TEXT,pr_number INTEGER,ceiling TEXT)").execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE task_runs(id TEXT PRIMARY KEY,objective_id TEXT)").execute(&db).await.unwrap(); sqlx::query("INSERT INTO task_runs VALUES('task','objective')").execute(&db).await.unwrap();
        (dir,bare,db,state)
    }
    fn context(dir:&std::path::Path,db:sqlx::SqlitePool)->crate::tools::ExecCtx { let mut c=crate::tools::ExecCtx::new(dir.to_path_buf(),Some(db)); c.session_id=Some("s".into()); c.task_id=Some("task".into()); let mut s=crate::config::settings::Settings::default(); s.delivery_ceiling=DeliveryCeiling::ThroughMerge; c.settings=Some(s); c }

    #[tokio::test]
    async fn query_failure_downgrades_to_pr_only_and_never_arms(){let(d,_b,db,state)=setup().await;fs::write(state.with_extension("fail_list_once"),"1").unwrap();let o=crate::tools::dispatch("deliver_changes",json!({"title":"fix: t","body":"b","ceiling":"through_merge"}),&context(d.path(),db)).await.unwrap();assert_eq!(o.status,crate::tools::ToolExecutionStatus::Waiting,"{}",o.content);let v:serde_json::Value=serde_json::from_str(&o.content).unwrap();assert_eq!(v["effective_ceiling"],"pr_only");assert!(!fs::read_to_string(state.with_extension("log")).unwrap_or_default().contains("--add-label"));}
    #[tokio::test]
    async fn draft_revocation_persists_across_restart_until_new_authorization(){let(d,_b,db,state)=setup().await;fs::write(state.with_extension("draft"),"true").unwrap();sqlx::query("INSERT INTO thin_delivery_refs VALUES('s','feature',101,'ThroughMerge')").execute(&db).await.unwrap();let a=crate::tools::dispatch("deliver_changes",json!({"title":"fix: t","body":"b","ceiling":"through_merge"}),&context(d.path(),db.clone())).await.unwrap();assert!(a.is_error&&a.content.contains("草稿"),"{}",a.content);let b=crate::tools::dispatch("deliver_changes",json!({"title":"fix: t","body":"b","ceiling":"through_merge"}),&context(d.path(),db.clone())).await.unwrap();assert!(b.is_error&&b.content.contains("撤回"),"{}",b.content);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT delivery_authorized FROM sessions WHERE id='s'").fetch_one(&db).await.unwrap(),0);}
    #[tokio::test]
    async fn remote_title_and_body_are_preserved_without_rewrite(){let(d,_b,db,state)=setup().await;let o=crate::tools::dispatch("deliver_changes",json!({"title":"fix: local","body":"local","ceiling":"through_ci_green"}),&context(d.path(),db)).await.unwrap();assert!(!o.is_error,"{}",o.content);let l=fs::read_to_string(state.with_extension("log")).unwrap_or_default();assert!(!l.contains("--title")&&!l.contains("--body"),"{l}");}
    #[tokio::test]
    async fn authorized_not_revoked_arms_normally(){let(d,_b,db,state)=setup().await;let o=crate::tools::dispatch("deliver_changes",json!({"title":"fix: t","body":"b","ceiling":"through_merge"}),&context(d.path(),db)).await.unwrap();assert_eq!(o.status,crate::tools::ToolExecutionStatus::Waiting,"{}",o.content);assert!(fs::read_to_string(state.with_extension("log")).unwrap_or_default().contains("--add-label"));}
}
