// SPDX-License-Identifier: Apache-2.0
//! M224: executable acceptance of the shared process sandbox (no desktop needed).
#[path = "../src/command_sandbox.rs"]
mod command_sandbox;
use command_sandbox::{CommandSandbox, NetworkPolicy, WritablePath};
use std::path::PathBuf;

#[test]
fn m224_bypass_is_denied_with_recovery_guidance() {
    for command in [
        "git commit --no-verify -m test",
        "git -c core.hooksPath=/tmp/hooks commit -m test",
        "CODEFACTORY_SKIP_SYNC_GATE=1 git commit -m test",
        "git config core.hooksPath /tmp/hooks",
        "git config --unset core.hooksPath",
    ] {
        let reason = command_sandbox::gate_bypass(command).expect(command);
        assert!(
            reason.contains("main") && reason.contains("禁止"),
            "{reason}"
        );
    }
    assert!(command_sandbox::gate_bypass("git config core.hooksPath .githooks").is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn m224_real_seatbelt_default_deny_and_children() {
    let root = std::env::temp_dir().join(format!("m224-{}", std::process::id()));
    let app = root.join("app-data");
    let ws = app.join("execution-workspaces/current");
    let other = app.join("execution-workspaces/other");
    for dir in [&ws, &other, &root.join("main"), &root.join(".ssh")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let sandbox = CommandSandbox::new(
        &ws,
        vec![WritablePath::tree(ws.clone())],
        NetworkPolicy::Enabled,
    )
    .unwrap();
    let execute = |command: &str| {
        let launch = sandbox.wrap("/bin/sh", &["-c".into(), command.into()]);
        std::process::Command::new(launch.program)
            .args(launch.args)
            .current_dir(&ws)
            .output()
            .unwrap()
    };
    let output = execute("printf owned > owned");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(ws.join("owned")).unwrap(), "owned");
    for target in [
        app.join("settings.json"),
        app.join("sessions.db"),
        app.join("logs/run.log"),
        app.join("root-write"),
        other.join("probe"),
        root.join("main/probe"),
        root.join(".ssh/probe"),
        root.join("outside"),
    ] {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let command = format!(
            "/usr/bin/python3 -c 'import pathlib; p=pathlib.Path({:?}); p.write_text(\"escape\")'",
            target.to_string_lossy()
        );
        let output = execute(&command);
        assert!(!output.status.success(), "escaped: {}", target.display());
        assert!(!target.exists(), "created: {}", target.display());
        let diagnostic = sandbox.failure_hint(&String::from_utf8_lossy(&output.stderr));
        assert!(diagnostic.contains("沙箱") && diagnostic.contains("登记工作区"));
        assert!(diagnostic.contains(target.file_name().unwrap().to_str().unwrap()));
    }
    // A dynamically composed path cannot be protected by scanning shell text.
    let output = execute("/usr/bin/python3 -c 'import pathlib; p=pathlib.Path.cwd().parents[2]/(\"set\"+\"tings.json\"); p.write_text(\"escape\")'");
    assert!(!output.status.success());
    assert!(!app.join("settings.json").exists());
    assert!(sandbox
        .profile()
        .contains(&ws.canonicalize().unwrap().to_string_lossy().to_string()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn m224_invalid_roots_and_unavailable_runtime_fail_closed() {
    assert!(
        CommandSandbox::new(&PathBuf::from("relative"), vec![], NetworkPolicy::Enabled).is_err()
    );
    assert!(command_sandbox::probe_program("/m224-missing-sandbox-exec").is_err());
}
