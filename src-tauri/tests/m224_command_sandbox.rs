// SPDX-License-Identifier: Apache-2.0
//! M224: executable acceptance of the shared process sandbox (no desktop needed).
#[path = "../src/command_sandbox.rs"]
mod command_sandbox;
use command_sandbox::{CommandSandbox, NetworkPolicy, WritablePath};
use std::path::PathBuf;

/// 集成测试独立判定嵌套沙箱（`#[path]` 引入的模块拿不到库内 util）。
fn nested_sandbox_refused() -> bool {
    std::process::Command::new(command_sandbox::MACOS_SANDBOX_EXEC)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .output()
        .map(|output| {
            !output.status.success()
                && String::from_utf8_lossy(&output.stderr).contains("sandbox_apply")
        })
        .unwrap_or(false)
}

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
    // macOS 不允许嵌套沙箱：会话本身在沙箱里跑全量测试时跳过，宿主侧仍执行。
    if nested_sandbox_refused() {
        eprintln!("skip: macOS 拒绝嵌套沙箱（本用例需要新建沙箱）");
        return;
    }
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
        vec![],
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
        CommandSandbox::new(
            &PathBuf::from("relative"),
            vec![],
            vec![],
            NetworkPolicy::Enabled
        )
        .is_err()
    );
    assert!(command_sandbox::probe_program("/m224-missing-sandbox-exec").is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn m224_policy_variants_and_accessors_are_observable() {
    let root = std::env::temp_dir().join(format!("m224-api-{}", std::process::id()));
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let extra = root.join("extra-file");
    std::fs::write(&extra, b"x").unwrap();
    let other = root.join("other-file");
    std::fs::write(&other, b"x").unwrap();

    let sandbox = CommandSandbox::new(
        &ws,
        vec![WritablePath::literal(other.clone())],
        vec![],
        NetworkPolicy::Disabled,
    )
    .unwrap();

    let canonical_ws = ws.canonicalize().unwrap();
    let canonical_other = other.canonicalize().unwrap();
    assert_eq!(sandbox.workspace(), canonical_ws.as_path());
    assert_eq!(
        sandbox.runtime(),
        std::path::Path::new(command_sandbox::MACOS_SANDBOX_EXEC)
    );
    assert_eq!(sandbox.network_policy(), NetworkPolicy::Disabled);

    let declared = sandbox.writable();
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].path(), canonical_other.as_path());
    assert!(!declared[0].is_recursive());

    let profile = sandbox.profile();
    assert!(!profile.contains("(allow network*)"));
    assert!(profile.contains(&format!(
        "(allow file-write* (literal \"{}\"))",
        canonical_ws.display()
    )));
    assert!(profile.contains(&format!(
        "(allow file-write* (literal \"{}\"))",
        canonical_other.display()
    )));
    assert!(
        !profile.contains(&format!("(subpath \"{}\")", canonical_other.display())),
        "literal entries must not widen to a subtree"
    );

    let literal_entry = WritablePath::literal(other.clone());
    assert!(!literal_entry.is_recursive());
    assert_eq!(literal_entry.path(), other.as_path());
    let tree_entry = WritablePath::tree(ws.clone());
    assert!(tree_entry.is_recursive());

    std::fs::remove_dir_all(root).unwrap();
}
