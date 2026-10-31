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

// M215: the shapes the model actually wrote in the 2026-10-31 trusted session —
// `cd "$(pwd)" && python3 - <<'PY'` with `io.open`/`pathlib` on a file of its own
// workspace — must be proved instead of prompting.
fn m215_headers(root: &std::path::Path) -> Vec<String> {
    let python = if cfg!(windows) { "python -" } else { "python3 -" };
    let mut headers = vec![
        format!("{python} <<'PY'"),
        format!("cd \"$(pwd)\" && {python} <<'PY'"),
        format!("cd \"{}\" && {python} <<'PY'", root.display()),
    ];
    if cfg!(windows) { for header in headers.iter_mut() { *header = header.replace("python3 -", "python -"); } }
    headers
}

#[test]
fn m215_own_workspace_heredoc_is_proved() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/main.rs"), "old\n").unwrap();
    let bodies = [
        "import io\np='src/main.rs'\ns=io.open(p, encoding='utf-8').read()\nio.open(p,'w', encoding='utf-8').write(s.replace('old','new'))\nprint('done')",
        "import pathlib\np=pathlib.Path('src/main.rs')\np.write_text(p.read_text().replace('old','new'))",
        "with open('src/main.rs','w', encoding='utf-8') as f:\n    f.write('x')",
        "from pathlib import Path\nPath('src/main.rs').write_text('x')",
    ];
    for header in m215_headers(root.path()) {
        for body in bodies {
            let command = format!("{header}\n{body}\nPY");
            assert_eq!(
                trusted_git::python_heredoc_proof(Some(root.path()), &command),
                Ok(()),
                "own-workspace edit must be proved: {command}"
            );
        }
    }
}

#[test]
fn m215_unproved_heredoc_names_the_line_and_stays_closed() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    let python = if cfg!(windows) { "python -" } else { "python3 -" };
    for body in [
        "import io\nio.open('../outside').read()",
        "import subprocess\nsubprocess.run(['git', 'push'])",
        "import os\nos.system('git push --force')",
        "import shutil\nshutil.rmtree('src')",
        "import urllib.request\nurllib.request.urlopen('https://example.com')",
        "import socket\nsocket.create_connection(('example.com', 80))",
        "x = 1\neval('1')",
    ] {
        let command = format!("{python} <<'PY'\n{body}\nPY");
        let error = trusted_git::python_heredoc_proof(Some(root.path()), &command).unwrap_err();
        assert!(error.contains("行无法证明"), "{body} -> {error}");
    }
    let outside = if cfg!(windows) { "C:/Windows/system.ini" } else { "/etc/hosts" };
    for command in [
        format!("{python} <<'PY'\nopen('{outside}').read()\nPY"),
        format!("cd / && {python} <<'PY'\nopen('etc/hosts').read()\nPY"),
        format!("cd .. && {python} <<'PY'\nopen('outside').read()\nPY"),
    ] {
        assert!(trusted_git::python_heredoc_proof(Some(root.path()), &command).is_err(), "must stay unproved: {command}");
    }
}

#[test]
fn m215_registered_workspace_merge_started_with_pwd_cd_is_proved() {
    let root = tempfile::tempdir().unwrap();
    for args in [vec!["init", "-q"], vec!["config", "user.name", "Synthetic"], vec!["config", "user.email", "synthetic@example.invalid"], vec!["commit", "--allow-empty", "-qm", "fixture"]] {
        assert!(std::process::Command::new("git").current_dir(root.path()).args(args).status().unwrap().success());
    }
    let command = "cd \"$(pwd)\" && git merge origin/main --no-edit 2>&1 | tail -5 && git log --oneline -3";
    assert!(trusted_git::local_rewrite(root.path(), command), "own-workspace merge must be proved: {command}");
}

#[test]
fn m215_prover_reason_survives_a_legacy_console_code_page() {
    // A Windows host console defaults to a legacy code page, which used to turn
    // the Chinese diagnostic into `\u7b2c2\u884c...`. The prover pins stdio to
    // UTF-8, so the escapes must never reach the gate.
    let escaped = trusted_git::decode_prover_escapes("\\u7b2c2\\u884c\\u65e0\\u6cd5\\u8bc1\\u660e\\uff1aoutside path");
    assert_eq!(escaped, "第2行无法证明：outside path");
    assert_eq!(trusted_git::decode_prover_escapes("第2行无法证明：outside path"), "第2行无法证明：outside path");
    assert_eq!(trusted_git::decode_prover_escapes("C:\\u nope"), "C:\\u nope");

    let root = tempfile::tempdir().unwrap();
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut child = std::process::Command::new(python)
        .args(["-I", "-c", include_str!("../src/agent/trusted_python_ast.py"), root.path().to_str().unwrap()])
        .env("PYTHONIOENCODING", "cp1252")
        .env("PYTHONUTF8", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"import io\nio.open('../outside').read()\n").unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success(), "outside path must stay unproved");
    let stderr = String::from_utf8(output.stderr).expect("prover stderr must be UTF-8");
    assert!(stderr.contains("行无法证明"), "legacy code page must not escape the reason: {stderr}");
    assert!(!stderr.contains("\\u7b2c"), "reason must not arrive escaped: {stderr}");
}
