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
        let output = Command::new(if cfg!(windows) { "bash" } else { "sh" }).args(["-c", &command]).current_dir(&root).output().unwrap();
        assert!(output.status.success());
        assert_eq!(git(&root, &["show", &format!("{backup}:generated.txt")]), "untracked content");
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn m159_r2_escapes_remote_unknown_syntax_require_approval() {
    let root = fixture();
    for command in ["git reset --hard HEAD && rm -rf ../other", "git -C ../other reset --hard HEAD", "git checkout -- ../other", "git clean -ffdx", "git reset --hard HEAD; echo x", "git restore --source=origin/main .", "git stash pop", "git push --force", "git push origin :main", "git push --delete origin main", "git reset --hard $(echo HEAD)", "GIT_DIR=/tmp/git git reset --hard HEAD", "git clean -f /tmp/file", "git checkout main", "git clean -f :(top)../other", "git reset --hard HEAD && git push origin main"] {
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
