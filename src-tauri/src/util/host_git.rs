// SPDX-License-Identifier: Apache-2.0
//! 宿主运行 git 的唯一入口。
//!
//! 始终关闭 fsmonitor、清空凭据助手后再按需挂上宿主自己的助手，并禁止 ext 协议。
//! hooks 目录若落在任一沙箱可写位置，则改为 `/dev/null`（Windows 上为 `NUL`），
//! 并在当前进程里对每个工作区提示一次。

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::{Mutex, OnceLock};

use crate::util::command_env;
use crate::util::no_window::NoWindow;

static NOTICES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static NOTIFIED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static HOST_HELPERS: OnceLock<Vec<String>> = OnceLock::new();

fn null_hooks() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// 取出尚未展示过的「已跳过工作区内 hooks」提示。每个工作区只产生一次。
pub fn take_notices() -> Vec<String> {
    let mut notices = NOTICES.lock().unwrap_or_else(|error| error.into_inner());
    std::mem::take(&mut *notices)
}

pub fn command() -> HostGit {
    HostGit::new(false)
}

/// 交付与 sync_remote：在清空仓库凭据配置之后，显式挂上宿主全局助手。
pub fn command_with_host_credentials() -> HostGit {
    HostGit::new(true)
}

pub struct HostGit {
    command: Command,
    cwd: Option<PathBuf>,
    host_credentials: bool,
    finalized: bool,
}

impl HostGit {
    fn new(host_credentials: bool) -> Self {
        let program = command_env::resolve_developer_command("git");
        let mut command = Command::new(program).no_window();
        command_env::apply_developer_path_std(&mut command);
        Self {
            command,
            cwd: None,
            host_credentials,
            finalized: false,
        }
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.command.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command.args(args);
        self
    }

    pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Self {
        let dir = dir.as_ref();
        self.cwd = Some(dir.to_path_buf());
        self.command.current_dir(dir);
        self
    }

    pub fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.command.env(key, value);
        self
    }

    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.command.env_remove(key);
        self
    }

    pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stdin(cfg);
        self
    }

    pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stdout(cfg);
        self
    }

    pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stderr(cfg);
        self
    }

    pub fn output(&mut self) -> io::Result<Output> {
        self.finalize();
        self.command.output()
    }

    pub fn status(&mut self) -> io::Result<ExitStatus> {
        self.finalize();
        self.command.status()
    }

    pub fn into_std(mut self) -> Command {
        self.finalize();
        self.command
    }

    pub fn into_tokio(mut self) -> tokio::process::Command {
        self.finalize();
        let mut command = tokio::process::Command::new(self.command.get_program());
        command.args(self.command.get_args());
        if let Some(dir) = &self.cwd {
            command.current_dir(dir);
        }
        for (key, value) in self.command.get_envs() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        command
    }

    pub fn spawn(&mut self) -> io::Result<std::process::Child> {
        self.finalize();
        self.command.spawn()
    }

    /// 给已经需要 `&mut Command` 的交付超时包装用。调用后不要再改参数。
    pub fn finalize_into(&mut self) -> &mut Command {
        self.finalize();
        &mut self.command
    }

    fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        let cwd = self.cwd.clone().or_else(|| dash_c_workdir(&self.command));
        let mut prefix: Vec<OsString> = vec![
            OsString::from("-c"),
            OsString::from("core.fsmonitor="),
            OsString::from("-c"),
            OsString::from("credential.helper="),
            OsString::from("-c"),
            OsString::from("protocol.ext.allow=never"),
        ];
        if self.host_credentials {
            for helper in host_credential_helpers() {
                prefix.push(OsString::from("-c"));
                prefix.push(OsString::from(format!("credential.helper={helper}")));
            }
        }
        if let Some(cwd) = cwd.as_deref() {
            if hooks_are_sandbox_writable(cwd) {
                prefix.push(OsString::from("-c"));
                prefix.push(OsString::from(format!("core.hooksPath={}", null_hooks())));
                note_skipped_hooks(cwd);
            }
        }
        let existing: Vec<OsString> = self.command.get_args().map(|arg| arg.to_os_string()).collect();
        let envs: Vec<(OsString, Option<OsString>)> = self
            .command
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(|value| value.to_os_string())))
            .collect();
        self.command = {
            let program = self.command.get_program().to_os_string();
            let mut rebuilt = Command::new(program).no_window();
            for (key, value) in envs {
                match value {
                    Some(value) => {
                        rebuilt.env(key, value);
                    }
                    None => {
                        rebuilt.env_remove(key);
                    }
                }
            }
            command_env::apply_developer_path_std(&mut rebuilt);
            rebuilt.args(prefix);
            rebuilt.args(existing);
            if let Some(dir) = &self.cwd {
                rebuilt.current_dir(dir);
            }
            rebuilt
        };
    }
}

impl NoWindow for HostGit {
    fn no_window(self) -> Self {
        self
    }
}

fn note_skipped_hooks(cwd: &Path) {
    let key = cwd.to_string_lossy().to_string();
    let mut seen = NOTIFIED.lock().unwrap_or_else(|error| error.into_inner());
    if seen.iter().any(|item| item == &key) {
        return;
    }
    seen.push(key);
    drop(seen);
    NOTICES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push("已跳过工作区内 hooks。这些钩子位于沙箱可写目录，宿主提交不会运行它们。".into());
}

fn dash_c_workdir(command: &Command) -> Option<PathBuf> {
    let args: Vec<OsString> = command.get_args().map(|arg| arg.to_os_string()).collect();
    let mut index = 0;
    while index + 1 < args.len() {
        if args[index] == "-C" {
            return Some(PathBuf::from(&args[index + 1]));
        }
        index += 1;
    }
    None
}

fn hooks_are_sandbox_writable(cwd: &Path) -> bool {
    // 查询时不能带 `-c core.hooksPath=`，否则 git 会把配置值读成空。
    let configured = configured_hooks_path(cwd);
    let Some(configured) = configured.filter(|value| !value.is_empty()) else {
        return false;
    };
    let path = if Path::new(&configured).is_absolute() {
        PathBuf::from(configured)
    } else {
        cwd.join(configured)
    };
    crate::agent::trusted_sandbox::is_sandbox_writable(&path, cwd)
}

fn configured_hooks_path(cwd: &Path) -> Option<String> {
    let program = command_env::resolve_developer_command("git");
    let output = Command::new(program)
        .no_window()
        .arg("-C")
        .arg(cwd)
        .args(["-c", "core.fsmonitor=", "config", "--local", "--get", "core.hooksPath"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn raw_config(cwd: &Path, args: &[&str]) -> Option<String> {
    let program = command_env::resolve_developer_command("git");
    let output = Command::new(program)
        .no_window()
        .current_dir(cwd)
        .args(["-c", "core.hooksPath=", "-c", "core.fsmonitor=", "-c", "credential.helper=", "-c", "protocol.ext.allow=never"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn host_credential_helpers() -> &'static [String] {
    HOST_HELPERS.get_or_init(|| {
        let mut helpers = Vec::new();
        for scope in ["--system", "--global"] {
            if let Some(text) = raw_config(Path::new("."), &["config", scope, "--get-all", "credential.helper"]) {
                for line in text.lines() {
                    let helper = line.trim();
                    if helper.is_empty() || helper_is_sandbox_writable(helper) {
                        continue;
                    }
                    if !helpers.iter().any(|existing| existing == helper) {
                        helpers.push(helper.to_string());
                    }
                }
            }
        }
        helpers
    })
}

fn helper_is_sandbox_writable(helper: &str) -> bool {
    let executable = helper.trim().trim_start_matches('!').split_whitespace().next().unwrap_or("");
    let path = Path::new(executable);
    if !path.is_absolute() {
        return false;
    }
    crate::agent::trusted_sandbox::is_sandbox_writable(path, Path::new("."))
}

/// 读取 git 目录时用的原始调用。不走 hooks 判定，避免和沙箱路径计算互相递归。
pub fn git_dirs(workspace: &Path) -> Option<(PathBuf, PathBuf)> {
    let program = command_env::resolve_developer_command("git");
    let output = Command::new(program)
        .no_window()
        .arg("-C")
        .arg(workspace)
        .args([
            "-c",
            "core.hooksPath=",
            "-c",
            "core.fsmonitor=",
            "-c",
            "credential.helper=",
            "-c",
            "protocol.ext.allow=never",
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let git_dir = PathBuf::from(lines.next()?);
    let common = PathBuf::from(lines.next()?);
    Some((git_dir, common))
}

/// `dev_command("git")` 的替代：返回已经带上固定 `-c` 的标准命令。
/// hooks 判定依赖工作目录，调用方有 cwd 时应改用 [`command`] 并 `current_dir`。
pub fn prepared_stdio(host_credentials: bool) -> Command {
    let mut git = HostGit::new(host_credentials);
    git.finalize();
    git.command
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn repo(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cf-host-git-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git").current_dir(&root).args(args).status().unwrap().success());
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "host-git@example.com"]);
        git(&["config", "user.name", "host-git"]);
        root
    }

    #[test]
    fn h5_workspace_hooks_do_not_run_and_git_hooks_do() {
        let root = repo("hooks");
        let inside = root.join(".githooks");
        std::fs::create_dir_all(&inside).unwrap();
        let hook = inside.join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nroot=$(git rev-parse --show-toplevel)\nprintf RAN > \"$root/hook-ran.txt\"\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Command::new("git").current_dir(&root).args(["config", "core.hooksPath", ".githooks"]).status().unwrap().success());
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        let status = command()
            .current_dir(&root)
            .args(["add", "a.txt"])
            .status()
            .unwrap();
        assert!(status.success());
        let output = command()
            .current_dir(&root)
            .args(["commit", "-qm", "skip hooks"])
            .env("GIT_AUTHOR_NAME", "host-git")
            .env("GIT_AUTHOR_EMAIL", "host-git@example.com")
            .env("GIT_COMMITTER_NAME", "host-git")
            .env("GIT_COMMITTER_EMAIL", "host-git@example.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "commit via host_git: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!root.join("hook-ran.txt").exists(), "workspace hooks must not run");
        let notices = take_notices();
        assert!(notices.iter().any(|notice| notice.contains("已跳过工作区内 hooks")), "{notices:?}");

        assert!(Command::new("git").current_dir(&root).args(["config", "--unset", "core.hooksPath"]).status().unwrap().success());
        let dot_git_hooks = root.join(".git").join("hooks");
        std::fs::create_dir_all(&dot_git_hooks).unwrap();
        let native = dot_git_hooks.join("pre-commit");
        std::fs::write(&native, "#!/bin/sh\nroot=$(git rev-parse --show-toplevel)\nprintf NATIVE > \"$root/native-hook.txt\"\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        assert!(command().current_dir(&root).args(["add", "b.txt"]).status().unwrap().success());
        assert!(command()
            .current_dir(&root)
            .args(["commit", "-qm", "native hook"])
            .env("GIT_AUTHOR_NAME", "host-git")
            .env("GIT_AUTHOR_EMAIL", "host-git@example.com")
            .env("GIT_COMMITTER_NAME", "host-git")
            .env("GIT_COMMITTER_EMAIL", "host-git@example.com")
            .status()
            .unwrap()
            .success());
        assert!(root.join("native-hook.txt").exists(), "hooks inside .git/hooks still run");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn h5_fsmonitor_and_credential_helper_are_forced_off() {
        let root = repo("cfg");
        assert!(Command::new("git").current_dir(&root).args(["config", "core.fsmonitor", "true"]).status().unwrap().success());
        assert!(Command::new("git").current_dir(&root).args(["config", "credential.helper", "store"]).status().unwrap().success());
        let fsmonitor = command().current_dir(&root).args(["config", "--get", "core.fsmonitor"]).output().unwrap();
        let helper = command().current_dir(&root).args(["config", "--get", "credential.helper"]).output().unwrap();
        assert!(!fsmonitor.status.success() || String::from_utf8_lossy(&fsmonitor.stdout).trim().is_empty());
        assert!(!helper.status.success() || String::from_utf8_lossy(&helper.stdout).trim().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
