// SPDX-License-Identifier: Apache-2.0
//! Git-worktree isolation for parallel subagents.
//!
//! When `Settings::subagent_isolation` is `Worktree`, every dispatched task
//! gets its own `git worktree` (checked out from the repo's current HEAD on
//! a task-scoped branch) under the app data dir — never inside the user's
//! project, so nothing shows up in their `git status`. The subagent runs
//! with its cwd remapped into the worktree; after verification passes, the
//! resulting diff is applied back onto the user's working tree as ordinary
//! uncommitted edits.
//!
//! Design points (mirroring `checkpoint.rs`):
//!
//!   * Shell out to the `git` binary with [`NoWindow`] — same conventions
//!     and failure surface as checkpoints.
//!
//!   * CodeFactory never commits to the user's branch. The task branch only
//!     ever advances inside the worktree; merge-back is `git apply` of the
//!     task diff, all-or-nothing (`--check` first), so a conflict leaves the
//!     user's tree untouched and the task fails with the preserved patch +
//!     branch for manual recovery.
//!
//!   * Degrade gracefully: if the task cwd is not a git repository (or has
//!     no commits yet), callers fall back to shared-cwd mode.
//!
//!   * `.codefactory/` context (project memory) and the shared session brief
//!     are snapshot-copied into the worktree so the subagent keeps project
//!     context, but they are excluded from merge-back — the parent copies
//!     stay canonical.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::no_window::NoWindow;

/// Name prefix for task branches and worktree directories. Kept short so
/// Windows path-length limits stay far away.
const BRANCH_PREFIX: &str = "codefactory/task-";

/// Context files snapshot-copied into the worktree and excluded from
/// merge-back.
const CONTEXT_BRIEF: &str = "_codefactory_brief.md";
const CONTEXT_DIR: &str = ".codefactory";
/// Subdirectories of `.codefactory/` that are never copied (bulky or
/// meaningless outside the parent checkout).
const CONTEXT_DIR_SKIP: &[&str] = &["evidence", "worktrees"];

/// A live task worktree. Created by [`create`], consumed by
/// [`merge_back`] + [`cleanup`].
#[derive(Debug, Clone)]
pub struct TaskWorktree {
    /// Root of the checked-out worktree.
    pub worktree_root: PathBuf,
    /// The task cwd remapped into the worktree (same relative position the
    /// original cwd had inside the repo).
    pub effective_cwd: PathBuf,
    /// Task-scoped branch the worktree is on.
    pub branch: String,
    /// Repo-root of the user's checkout (merge-back target).
    pub repo_root: PathBuf,
    /// HEAD sha the worktree was created from — the diff base.
    pub base_sha: String,
    /// Where the merge-back patch is written (kept on conflict for manual
    /// recovery).
    pub patch_path: PathBuf,
}

/// Outcome of [`merge_back`].
#[derive(Debug)]
pub enum MergeOutcome {
    /// The task diff applied cleanly onto the user's working tree.
    Applied,
    /// The subagent changed nothing outside the excluded context files.
    NoChanges,
    /// The diff does not apply onto the user's current tree. Nothing was
    /// modified; the patch and branch are preserved for manual recovery.
    Conflict { message: String },
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .no_window()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Repo root containing `cwd`, or `None` when it isn't inside a git repo.
pub fn discover_repo_root(cwd: &Path) -> Option<PathBuf> {
    git(cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Short, filesystem-safe id derived from the task id.
fn short_id(task_id: &str) -> String {
    task_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect::<String>()
        .to_lowercase()
}

/// Create an isolated worktree for `task_id`, checked out from the current
/// HEAD of the repo containing `base_cwd`. `container` is the directory the
/// worktree (and its merge-back patch) live under — callers pass a location
/// in the app data dir so the user's project stays untouched.
pub fn create(base_cwd: &Path, task_id: &str, container: &Path) -> Result<TaskWorktree, String> {
    let repo_root =
        discover_repo_root(base_cwd).ok_or_else(|| "not a git repository".to_string())?;
    let base_sha = git(&repo_root, &["rev-parse", "HEAD"])
        .map_err(|e| format!("repository has no usable HEAD: {e}"))?;

    let sid = short_id(task_id);
    if sid.is_empty() {
        return Err("task id yields an empty worktree name".into());
    }
    let branch = format!("{BRANCH_PREFIX}{sid}");
    let worktree_root = container.join(format!("task-{sid}"));
    let patch_path = container.join(format!("task-{sid}.patch"));

    std::fs::create_dir_all(container)
        .map_err(|e| format!("cannot create worktree container: {e}"))?;

    // Clear stale leftovers from a previous run of the same task id: a
    // registered-but-dead worktree blocks both `worktree add` and branch
    // reuse, so prune first, then remove the directory, then retake the
    // branch with -B.
    let _ = git(&repo_root, &["worktree", "prune"]);
    if worktree_root.exists() {
        let wt = worktree_root.display().to_string();
        let _ = git(&repo_root, &["worktree", "remove", "--force", &wt]);
        if worktree_root.exists() {
            std::fs::remove_dir_all(&worktree_root)
                .map_err(|e| format!("cannot clear stale worktree dir: {e}"))?;
            let _ = git(&repo_root, &["worktree", "prune"]);
        }
    }

    git(
        &repo_root,
        &[
            "worktree",
            "add",
            "-B",
            &branch,
            &worktree_root.display().to_string(),
            &base_sha,
        ],
    )
    .map_err(|e| format!("git worktree add failed: {e}"))?;

    // Remap the task cwd into the worktree. `base_cwd` may be the repo root
    // itself or a subdirectory of it.
    let effective_cwd = match base_cwd
        .canonicalize()
        .unwrap_or_else(|_| base_cwd.to_path_buf())
        .strip_prefix(
            repo_root
                .canonicalize()
                .unwrap_or_else(|_| repo_root.clone()),
        ) {
        Ok(rel) if rel.as_os_str().is_empty() => worktree_root.clone(),
        Ok(rel) => worktree_root.join(rel),
        Err(_) => worktree_root.clone(),
    };
    // The subdirectory may be untracked (checkout won't materialize it) —
    // make sure the subagent's cwd exists either way.
    std::fs::create_dir_all(&effective_cwd)
        .map_err(|e| format!("cannot create effective cwd in worktree: {e}"))?;

    copy_context(base_cwd, &effective_cwd);

    Ok(TaskWorktree {
        worktree_root,
        effective_cwd,
        branch,
        repo_root,
        base_sha,
        patch_path,
    })
}

/// Snapshot-copy parent context (session brief + `.codefactory/`, minus bulky
/// subdirs) into the worktree so the subagent keeps project memory. Best
/// effort — missing context is not an error.
fn copy_context(base_cwd: &Path, effective_cwd: &Path) {
    let brief_src = base_cwd.join(CONTEXT_BRIEF);
    if brief_src.is_file() {
        let _ = std::fs::copy(&brief_src, effective_cwd.join(CONTEXT_BRIEF));
    }
    let ctx_src = base_cwd.join(CONTEXT_DIR);
    if ctx_src.is_dir() {
        copy_dir_shallow(&ctx_src, &effective_cwd.join(CONTEXT_DIR), CONTEXT_DIR_SKIP);
    }
}

/// Recursive copy skipping `skip` top-level subdirectory names. Existing
/// files in `dst` (from tracked checkout) are left as checked out.
fn copy_dir_shallow(src: &Path, dst: &Path, skip: &[&str]) {
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    let _ = std::fs::create_dir_all(dst);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            if skip.iter().any(|s| name.to_string_lossy() == *s) {
                continue;
            }
            copy_dir_shallow(&from, &to, &[]);
        } else if !to.exists() {
            let _ = std::fs::copy(&from, &to);
        }
    }
}

/// Commit the worktree's changes on the task branch and apply the resulting
/// diff back onto the user's working tree (as uncommitted edits).
///
/// All-or-nothing: `git apply --check` runs first, so a conflicting diff
/// changes nothing in the user's tree and comes back as
/// [`MergeOutcome::Conflict`] with the patch preserved at
/// [`TaskWorktree::patch_path`].
pub fn merge_back(wt: &TaskWorktree) -> Result<MergeOutcome, String> {
    // Stage everything except the snapshot-copied context — parent copies
    // stay canonical for those. Long-form exclude pathspecs: the short `:!`
    // form would parse the leading `_` of the brief filename as (bogus)
    // pathspec magic.
    git(
        &wt.worktree_root,
        &[
            "add",
            "-A",
            "--",
            ".",
            &format!(":(exclude){CONTEXT_BRIEF}"),
            &format!(":(exclude){CONTEXT_DIR}"),
        ],
    )
    .map_err(|e| format!("git add in worktree failed: {e}"))?;

    // Anything staged? `diff --cached --quiet` exits 1 when there are changes.
    let staged = Command::new("git")
        .no_window()
        .arg("-C")
        .arg(&wt.worktree_root)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    if staged.success() {
        return Ok(MergeOutcome::NoChanges);
    }

    // Commit with a fixed identity so merge-back never depends on the
    // user's git config. This commit only ever exists on the task branch.
    git(
        &wt.worktree_root,
        &[
            "-c",
            "user.name=CodeFactory",
            "-c",
            "user.email=noreply@codefactory.local",
            "commit",
            "--no-verify",
            "-m",
            &format!("codefactory: task snapshot ({})", wt.branch),
        ],
    )
    .map_err(|e| format!("git commit in worktree failed: {e}"))?;

    // Full binary diff base..task-branch, written to a file for both apply
    // and (on conflict) manual recovery.
    let patch_out = Command::new("git")
        .no_window()
        .arg("-C")
        .arg(&wt.worktree_root)
        .args(["diff", "--binary", &wt.base_sha, "HEAD"])
        .output()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    if !patch_out.status.success() {
        return Err(format!(
            "git diff for merge-back failed: {}",
            String::from_utf8_lossy(&patch_out.stderr).trim()
        ));
    }
    if patch_out.stdout.is_empty() {
        return Ok(MergeOutcome::NoChanges);
    }
    std::fs::write(&wt.patch_path, &patch_out.stdout)
        .map_err(|e| format!("cannot write merge-back patch: {e}"))?;

    let patch = wt.patch_path.display().to_string();
    if let Err(check) = git(
        &wt.repo_root,
        &["apply", "--check", "--whitespace=nowarn", &patch],
    ) {
        return Ok(MergeOutcome::Conflict {
            message: format!("task changes no longer apply onto the current working tree: {check}"),
        });
    }
    git(&wt.repo_root, &["apply", "--whitespace=nowarn", &patch])
        .map_err(|e| format!("git apply failed after successful check: {e}"))?;
    Ok(MergeOutcome::Applied)
}

/// Remove the worktree (and optionally its branch + patch). Called with
/// `delete_branch = true` after a successful merge-back / no-change run;
/// failed tasks keep everything for inspection.
pub fn cleanup(wt: &TaskWorktree, delete_branch: bool) {
    let path = wt.worktree_root.display().to_string();
    if git(&wt.repo_root, &["worktree", "remove", "--force", &path]).is_err() {
        let _ = std::fs::remove_dir_all(&wt.worktree_root);
        let _ = git(&wt.repo_root, &["worktree", "prune"]);
    }
    if delete_branch {
        let _ = git(&wt.repo_root, &["branch", "-D", &wt.branch]);
        let _ = std::fs::remove_file(&wt.patch_path);
    }
}

/// Rewrite absolute paths inside the worktree back to their location in the
/// user's checkout, so `files_changed` drill-downs point at real files.
pub fn remap_paths(files: Vec<String>, wt: &TaskWorktree) -> Vec<String> {
    let wt_prefix = wt.worktree_root.display().to_string();
    let repo_prefix = wt.repo_root.display().to_string();
    files
        .into_iter()
        .map(|f| {
            if f.starts_with(&wt_prefix) {
                format!("{}{}", repo_prefix, &f[wt_prefix.len()..])
            } else {
                f
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal throwaway repo with one commit. Returns (repo dir, container dir).
    fn make_repo(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "cf-worktree-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = base.join("repo");
        let container = base.join("container");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |args: &[&str]| git(&repo, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.name", "t"]);
        g(&["config", "user.email", "t@t"]);
        std::fs::write(repo.join("a.txt"), "line1\n").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-q", "-m", "init"]);
        (repo, container)
    }

    fn destroy(repo: &Path) {
        let base = repo.parent().unwrap();
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn create_checks_out_head_and_remaps_cwd() {
        let (repo, container) = make_repo("create");
        let wt = create(&repo, "task-ABC-123", &container).unwrap();
        assert!(wt.effective_cwd.join("a.txt").is_file());
        assert_eq!(wt.effective_cwd, wt.worktree_root);
        assert!(wt.branch.starts_with(BRANCH_PREFIX));
        cleanup(&wt, true);
        destroy(&repo);
    }

    #[test]
    fn create_fails_outside_git_repo() {
        let dir = std::env::temp_dir().join(format!(
            "cf-worktree-nongit-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(create(&dir, "t1", &dir.join("c")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_back_applies_worktree_edits_as_uncommitted_changes() {
        let (repo, container) = make_repo("apply");
        let wt = create(&repo, "t-apply", &container).unwrap();

        std::fs::write(wt.effective_cwd.join("a.txt"), "line1\nline2\n").unwrap();
        std::fs::write(wt.effective_cwd.join("new.txt"), "created\n").unwrap();

        match merge_back(&wt).unwrap() {
            MergeOutcome::Applied => {}
            other => panic!("expected Applied, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "line1\nline2\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("new.txt")).unwrap(),
            "created\n"
        );
        // The user's branch must not gain commits — changes arrive uncommitted.
        let head_subject = git(&repo, &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(head_subject, "init");

        cleanup(&wt, true);
        // Branch is gone after successful cleanup.
        assert!(git(&repo, &["rev-parse", "--verify", &wt.branch]).is_err());
        assert!(!wt.worktree_root.exists());
        destroy(&repo);
    }

    #[test]
    fn merge_back_reports_no_changes_for_untouched_worktree() {
        let (repo, container) = make_repo("nochange");
        let wt = create(&repo, "t-noop", &container).unwrap();
        match merge_back(&wt).unwrap() {
            MergeOutcome::NoChanges => {}
            other => panic!("expected NoChanges, got {other:?}"),
        }
        cleanup(&wt, true);
        destroy(&repo);
    }

    #[test]
    fn merge_back_conflict_leaves_user_tree_untouched() {
        let (repo, container) = make_repo("conflict");
        let wt = create(&repo, "t-conf", &container).unwrap();

        // Diverge: worktree and user tree both rewrite the same line.
        std::fs::write(wt.effective_cwd.join("a.txt"), "worktree version\n").unwrap();
        std::fs::write(repo.join("a.txt"), "user edited meanwhile\n").unwrap();

        match merge_back(&wt).unwrap() {
            MergeOutcome::Conflict { message } => {
                assert!(!message.is_empty());
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // All-or-nothing: user's edit survives untouched, patch preserved.
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "user edited meanwhile\n"
        );
        assert!(wt.patch_path.is_file());

        cleanup(&wt, false);
        // Branch preserved for manual recovery on the failure path.
        assert!(git(&repo, &["rev-parse", "--verify", &wt.branch]).is_ok());
        destroy(&repo);
    }

    #[test]
    fn context_files_are_copied_in_but_never_merged_back() {
        let (repo, container) = make_repo("context");
        std::fs::create_dir_all(repo.join(".codefactory")).unwrap();
        std::fs::write(repo.join(".codefactory/memory.md"), "remember\n").unwrap();
        std::fs::write(repo.join("_codefactory_brief.md"), "brief\n").unwrap();

        let wt = create(&repo, "t-ctx", &container).unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.effective_cwd.join(".codefactory/memory.md")).unwrap(),
            "remember\n"
        );
        assert_eq!(
            std::fs::read_to_string(wt.effective_cwd.join("_codefactory_brief.md")).unwrap(),
            "brief\n"
        );

        // Subagent mutates its context snapshot + does real work.
        std::fs::write(wt.effective_cwd.join(".codefactory/memory.md"), "mutated\n").unwrap();
        std::fs::write(wt.effective_cwd.join("real.txt"), "work\n").unwrap();

        match merge_back(&wt).unwrap() {
            MergeOutcome::Applied => {}
            other => panic!("expected Applied, got {other:?}"),
        }
        // Real work merged; parent context copies stayed canonical.
        assert!(repo.join("real.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(repo.join(".codefactory/memory.md")).unwrap(),
            "remember\n"
        );
        cleanup(&wt, true);
        destroy(&repo);
    }

    #[test]
    fn subdirectory_cwd_is_remapped_into_worktree() {
        let (repo, container) = make_repo("subdir");
        std::fs::create_dir_all(repo.join("pkg/app")).unwrap();
        std::fs::write(repo.join("pkg/app/f.txt"), "x\n").unwrap();
        git(&repo, &["add", "-A"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "subdir"]).unwrap();

        let wt = create(&repo.join("pkg/app"), "t-sub", &container).unwrap();
        assert!(wt.effective_cwd.ends_with(Path::new("pkg/app")));
        assert!(wt.effective_cwd.join("f.txt").is_file());
        cleanup(&wt, true);
        destroy(&repo);
    }

    #[test]
    fn stale_worktree_from_previous_run_is_replaced() {
        let (repo, container) = make_repo("stale");
        let first = create(&repo, "t-stale", &container).unwrap();
        // Simulate a crash: no cleanup, then the same task id runs again.
        let second = create(&repo, "t-stale", &container).unwrap();
        assert_eq!(first.worktree_root, second.worktree_root);
        assert!(second.effective_cwd.join("a.txt").is_file());
        cleanup(&second, true);
        destroy(&repo);
    }

    #[test]
    fn remap_paths_rewrites_worktree_prefix() {
        let wt = TaskWorktree {
            worktree_root: PathBuf::from("/data/wt/task-1"),
            effective_cwd: PathBuf::from("/data/wt/task-1"),
            branch: "codefactory/task-1".into(),
            repo_root: PathBuf::from("/home/user/proj"),
            base_sha: "deadbeef".into(),
            patch_path: PathBuf::from("/data/wt/task-1.patch"),
        };
        let mapped = remap_paths(
            vec![
                "/data/wt/task-1/src/a.rs".into(),
                "relative/b.rs".into(),
                "/elsewhere/c.rs".into(),
            ],
            &wt,
        );
        assert_eq!(mapped[0], "/home/user/proj/src/a.rs");
        assert_eq!(mapped[1], "relative/b.rs");
        assert_eq!(mapped[2], "/elsewhere/c.rs");
    }
}
