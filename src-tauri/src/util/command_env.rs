// SPDX-License-Identifier: Apache-2.0
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use tokio::process::Command;

use super::no_window::NoWindow;

/// GUI-launched desktop apps on macOS do not inherit a login-shell PATH.
/// Prepending the common developer-tool locations keeps `node`, `npm`, `pnpm`,
/// `cargo`, and similar tools available without overriding the user's PATH.
#[cfg(target_os = "macos")]
const EXTRA_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

#[cfg(all(unix, not(target_os = "macos")))]
const EXTRA_PATHS: &[&str] = &["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"];

#[cfg(windows)]
const EXTRA_PATHS: &[&str] = &[];

pub fn apply_developer_path(cmd: &mut Command) {
    if let Some(path) = developer_path() {
        cmd.env("PATH", path);
    }
}

pub fn apply_developer_path_std(cmd: &mut StdCommand) {
    if let Some(path) = developer_path() {
        cmd.env("PATH", path);
    }
}

fn developer_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = EXTRA_PATHS.iter().map(PathBuf::from).collect();
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    paths
}

fn developer_path() -> Option<OsString> {
    env::join_paths(developer_paths()).ok()
}

/// Resolve a developer CLI using the same augmented PATH given to shell tools.
/// This matters for GUI-launched apps on macOS, where `/opt/homebrew/bin` is
/// commonly absent from the inherited environment.
pub fn resolve_developer_command(program: &str) -> PathBuf {
    let requested = Path::new(program);
    if requested.components().count() > 1 {
        return requested.to_path_buf();
    }

    #[cfg(windows)]
    let executable_name = if requested.extension().is_none() {
        format!("{program}.exe")
    } else {
        program.to_owned()
    };
    #[cfg(not(windows))]
    let executable_name = program.to_owned();

    for directory in developer_paths() {
        let candidate = directory.join(&executable_name);
        if candidate.is_file() {
            return candidate;
        }
    }

    #[cfg(windows)]
    {
        let known_roots = [
            env::var_os("ProgramFiles").map(PathBuf::from),
            env::var_os("LOCALAPPDATA").map(PathBuf::from),
        ];
        for root in known_roots.into_iter().flatten() {
            let candidate = if root.ends_with("Local") {
                root.join("Programs").join("GitHub CLI").join("gh.exe")
            } else {
                root.join("GitHub CLI").join("gh.exe")
            };
            if program.eq_ignore_ascii_case("gh") && candidate.is_file() {
                return candidate;
            }
        }
    }

    PathBuf::from(executable_name)
}

pub struct ShellInvocation {
    pub program: &'static str,
    pub args: Vec<String>,
}

pub fn shell_invocation(command: &str) -> ShellInvocation {
    #[cfg(windows)]
    {
        if command_exists("powershell.exe") {
            return ShellInvocation {
                program: "powershell.exe",
                args: vec![
                    "-NonInteractive".into(),
                    "-NoProfile".into(),
                    "-Command".into(),
                    command.into(),
                ],
            };
        }
        return ShellInvocation {
            program: "cmd.exe",
            args: vec!["/C".into(), command.into()],
        };
    }

    #[cfg(unix)]
    {
        if command_exists("/bin/zsh") {
            return ShellInvocation {
                program: "/bin/zsh",
                args: vec!["-lc".into(), command.into()],
            };
        }
        ShellInvocation {
            program: "/bin/sh",
            args: vec!["-lc".into(), command.into()],
        }
    }
}

fn command_exists(program: &str) -> bool {
    StdCommand::new(program)
        .no_window()
        .arg("--version")
        .output()
        .is_ok()
        || StdCommand::new(program)
            .no_window()
            .arg("-c")
            .arg("exit 0")
            .output()
            .is_ok()
}
