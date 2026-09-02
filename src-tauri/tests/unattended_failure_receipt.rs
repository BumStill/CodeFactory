// SPDX-License-Identifier: Apache-2.0

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn formal_binary_writes_a_safe_failure_receipt_before_exit_one() {
    let sandbox = tempfile::tempdir().expect("create failure-receipt sandbox");
    let invalid_temp_root = sandbox.path().join("not-a-directory");
    let receipt_path = sandbox.path().join("raw-receipt.json");
    std::fs::write(&invalid_temp_root, b"synthetic fixture boundary\n")
        .expect("create invalid temporary root");

    let mut child = Command::new(env!("CARGO_BIN_EXE_codefactory"))
        .args([
            "--unattended-long-task-smoke",
            receipt_path.to_str().unwrap(),
        ])
        .env("TMPDIR", &invalid_temp_root)
        .env("TEMP", &invalid_temp_root)
        .env("TMP", &invalid_temp_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start formal CodeFactory binary");

    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll formal binary") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("formal binary did not finish its synthetic failure within 15 seconds");
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture formal binary stderr")
        .read_to_string(&mut stderr)
        .expect("read formal binary stderr");
    assert_eq!(status.code(), Some(1), "unexpected stderr: {stderr}");

    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&receipt_path).expect("failure receipt must exist before process exit"),
    )
    .expect("failure receipt must be valid JSON");
    assert_eq!(receipt["ok"], false);
    assert_eq!(receipt["error"], "unattended_smoke_failed");
    assert_eq!(receipt["observation_schema_version"], 1);
    assert_eq!(receipt["case_id"], "E2E-001");
    assert_eq!(receipt["descendant_process_count"], 0);
    assert_eq!(receipt["cleanup_attempted"], false);
    assert_eq!(receipt["orphan_sweep_performed"], false);
    let expected_leaked_resource_count = if cfg!(windows) { 0 } else { 1 };
    assert_eq!(
        receipt["leaked_resource_count"],
        expected_leaked_resource_count
    );
    assert_eq!(receipt["cleanup_ok"], false);

    let rendered = receipt.to_string();
    for forbidden in [
        sandbox.path().to_string_lossy(),
        invalid_temp_root.to_string_lossy(),
        "token=".into(),
        "TMPDIR".into(),
    ] {
        assert!(
            !rendered.contains(forbidden.as_ref()),
            "failure receipt leaked {forbidden}"
        );
    }
}
