// SPDX-License-Identifier: Apache-2.0
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util {
    pub use crate::no_window;
    pub mod host_git {
        use crate::no_window::NoWindow;
        pub fn command() -> std::process::Command {
            std::process::Command::new("git").no_window()
        }
        pub fn command_with_host_credentials() -> std::process::Command { command() }
    }
}
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;

#[test]
fn m248_local_discovery_arguments_are_not_remote_actions() {
    for query in [
        "grep -n 'gh token git push' tracked.txt",
        "grep -n 'ssh|curl|wget' tracked.txt",
        "printf '%s' 'git push origin main'",
    ] {
        let command = format!("git branch --show-current; git fetch --prune origin main && git merge origin/main && git config core.hooksPath .githooks; {query}; cat tracked.txt");
        assert!(!trusted_git::outside_write_sandbox_scope(&command), "quoted search data is not executable remote authority: {command}");
    }
}

#[test]
fn m248_remote_and_history_actions_keep_their_existing_fence() {
    for command in ["git push origin main", "git push --force origin main", "git push origin --delete someone-else", "gh auth token"] {
        assert!(trusted_git::outside_write_sandbox_scope(command), "{command}");
    }
    for command in ["cat ~/.ssh/id_ed25519", "cat ../other/token", "grep synthetic .aws/credentials", "gh pr view 7 --json state", "git config --global user.name test", "git checkout -- ../other", "curl https://example.invalid"] {
        assert!(!trusted_git::outside_write_sandbox_scope(command), "filesystem scope is kernel-owned: {command}");
    }
    assert!(trusted_git::sensitive("git reset --hard HEAD"));
    assert!(trusted_git::sensitive("git rebase origin/main"));
}

#[test]
fn m248_sync_is_not_a_destructive_rewrite() {
    let root = tempfile::tempdir().unwrap();
    for args in [vec!["init", "-q"], vec!["config", "user.name", "Synthetic"], vec!["config", "user.email", "synthetic@example.invalid"], vec!["commit", "--allow-empty", "-qm", "base"], vec!["update-ref", "refs/remotes/origin/main", "HEAD"]] {
        assert!(std::process::Command::new("git").current_dir(root.path()).args(args).status().unwrap().success());
    }
    let command = "git branch --show-current; git fetch --prune origin main && git merge origin/main && git config core.hooksPath .githooks; grep -n 'gh token git push' tracked.txt; cat tracked.txt";
    let rewrite = trusted_git::registered_rewrite(&[root.path().into()], root.path(), command);
    let remote = codefactory_agent_loop::policy::is_delivery_command(command);
    let outside = trusted_git::outside_write_sandbox_scope(command);
    assert!(!remote && !outside);
    assert!(rewrite, "existing registration/confinement proof must remain true");
    assert!(!trusted_git::destructive_intent(command), "ordinary sync must not be classified as destructive");
}
