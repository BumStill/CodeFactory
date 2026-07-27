// SPDX-License-Identifier: Apache-2.0
use codefactory_agent_core::effective_command_timeout_sec;
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::util::command_env;
use crate::util::no_window::NoWindow;
use crate::util::process_tree::{self, ProcessOutputError};

use super::{shell_policy, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

const OUTPUT_LIMIT: usize = 30_000;
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 300;

#[derive(Deserialize)]
struct Args {
    command: String,
    // Advertised in the tool schema (see definition() below) so the model sends a
    // one-line summary; accepted but not surfaced yet.
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "bash".into(),
            description:
                "Run a shell command in the project directory. Returns stdout+stderr. Timeout 120s, or up to 300s for builds and dependency installation."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command":     { "type": "string" },
                    "description": { "type": "string", "description": "One-line human summary" }
                },
                "required": ["command"]
            }),
        },
    }
}

/// Optional container isolation for shell commands (WorkBuddy-gap P0-2).
pub mod sandbox {
    use std::path::Path;

    pub struct SandboxInvocation {
        pub program: String,
        pub args: Vec<String>,
    }

    pub const MISSING_DOCKER_ERROR: &str = "沙箱模式已开启,但找不到可用的 docker 运行时。\
请安装/启动 Docker,或在设置中将沙箱模式改回 off。为保证隔离承诺,不会自动回退到本机执行。";

    /// Build the docker invocation: disposable container, ONLY the project
    /// directory mounted (rw, same absolute path so tool output paths stay
    /// meaningful), workdir there, command via bash -lc. Network stays on —
    /// builds need registries; the isolation goal is the filesystem.
    pub fn docker_invocation(command: &str, cwd: &Path, image: &str) -> SandboxInvocation {
        let cwd = cwd.to_string_lossy();
        SandboxInvocation {
            program: "docker".into(),
            args: vec![
                "run".into(),
                "--rm".into(),
                "--init".into(),
                "-v".into(),
                format!("{cwd}:{cwd}"),
                "-w".into(),
                cwd.to_string(),
                image.to_string(),
                "bash".into(),
                "-lc".into(),
                command.to_string(),
            ],
        }
    }

    /// Cheap liveness probe for the container runtime binary.
    pub fn runtime_available(program: &str) -> bool {
        std::process::Command::new(program)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    execute_inner(args, ctx, None).await
}

#[cfg(test)]
async fn execute_with_timeout(
    args: Value,
    ctx: &ExecCtx,
    timeout_duration: Duration,
) -> Result<ToolOutput> {
    execute_inner(args, ctx, Some(timeout_duration)).await
}

async fn execute_inner(
    args: Value,
    ctx: &ExecCtx,
    timeout_override: Option<Duration>,
) -> Result<ToolOutput> {
    let a: Args = match serde_json::from_value(args.clone()) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "Invalid arguments for bash: {e}. Received: {}",
                serde_json::to_string(&args)
                    .unwrap_or_else(|_| "<unprintable>".into())
                    .chars()
                    .take(300)
                    .collect::<String>(),
            )))
        }
    };

    let policy = shell_policy::classify_command(&a.command);
    if let shell_policy::ShellCommandPolicy::Deny { reason } = policy {
        return Ok(ToolOutput::err(format!(
            "Command denied by safety policy: {reason}"
        )));
    }
    let risk = policy.risk();

    let sandbox_mode = ctx
        .settings
        .as_ref()
        .map(|s| s.sandbox_mode)
        .unwrap_or_default();
    let mut launched_program = String::new();
    let mut cmd = match sandbox_mode {
        crate::config::settings::SandboxMode::Docker => {
            if cfg!(windows) {
                return Ok(ToolOutput::err(
                    "沙箱模式暂不支持 Windows;请在设置中将沙箱模式改回 off。",
                ));
            }
            if !sandbox::runtime_available("docker") {
                return Ok(ToolOutput::err(sandbox::MISSING_DOCKER_ERROR));
            }
            let image = ctx
                .settings
                .as_ref()
                .map(|s| s.sandbox_image.clone())
                .unwrap_or_else(|| "ubuntu:24.04".into());
            // Canonicalize before mounting: macOS paths like /tmp are
            // symlinks, and Docker mounts the LINK path as an empty dir.
            let mount_cwd = ctx.cwd.canonicalize().unwrap_or_else(|_| ctx.cwd.clone());
            let inv = sandbox::docker_invocation(&a.command, &mount_cwd, &image);
            launched_program = inv.program.clone();
            let mut cmd = Command::new(inv.program).no_window();
            cmd.args(inv.args)
                .current_dir(&ctx.cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            cmd
        }
        crate::config::settings::SandboxMode::Off => {
            let shell = command_env::shell_invocation(&a.command);
            launched_program = shell.program.to_string();
            let mut cmd = Command::new(shell.program).no_window();
            cmd.args(shell.args)
                .current_dir(&ctx.cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command_env::apply_developer_path(&mut cmd);
            cmd
        }
    };

    let timeout_secs =
        effective_command_timeout_sec(&a.command, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
    let timeout_duration = timeout_override.unwrap_or(Duration::from_secs(timeout_secs));
    let result = process_tree::output_with_timeout(cmd, timeout_duration).await;

    match result {
        Err(ProcessOutputError::Timeout) => Ok(ToolOutput::err(format!(
            "Command timed out after {timeout_secs}s"
        ))),
        Err(ProcessOutputError::Unavailable | ProcessOutputError::Failed) => {
            Ok(ToolOutput::err(format!(
                "Failed to execute shell '{}'. PATH={}",
                launched_program,
                std::env::var("PATH").unwrap_or_else(|_| "<unset>".into())
            )))
        }
        Ok(output) => {
            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            combined.push_str(&String::from_utf8_lossy(&output.stderr));

            if combined.len() > OUTPUT_LIMIT {
                combined.truncate(OUTPUT_LIMIT);
                combined.push_str("\n[output truncated]");
            }
            append_audit(&mut combined, &ctx.cwd, output.status.code(), risk);

            let is_error = !output.status.success();
            if is_error {
                Ok(ToolOutput::err(combined))
            } else {
                Ok(ToolOutput::ok(combined))
            }
        }
    }
}

fn append_audit(
    output: &mut String,
    cwd: &std::path::Path,
    exit_code: Option<i32>,
    risk: shell_policy::ShellRisk,
) {
    if !output.ends_with('\n') && !output.is_empty() {
        output.push('\n');
    }
    output.push_str(&shell_policy::audit_footer(cwd, exit_code, risk));
}

#[cfg(test)]
mod tests {
    #[test]
    fn docker_invocation_mounts_only_the_project_directory() {
        // WorkBuddy-gap P0-2: optional container isolation. The wrapper must
        // mount ONLY the project dir (rw, same absolute path), set it as the
        // workdir, and run the exact command via bash -lc. No home dir, no
        // extra host mounts.
        let inv = super::sandbox::docker_invocation(
            "cargo test && printf done",
            std::path::Path::new("/Users/leo/Projects/Demo"),
            "ubuntu:24.04",
        );
        assert_eq!(inv.program, "docker");
        let args = inv.args.join(" ");
        assert!(args.starts_with("run --rm --init"));
        assert!(args.contains("-v /Users/leo/Projects/Demo:/Users/leo/Projects/Demo"));
        assert!(args.contains("-w /Users/leo/Projects/Demo"));
        assert!(args.ends_with("ubuntu:24.04 bash -lc cargo test && printf done"));
        // Exactly one volume mount.
        assert_eq!(inv.args.iter().filter(|a| *a == "-v").count(), 1);
    }

    #[test]
    fn sandbox_defaults_are_off_with_a_stock_image() {
        let settings = crate::config::settings::Settings::default();
        assert_eq!(
            settings.sandbox_mode,
            crate::config::settings::SandboxMode::Off
        );
        assert_eq!(settings.sandbox_image, "ubuntu:24.04");
    }

    /// Real-runtime smoke: only meaningful on a machine with docker; CI
    /// runners without it skip. Runs a trivial command through the ACTUAL
    /// docker wrapper and checks the output and that host paths don't leak
    /// beyond the mounted project dir. Unix-only: Windows CI runners DO
    /// ship docker, but sandbox mode itself reports unsupported there and
    /// $HOME does not exist.
    #[cfg(unix)]
    #[tokio::test]
    async fn docker_sandbox_executes_a_real_command_when_docker_is_present() {
        if !super::sandbox::runtime_available("docker") {
            eprintln!("skipping docker sandbox smoke: docker not available");
            return;
        }
        // Under $HOME so the path sits inside Docker Desktop's default
        // file-sharing scope (paths outside it mount as EMPTY dirs — a
        // documented limitation surfaced by this smoke's first version).
        let home = std::env::var("HOME").expect("HOME set on unix");
        let cwd = std::path::PathBuf::from(home)
            .join(".cache")
            .join(format!("cf-sandbox-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("probe.txt"), "sandboxed").unwrap();

        let mut settings = crate::config::settings::Settings::default();
        settings.sandbox_mode = crate::config::settings::SandboxMode::Docker;
        let mut ctx = crate::tools::ExecCtx::new(cwd.clone(), None);
        ctx.settings = Some(settings);

        let host_home = std::env::var("HOME").expect("HOME set on unix");
        let out = super::execute(
            serde_json::json!({
                "command": format!("cat probe.txt && ls {host_home} 2>/dev/null; true")
            }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("sandboxed"));
        // Only the mount chain is visible in the container's view of the
        // host home — real host content (e.g. the Projects tree) must not
        // leak in.
        assert!(
            !out.content.contains("Projects"),
            "host home content leaked into sandbox: {}",
            out.content
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn missing_docker_is_a_hard_actionable_error_never_a_silent_host_fallback() {
        // Falling back to the host would silently void the isolation the
        // user opted into — the error must name the fix instead.
        assert!(super::sandbox::MISSING_DOCKER_ERROR.contains("docker"));
        assert!(super::sandbox::MISSING_DOCKER_ERROR.contains("沙箱"));
        assert!(!super::sandbox::runtime_available("definitely-not-a-real-binary-xyz"));
    }

    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn full_access_still_honors_built_in_deny_list() {
        let cwd = std::env::temp_dir().join(format!("codefactory-bash-deny-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let output = execute(
            json!({
                "command": "Write-Output 'shutdown'",
            }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(cwd);

        assert!(output.is_error);
        assert!(output.content.contains("Command denied by safety policy"));
    }

    #[tokio::test]
    async fn successful_command_output_includes_shell_audit_metadata() {
        if std::process::Command::new("powershell")
            .args([
                "-NonInteractive",
                "-NoProfile",
                "-Command",
                "$PSVersionTable.PSVersion",
            ])
            .output()
            .is_err()
        {
            eprintln!("skipping powershell audit test: powershell executable not found");
            return;
        }

        let cwd = std::env::temp_dir().join(format!("codefactory-bash-audit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let output = execute(
            json!({
                "command": "Write-Output 'ok'",
            }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(cwd);

        assert!(!output.is_error);
        assert!(output.content.contains("ok"));
        assert!(output.content.contains("[shell-audit]"));
        assert!(output.content.contains("risk=low"));
        assert!(output.content.contains("exit_code=0"));
        assert!(output.content.contains("cwd="));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_command_uses_available_platform_shell() {
        let cwd = std::env::temp_dir().join(format!("codefactory-bash-unix-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let output = execute(
            json!({
                "command": "printf codefactory-shell-ok",
            }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(cwd);

        assert!(
            !output.is_error,
            "expected platform shell success, got: {}",
            output.content
        );
        assert!(output.content.contains("codefactory-shell-ok"));
        assert!(output.content.contains("[shell-audit]"));
        assert!(output.content.contains("exit_code=0"));
    }

    /// Polls until the backgrounded descendant has recorded its pid, or
    /// `deadline` passes. Commands run through a LOGIN shell (`zsh -lc`), so
    /// profile startup plus two forks sit between the spawn and that write —
    /// ~30ms on an idle machine but ~250ms under load.
    #[cfg(unix)]
    async fn wait_for_recorded_pid(pid_path: &std::path::Path, deadline: Duration) -> Option<i32> {
        let start = std::time::Instant::now();
        loop {
            // A torn read of a half-written file parses as a different, live
            // pid, so only accept a line `echo` has already terminated.
            if let Some(pid) = std::fs::read_to_string(pid_path)
                .ok()
                .filter(|raw| raw.ends_with('\n'))
                .and_then(|raw| raw.trim().parse::<i32>().ok())
            {
                return Some(pid);
            }
            if start.elapsed() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Waits for `pid` to leave the process table, reporting whether it did so
    /// inside `deadline`. `kill(pid, 0)` still answers 0 for a process that has
    /// been SIGKILLed but not yet torn down — on macOS it shows as `E`/exiting,
    /// already reparented to pid 1 — and that teardown window is microseconds
    /// wide, so a single sample lands inside it under load. A descendant that
    /// genuinely survived stays alive for its whole `sleep 30`, so waiting for
    /// the exit proves the same thing without racing teardown.
    #[cfg(unix)]
    async fn wait_for_process_exit(pid: i32, deadline: Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return true;
            }
            if start.elapsed() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_terminates_descendants_in_the_shell_process_group() {
        // Two separate races used to make this flake under a loaded parallel
        // run, neither of which says anything about group termination:
        //
        //   1. The descendant raced the timeout. If the kill fired before `sh`
        //      was spawned into the process group, it survived and wrote its
        //      pid afterwards. #213 widened the budget to 2s for this; the
        //      window here stays comfortably clear of `sleep 30`, and the pid
        //      is now waited for rather than assumed.
        //   2. The liveness probe raced teardown — see wait_for_process_exit.
        //      This one outlived #213: at 2s the test still failed 7/8 under
        //      load, every time with "survived", because a killed descendant
        //      still answers kill(pid, 0) for a few microseconds.
        //
        // Both are now waited out instead of assumed, so the timeout path
        // under test is unchanged and the assertions below mean what they say.
        const RUN_TIMEOUT: Duration = Duration::from_secs(3);

        let cwd = std::env::temp_dir().join(format!("codefactory-bash-timeout-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let pid_path = cwd.join("descendant.pid");
        let command = format!("sh -c 'echo $$ > {}; sleep 30' & wait", pid_path.display());

        let recorded_pid = tokio::spawn({
            let pid_path = pid_path.clone();
            async move { wait_for_recorded_pid(&pid_path, RUN_TIMEOUT).await }
        });

        let output = execute_with_timeout(
            json!({"command": command}),
            &ExecCtx::new(cwd.clone(), None),
            RUN_TIMEOUT,
        )
        .await
        .expect("tool returns timeout output");

        let pid = recorded_pid
            .await
            .expect("pid watcher task")
            .expect("descendant never recorded its pid inside the run window");
        let exited = wait_for_process_exit(pid, Duration::from_secs(2)).await;
        let _ = std::fs::remove_dir_all(cwd);

        assert!(output.is_error);
        assert!(output.content.contains("Command timed out"));
        assert!(exited, "timed-out descendant process {pid} survived");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_service_survives_after_the_shell_tool_returns() {
        let cwd = std::env::temp_dir().join(format!("codefactory-bash-service-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let output = execute_with_timeout(
            json!({
                "command": "sleep 30 > service.log 2>&1 & echo $! > service.pid"
            }),
            &ExecCtx::new(cwd.clone(), None),
            Duration::from_secs(2),
        )
        .await
        .expect("tool returns output");

        let pid = std::fs::read_to_string(cwd.join("service.pid"))
            .expect("service pid written")
            .trim()
            .parse::<i32>()
            .expect("numeric service pid");
        let process_exists = unsafe { libc::kill(pid, 0) } == 0;
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(cwd);

        assert!(!output.is_error, "service start failed: {}", output.content);
        assert!(process_exists, "background service {pid} did not survive");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_service_without_redirect_cannot_hold_output_pipes_forever() {
        let cwd =
            std::env::temp_dir().join(format!("codefactory-bash-service-pipes-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            execute_with_timeout(
                json!({"command": "sleep 30 & echo $! > service.pid"}),
                &ExecCtx::new(cwd.clone(), None),
                Duration::from_secs(1),
            ),
        )
        .await;

        let pid = std::fs::read_to_string(cwd.join("service.pid"))
            .expect("service pid written")
            .trim()
            .parse::<i32>()
            .expect("numeric service pid");
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(cwd);

        let output = result
            .expect("background process held output pipes past the tool timeout")
            .expect("tool returns output");
        assert!(!output.is_error, "service start failed: {}", output.content);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_service_holding_only_stderr_preserves_completed_stdout() {
        let cwd = std::env::temp_dir().join(format!(
            "codefactory-bash-service-one-pipe-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            execute_with_timeout(
                json!({
                    "command": "echo ready; sleep 30 >/dev/null & echo $! > service.pid"
                }),
                &ExecCtx::new(cwd.clone(), None),
                Duration::from_secs(1),
            ),
        )
        .await;

        let pid = std::fs::read_to_string(cwd.join("service.pid"))
            .expect("service pid written")
            .trim()
            .parse::<i32>()
            .expect("numeric service pid");
        let process_exists = unsafe { libc::kill(pid, 0) } == 0;
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(cwd);

        let output = result
            .expect("single inherited pipe held the shell tool forever")
            .expect("tool returns output");
        assert!(!output.is_error, "service start failed: {}", output.content);
        assert!(
            output.content.contains("ready"),
            "stdout was discarded: {}",
            output.content
        );
        assert!(process_exists, "background service {pid} did not survive");
    }
}
