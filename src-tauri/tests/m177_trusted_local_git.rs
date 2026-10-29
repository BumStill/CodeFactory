// SPDX-License-Identifier: Apache-2.0
//! M177: a trusted session must not prompt for local Git rewrites of its own
//! workspace, even when they are chained with `git fetch`.
//!
//! Evidence (v1.91.35, already containing M172 #870): session aeba516d waited
//! for approval on
//! `git fetch --prune origin main && git stash push -u -m m174-sync-before-delivery
//! && git merge origin/main && git stash pop`
//! although every segment stays inside the session workspace.
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util { pub use crate::no_window; }
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;
use std::{path::Path, path::PathBuf, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").current_dir(root).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "cf-m177-{}-{:?}-{}",
        std::process::id(),
        std::thread::current().id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Synthetic"]);
    git(&root, &["config", "user.email", "synthetic@example.invalid"]);
    std::fs::write(root.join("tracked.txt"), "original").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "synthetic base"]);
    // A local remote-tracking ref is enough: the proof layer must decide from
    // the command shape, never by talking to a remote.
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    std::fs::create_dir_all(root.join("outside")).ok();
    root
}

/// The exact command that waited for approval on 2026-10-26.
const INCIDENT: &str = "git fetch --prune origin main && \
git stash push -u -m m174-sync-before-delivery && \
git merge origin/main && git stash pop";

#[test]
fn m177_incident_composition_is_provable_without_approval() {
    let root = fixture();
    assert!(
        trusted_git::local_rewrite(&root, INCIDENT),
        "the 2026-10-26 incident composition must be provable inside the session workspace"
    );
    let backup = trusted_git::backup(&root, INCIDENT).expect("M159 snapshot must still precede the rewrite");
    assert!(backup.starts_with("refs/codefactory/backup/"), "{backup}");
}

#[test]
fn m177_sync_compositions_from_the_incident_family_are_provable() {
    let root = fixture();
    for command in [
        INCIDENT,
        // Same sync, spelled with the explicit no-edit merge.
        "git fetch --prune origin main && git stash push -u -m sync && git merge --no-edit origin/main && git stash pop",
        // Fetch with a refspec target and a stash apply instead of pop.
        "git fetch origin main && git stash push --include-untracked --message sync && git merge --ff-only origin/main && git stash apply",
        // Non-interactive rebase sync (interactive rebase stays out).
        "git fetch --prune origin main && git stash push -u -m sync && git rebase origin/main && git stash pop",
        // Plain local history/worktree maintenance inside the workspace.
        "git reset --hard HEAD",
        "git restore --staged tracked.txt",
        "git restore tracked.txt",
        "git checkout -- .",
        "git checkout origin/main -- src/main.rs",
        "git clean -fd outside",
        "git switch -c m177-local-branch",
        "git switch m177-local-branch",
        "git merge --abort",
        "git rebase --abort",
        "git add -A && git commit -m \"m177 sync\"",
        "git status --short && git log --oneline -3",
    ] {
        assert!(trusted_git::local_rewrite(&root, command), "must not prompt: {command}");
        assert!(trusted_git::backup(&root, command).is_some(), "snapshot must succeed: {command}");
    }
}

#[test]
fn m177_previously_blocked_read_only_commands_do_not_ask() {
    // 10-23 .. 10-26 recorded false positives: read-only, no rewrite at all.
    let script = "node -e 'const r=require(\"./docs/testing/scenario-registry.json\");console.log(r.scenarios.length)' | head -5";
    assert!(!trusted_git::sensitive(script), "quoted script data must not ask");
    assert!(!trusted_git::sensitive("cat src/main.rs"));
    assert!(!trusted_git::sensitive("cat tracked.txt && git status --short"));
    assert!(trusted_git::sensitive("git add -A && git commit -m \"m177 sync\""), "M182 requires snapshots for local index/history writes");
    // M159 still requires a snapshot before a local rewrite is allowed; M177
    // only removes the *approval prompt* for workspace-local compositions.
    assert!(trusted_git::sensitive(INCIDENT));
}

#[test]
fn m177_remote_out_of_bounds_and_destructive_actions_still_ask() {
    let root = fixture();
    for command in [
        // Remote mutation is delivery authority, not local trust.
        "git push --force origin main",
        "git push --force-with-lease origin main",
        "git push origin --delete m177-local-branch",
        "git push origin :refs/heads/m177-local-branch",
        "git -c core.pager=less status",
        "gh pr merge 877 --squash",
        // Out of the session workspace.
        "git -C ../elsewhere reset --hard HEAD",
        "git checkout -- ../outside/tracked.txt",
        "git clean -f ../outside",
        "git reset --hard /etc/passwd",
        // Unrecoverable: the snapshot cannot restore ignored files.
        "git clean -fdx",
        "git stash drop",
        "git stash clear",
        // Interactive execution cannot be judged up front.
        "git rebase -i origin/main",
    ] {
        assert!(
            !trusted_git::local_rewrite(&root, command),
            "must still require approval: {command}"
        );
    }
    assert!(trusted_git::sensitive("git push --force origin main"));
    assert!(trusted_git::sensitive("git clean -fdx"));
    assert!(trusted_git::sensitive("git rebase -i origin/main"));
}
