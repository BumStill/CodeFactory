// SPDX-License-Identifier: Apache-2.0
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util { pub use crate::no_window; }
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;

#[test]
    fn m207_non_session_reset_and_remote_helpers_stay_fenced() {
        let root = tempfile::tempdir().unwrap();
        for args in [vec!["init", "-q"], vec!["config", "user.name", "Synthetic"], vec!["config", "user.email", "synthetic@example.invalid"], vec!["commit", "--allow-empty", "-qm", "base"], vec!["update-ref", "refs/remotes/origin/main", "HEAD"], vec!["commit", "--allow-empty", "-qm", "published"], vec!["update-ref", "refs/remotes/origin/main", "HEAD"]] {
            assert!(std::process::Command::new("git").current_dir(root.path()).args(args).status().unwrap().success());
        }
        for command in ["git reset --hard HEAD~1", "git push origin other", "git branch -D someone-else", "git fetch 'ext::evil'", "git fetch --upload-pack=evil origin", "git show --ext-diff HEAD"] {
            assert!(!trusted_git::registered_rewrite(&[root.path().into()], root.path(), command), "danger: {command}");
        }
    }

#[test]
fn m207_non_git_segments_use_the_standalone_trusted_policy() {
    let root = tempfile::tempdir().unwrap();
    for args in [vec!["init", "-q"], vec!["config", "user.name", "Synthetic"], vec!["config", "user.email", "synthetic@example.invalid"], vec!["commit", "--allow-empty", "-qm", "fixture"]] {
        assert!(std::process::Command::new("git").current_dir(root.path()).args(args).status().unwrap().success());
    }
    let command = "git diff --check && python3.11 tools/governance/run_scenario_harness_gate.py --stage local; grep -n -E 'synthetic' '/synthetic/Application Support/com.codefactory.app/background-tasks/bg-test.log' | tail -12; git config core.hooksPath .githooks; git fetch --prune origin main; git merge-base --is-ancestor origin/main HEAD";
    assert!(trusted_git::registered_rewrite(&[root.path().into()], root.path(), command), "M207 original command shape must not acquire an extra non-Git whitelist");
    let mut pool = vec![
        "git show HEAD".to_string(), "git rev-parse HEAD".into(), "git branch --list".into(),
        "git stash list".into(), "git merge HEAD".into(), "echo synthetic".into(),
        "python3.11 check.py".into(), "grep synthetic '/synthetic/Application Support/background-tasks/test.log'".into(),
        "gh auth token".into(), "git push origin other".into(), "git branch -D someone-else".into(),
        "git fetch 'ext::evil'".into(), "git fetch --upload-pack=evil origin".into(),
    ];
    let family: serde_json::Value = serde_json::from_str(include_str!("fixtures/m198-trusted-family.json")).unwrap();
    for row in family.as_array().unwrap() {
        pool.extend(trusted_git::command_segments(row["command"].as_str().unwrap()).unwrap());
    }
    pool.extend(trusted_git::command_segments(command).unwrap());
    pool.extend(trusted_git::command_segments("git fetch --prune origin main && git merge origin/main; /opt/homebrew/bin/python3 tools/governance/validate_scenario_test_governance.py --ci --base-ref origin/main --title 'synthetic'").unwrap());
    #[path = "../src/tools/shell_policy.rs"]
    mod standalone_policy;
    let allowed = |segment: &str| {
        if segment.trim_start().starts_with("git ") {
            trusted_git::registered_rewrite(&[root.path().into()], root.path(), segment)
        } else {
            matches!(standalone_policy::classify_command(segment), standalone_policy::ShellCommandPolicy::Allow { .. })
                && !codefactory_agent_loop::policy::is_delivery_command(segment)
                && !trusted_git::sensitive(segment)
        }
    };
    let mut seed = 207_u64;
    for _ in 0..256 {
        let mut selected = vec!["git config core.hooksPath .githooks".to_string()];
        for _ in 0..3 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            selected.push(pool[(seed >> 32) as usize % pool.len()].clone());
        }
        let expected = selected.iter().all(|segment| allowed(segment));
        let joined = selected.join(["; ", " && ", " || "][seed as usize % 3]);
        assert_eq!(trusted_git::registered_rewrite(&[root.path().into()], root.path(), &joined), expected, "most restrictive standalone outcome: {joined}");
    }
}
