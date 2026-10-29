// SPDX-License-Identifier: Apache-2.0
//! Standalone synthetic regressions at the production trusted-Git parser.
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util { pub use crate::no_window; }
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;

#[test]
fn m198_node_local_edit_and_danger_pairs() {
    let local = "node <<'JS'\nconst fs = require('fs');\nconst p = 'src/main.rs';\nlet s = fs.readFileSync(p, 'utf8');\ns = s.replace('old', 'new');\nfs.writeFileSync(p, s);\nJS";
    assert!(!trusted_git::sensitive(local), "workspace-local Node edit must not prompt");
    for command in [local.replace("src/main.rs", "../outside"), local.replace("fs.writeFileSync(p, s);", "require('child_process').execSync('git push origin main');")] {
        assert!(trusted_git::sensitive(&command));
    }
}

#[test]
fn m198_fixed_family_fixture() {
    let root = tempfile::tempdir().unwrap();
    for args in [vec!["init", "-q"], vec!["config", "user.name", "Synthetic"], vec!["config", "user.email", "synthetic@example.invalid"], vec!["commit", "--allow-empty", "-qm", "fixture"]] {
        assert!(std::process::Command::new("git").current_dir(root.path()).args(args).status().unwrap().success());
    }
    let rows: serde_json::Value = serde_json::from_str(include_str!("fixtures/m198-trusted-family.json")).unwrap();
    for row in rows.as_array().unwrap() {
        let command = row["command"].as_str().unwrap();
        assert_eq!(trusted_git::local_rewrite(root.path(), command), row["allow"].as_bool().unwrap(), "{}: {command}", row["reason"]);
    }
}

const LOCAL: &str = "cd \"$(pwd)\"; python3 - <<'PY'\np='src/main.rs'\ns=open(p).read()\norig=s\ns=s.replace('old', 'new')\nopen(p,'w').write(s)\nPY";

#[test]
fn m198_quoted_python_is_not_shell_syntax() {
    assert!(!trusted_git::sensitive(LOCAL));
    assert!(!trusted_git::sensitive("python3 - <<'PY'\ns='''the extension isn\'t connected; `read` after `select_tab`'''\nPY"));
}

#[test]
fn m198_heredoc_danger_is_never_treated_as_inert_data() {
    for command in [
        "python3 - <<'PY'\nimport os\nos.system('git push --force origin main')\nPY",
        "python3 - <<'PY'\nopen('../outside', 'w').write('bad')\nPY",
        "python3 - <<'PY'\nopen('src/main.rs', 'w').write('bad')\nPY\ngit push origin main",
        "python3 - <<PY\nprint('$(git push origin main)')\nPY",
        "python3 - <<'PY'\nprint('unterminated')",
    ] {
        assert!(trusted_git::sensitive(command), "must retain proof/approval: {command}");
    }
}
