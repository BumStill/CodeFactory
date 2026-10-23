// SPDX-License-Identifier: Apache-2.0
//! Synthetic Git fixtures for M159.
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util { pub use crate::no_window; }
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;
use std::{path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").current_dir(root).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn fixture() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("cf-m159-{}-{:?}-{}", std::process::id(), std::thread::current().id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Synthetic"]);
    git(&root, &["config", "user.email", "synthetic@example.invalid"]);
    std::fs::write(root.join("tracked.txt"), "original").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "synthetic base"]);
    root
}
#[test]
fn m159_r1_four_incident_shapes_and_backup_restore() {
    for shape in 0..4 {
        let root = fixture();
        let base = git(&root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("tracked.txt"), "unpublished commit").unwrap();
        git(&root, &["commit", "-qam", "synthetic unpublished"]);
        let head = git(&root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("tracked.txt"), "staged content").unwrap();
        git(&root, &["add", "."]);
        std::fs::write(root.join("tracked.txt"), "unstaged content").unwrap();
        std::fs::write(root.join("generated.txt"), "untracked content").unwrap();
        let command = match shape {
            0 | 3 => format!("git reset --hard {base}"),
            1 => "git restore --staged tracked.txt && git clean -fq generated.txt".into(),
            _ => "git checkout -- . && rm -rf generated.txt".into(),
        };
        assert!(trusted_git::local_rewrite(&root, &command), "{command}");
        let backup = trusted_git::backup(&root, &command).expect("backup must precede allow");
        assert!(backup.starts_with("refs/codefactory/backup/"));
        assert_eq!(git(&root, &["show", &format!("{backup}:tracked.txt")]), "unstaged content");
        assert_eq!(git(&root, &["show", &format!("{backup}:generated.txt")]), "untracked content");
        assert_eq!(git(&root, &["show", &format!("{backup}^2:tracked.txt")]), "staged content");
        assert_eq!(git(&root, &["rev-parse", &format!("{backup}^1")]), head);
        assert_eq!(git(&root, &["show", ":tracked.txt"]), "staged content");
        assert_eq!(std::fs::read_to_string(root.join("tracked.txt")).unwrap(), "unstaged content");
        let shell = if cfg!(windows) {
            let exec_path = std::path::PathBuf::from(git(&root, &["--exec-path"]));
            exec_path.ancestors().map(|base| base.join("bin/bash.exe"))
                .find(|candidate| candidate.is_file())
                .expect("Git for Windows installation must supply Git Bash")
        } else { std::path::PathBuf::from("sh") };
        let output = Command::new(shell).args(["-c", &command]).current_dir(&root).output().unwrap();
        assert!(output.status.success(), "{command}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(git(&root, &["show", &format!("{backup}:generated.txt")]), "untracked content");
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn m166_r1_compound_stash_fetch_merge_and_literal_cwd() {
    let root = fixture();
    let command = format!("cd \"{}\"; git stash push -m prefill; git fetch --prune origin main; git merge origin/main --no-edit; git stash pop; git status --porcelain | head", root.display());
    assert!(trusted_git::local_rewrite(&root, &command), "{command}");
    let reference = trusted_git::backup(&root, &command).expect("compound command needs durable backup");
    assert_eq!(git(&root, &["show", &format!("{reference}:tracked.txt")]), "original");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn m166_r3_registered_worktree_remove_snapshots_and_foreign_fences() {
    let root = fixture();
    let target = root.with_extension("owned-worktree");
    git(&root, &["worktree", "add", "-qb", "synthetic-owned", target.to_str().unwrap()]);
    let branch = git(&root, &["branch", "--show-current"]);
    std::fs::write(target.join("tracked.txt"), "owned dirty data").unwrap();
    let command = format!("S=\"{}\"; A=\"{}\"; git -C \"$S\" worktree remove --force \"$A\"; git -C \"$S\" checkout {branch}", root.display(), target.display());
    let roots = vec![root.clone(), target.clone()];
    assert!(trusted_git::registered_rewrite(&roots, &root, &command));
    assert!(!trusted_git::registered_rewrite(&[root.clone()], &root, &command));
    let foreign = fixture();
    for replacement in [&foreign, &root] {
        let escaped = command.replace(&format!("A=\"{}\"", target.display()), &format!("A=\"{}\"", replacement.display()));
        assert!(!trusted_git::registered_rewrite(&roots, &root, &escaped));
    }
    assert!(!trusted_git::registered_rewrite(&roots, &root, &format!("{command}; git push --force origin main")));
    let references = trusted_git::backup_registered(&roots, &root, &command).unwrap();
    let reference = references.split(',').find(|r| git(&root, &["show", &format!("{r}:tracked.txt")]) == "owned dirty data").unwrap();
    let output = Command::new("sh").args(["-c", &command]).current_dir(&root).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!target.exists());
    assert_eq!(git(&root, &["show", &format!("{reference}:tracked.txt")]), "owned dirty data");
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(foreign).unwrap();
}

#[test]
fn m166_r2_unknown_expansion_and_compound_escapes_require_approval() {
    let root = fixture();
    for command in [
        "S=$HOME; git -C \"$S\" stash push",
        "S=$(pwd); git -C \"$S\" stash push",
        "git -C \"$UNKNOWN\" stash pop",
        "git stash push; git push --force origin main",
        "git stash push | cd ../other; git stash pop",
        "git stash push; git -C ../other status",
        "git stash push; git diff --output=../outside",
        "git stash push > ../outside",
        "GIT_DIR=elsewhere; git stash push",
    ] {
        assert!(!trusted_git::local_rewrite(&root, command), "{command}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn m159_r2_escapes_remote_unknown_syntax_require_approval() {
    let root = fixture();
    for command in ["git reset --hard HEAD && rm -rf ../other", "git -C ../other reset --hard HEAD", "git checkout -- ../other", "git clean -ffdx", "git reset --hard HEAD; echo x", "git restore --source=origin/main .", "git push --force", "git push origin :main", "git push --delete origin main", "git reset --hard $(echo HEAD)", "GIT_DIR=/tmp/git git reset --hard HEAD", "git clean -f /tmp/file", "git checkout main", "git clean -f :(top)../other", "git reset --hard HEAD && git push origin main"] {
        assert!(!trusted_git::local_rewrite(&root, command), "{command}");
    }
    #[cfg(unix)] {
        std::os::unix::fs::symlink(std::env::temp_dir(), root.join("escape")).unwrap();
        assert!(!trusted_git::local_rewrite(&root, "git checkout -- escape/file"));
        assert!(!trusted_git::local_rewrite(&root, "git clean -f escape/file"));
    }
    assert!(trusted_git::sensitive("git push --force-with-lease origin main"));
    assert!(trusted_git::sensitive("git push origin :main"));
    assert!(trusted_git::sensitive("git push origin +HEAD:main"));
    std::fs::remove_dir_all(root).unwrap();
}
