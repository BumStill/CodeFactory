// SPDX-License-Identifier: Apache-2.0
//! M250 regression: real process output, plus observed Seatbelt diagnostics.
//! Run directly with rustc --test as well as through cargo test.
#[path = "../src/command_sandbox.rs"]
mod command_sandbox;
use command_sandbox::{CommandSandbox, NetworkPolicy};

#[cfg(target_os = "macos")]
fn sandbox() -> CommandSandbox {
    CommandSandbox::new(&std::env::current_dir().unwrap(), vec![], vec![], NetworkPolicy::Enabled).unwrap()
}

#[cfg(target_os = "macos")]
#[test]
fn m250_ordinary_failures_have_no_sandbox_hint() {
    let sandbox = sandbox();
    for (command, code) in [
        ("false", 1),
        ("printf 'sample\\n' | grep absent", 1),
        ("git config --file /dev/null --get m250.missing", 1),
        ("printf 'test assertion failed\\n' >&2; exit 7", 7),
        ("printf 'compiler error: invalid type\\n' >&2; exit 2", 2),
    ] {
        let output = std::process::Command::new("/bin/sh").args(["-c", command]).output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{command}");
        let raw = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert_eq!(sandbox.failure_hint(&raw), "", "ordinary failure: {command}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn m250_observed_denials_keep_hint_and_path() {
    let sandbox = sandbox();
    // Shape observed from an actual denied Git config write in Seatbelt.
    let raw = "error: could not lock config file /synthetic/outside/config: Operation not permitted";
    let hint = sandbox.failure_hint(raw);
    assert!(hint.contains("沙箱拦截") && hint.contains("/synthetic/outside/config"));
    let hint = sandbox.failure_hint("sandbox-exec: sandbox_apply: Operation not permitted");
    assert!(hint.contains("沙箱拦截"));
    assert!(hint.contains("未能从输出解析出具体路径"));
}

#[cfg(target_os = "macos")]
#[test]
fn m250_real_seatbelt_acceptance() {
    let probe = std::process::Command::new(command_sandbox::MACOS_SANDBOX_EXEC)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"]).output().unwrap();
    if !probe.status.success() {
        eprintln!("NOT COVERED: real Seatbelt acceptance unavailable: {}", String::from_utf8_lossy(&probe.stderr));
        return;
    }
    let sandbox = sandbox();
    let target = std::env::temp_dir().join(format!("m250-denied-{}", std::process::id()));
    assert!(!target.exists());
    for (command, denied) in [
        ("false".to_string(), false),
        ("printf sample | grep absent".to_string(), false),
        ("git config --file /dev/null --get m250.missing".to_string(), false),
        (format!("printf forbidden > '{}'", target.display()), true),
    ] {
        let launch = sandbox.wrap("/bin/sh", &["-c".into(), command]);
        let output = std::process::Command::new(launch.program).args(launch.args).output().unwrap();
        assert!(!output.status.success());
        let raw = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let hint = sandbox.failure_hint(&raw);
        assert_eq!(hint.contains("沙箱拦截"), denied, "{raw}");
        if denied { assert!(hint.contains(&target.display().to_string())); }
    }
    assert!(!target.exists());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn m250_platform_coverage_notice() {
    eprintln!("NOT COVERED: Seatbelt process and hint acceptance requires macOS; no sandbox acceptance claimed on this platform");
}
