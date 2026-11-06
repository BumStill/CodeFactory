// SPDX-License-Identifier: Apache-2.0
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util { pub use crate::no_window; }
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;

const SAMPLES: [&str; 3] = [
    "git fetch --prune origin main && git merge origin/main && git config core.hooksPath .githooks; gh auth status; sed ...; rg ...; cat ...",
    "git fetch --prune origin main && git merge-base --is-ancestor origin/main HEAD && git config core.hooksPath .githooks; cat ...; rg ...; ls '<应用数据目录>'; ...",
    "git add docs/specs/... && git commit -m 'docs: ...'; sed ...; grep -n 'fn git' ...; grep -n 'pub.*register\\|fn.*execut\\|static.*' ...; command -v cargo; df -h .; git status --short",
];

#[test]
fn m248b_real_samples_are_not_destructive_or_outside_sandbox_authority() {
    for command in SAMPLES {
        assert!(!codefactory_agent_loop::policy::is_delivery_command(command), "remote: {command}");
        assert!(!trusted_git::outside_write_sandbox_scope(command), "outside_scope: {command}");
        assert!(!trusted_git::destructive_intent(command), "destructive: {command}");
    }
}

#[test]
fn m248b_history_and_remote_fences() {
    for command in ["git reset --hard HEAD", "git push --force origin HEAD", "git push origin --delete someone-else", "git status; git reset --hard HEAD"] {
        assert!(trusted_git::destructive_intent(command), "{command}");
    }
    for command in ["git push origin HEAD", "gh pr edit 42 --title synthetic", "gh pr view 42; gh pr edit 42 --title synthetic"] {
        assert!(codefactory_agent_loop::policy::is_delivery_command(command), "{command}");
    }
}

#[test]
fn m248b_sandboxed_external_database_read_has_no_path_proof() {
    let root = tempfile::tempdir().unwrap();
    let command = "python3 - <<'PY'\nimport sqlite3\nc = sqlite3.connect('file:/synthetic/outside/app.db?mode=ro', uri=True)\nprint(c.execute('SELECT count(*) FROM permission_intents').fetchall())\nPY";
    assert!(trusted_git::sandboxed_heredoc_proof(root.path(), command).is_ok());
    assert!(trusted_git::python_heredoc_proof(Some(root.path()), "python3 - <<'PY'\nprint(open('/synthetic/outside.txt').read())\nPY").is_err());
    for body in ["import socket\nsocket.create_connection(('example.invalid',443))", "import subprocess\nsubprocess.run(['git','push'])", "import sqlite3\nc=sqlite3.connect('file:/outside.db?mode=rw',uri=True)\nc.execute('DELETE FROM permission_intents')"] {
        assert!(trusted_git::sandboxed_heredoc_proof(root.path(), &format!("python3 - <<'PY'\n{body}\nPY")).is_err());
    }
}
#[test]
fn m248b_reports_legacy_proof_predicates() {
    let root = tempfile::tempdir().unwrap();
    for args in [vec!["init", "-q"], vec!["config", "user.name", "Synthetic"], vec!["config", "user.email", "synthetic@example.invalid"], vec!["commit", "--allow-empty", "-qm", "base"], vec!["update-ref", "refs/remotes/origin/main", "HEAD"]] {
        assert!(std::process::Command::new("git").current_dir(root.path()).args(args).status().unwrap().success());
    }
    for (index, command) in SAMPLES.iter().enumerate() {
        eprintln!("sample={} rewrite={} remote={} outside_scope={} destructive={}", index + 1,
            trusted_git::registered_rewrite(&[root.path().into()], root.path(), command),
            codefactory_agent_loop::policy::is_delivery_command(command),
            trusted_git::outside_write_sandbox_scope(command), trusted_git::destructive_intent(command));
    }
}
