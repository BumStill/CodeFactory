// SPDX-License-Identifier: Apache-2.0
//! Standalone synthetic regressions at the production trusted-Git parser.
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util { pub use crate::no_window; }
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;

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
