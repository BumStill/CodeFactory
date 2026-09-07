// SPDX-License-Identifier: Apache-2.0
//! Structural early feedback only: the scenario runner still executes the named
//! test and rejects a zero-test result independently on the real platform.

#[test]
fn required_skill_symlink_case_is_not_compiled_out_on_windows() {
    let source = include_str!("../src/commands/skills.rs").replace("\r\n", "\n");
    let name = "fn symlinked_skill_payload_fails_closed_without_replacing_existing_install()";
    let position = source.find(name).expect("registered scenario test exists");
    let start = source[..position].rfind("\n\n").unwrap();
    let attributes = &source[start..position];
    assert!(attributes.contains("#[test]"));
    assert!(
        !attributes.contains("#[cfg(unix)]"),
        "the required Windows target must not be compiled out by cfg(unix)"
    );
    let end = source[position..].find("\n    }\n").unwrap() + position;
    let body = &source[position..end];
    assert!(body.contains("std::os::windows::fs::symlink_file"));
    assert!(body.contains("std::os::unix::fs::symlink"));
    assert!(!body.contains("/etc/passwd"), "use synthetic payload only");
    assert!(body.contains("outside-sentinel"));
}
