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

pub fn apply_noninteractive(cmd: &mut Command) {
    cmd.stdin(std::process::Stdio::null())
        .env("CI", "1")
        .env("npm_config_yes", "true")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
}

/// Only the final nonempty output line can be an outstanding input prompt.
pub fn waiting_for_input(output: &str) -> bool {
    let line = output.trim_end().rsplit('\n').next().unwrap_or("").trim().to_lowercase();
    line.contains("press any key")
        || line.ends_with("(y/n)") || line.ends_with("[y/n]")
        || line.ends_with("(y/n):") || line.ends_with("[y/n]:")
        || (line.starts_with("read-host") && line.ends_with(':'))
}

pub const INPUT_REQUIRED_MESSAGE: &str = "命令正在等待输入，已结束并清理进程树。请使用非交互参数（如 --yes、--no-input），不要使用 Read-Host 或交互确认后重试。";

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
        let (pwsh, powershell) = windows_shell_availability();
        return windows_shell_invocation(command, pwsh, powershell);
    }

    #[cfg(unix)]
    {
        // bash first, deliberately — even on macOS where the user's login
        // shell is zsh.
        //
        // The commands here are written by the model, and every prompt, doc
        // and example in this project is POSIX/bash-flavoured. zsh reads that
        // dialect differently, sometimes loudly and sometimes not:
        //   - `status=…` fails outright; it is a read-only special (`$?`), and
        //     also the most natural name for a CI-polling script's variable.
        //     Eight scripts died on this in one 2026-07 session. `emulate sh`
        //     does not lift it.
        //   - an unmatched glob aborts the entire command instead of passing
        //     through, so later steps never run.
        //   - unquoted `$var` does NOT word-split, so `for f in $files` walks
        //     one item instead of N — no error, just a wrong answer.
        //
        // PATH does not depend on this choice: `apply_developer_path` sets it
        // explicitly, overriding whatever the login shell would compute.
        if command_exists("/bin/bash") {
            return ShellInvocation {
                program: "/bin/bash",
                args: vec!["-lc".into(), command.into()],
            };
        }
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

/// Cache executable discovery, not a process launch on every tool definition.
#[cfg(windows)]
fn windows_shell_availability() -> (bool, bool) {
    static AVAILABLE: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let exists = |program: &str| resolve_developer_command(program).is_file();
        (exists("pwsh.exe"), exists("powershell.exe"))
    })
}

#[cfg(any(windows, test))]
pub(crate) fn windows_shell_invocation(command: &str, pwsh: bool, powershell: bool) -> ShellInvocation {
    if pwsh || powershell {
        ShellInvocation {
            program: if pwsh { "pwsh.exe" } else { "powershell.exe" },
            args: vec!["-NonInteractive".into(), "-NoProfile".into(), "-Command".into(),
                format!("[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); $OutputEncoding=[Console]::OutputEncoding; {command}")],
        }
    } else {
        ShellInvocation { program: "cmd.exe", args: vec!["/D".into(), "/C".into(), format!("chcp 65001 >nul & {command}")] }
    }
}

pub fn shell_description() -> String {
    #[cfg(windows)]
    {
        let (pwsh, powershell) = windows_shell_availability();
        if pwsh {
            return "Current shell: PowerShell 7 (pwsh.exe); &&/|| supported. Double-quote paths; environment variables: $env:NAME.".into();
        }
        if powershell {
            return "Current shell: Windows PowerShell 5.1 (powershell.exe); &&/|| are NOT supported. Use ; and if ($?) { ... } for conditional success, if (-not $?) { ... } for failure. Double-quote paths; environment variables: $env:NAME.".into();
        }
        return "Current shell: cmd.exe; &&/|| supported; double-quote paths; environment variables: %NAME%.".into();
    }
    #[cfg(not(windows))]
    format!("Current shell: {} (POSIX syntax).", shell_invocation("").program)
}

/// Keep original command and exit status; never rewrite arbitrary script syntax.
pub fn shell_failure_hint(program: &str, output: &str) -> &'static str {
    if !output.contains("ParserError") && !output.contains("InvalidEndOfLine") {
        return "";
    }
    match program {
        "powershell.exe" => "\n当前是 Windows PowerShell 5.1，不支持 &&/||；用 ; 与 if ($?) { … }（失败用 if (-not $?) { … }）修正后重试。路径用双引号，环境变量用 $env:NAME。\n",
        "pwsh.exe" => "\n当前是 PowerShell 7；请检查引号、括号和 PowerShell 语法后重试。\n",
        _ => "",
    }
}

pub fn decode_output(bytes: &[u8]) -> String {
    #[cfg(windows)]
    {
        // CP_ACP (0) means the current Windows system ANSI code page.
        decode_output_for_code_page(bytes, 0)
    }
    #[cfg(not(windows))]
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(any(windows, test))]
pub(crate) fn decode_output_for_code_page(bytes: &[u8], code_page: u32) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    #[cfg(windows)]
    {
        // MultiByteToWideChar(CP_ACP) uses GetACP semantics without assuming CP936.
        #[link(name = "kernel32")]
        extern "system" {
            fn MultiByteToWideChar(cp: u32, flags: u32, input: *const u8, len: i32, output: *mut u16, size: i32) -> i32;
        }
        if let Ok(len) = i32::try_from(bytes.len()) {
            unsafe {
                let size = MultiByteToWideChar(code_page, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
                if size > 0 {
                    let mut wide = vec![0u16; size as usize];
                    let written = MultiByteToWideChar(code_page, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), size);
                    if written > 0 { return String::from_utf16_lossy(&wide[..written as usize]); }
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let encoding = if code_page == 936 { encoding_rs::GBK } else { encoding_rs::WINDOWS_1252 };
        return encoding.decode(bytes).0.into_owned();
    }
    #[cfg(windows)]
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod m143_tests {
    use super::*;

    #[test]
    fn m143_noninteractive_environment_and_prompt_detection() {
        let mut cmd = Command::new("unused");
        apply_noninteractive(&mut cmd);
        let env: std::collections::HashMap<_, _> = cmd.as_std().get_envs().map(|(k,v)| (k.to_string_lossy().to_string(), v.unwrap().to_string_lossy().to_string())).collect();
        assert_eq!(env["CI"], "1");
        assert_eq!(env["npm_config_yes"], "true");
        assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
        assert_eq!(env["GCM_INTERACTIVE"], "never");
        for prompt in ["Press any key to continue . . .", "Continue? (y/N)", "Read-Host: enter name:"] {
            assert!(waiting_for_input(prompt), "{prompt}");
        }
        assert!(!waiting_for_input("Read-Host documentation\nall tests passed\n"));
        assert!(!waiting_for_input("ordinary build output"));
    }

    #[test]
    fn m143_windows_shell_prefers_pwsh_and_initializes_utf8() {
        let modern = windows_shell_invocation("echo 中文 && echo ok", true, true);
        assert_eq!(modern.program, "pwsh.exe");
        assert!(modern.args.last().unwrap().contains("[Console]::OutputEncoding"));
        assert!(modern.args.last().unwrap().contains("$OutputEncoding"));
        assert!(modern.args.last().unwrap().ends_with("echo 中文 && echo ok"));
        assert_eq!(windows_shell_invocation("echo 中文", false, true).program, "powershell.exe");
        let cmd = windows_shell_invocation("echo 中文", false, false);
        assert_eq!(cmd.program, "cmd.exe");
        assert!(cmd.args.last().unwrap().starts_with("chcp 65001 >nul & "));
    }

    #[test]
    fn m143_decodes_utf8_and_legacy_chinese_without_replacement() {
        assert_eq!(decode_output_for_code_page("中文🙂".as_bytes(), 936), "中文🙂");
        assert_eq!(decode_output_for_code_page(&[0xd6, 0xd0, 0xce, 0xc4], 936), "中文");
        assert_eq!(decode_output_for_code_page(&[0xe9], 1252), "é");
    }

    #[test]
    fn m143_legacy_parser_failure_has_actionable_advice() {
        let hint = shell_failure_hint("powershell.exe", "ParserError: InvalidEndOfLine");
        assert!(hint.contains("5.1"));
        assert!(hint.contains("if ($?)"));
        assert!(hint.contains("$env:NAME"));
        assert!(shell_failure_hint("pwsh.exe", "ParserError").contains("PowerShell 7"));
        assert!(shell_failure_hint("powershell.exe", "ordinary error").is_empty());
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
