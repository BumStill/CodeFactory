// SPDX-License-Identifier: Apache-2.0
use std::env;
use std::ffi::OsString;
use std::process::Command as StdCommand;

use tokio::process::Command;

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

fn developer_path() -> Option<OsString> {
    if EXTRA_PATHS.is_empty() {
        return None;
    }

    let mut paths: Vec<std::path::PathBuf> =
        EXTRA_PATHS.iter().map(std::path::PathBuf::from).collect();
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).ok()
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
    StdCommand::new(program).arg("--version").output().is_ok()
        || StdCommand::new(program)
            .arg("-c")
            .arg("exit 0")
            .output()
            .is_ok()
}
