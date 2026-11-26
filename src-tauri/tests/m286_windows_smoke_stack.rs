// SPDX-License-Identifier: Apache-2.0
use std::fs;

#[test]
fn windows_msvc_binary_reserves_enough_main_thread_stack_for_unattended_smoke() {
    let build = fs::read_to_string("build.rs").expect("read build.rs");
    assert!(build.contains("CARGO_CFG_TARGET_OS"));
    assert!(build.contains("CARGO_CFG_TARGET_ENV"));
    assert!(build.contains("/STACK:8388608"));
    assert!(build.contains("Ok(\"windows\")"));
    assert!(build.contains("Ok(\"msvc\")"));
}
