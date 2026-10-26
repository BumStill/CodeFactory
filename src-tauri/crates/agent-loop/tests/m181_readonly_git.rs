use codefactory_agent_core::classify_command;
use codefactory_agent_loop::policy::capability_denial;
use codefactory_agent_loop::run::TurnCapability;

fn denial(capability: TurnCapability, command: &str) -> Option<String> {
    capability_denial(capability, "bash", command, &classify_command(command, 30_000), &serde_json::json!({"command": command}))
}

#[test]
fn m181_readonly_git_queries() {
    for command in [
        "git status", "git log -1 --oneline", "git show HEAD", "git diff",
        "git rev-parse HEAD", "git merge-base HEAD origin/main", "git branch --list",
        "git worktree list", "git ls-files", "git blame README.md", "git describe",
        "git remote -v", "git config --get user.name", "git config --global --get user.name",
        "git -C /synthetic/repo worktree list", "git --git-dir=/synthetic/repo/.git worktree list",
        "gh pr view 12", "gh pr list", "gh pr checks 12", "gh run list", "gh run view 12", "gh auth status",
        "sed -n '1,20p' README.md; git worktree list | head -3; git log -1 --oneline origin/main",
    ] {
        assert!(denial(TurnCapability::ReviewOnly, command).is_none(), "{command}");
    }
}

#[test]
fn m181_readonly_git_writes() {
    for command in [
        "git fetch --prune origin main", "git commit -m synthetic", "git checkout main",
        "git reset --hard", "git stash", "git merge topic", "git rebase main",
        "git worktree add /synthetic/new topic", "git worktree remove /synthetic/old",
        "git config user.name synthetic", "git config --get user.name --unset user.name",
        "git config --get user.name --set user.name synthetic", "git remote add demo https://example.invalid/repo",
        "git -C /synthetic/repo fetch origin", "git fetch origin && git merge-base HEAD origin/main",
        "git worktree list | head -3; git fetch origin", "gh pr merge 12",
    ] {
        let reason = denial(TurnCapability::ReviewOnly, command).unwrap_or_else(|| panic!("allowed: {command}"));
        assert!(reason.contains("写入"), "{reason}");
    }
}

#[test]
fn m181_implementation_keeps_permission_gateway_authority() {
    for capability in [TurnCapability::Implement, TurnCapability::Deliver] {
        for command in ["git fetch origin", "git commit -m synthetic", "git worktree add /synthetic/new topic"] {
            assert!(denial(capability, command).is_none(), "{command}");
        }
    }
}

#[test]
fn m181_denial_is_user_facing() {
    for reason in [
        denial(TurnCapability::ReviewOnly, "git commit -m synthetic").unwrap(),
        capability_denial(TurnCapability::ReviewOnly, "write_file", "", &codefactory_agent_core::ToolKind::Mutation, &serde_json::json!({"path":"src/example.rs"})).unwrap(),
    ] {
        assert!(!reason.contains("不要要求用户重复确认"), "{reason}");
        assert!(reason.contains("只读任务"), "{reason}");
    }
}
