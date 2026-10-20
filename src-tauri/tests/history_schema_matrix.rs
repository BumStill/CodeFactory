// SPDX-License-Identifier: Apache-2.0
//! R1–R4: exercise the installed formal binary, not a mock admission path.
#[test]
fn historical_schema_matrix_preserves_identity() {
    let root = tempfile::tempdir().unwrap();
    let receipt = root.path().join("receipt.json");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_codefactory"))
        .arg("--history-session-smoke")
        .arg(&receipt)
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
    let matrix = value["schema_matrix"].as_array().expect("missing old-schema matrix");
    assert_eq!(matrix.len(), 4);
    for (row, version) in matrix.iter().zip([1, 6, 10, 19]) {
        assert_eq!(row["source_version"], version);
        assert_eq!(row["migrated"], true);
        assert_eq!(row["identity_preserved"], true);
        assert_eq!(row["reopen_idempotent"], true);
        assert_eq!(row["side_effect_count"], 0);
        assert_eq!(row["human_prompt_count"], 0);
        assert_eq!(row["outcome"], "continued");
    }
    assert_eq!(value["cleanup_ok"], true);
}
