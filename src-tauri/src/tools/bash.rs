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
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const MAX_TIMEOUT_SECS: u64 = 1800;

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
                "Run a shell command in the project directory. Returns stdout+stderr. Default timeout 300s; long-running builds, installs, CI watches, and release polling may run up to 1800s."
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

    use crate::util::no_window::NoWindow;

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

    /// Cheap liveness probe for the container runtime binary. Answers "is the
    /// CLI installed", NOT "can this machine actually run a container" — see
    /// `daemon_available` for that.
    pub fn runtime_available(program: &str) -> bool {
        std::process::Command::new(program)
            .no_window()
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Reachability probe for the container runtime *daemon*: `docker info`
    /// succeeds only when the client can actually talk to a running daemon.
    /// `--version` answers a different question and stays green on the very
    /// common developer setup where Docker Desktop or Colima is installed but
    /// stopped, so anything about to really run a container must ask this.
    ///
    /// Bounded: a wedged or still-booting daemon can leave `info` hanging, and
    /// a probe that never returns is worse than one that says "not reachable".
    ///
    /// Never a pre-flight check. Asking this before every sandboxed command
    /// would bill each one an extra `docker info` round trip — measured at a
    /// ~20ms median against a healthy local colima, on top of a ~148ms
    /// `docker run`, and slower against Docker Desktop — to learn what the
    /// command itself is about to prove anyway. The live path calls it only
    /// *after* a command already failed in a way that looks like a connection
    /// error; see `looks_like_daemon_unreachable`.
    ///
    /// Blocking by design, so the synchronous test guards can call it directly;
    /// async callers must hand it to `spawn_blocking`.
    pub fn daemon_available(program: &str) -> bool {
        let Ok(mut child) = std::process::Command::new(program)
            .no_window()
            .arg("info")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return false;
        };

        let deadline = std::time::Instant::now() + DAEMON_PROBE_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(_) => return false,
            }
        }
    }

    const DAEMON_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    /// Docker CLI wordings for "the client could not reach a daemon". Both live
    /// spellings are covered: `Cannot connect to the Docker daemon at unix://…`
    /// (CLI <= 26 and Docker Desktop) and `failed to connect to the docker API
    /// at unix://…` (CLI >= 27); `dial unix` catches the `error during connect:
    /// …` shapes that wrap the same Go dial failure.
    ///
    /// Deliberately absent: `permission denied while trying to connect to the
    /// Docker daemon socket`. That daemon is running — the fix is group
    /// membership, not starting Docker — and the CLI's own English text says so
    /// more usefully than our notice would.
    const DAEMON_UNREACHABLE_MARKERS: [&str; 4] = [
        "cannot connect to the docker daemon",
        "failed to connect to the docker api",
        "error during connect",
        "dial unix",
    ];

    /// Does this failed `docker run` *look* like the daemon was unreachable?
    ///
    /// The CLI reports an unreachable daemon as an ordinary non-zero exit, so
    /// it is indistinguishable from a failed user command until the text is
    /// read. Reading it is free, which is the whole point: a healthy daemon
    /// never pays for this diagnosis.
    ///
    /// A prefilter, not a verdict — a command is perfectly capable of *printing*
    /// these words (`grep` over a CI log, say), so callers must confirm with
    /// `daemon_available` before replacing the output.
    pub fn looks_like_daemon_unreachable(stdout: &[u8], stderr: &[u8]) -> bool {
        // A real connection failure means the container never started, so it
        // cannot have produced stdout. Anything that did ran fine and failed
        // for its own reasons.
        if !stdout.is_empty() {
            return false;
        }
        let stderr = String::from_utf8_lossy(stderr).to_lowercase();
        DAEMON_UNREACHABLE_MARKERS
            .iter()
            .any(|marker| stderr.contains(marker))
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
    // Set by whichever sandbox branch actually builds the command; the branches
    // that bail out return before anything reads it.
    let launched_program;
    let sandboxed = matches!(sandbox_mode, crate::config::settings::SandboxMode::Docker);
    let cmd = match sandbox_mode {
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
            // The guard before launch only knows whether the docker CLI is
            // installed; a stopped Docker Desktop or Colima sails past it and
            // fails here instead, handing the user the CLI's English socket
            // dump. Recover the actionable notice after the fact rather than
            // pre-flighting `docker info` on every command: the healthy daemon
            // — the overwhelmingly common case — pays nothing for this, and
            // only a failure that already looks like a connection error is
            // worth one probe to confirm.
            if sandboxed
                && !output.status.success()
                && sandbox::looks_like_daemon_unreachable(&output.stdout, &output.stderr)
            {
                let daemon_reachable =
                    tokio::task::spawn_blocking(|| sandbox::daemon_available("docker"))
                        .await
                        // The probe itself broke; keep the raw output rather
                        // than assert a cause we never confirmed.
                        .unwrap_or(true);
                if !daemon_reachable {
                    return Ok(ToolOutput::err(sandbox::MISSING_DOCKER_ERROR));
                }
            }

            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            combined.push_str(&String::from_utf8_lossy(&output.stderr));

            if combined.len() > OUTPUT_LIMIT {
                combined.truncate(OUTPUT_LIMIT);
                combined.push_str("\n[output truncated]");
            }
            append_audit(
                &mut combined,
                &ctx.cwd,
                output.status.code(),
                risk,
                shell_policy::touches_github_api(&a.command)
                    .then(read_github_quota)
                    .flatten(),
            );

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
    quota: Option<shell_policy::GithubQuota>,
) {
    if !output.ends_with('\n') && !output.is_empty() {
        output.push('\n');
    }
    output.push_str(&shell_policy::audit_footer_with_quota(
        cwd, exit_code, risk, quota,
    ));
}

/// Read the caller's remaining GitHub quota, or `None` when unavailable.
fn read_github_quota() -> Option<shell_policy::GithubQuota> {
    crate::util::github_cli::read_core_quota()
        .map(|(remaining, limit)| shell_policy::GithubQuota { remaining, limit })
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

    /// Real-runtime smoke: only meaningful on a machine that can actually run
    /// a container; everywhere else it skips. Runs a trivial command through
    /// the ACTUAL docker wrapper and checks the output and that host paths
    /// don't leak beyond the mounted project dir. Unix-only: Windows CI
    /// runners DO ship docker, but sandbox mode itself reports unsupported
    /// there and $HOME does not exist.
    ///
    /// The guard checks daemon *reachability*, not just that the CLI is
    /// installed: on a dev box with Docker Desktop or Colima installed but
    /// stopped, a presence-only guard let this smoke through and it panicked
    /// on the connection error instead of skipping.
    #[cfg(unix)]
    #[tokio::test]
    async fn docker_sandbox_executes_a_real_command_when_docker_is_present() {
        if !super::sandbox::runtime_available("docker") {
            eprintln!("skipping docker sandbox smoke: docker CLI not on PATH");
            return;
        }
        if !super::sandbox::daemon_available("docker") {
            eprintln!(
                "skipping docker sandbox smoke: docker CLI is installed but the daemon is not \
                 reachable — start it (Docker Desktop, or `colima start`) to really run this test"
            );
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

    /// The other half of the recovery, proved against a live daemon: a command
    /// that fails while *printing* the daemon error must keep its own output.
    ///
    /// This is what the `daemon_available` confirmation buys. The text
    /// prefilter alone says "unreachable" here — same wording, same empty
    /// stdout, same non-zero exit as the real thing — and mislabelling it would
    /// throw away the output the user asked for and blame their Docker install
    /// for a failure that had nothing to do with it.
    ///
    /// Runs only where a container can really start; skips elsewhere.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_live_daemon_keeps_command_output_that_looks_like_a_connection_error() {
        if !super::sandbox::runtime_available("docker")
            || !super::sandbox::daemon_available("docker")
        {
            eprintln!("skipping live-daemon false-positive test: no reachable docker daemon");
            return;
        }

        let home = std::env::var("HOME").expect("HOME set on unix");
        let cwd = std::path::PathBuf::from(home)
            .join(".cache")
            .join(format!("cf-sandbox-falsepos-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let mut settings = crate::config::settings::Settings::default();
        settings.sandbox_mode = crate::config::settings::SandboxMode::Docker;
        let mut ctx = crate::tools::ExecCtx::new(cwd.clone(), None);
        ctx.settings = Some(settings);

        // Verbatim docker wording, on stderr, no stdout, non-zero exit: every
        // signal the prefilter keys on, produced by a container that ran fine.
        let echoed = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock";
        let out = super::execute(
            serde_json::json!({ "command": format!("echo '{echoed}' >&2; exit 7") }),
            &ctx,
        )
        .await
        .expect("tool returns output");
        let _ = std::fs::remove_dir_all(&cwd);

        assert!(out.is_error, "exit 7 is still a failure");
        assert_ne!(
            out.content,
            super::sandbox::MISSING_DOCKER_ERROR,
            "a reachable daemon must never be reported as missing"
        );
        assert!(
            out.content.contains("Cannot connect to the Docker daemon"),
            "the command's own output must survive: {}",
            out.content
        );
        assert!(
            out.content.contains("exit_code=7"),
            "the real exit code must survive: {}",
            out.content
        );
    }

    /// The regression the smoke guard above depends on: "the CLI is installed"
    /// and "the daemon is reachable" are different questions, and a guard that
    /// asks the first one panics on every machine where Docker Desktop or
    /// Colima is installed but not started. `rustc` is a deterministic stand-in
    /// for exactly that shape — it answers `--version` but has no `info`
    /// subcommand — and is always present wherever this suite runs.
    #[test]
    fn daemon_probe_rejects_a_cli_that_answers_version_but_cannot_be_reached() {
        assert!(
            super::sandbox::runtime_available("rustc"),
            "rustc must be present wherever cargo test runs"
        );
        assert!(
            !super::sandbox::daemon_available("rustc"),
            "presence of the binary must not be read as a reachable daemon"
        );
        // A missing binary is "not reachable", never a panic.
        assert!(!super::sandbox::daemon_available(
            "definitely-not-a-real-binary-xyz"
        ));
    }

    /// Both docker CLI generations must be recognized. The first string is the
    /// verbatim stderr of `docker run` against a stopped Colima on CLI 29.6.1;
    /// the second is the wording CLI <= 26 and Docker Desktop still emit.
    #[test]
    fn daemon_unreachable_is_recognized_across_docker_cli_wordings() {
        let modern = b"failed to connect to the docker API at unix:///var/run/docker.sock; \
check if the path is correct and if the daemon is running: dial unix \
/var/run/docker.sock: connect: no such file or directory\n";
        let classic = b"Cannot connect to the Docker daemon at unix:///var/run/docker.sock. \
Is the docker daemon running?\n";
        let wrapped = b"error during connect: Get \"http://%2Fvar%2Frun%2Fdocker.sock/v1.47/\
containers/create\": dial unix /var/run/docker.sock: connect: connection refused\n";

        for stderr in [modern.as_slice(), classic.as_slice(), wrapped.as_slice()] {
            assert!(
                super::sandbox::looks_like_daemon_unreachable(b"", stderr),
                "unrecognized daemon failure: {}",
                String::from_utf8_lossy(stderr)
            );
        }
    }

    /// Why the live path confirms with a probe instead of trusting the text: a
    /// sandboxed command can legitimately *print* these words — grepping a CI
    /// log, replaying a captured build failure — and misreading that as "your
    /// Docker isn't running" would throw away the output the user asked for and
    /// blame the wrong thing entirely.
    #[test]
    fn a_command_that_merely_prints_the_daemon_error_is_not_mistaken_for_one() {
        let echoed = b"Cannot connect to the Docker daemon at unix:///var/run/docker.sock\n";
        // Produced stdout, so the container plainly ran: not a connect failure.
        assert!(!super::sandbox::looks_like_daemon_unreachable(
            echoed,
            b"grep: matched 1 line\n"
        ));
        // On stderr with no stdout the text alone is genuinely ambiguous, so
        // the prefilter forwards it — and `daemon_available` settles it.
        assert!(super::sandbox::looks_like_daemon_unreachable(b"", echoed));
    }

    #[test]
    fn ordinary_command_failures_are_left_alone() {
        assert!(!super::sandbox::looks_like_daemon_unreachable(
            b"",
            b"error[E0308]: mismatched types\n --> src/main.rs:4:9\n"
        ));
        assert!(!super::sandbox::looks_like_daemon_unreachable(
            b"",
            b"bash: line 1: cargo: command not found\n"
        ));
        // Socket permission trouble is a different fix (group membership), so
        // it must keep the CLI's own wording rather than "please start Docker".
        assert!(!super::sandbox::looks_like_daemon_unreachable(
            b"",
            b"permission denied while trying to connect to the Docker daemon socket at \
unix:///var/run/docker.sock\n"
        ));
        assert!(!super::sandbox::looks_like_daemon_unreachable(b"", b""));
    }

    /// The user-visible gap: the pre-flight guard only asks whether the docker
    /// CLI is installed, so on the very common "Docker Desktop/Colima installed
    /// but never started" box a sandboxed command sails past it, `docker run`
    /// fails on the socket, and the user is handed the CLI's raw English dump
    /// instead of the actionable notice this repo already wrote for exactly
    /// this situation.
    ///
    /// Runs only where the bug actually bites — CLI present, daemon down —
    /// and skips elsewhere rather than pretending to cover it. Stop the daemon
    /// (`colima stop`, or quit Docker Desktop) to really exercise this.
    #[cfg(unix)]
    #[tokio::test]
    async fn stopped_daemon_reports_the_actionable_notice_not_a_raw_socket_error() {
        if !super::sandbox::runtime_available("docker") {
            eprintln!("skipping stopped-daemon test: docker CLI not on PATH");
            return;
        }
        if super::sandbox::daemon_available("docker") {
            eprintln!(
                "skipping stopped-daemon test: the docker daemon is reachable — stop it \
                 (`colima stop`, or quit Docker Desktop) to really run this test"
            );
            return;
        }

        let cwd = std::env::temp_dir().join(format!("cf-sandbox-daemon-down-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let mut settings = crate::config::settings::Settings::default();
        settings.sandbox_mode = crate::config::settings::SandboxMode::Docker;
        let mut ctx = crate::tools::ExecCtx::new(cwd.clone(), None);
        ctx.settings = Some(settings);

        let out = super::execute(serde_json::json!({ "command": "echo hi" }), &ctx)
            .await
            .expect("tool returns output");
        let _ = std::fs::remove_dir_all(&cwd);

        assert!(out.is_error, "a stopped daemon must not read as success");
        assert_eq!(
            out.content,
            super::sandbox::MISSING_DOCKER_ERROR,
            "stopped daemon must surface the actionable notice, not the raw docker error"
        );
    }

    /// The same recovery, but entered the way the running app enters it: the
    /// `"sandbox_mode": "docker"` string the Settings page persists, read back
    /// through `Settings` deserialization, carried in `ExecCtx` exactly as
    /// `agent::tool_backend` builds it, and dispatched by name through
    /// `tools::dispatch`. Covers the plumbing the direct-`execute` test above
    /// stubs — everything from the persisted toggle down, short of the window.
    ///
    /// (`ToolBackend` itself stays out of reach here: it owns a Tauri
    /// `AppHandle`, and constructing that inside the test EXE is what triggered
    /// the Windows `STATUS_ENTRYPOINT_NOT_FOUND` loader abort in hotfix #166.)
    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_docker_sandbox_setting_reaches_the_tool_and_reports_the_notice() {
        if !super::sandbox::runtime_available("docker") {
            eprintln!("skipping persisted-setting test: docker CLI not on PATH");
            return;
        }
        if super::sandbox::daemon_available("docker") {
            eprintln!("skipping persisted-setting test: the docker daemon is reachable");
            return;
        }

        // Round-trip through JSON so the persisted spelling is what is tested,
        // not a hand-built enum a rename could silently desync from.
        let mut persisted = serde_json::to_value(crate::config::settings::Settings::default())
            .expect("settings serialize");
        persisted["sandbox_mode"] = serde_json::json!("docker");
        let settings: crate::config::settings::Settings =
            serde_json::from_value(persisted).expect("settings round-trip");
        assert_eq!(
            settings.sandbox_mode,
            crate::config::settings::SandboxMode::Docker,
            "the Settings page writes \"docker\"; the tool must read it as Docker"
        );

        let cwd = std::env::temp_dir().join(format!("cf-sandbox-dispatch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let mut ctx = crate::tools::ExecCtx::new(cwd.clone(), None);
        ctx.settings = Some(settings);

        let out = crate::tools::dispatch("bash", serde_json::json!({ "command": "echo hi" }), &ctx)
            .await
            .expect("dispatch returns output");
        let _ = std::fs::remove_dir_all(&cwd);

        assert!(out.is_error);
        assert_eq!(out.content, super::sandbox::MISSING_DOCKER_ERROR);
    }

    #[test]
    fn missing_docker_is_a_hard_actionable_error_never_a_silent_host_fallback() {
        // Falling back to the host would silently void the isolation the
        // user opted into — the error must name the fix instead.
        assert!(super::sandbox::MISSING_DOCKER_ERROR.contains("docker"));
        assert!(super::sandbox::MISSING_DOCKER_ERROR.contains("沙箱"));
        assert!(!super::sandbox::runtime_available(
            "definitely-not-a-real-binary-xyz"
        ));
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
            .no_window()
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

    /// The model writes POSIX/bash-flavoured shell — every prompt, doc and
    /// example in this project assumes it. Running that under zsh silently
    /// changes what it means.
    ///
    /// Field evidence, 2026-07: `zsh: read-only variable: status` killed eight
    /// agent-written CI polling scripts in one session, because `status` is a
    /// read-only special in zsh (an alias for `$?`) and is also the most
    /// natural name for the variable those scripts need. `emulate sh` does not
    /// lift it.
    ///
    /// These assert the DIALECT the tool runs, not which binary provides it.
    #[cfg(unix)]
    #[tokio::test]
    async fn shell_runs_posix_dialect_not_zsh_specials() {
        let cwd = std::env::temp_dir().join(format!("codefactory-dialect-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        // 1. `status` must be an ordinary variable.
        let assigned = execute(
            json!({ "command": "status=ok; printf 'got:%s' \"$status\"" }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");
        assert!(
            !assigned.is_error && assigned.content.contains("got:ok"),
            "status= must not be read-only: {}",
            assigned.content,
        );

        // 2. Unquoted expansion word-splits. zsh returns 1 here and says
        //    nothing about it — the failure mode is a wrong answer, not an
        //    error.
        let split = execute(
            json!({ "command": "f='a b'; set -- $f; printf 'count:%s' \"$#\"" }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");
        assert!(
            split.content.contains("count:2"),
            "unquoted expansion must word-split: {}",
            split.content,
        );

        // 3. A glob that matches nothing passes through instead of aborting
        //    the whole command, so a later step still runs.
        let glob = execute(
            json!({ "command": "ls *.codefactory-no-such-suffix 2>/dev/null; printf 'reached'" }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");
        assert!(
            glob.content.contains("reached"),
            "an unmatched glob must not abort the command: {}",
            glob.content,
        );

        let _ = std::fs::remove_dir_all(cwd);
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

    /// Reports whether a background service is still up, by requiring it to
    /// stay up rather than sampling once.
    ///
    /// The same teardown window `wait_for_process_exit` exists for also breaks
    /// the opposite question, and there it FALSE-PASSES: a service that the
    /// tool has just SIGKILLed still answers `kill(pid, 0)` with 0 until it
    /// leaves the process table, so a single sample reads it as alive.
    /// Mutating `output_with_timeout` to group-kill on its success path —
    /// capturing the pgid before the wait, since `Child::id()` is None once
    /// the child is reaped — left the survival assertions GREEN. Every service
    /// in these tests is a `sleep 30`, so one that really survived is still
    /// there after the settle and one that was killed is long gone.
    #[cfg(unix)]
    async fn service_stayed_up(pid: i32) -> bool {
        !wait_for_process_exit(pid, Duration::from_millis(500)).await
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
        //
        // #216 then set this to 3s, which still loses race 1 under heavier
        // fork/exec load — 3 runs in 5, every one of them "descendant never
        // recorded its pid". The budget is only the window the descendant has
        // to reach `echo $$`, and it pays TWO shell startups to get there
        // (`zsh -lc` then `sh -c`), the login one costing 250ms+ under load.
        // 5s matches the SHELL_START_BUDGET its sibling tests settled on and
        // stays as far from `sleep 30` as 3s did; the tool still times out
        // here, since `& wait` blocks until the kill, so the path under test
        // is unchanged. This test's runtime equals this budget, so it buys
        // headroom at 1s per second — enough for the load that broke 3s, not
        // a guarantee at any load.
        const RUN_TIMEOUT: Duration = Duration::from_secs(5);

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
        const SHELL_START_BUDGET: Duration = Duration::from_secs(5);
        let cwd = std::env::temp_dir().join(format!("codefactory-bash-service-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let output = execute_with_timeout(
            json!({
                "command": "sleep 30 > service.log 2>&1 & echo $! > service.pid"
            }),
            &ExecCtx::new(cwd.clone(), None),
            SHELL_START_BUDGET,
        )
        .await
        .expect("tool returns output");
        assert!(!output.is_error, "service start failed: {}", output.content);

        let pid = wait_for_recorded_pid(&cwd.join("service.pid"), Duration::from_secs(2))
            .await
            .expect("service pid written");
        let survived = service_stayed_up(pid).await;
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(cwd);

        assert!(survived, "background service {pid} did not survive");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_service_without_redirect_cannot_hold_output_pipes_forever() {
        const SHELL_START_BUDGET: Duration = Duration::from_secs(5);
        let cwd =
            std::env::temp_dir().join(format!("codefactory-bash-service-pipes-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let result = tokio::time::timeout(
            Duration::from_secs(8),
            execute_with_timeout(
                json!({"command": "sleep 30 & echo $! > service.pid"}),
                &ExecCtx::new(cwd.clone(), None),
                SHELL_START_BUDGET,
            ),
        )
        .await;

        let output = result
            .expect("background process held output pipes past the tool timeout")
            .expect("tool returns output");
        assert!(!output.is_error, "service start failed: {}", output.content);
        let pid = wait_for_recorded_pid(&cwd.join("service.pid"), Duration::from_secs(2))
            .await
            .expect("service pid written");
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_service_holding_only_stderr_preserves_completed_stdout() {
        const SHELL_START_BUDGET: Duration = Duration::from_secs(5);
        let cwd = std::env::temp_dir().join(format!(
            "codefactory-bash-service-one-pipe-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let result = tokio::time::timeout(
            Duration::from_secs(8),
            execute_with_timeout(
                json!({
                    "command": "echo ready; sleep 30 >/dev/null & echo $! > service.pid"
                }),
                &ExecCtx::new(cwd.clone(), None),
                SHELL_START_BUDGET,
            ),
        )
        .await;

        let output = result
            .expect("single inherited pipe held the shell tool forever")
            .expect("tool returns output");
        assert!(!output.is_error, "service start failed: {}", output.content);
        let pid = wait_for_recorded_pid(&cwd.join("service.pid"), Duration::from_secs(2))
            .await
            .expect("service pid written");
        let survived = service_stayed_up(pid).await;
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(cwd);
        assert!(
            output.content.contains("ready"),
            "stdout was discarded: {}",
            output.content
        );
        assert!(survived, "background service {pid} did not survive");
    }
}
