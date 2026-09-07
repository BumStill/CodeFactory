// SPDX-License-Identifier: Apache-2.0
//! Compile the actual formal main against a recording facade. This checks Rust
//! dispatch semantics, not a substring that a comment could accidentally satisfy.

use std::process::Command;

fn compile(command: &mut Command) {
    let output = command
        .output()
        .expect("start rustc for dispatcher fixture");
    assert!(
        output.status.success(),
        "dispatcher fixture did not compile: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn formal_main_dispatches_unattended_first_and_preserves_other_entrypoint_order() {
    let sandbox = tempfile::tempdir().expect("create dispatcher fixture");
    let library = sandbox.path().join("lib.rs");
    let main = sandbox.path().join("main.rs");
    let rlib = sandbox.path().join("libcodefactory_lib.rlib");
    let binary = sandbox.path().join(if cfg!(windows) {
        "dispatcher.exe"
    } else {
        "dispatcher"
    });
    std::fs::write(&main, include_str!("../src/main.rs")).expect("copy actual formal main");
    std::fs::write(
        &library,
        r#"
fn dispatch(name: &str, flag: &str) -> bool {
    println!("{name}");
    std::env::args().nth(1).as_deref() == Some(flag)
}
pub mod unattended_smoke_cli {
    pub fn run() -> bool {
        println!("unattended");
        matches!(std::env::args().nth(1).as_deref(),
            Some("--unattended-long-task-smoke" | "--unattended-long-task-worker"))
    }
}
// The legacy symbol lets the pre-fix main compile, so the regression fails on
// the observed dispatch order rather than a missing fixture symbol.
pub fn run_unattended_long_task_smoke_cli() -> bool { unattended_smoke_cli::run() }
pub fn run_history_session_smoke_cli() -> bool { dispatch("history", "--history-session-smoke") }
pub fn run_delivery_recovery_smoke_cli() -> bool { dispatch("delivery", "--delivery-recovery-smoke") }
pub fn run_managed_workspace_cleanup_smoke_cli() -> bool { dispatch("cleanup", "--managed-workspace-cleanup-smoke") }
pub fn run_evolution_smoke_cli() -> bool { dispatch("evolution", "--evolution-smoke") }
pub fn run_browser_session_smoke_cli() -> bool { dispatch("browser-session", "--browser-session-smoke") }
pub fn run_update_upgrade_smoke_cli() -> bool { dispatch("upgrade", "--update-upgrade-smoke") }
pub fn run_browser_chrome_attach_smoke_cli() -> bool { dispatch("chrome-attach", "--browser-chrome-attach-smoke") }
pub fn run_headless_smoke_cli() -> bool { dispatch("headless", "--headless-smoke") }
pub fn run() { println!("app"); }
"#,
    )
    .expect("write recording facade");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    compile(
        Command::new(&rustc)
            .args([
                "--edition=2021",
                "--crate-type=rlib",
                "--crate-name=codefactory_lib",
            ])
            .arg(&library)
            .arg("-o")
            .arg(&rlib),
    );
    compile(
        Command::new(&rustc)
            .arg("--edition=2021")
            .arg(&main)
            .arg("--extern")
            .arg(format!("codefactory_lib={}", rlib.display()))
            .arg("-o")
            .arg(&binary),
    );

    for (flag, expected) in [
        ("--unattended-long-task-smoke", "unattended\n"),
        ("--unattended-long-task-worker", "unattended\n"),
        ("--history-session-smoke", "unattended\nhistory\n"),
        (
            "--delivery-recovery-smoke",
            "unattended\nhistory\ndelivery\n",
        ),
        (
            "--not-a-smoke",
            "unattended\nhistory\ndelivery\ncleanup\nevolution\nbrowser-session\nupgrade\nchrome-attach\nheadless\napp\n",
        ),
    ] {
        let output = Command::new(&binary)
            .arg(flag)
            .output()
            .expect("run dispatcher fixture");
        assert!(output.status.success(), "dispatcher failed for {flag}");
        assert_eq!(
            String::from_utf8(output.stdout)
                .expect("dispatcher stdout must be UTF-8")
                .replace("\r\n", "\n"),
            expected,
            "unexpected dispatch order for {flag}"
        );
    }
}
