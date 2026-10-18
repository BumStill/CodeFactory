// SPDX-License-Identifier: Apache-2.0
//! CF-BG R1/R3 的工具面：`background_task_start|list|output|stop`。
//!
//! 工具只做参数解析与会话归属校验；进程、持久化、对账、上限都在
//! `crate::agent::background_tasks` 里，因此 headless 测试覆盖的是真实实现。

use serde::Deserialize;
use serde_json::{json, Value};

use super::{ExecCtx, ToolOutput};
use crate::agent::background_tasks::{
    self, StartRequest, TaskSource, DEFAULT_TASK_SECS, MAX_TASK_SECS, OUTPUT_TOOL_LIMIT,
};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

fn definition(name: &str, description: &str, parameters: Value) -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: name.into(),
            description: description.into(),
            parameters,
        },
    }
}

pub fn start_definition() -> ToolDefinition {
    definition(
        "background_task_start",
        "Start a shell command in this session's working directory as a background task and return a task_id immediately. \
         When the command ends (success, failure or timeout) the system wakes this session with the exit code and the output tail. \
         Use it for anything you would otherwise wait on: CI, long builds, deploys, long test runs, periodic status checks.",
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to run in the background" },
                "description": { "type": "string", "description": "One-line human summary" },
                "timeout_sec": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TASK_SECS,
                    "description": format!("Kill the task after this many seconds (default {DEFAULT_TASK_SECS}, max {MAX_TASK_SECS})")
                },
                "source": {
                    "type": "string",
                    "enum": ["agent_decided", "user_requested"],
                    "description": "agent_decided when you chose to background it; user_requested when the user asked you to watch something"
                }
            },
            "required": ["command"]
        }),
    )
}

pub fn list_definition() -> ToolDefinition {
    definition(
        "background_task_list",
        "List background tasks started by this session, with status, exit code, command and timestamps.",
        json!({ "type": "object", "properties": {} }),
    )
}

pub fn output_definition() -> ToolDefinition {
    definition(
        "background_task_output",
        "Read the tail of a background task's output (bounded).",
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": OUTPUT_TOOL_LIMIT }
            },
            "required": ["task_id"]
        }),
    )
}

pub fn stop_definition() -> ToolDefinition {
    definition(
        "background_task_stop",
        "Stop a background task by task_id and kill its whole process tree.",
        json!({
            "type": "object",
            "properties": { "task_id": { "type": "string" } },
            "required": ["task_id"]
        }),
    )
}

fn session_id(ctx: &ExecCtx) -> std::result::Result<String, String> {
    ctx.session_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            "后台任务属于会话：当前上下文没有 session_id，无法创建或管理后台任务。".to_string()
        })
}

fn pool(ctx: &ExecCtx) -> std::result::Result<sqlx::SqlitePool, String> {
    ctx.db
        .clone()
        .ok_or_else(|| "后台任务需要应用数据库：当前上下文没有可用的 SQLite 连接。".to_string())
}

/// R4：后台输出只写应用数据目录；测试/headless 构建落到临时目录。
fn output_dir(ctx: &ExecCtx) -> std::path::PathBuf {
    #[cfg(not(test))]
    if let Some(app) = ctx.app.as_ref() {
        return background_tasks::background_dir(app);
    }
    let _ = ctx;
    std::env::temp_dir().join("codefactory-background-tasks")
}

#[derive(Deserialize)]
struct StartArgs {
    command: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    timeout_sec: Option<u64>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
struct OutputArgs {
    task_id: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct StopArgs {
    task_id: String,
}

pub async fn execute_start(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let args: StartArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(error) => return Ok(ToolOutput::err(format!("background_task_start 参数错误：{error}"))),
    };
    let session_id = match session_id(ctx) {
        Ok(id) => id,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let pool = match pool(ctx) {
        Ok(pool) => pool,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    if args.command.trim().is_empty() {
        return Ok(ToolOutput::err("command 不能为空。"));
    }
    let source = match args.source.as_deref() {
        Some("user_requested") => TaskSource::UserRequested,
        _ => TaskSource::AgentDecided,
    };
    let request = StartRequest {
        session_id: session_id.clone(),
        root_turn_id: ctx.root_turn_id.clone(),
        cwd: ctx.cwd.clone(),
        output_dir: output_dir(ctx),
        command: args.command.clone(),
        source,
        timeout_sec: args.timeout_sec,
    };
    match background_tasks::start(&pool, request).await {
        Ok(started) => {
            let summary = args
                .description
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or(args.command.trim());
            Ok(ToolOutput::ok(format!(
                "已在后台启动 {summary}\n\
                 task_id: {}\npid: {}\n输出文件: {}\n\
                 命令结束（成功/失败/超时）后会自动唤醒本会话，并带上退出码与输出尾部。\
                 需要提前停止时用 background_task_stop task_id={}；查看进度用 background_task_output。",
                started.id,
                started
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "未知".to_string()),
                started.output_path,
                started.id
            ))
            .with_metadata(json!({
                "task_id": started.id,
                "pid": started.pid,
                "output_path": started.output_path,
                "source": source.as_str(),
            })))
        }
        Err(message) => Ok(ToolOutput::err(message)),
    }
}

pub async fn execute_list(_args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let session_id = match session_id(ctx) {
        Ok(id) => id,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let pool = match pool(ctx) {
        Ok(pool) => pool,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    match background_tasks::list_session(&pool, &session_id).await {
        Ok(tasks) if tasks.is_empty() => Ok(ToolOutput::ok("本会话没有后台任务。")),
        Ok(tasks) => {
            let mut content = String::from("本会话的后台任务：\n");
            for task in &tasks {
                content.push_str(&format!(
                    "- {} [{}] {} | 退出码 {} | 来源 {} | 创建 {} | 结束 {}\n",
                    task.id,
                    task.status.as_str(),
                    task.command,
                    task.exit_code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    task.source.as_str(),
                    task.created_at,
                    task.finished_at
                        .map(|at| at.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ));
            }
            Ok(ToolOutput::ok(content))
        }
        Err(message) => Ok(ToolOutput::err(message)),
    }
}

pub async fn execute_output(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let args: OutputArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(error) => return Ok(ToolOutput::err(format!("background_task_output 参数错误：{error}"))),
    };
    let session_id = match session_id(ctx) {
        Ok(id) => id,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let pool = match pool(ctx) {
        Ok(pool) => pool,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let task = match background_tasks::get(&pool, &args.task_id).await {
        Ok(Some(task)) => task,
        Ok(None) => return Ok(ToolOutput::err(format!("后台任务 {} 不存在。", args.task_id))),
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    if task.session_id != session_id {
        return Ok(ToolOutput::err(format!(
            "后台任务 {} 不属于本会话。",
            args.task_id
        )));
    }
    let limit = args.limit.unwrap_or(OUTPUT_TOOL_LIMIT).min(OUTPUT_TOOL_LIMIT);
    let tail = background_tasks::output_tail(std::path::Path::new(&task.output_path), limit);
    let mut content = format!(
        "任务 {} [{}] 退出码 {}\n",
        task.id,
        task.status.as_str(),
        task.exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    if tail.trim().is_empty() {
        content.push_str("（暂无输出）");
    } else {
        content.push_str(&tail);
    }
    Ok(ToolOutput::ok(content))
}

pub async fn execute_stop(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let args: StopArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(error) => return Ok(ToolOutput::err(format!("background_task_stop 参数错误：{error}"))),
    };
    let session_id = match session_id(ctx) {
        Ok(id) => id,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let pool = match pool(ctx) {
        Ok(pool) => pool,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    match background_tasks::stop(&pool, &session_id, &args.task_id).await {
        Ok(task) => Ok(ToolOutput::ok(format!(
            "后台任务 {} 现在是 [{}]（进程树已结束）。",
            task.id,
            task.status.as_str()
        ))),
        Err(message) => Ok(ToolOutput::err(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn background_task_tools_are_advertised() {
        for definition in [
            start_definition(),
            list_definition(),
            output_definition(),
            stop_definition(),
        ] {
            assert!(definition.function.name.starts_with("background_task_"));
            assert_eq!(definition.function.parameters["type"], "object");
        }
    }

    #[tokio::test]
    async fn background_tasks_refuse_to_run_without_a_session() {
        let dir = std::env::temp_dir().join(format!("cf-bg-nosession-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = ExecCtx::new(dir.clone(), None);
        let out = execute_start(json!({ "command": "echo hi" }), &ctx)
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("session_id"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
