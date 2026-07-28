// SPDX-License-Identifier: Apache-2.0
//! Regression tests for Windows command-window flashes.
//!
//! In a Tauri GUI app on Windows, spawning console programs without
//! `CREATE_NO_WINDOW` creates a visible black console window for a moment. That
//! is acceptable only for the embedded terminal/PTY feature; background tool,
//! probe, git, hook, and helper commands must opt into `NoWindow`.

use std::path::Path;

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri parent")
        .to_path_buf()
}

fn line_has_command_new(line: &str) -> bool {
    line.contains("Command::new(") || line.contains("process::Command::new(")
}

fn allow_bare_command(_path: &Path, snippet: &str) -> bool {
    // Test fixtures are allowed to spawn the current test executable directly.
    if snippet.contains("#[test]")
        || snippet.contains("#[tokio::test")
        || snippet.contains("current_exe()")
        || snippet.contains("command-that-does-not-exist")
    {
        return true;
    }

    // Comments in tests/documentation mention Command::new patterns without
    // spawning anything.
    if snippet.contains("production_gh_git_spawns_go_through_dev_command")
        || snippet.contains("\"Command::new(")
        || snippet.lines().all(|line| line.trim_start().starts_with("//"))
    {
        return true;
    }

    false
}

#[test]
fn production_background_commands_use_no_window() {
    let root = repo_root().join("src-tauri/src");
    let mut offenders = Vec::new();
    scan_rs_files(&root, &mut |path, source| {
        let lines: Vec<&str> = source.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if !line_has_command_new(line) || line.trim_start().starts_with("//") {
                continue;
            }
            let end = usize::min(lines.len(), idx + 10);
            let snippet = lines[idx..end].join("\n");
            if snippet.contains(".no_window()") || allow_bare_command(path, &snippet) {
                continue;
            }
            offenders.push(format!(
                "{}:{}: {}",
                path.strip_prefix(repo_root()).unwrap_or(path).display(),
                idx + 1,
                line.trim()
            ));
        }
    });

    assert!(
        offenders.is_empty(),
        "Windows background commands must call `.no_window()` before output/status/spawn; offenders:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn no_window_trait_sets_create_no_window_for_std_and_tokio_commands() {
    let source = include_str!("../src/util/no_window.rs");
    assert!(source.contains("const CREATE_NO_WINDOW: u32 = 0x0800_0000"));
    assert!(source.contains("impl NoWindow for std::process::Command"));
    assert!(source.contains("impl NoWindow for tokio::process::Command"));
    assert!(source.contains("self.creation_flags(CREATE_NO_WINDOW)"));
}

fn scan_rs_files(dir: &Path, visit: &mut impl FnMut(&Path, &str)) {
    for entry in std::fs::read_dir(dir).expect("read source directory") {
        let entry = entry.expect("source entry");
        let path = entry.path();
        if path.is_dir() {
            scan_rs_files(&path, visit);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let source = std::fs::read_to_string(&path).expect("read rust source");
            visit(&path, &source);
        }
    }
}
