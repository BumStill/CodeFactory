// SPDX-License-Identifier: Apache-2.0
//! Proof that an action stays entirely inside the session's own managed
//! workspace (M41, CF-TPP-R1 / CF-TPP-R2).
//!
//! Trusted mode means "the user trusts the agent to do whatever it needs
//! **inside this task's own workspace**". It does not mean "anything goes".
//! Deleting the directory an agent generated, overwriting a file it created,
//! or running a script that lives in the workspace are all part of the loop the
//! user already approved, so they must not re-prompt.
//!
//! Everything else keeps the existing confirmation. The rules below are
//! deliberately narrow — unknown syntax, `..`, symlinks that leave the
//! workspace, credentials, repository metadata, absolute paths, and the
//! workspace root itself all answer `false` and fall back to the normal gate.
//! A wrong `true` here silently widens an automatic-allow rule across a
//! security boundary, so the default answer is always `false`.

use std::path::{Component, Path};

/// Programs that only ever mutate the targets named on their own command line.
const LOCAL_MUTATORS: &[&str] = &["rm", "rmdir", "unlink", "remove-item"];
/// Interpreters whose single script operand must be an existing file that
/// resolves inside the workspace.
const LOCAL_RUNNERS: &[&str] = &["bash", "sh", "zsh", "dash", "node", "python", "python3"];
/// Names that are never "the workspace's own content": repository metadata and
/// credentials travel with the repo *outside* the workspace's ownership.
const NEVER_LOCAL: &[&str] = &[".git", ".env", ".ssh", ".aws", ".gnupg", ".netrc"];

/// Deleting only what is inside the workspace (`false` = ask as usual).
pub(super) fn local_delete(root: &Path, command: &str) -> bool {
    let Some(argv) = tokenize(command) else {
        return false;
    };
    let Some((program, operands)) = argv.split_first() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let program = program.to_ascii_lowercase();

    if LOCAL_MUTATORS.contains(&program.as_str()) {
        let mut targets = 0;
        for operand in operands {
            if let Some(flag) = operand.strip_prefix('-') {
                // Only the flags this module has reviewed; anything else is
                // unknown syntax and asks.
                let reviewed = matches!(
                    flag.to_ascii_lowercase().as_str(),
                    "r" | "f" | "d" | "v" | "rf" | "fr" | "recurse" | "recursive" | "force"
                );
                if !reviewed {
                    return false;
                }
                continue;
            }
            targets += 1;
            if !path_stays_inside(root, operand) || existing_inside(root, operand).is_none() {
                // Deleting something that is not there is not provably the
                // workspace's own content, so it asks.
                return false;
            }
        }
        return targets > 0;
    }

    if LOCAL_RUNNERS.contains(&program.as_str()) {
        let mut scripts: Vec<&str> = Vec::new();
        for operand in operands {
            if operand.starts_with('-') {
                continue;
            }
            scripts.push(operand);
        }
        if scripts.len() != 1 {
            return false;
        }
        let script = scripts[0];
        let Some(candidate) = existing_inside(root, script) else {
            return false;
        };
        return candidate.is_file();
    }

    false
}

/// Overwriting only what is inside the workspace (`false` = ask as usual).
pub(super) fn local_write(root: &Path, path: &str) -> bool {
    path_stays_inside(root, path)
}

/// Split into shell words, or `None` when the command contains any syntax this
/// module does not fully understand.
fn tokenize(command: &str) -> Option<Vec<String>> {
    const REJECTED: &[&str] = &[
        ";", "&&", "||", "|", "&", ">", "<", "`", "$", "(", ")", "{", "}", "*", "?", "[", "]",
        "~", "\n", "\r", "\"", "'", "\\", "!", "#",
    ];
    if command.trim().is_empty() || REJECTED.iter().any(|token| command.contains(token)) {
        return None;
    }
    let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        None
    } else {
        Some(words)
    }
}

/// Is `raw` a relative path whose every effect stays inside `root`?
fn path_stays_inside(root: &Path, raw: &str) -> bool {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('~') {
        return false;
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return false;
    }
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if name.starts_with('$') || never_local(&name) {
                    return false;
                }
            }
            // `..`, a drive/UNC prefix, or a root: never the workspace's own.
            _ => return false,
        }
    }
    resolve_inside(root, raw).is_some()
}

/// The fully-resolved path, when it already exists strictly inside `root`.
fn existing_inside(root: &Path, raw: &str) -> Option<std::path::PathBuf> {
    let root = std::fs::canonicalize(root).ok()?;
    let real = std::fs::canonicalize(root.join(Path::new(raw))).ok()?;
    (real != root && real.starts_with(&root)).then_some(real)
}

/// Resolve `raw` against `root`, following symlinks, and require the result to
/// be strictly inside `root`. Non-existent tails are allowed only when the
/// deepest existing ancestor already resolves inside `root`.
fn resolve_inside(root: &Path, raw: &str) -> Option<std::path::PathBuf> {
    let root = std::fs::canonicalize(root).ok()?;
    let mut probe = root.join(Path::new(raw));
    loop {
        match std::fs::canonicalize(&probe) {
            // The workspace root itself is not "inside" the workspace: wiping
            // the whole root is a different decision than cleaning up inside it.
            Ok(real) => return (real != root && real.starts_with(&root)).then_some(real),
            Err(_) => {
                probe = probe.parent()?.to_path_buf();
                if probe == root {
                    return Some(probe);
                }
                if !probe.starts_with(&root) {
                    return None;
                }
            }
        }
    }
}

fn never_local(name: &str) -> bool {
    NEVER_LOCAL
        .iter()
        .any(|reserved| name == *reserved || name.starts_with(&format!("{reserved}.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: std::path::PathBuf,
        root: std::path::PathBuf,
        outside: std::path::PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cf-m41-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let root = dir.join("workspace");
            let outside = dir.join("checkout");
            std::fs::create_dir_all(root.join("generated")).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(root.join("build.sh"), "#!/bin/sh\necho ok\n").unwrap();
            std::fs::write(root.join("notes.txt"), "draft\n").unwrap();
            Fixture {
                dir,
                root,
                outside,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// CF-TPP-R1: the loop the user already approved must not re-prompt.
    #[test]
    fn r1_workspace_local_actions_are_allowed() {
        let fixture = Fixture::new("r1");
        for command in [
            "rm -rf generated",
            "rm -rf ./generated",
            "rm -r generated",
            "Remove-Item -Recurse -Force generated",
            "rmdir generated",
            "bash build.sh",
            "sh build.sh",
        ] {
            assert!(
                local_delete(&fixture.root, command),
                "in-workspace action must not prompt: {command}"
            );
        }
        for path in ["notes.txt", "generated/plot.png", "./notes.txt"] {
            assert!(
                local_write(&fixture.root, path),
                "in-workspace overwrite must not prompt: {path}"
            );
        }
    }

    /// CF-TPP-R2: nothing outside the workspace, and nothing the parser cannot
    /// prove, may become an automatic allow.
    #[test]
    fn r2_counterexamples_still_ask() {
        let fixture = Fixture::new("r2");
        let outside_file = fixture.outside.join("main.rs");
        std::fs::write(&outside_file, "fn main() {}\n").unwrap();

        // Escapes and outside targets.
        let escaped = [
            "rm -rf ../checkout".to_string(),
            "rm -rf ../../".to_string(),
            "rm -rf /tmp".to_string(),
            "rm -rf ~/Library".to_string(),
            format!("rm -rf {}", fixture.outside.display()),
            "rm -rf ./generated/../../checkout".to_string(),
            "bash ../checkout/script.sh".to_string(),
            format!("bash {}", outside_file.display()),
        ];
        for command in &escaped {
            assert!(
                !local_delete(&fixture.root, command),
                "outside-workspace action must still ask: {command}"
            );
        }

        // Credentials, repository metadata, system settings.
        for command in [
            "rm -rf .git",
            "rm -rf generated/.env",
            "rm -rf .ssh",
            "git push origin main",
            "git push origin master",
            "security delete-generic-password",
            "reg delete HKCU",
            "defaults write com.apple.dock autohide -bool true",
            "sudo rm -rf generated",
            "rm -rf $HOME",
        ] {
            assert!(
                !local_delete(&fixture.root, command),
                "must never auto-allow: {command}"
            );
        }

        // Unknown or composed syntax fails closed.
        for command in [
            "rm -rf *",
            "rm -rf .",
            "rm -rf generated; rm -rf ../checkout",
            "rm -rf generated && git push origin main",
            "rm -rf generated | tee log",
            "rm -rf \"generated\"",
            "rm -rf $(pwd)",
            "bash build.sh extra.sh",
        ] {
            assert!(
                !local_delete(&fixture.root, command),
                "unknown syntax must ask: {command}"
            );
        }

        for path in [
            "../checkout/main.rs",
            "/etc/hosts",
            "~/.zshrc",
            ".git/config",
            "src/.env",
        ] {
            assert!(
                !local_write(&fixture.root, path),
                "outside-workspace overwrite must still ask: {path}"
            );
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&fixture.outside, fixture.root.join("escape")).unwrap();
            std::fs::write(fixture.outside.join("seed.txt"), "x\n").unwrap();
            for command in ["rm -rf escape", "rm -rf escape/new", "bash escape/script.sh"] {
                assert!(
                    !local_delete(&fixture.root, command),
                    "symlink escape must ask: {command}"
                );
            }
            assert!(
                !local_write(&fixture.root, "escape/seed.txt"),
                "symlink escape write must ask"
            );
        }
    }

    /// A runner with a script that does not exist inside the workspace is not
    /// provable, so it asks.
    #[test]
    fn r2_missing_script_asks() {
        let fixture = Fixture::new("r2-missing");
        assert!(!local_delete(&fixture.root, "bash script.sh"));
        assert!(!local_delete(&fixture.root, "rm -rf not-generated"));
        assert!(!local_delete(&fixture.root, "rm"));
        assert!(!local_delete(&fixture.root, ""));
    }
}
