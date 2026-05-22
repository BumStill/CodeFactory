// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

use super::{ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

const OUTPUT_LIMIT: usize = 30_000;
const TIMEOUT_SECS: u64 = 120;

// Commands that are always denied regardless of user permissions.
static DENY_LIST: &[&str] = &[
    "rm -rf /",
    "format ",
    "del /f /s /q c:\\",
    "reg delete hklm",
    "shutdown",
    "rd /s /q c:\\",
];

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    description: Option<String>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "bash".into(),
            description: "Run a shell command via PowerShell. Returns stdout+stderr. Timeout 120s."
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

    let cmd_lower = a.command.to_lowercase();
    for denied in DENY_LIST {
        if cmd_lower.contains(denied) {
            return Ok(ToolOutput::err(format!(
                "Command denied by safety policy: matches '{denied}'"
            )));
        }
    }

    let result = timeout(
        Duration::from_secs(TIMEOUT_SECS),
        Command::new("powershell")
            .args(["-NonInteractive", "-NoProfile", "-Command", &a.command])
            .current_dir(&ctx.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;

    match result {
        Err(_) => Ok(ToolOutput::err(format!(
            "Command timed out after {TIMEOUT_SECS}s"
        ))),
        Ok(Err(e)) => Ok(ToolOutput::err(format!("Failed to spawn process: {e}"))),
        Ok(Ok(output)) => {
            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            combined.push_str(&String::from_utf8_lossy(&output.stderr));

            if combined.len() > OUTPUT_LIMIT {
                combined.truncate(OUTPUT_LIMIT);
                combined.push_str("\n[output truncated]");
            }

            let is_error = !output.status.success();
            if is_error {
                Ok(ToolOutput::err(combined))
            } else {
                Ok(ToolOutput::ok(combined))
            }
        }
    }
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
            &ExecCtx { cwd: cwd.clone() },
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(cwd);

        assert!(output.is_error);
        assert!(output.content.contains("Command denied by safety policy"));
    }
}
