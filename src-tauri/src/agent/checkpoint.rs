// SPDX-License-Identifier: Apache-2.0
//! Git-backed snapshots taken before the agent starts working on a user
//! message, so any change it makes can be reverted with a single click.
//!
//! Design points (per product principle: "AI 放手干，错了便宜回滚"):
//!
//!   * We use `git stash create` — it builds a commit object containing
//!     the working tree + index state at this moment **without** touching
//!     refs, HEAD, the working tree, or the stash list. The returned SHA
//!     is a stable handle we can later restore from.
//!
//!   * If the working tree is clean (nothing to snapshot), stash create
//!     returns empty — we fall back to using HEAD's SHA. Either way the
//!     consumer gets a non-empty handle.
//!
//!   * If the cwd is not a git repository at all, we silently skip
//!     checkpoint creation. The product still works; the user just
//!     doesn't get the rollback safety net.
//!
//!   * Reverting restores the working tree from the snapshot via
//!     `git read-tree` + `git checkout-index`. We do NOT move HEAD —
//!     the revert appears as ordinary working-tree changes, so the
//!     user can review, partially keep, or undo the revert itself.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Snapshot of a workspace at a point in time. Stored in DB so the UI
/// can list per-session checkpoints and offer revert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    pub id: String,
    pub session_id: String,
    pub message_id: Option<String>,
    pub cwd: String,
    /// SHA1 of the stash commit (or HEAD when tree was clean).
    pub git_sha: String,
    /// Human label. Usually a short version of the user message that
    /// triggered the snapshot.
    pub label: String,
    /// RFC3339 timestamp.
    pub created_at: String,
    /// True once the user has clicked "revert" on this checkpoint.
    pub reverted: bool,
}

/// File-level diff between the checkpoint and the current working tree.
/// Returned by `compute_checkpoint_changeset` for the UI's pre-revert
/// review panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointFileChange {
    pub path: String,
    /// "added" | "modified" | "deleted" | "renamed" | "typechange"
    pub status: String,
}

/// Errors are returned as plain strings so the Tauri command can surface
/// them verbatim in the UI without an AppError mapping detour.
pub type CheckpointResult<T> = std::result::Result<T, String>;

/// Try to create a checkpoint at `cwd`. Returns `Ok(None)` if cwd is not
/// a git repository (perfectly normal); `Ok(Some(...))` with the snapshot
/// SHA otherwise.
///
/// `label` should be a short human-readable hint — typically the first
/// 80 chars of the user message that triggered the snapshot.
pub fn create(cwd: &Path, label: &str) -> CheckpointResult<Option<String>> {
    if !is_git_repo(cwd) {
        return Ok(None);
    }

    // git stash create: returns a SHA when there's something to stash,
    // empty when the tree is already clean.
    let out = Command::new("git")
        .args(["stash", "create"])
        .arg("--include-untracked")
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git stash create failed to spawn: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git stash create failed: {stderr}"));
    }

    let stash_sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !stash_sha.is_empty() {
        // Tag the stash blob so git's GC doesn't reap it. We use a
        // per-checkpoint ref under refs/codefactory/checkpoints/ so the
        // anchors are easy to enumerate or clean up later.
        let ref_name = format!("refs/codefactory/checkpoints/{}", &stash_sha[..12]);
        let _ = Command::new("git")
            .args(["update-ref", &ref_name, &stash_sha])
            .current_dir(cwd)
            .output();
        let _ = label; // label is recorded by the caller in the DB row
        return Ok(Some(stash_sha));
    }

    // Working tree was clean. Use HEAD's SHA so the checkpoint still has
    // a stable handle the user can compare against later.
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git rev-parse HEAD failed: {e}"))?;
    if !head.status.success() {
        // Brand-new repo with no commits — return None gracefully.
        return Ok(None);
    }
    let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    Ok(if head_sha.is_empty() { None } else { Some(head_sha) })
}

/// Restore the working tree of `cwd` to the contents of the checkpoint
/// snapshot at `git_sha`.
///
/// We deliberately don't move HEAD, so the revert lands as ordinary
/// working-tree changes the user can review/commit/discard like any other
/// edit. That keeps "revert" from being a destructive scary operation —
/// it's just "give me my old files back."
pub fn revert(cwd: &Path, git_sha: &str) -> CheckpointResult<()> {
    if !is_git_repo(cwd) {
        return Err("Not a git repository".into());
    }

    // Validate the sha resolves to something we can read.
    let cat = Command::new("git")
        .args(["cat-file", "-e", git_sha])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git cat-file failed to spawn: {e}"))?;
    if !cat.status.success() {
        return Err(format!(
            "Checkpoint object {} no longer exists (was the repo GC'd?)",
            git_sha
        ));
    }

    // For a stash commit, the working-tree state lives in parent[2] (or
    // sometimes parent[0] for ancient git). The committed tree itself is
    // the merge of the index + worktree changes; checking it out is the
    // right "restore to that snapshot" semantics.
    let restore = Command::new("git")
        .args(["restore", "--source", git_sha, "--worktree", "--", "."])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git restore failed to spawn: {e}"))?;
    if !restore.status.success() {
        let stderr = String::from_utf8_lossy(&restore.stderr);
        return Err(format!("git restore failed: {stderr}"));
    }
    Ok(())
}

/// Diff a checkpoint against the current working tree. Used by the UI's
/// pre-revert confirmation so the user sees exactly what will change.
pub fn changeset(cwd: &Path, git_sha: &str) -> CheckpointResult<Vec<CheckpointFileChange>> {
    if !is_git_repo(cwd) {
        return Ok(vec![]);
    }
    let out = Command::new("git")
        .args(["diff", "--name-status", git_sha, "--"])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git diff failed to spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut changes = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.splitn(2, '\t');
        let code = it.next().unwrap_or("").trim();
        let path = it.next().unwrap_or("").trim().to_string();
        if path.is_empty() {
            continue;
        }
        let status = match code.chars().next().unwrap_or('?') {
            'A' => "added",
            'M' => "modified",
            'D' => "deleted",
            'R' => "renamed",
            'T' => "typechange",
            _ => "modified",
        }
        .to_string();
        changes.push(CheckpointFileChange { path, status });
    }
    Ok(changes)
}

fn is_git_repo(cwd: &Path) -> bool {
    let p: PathBuf = cwd.join(".git");
    p.exists()
        || Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(cwd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn init_repo(dir: &Path) {
        Command::new("git").args(["init"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["config", "user.name", "t"]).current_dir(dir).output().unwrap();
        // Disable Windows autocrlf so byte-exact comparisons in the asserts
        // below don't break on CRLF translation.
        Command::new("git").args(["config", "core.autocrlf", "false"]).current_dir(dir).output().unwrap();
        fs::write(dir.join("a.txt"), "hello\n").unwrap();
        Command::new("git").args(["add", "."]).current_dir(dir).output().unwrap();
        Command::new("git").args(["commit", "-m", "init"]).current_dir(dir).output().unwrap();
    }

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("cf-cp-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn skips_when_not_a_git_repo() {
        let dir = tmp();
        let r = create(&dir, "first message");
        assert!(matches!(r, Ok(None)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshots_dirty_tree() {
        let dir = tmp();
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "world\n").unwrap();
        fs::write(dir.join("new.txt"), "new\n").unwrap();
        let sha = create(&dir, "msg").expect("ok").expect("some sha");
        assert!(!sha.is_empty());

        // Revert blasts the working-tree edits back to the snapshot.
        // (At snapshot time the file already said "world".) The snapshot
        // preserves the dirty state itself, so re-modifying and reverting
        // should restore to "world" again.
        fs::write(dir.join("a.txt"), "different\n").unwrap();
        revert(&dir, &sha).expect("revert ok");
        let restored = fs::read_to_string(dir.join("a.txt")).unwrap();
        assert_eq!(restored, "world\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshots_clean_tree_returns_head() {
        let dir = tmp();
        init_repo(&dir);
        // No edits — stash create returns empty, we fall back to HEAD SHA.
        let sha = create(&dir, "msg").expect("ok").expect("some sha");
        assert_eq!(sha.len(), 40, "expected a 40-char SHA, got: {sha:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changeset_lists_modified_files() {
        let dir = tmp();
        init_repo(&dir);
        let sha = create(&dir, "msg").expect("ok").expect("some sha");
        fs::write(dir.join("a.txt"), "after\n").unwrap();
        fs::write(dir.join("b.txt"), "new\n").unwrap();
        let changes = changeset(&dir, &sha).expect("ok");
        let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
        assert!(paths.contains(&"a.txt"), "a.txt should be in changeset: {changes:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
