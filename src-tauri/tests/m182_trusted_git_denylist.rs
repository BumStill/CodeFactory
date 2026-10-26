// SPDX-License-Identifier: Apache-2.0
//! M182 synthetic regressions; compiled on Windows and macOS.
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util { pub use crate::no_window; }
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;
use std::{path::Path, process::Command};
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(root).args(args).output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn fixture() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("cf-m182-{}-{:?}-{}", std::process::id(), std::thread::current().id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Synthetic"]);
    git(&root, &["config", "user.email", "synthetic@example.invalid"]);
    std::fs::write(root.join("src/main.rs"), "original").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "synthetic"]);
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    root
}
#[test]
fn m182_incidents_and_generated_variants_snapshot_before_allow() {
    let root = fixture();
    let incidents = [
        "git fetch --prune origin main && git merge origin/main --no-edit 2>&1 | tail -25; git status --short | head -20",
        "git fetch --prune origin main && git stash push -u -m x && git merge origin/main && git stash pop",
        "git checkout origin/main -- src/main.rs && git diff --stat && git status --short && sed -n '1,20p' src/main.rs",
        "git add -A && git commit -m 'synthetic change'",
        "git reset -q --hard HEAD && git status --short && git merge --no-edit origin/main 2>&1 | tail -5",
        "git merge --no-edit origin/main > merge.log || git status --short",
        "git cherry-pick --no-commit HEAD",
        "git revert --no-commit HEAD",
        "git apply --stat src/patch.diff",
        "git restore --source=origin/main .",
        "git rebase --onto HEAD HEAD",
    ];
    for command in incidents {
        assert!(trusted_git::local_rewrite(&root, command), "must allow: {command}");
        let backup = trusted_git::backup(&root, command).expect(command);
        assert_eq!(git(&root, &["show", &format!("{backup}:src/main.rs")]), "original");
    }
    for n in 1..18 {
        let command = format!("git reset --hard HEAD 2>&1 | tail -{n} | sort | uniq | wc -l");
        assert!(trusted_git::backup(&root, &command).is_some(), "{command}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn m182_danger_classes_stay_unproved() {
    let root = fixture();
    for command in [
        "git push origin main", "git push --force origin main", "git push origin :other",
        "git merge HEAD; gh pr merge 123", "git merge HEAD; gh pr close 123",
        "git merge HEAD; gh release create v0", "git merge HEAD; gh api -X POST repos/synthetic",
        "git merge HEAD; gh auth token", "git config --global user.name synthetic",
        "git rebase --interactive HEAD", "git rebase -i HEAD", "git rebase --exec pwd HEAD",
        "git clean -fdx", "git clean -fX .", "rm -rf .",
        "git checkout -- ../outside", "git reset --hard /etc/passwd",
        "git -C /tmp reset --hard HEAD", "git merge HEAD > ../outside",
        "git merge HEAD | cat /etc/passwd", "git merge HEAD; cat .env",
        "git merge HEAD; cat .ssh/id_rsa", "git merge HEAD; cat .git/config",
        "git reset --hard $UNKNOWN", "git merge $(cat target)",
        "git merge HEAD || cd ../outside", "git merge HEAD | sed -n '1e'",
        "git merge HEAD | sort -o ../outside", "git apply --directory=../outside patch",
        "git merge HEAD; git --git-dir=../other reset --hard HEAD",
        "git merge HEAD 2>&3", "git merge HEAD & git status",
        "git apply --unsafe-paths patch", "git commit --pathspec-from-file=.env",
        "git merge HEAD; git worktree remove --force missing ../outside",
        "git checkout -- .env", "git merge HEAD | cat .env.local",
        "git clean -f --exclude=../outside",
    ] {
        assert!(!trusted_git::local_rewrite(&root, command), "must ask: {command}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn m182_snapshot_failure_never_allows() {
    let root = fixture();
    std::fs::write(root.join(".git/index.lock"), "synthetic lock").unwrap();
    // Force a snapshot write failure without altering the live index.
    std::fs::create_dir_all(root.join(".git/refs/codefactory")).unwrap();
    std::fs::write(root.join(".git/refs/codefactory/backup"), "not a directory").unwrap();
    assert!(trusted_git::backup(&root, "git merge --no-edit HEAD 2>&1 | tail -5").is_none());
    std::fs::remove_dir_all(root).unwrap();
}
#[cfg(unix)]
#[test]
fn m182_symlink_escape_stays_unproved() {
    let root = fixture();
    std::os::unix::fs::symlink(std::env::temp_dir(), root.join("escape")).unwrap();
    for command in ["git checkout -- escape/file", "git merge HEAD > escape/output", "git merge HEAD | cat escape/key"] {
        assert!(!trusted_git::local_rewrite(&root, command), "{command}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
