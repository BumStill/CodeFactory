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

    let shell = command_env::shell_invocation(&a.command);
    let mut cmd = Command::new(shell.program).no_window();
    cmd.args(shell.args)
        .current_dir(&ctx.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command_env::apply_developer_path(&mut cmd);

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
                shell.program,
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

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_terminates_descendants_in_the_shell_process_group() {
        let cwd = std::env::temp_dir().join(format!("codefactory-bash-timeout-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let pid_path = cwd.join("descendant.pid");
        let command = format!("sh -c 'echo $$ > {}; sleep 30' & wait", pid_path.display());

        let output = execute_with_timeout(
            json!({"command": command}),
            &ExecCtx::new(cwd.clone(), None),
            Duration::from_millis(150),
        )
        .await
        .expect("tool returns timeout output");

        let pid = std::fs::read_to_string(&pid_path)
            .expect("descendant pid written")
            .trim()
            .parse::<i32>()
            .expect("numeric descendant pid");
        let process_exists = unsafe { libc::kill(pid, 0) } == 0;
        let _ = std::fs::remove_dir_all(cwd);

        assert!(output.is_error);
        assert!(output.content.contains("Command timed out"));
        assert!(
            !process_exists,
            "timed-out descendant process {pid} survived"
        );
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
