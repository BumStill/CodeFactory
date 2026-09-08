// SPDX-License-Identifier: Apache-2.0

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn formal_chrome_attach_failure_receipt_reports_runtime_scope_not_artifact_provenance() {
    let sandbox = tempfile::tempdir().expect("create runtime-receipt sandbox");
    let receipt_path = sandbox.path().join("runtime-receipt.json");
    // A missing fixture fails before browser download, launch, bridge startup,
    // or Tauri initialization. Never use the developer's existing browser.
    let mut child = Command::new(env!("CARGO_BIN_EXE_codefactory"))
        .arg("--browser-chrome-attach-smoke")
        .arg(&receipt_path)
        .env_remove("CODEFACTORY_BROWSER_CHROME_FIXTURE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
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
            panic!("Chrome attachment failure did not finish within 15 seconds");
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
        &std::fs::read(&receipt_path).expect("failure receipt must exist before exit"),
    )
    .expect("failure receipt must be valid JSON");
    assert_eq!(receipt["scenario_id"], "RTE-003");
    assert_eq!(receipt["status"], "failed");
    assert_eq!(receipt["evidence_level"], "native_runtime_smoke");
    assert!(receipt["error"]
        .as_str()
        .expect("failure diagnostic must be a string")
        .contains("CODEFACTORY_BROWSER_CHROME_FIXTURE"));
    assert!(!receipt.to_string().contains("exact_release_artifact"));
}
