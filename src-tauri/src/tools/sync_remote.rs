// SPDX-License-Identifier: Apache-2.0
//! 只读远端同步。信任模式下免审批：它只 fetch，不 push，不改远端。

use serde_json::{json, Value};

use crate::openrouter::types::{FunctionDefinition, ToolDefinition};
use crate::util::host_git;

use super::{ExecCtx, ToolOutput};

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "sync_remote".into(),
            description: "Fetch from a remote without pushing, tagging, or changing the remote. Trusted mode runs this without an extra approval. Hooks inside the workspace are not executed.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "remote": { "type": "string", "description": "Remote name. Defaults to origin." }
                }
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> crate::errors::Result<ToolOutput> {
    let remote = args
        .get("remote")
        .and_then(|value| value.as_str())
        .unwrap_or("origin")
        .trim()
        .to_string();
    if remote.is_empty() || remote.chars().any(|ch| ch.is_whitespace()) {
        return Ok(ToolOutput::err("remote 名称无效。sync_remote 只执行 fetch。"));
    }
    let cwd = ctx.cwd.clone();
    let joined = tokio::task::spawn_blocking(move || {
        host_git::command_with_host_credentials()
            .current_dir(&cwd)
            .args(["fetch", "--prune", &remote])
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
    })
    .await;
    let output = match joined {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Ok(ToolOutput::err(format!("sync_remote 未能启动 git：{error}"))),
        Err(error) => return Ok(ToolOutput::err(format!("sync_remote 未能启动 git：{error}"))),
    };
    let mut text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for notice in host_git::take_notices() {
        text.push('\n');
        text.push_str(&notice);
    }
    if output.status.success() {
        Ok(ToolOutput::ok(format!("已从远端读取更新，没有改写远端。\n{text}")))
    } else {
        Ok(ToolOutput::err(format!("sync_remote 未能完成 fetch，远端未被修改。\n{text}")))
    }
}
