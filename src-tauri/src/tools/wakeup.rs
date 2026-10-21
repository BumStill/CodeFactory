// SPDX-License-Identifier: Apache-2.0
//! CF-BG 阶段 2（R2）的工具面：`schedule_wakeup|wakeup_list|wakeup_cancel`。
//!
//! 与阶段 1 的 `background_task_*` 同一层：工具只做参数解析与会话归属校验，
//! 定时、持久化、上限、回收都在 `crate::agent::background_tasks` 里，
//! 因此 headless 测试覆盖的是真实实现（真实 SQLite + 真实调度函数）。

use serde::Deserialize;
use serde_json::{json, Value};

use super::{ExecCtx, ToolOutput};
use crate::agent::background_tasks::{
    self, TaskSource, WakeupRequest, MAX_WAKEUP_SECS, MIN_WAKEUP_SECS,
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

pub fn schedule_definition() -> ToolDefinition {
    definition(
        "schedule_wakeup",
        "Wake this session later: after_sec (once) or every_sec (repeating), with one sentence of \
         guidance as the prompt. Use it for waits with no process to finish — CI queues, merge \
         queues, releases, periodic status checks. The wake arrives as a new input to this session \
         and the schedule survives an app restart. Minimum 60s, maximum 6h, cancel with wakeup_cancel.",
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "One sentence telling the woken session what to check or do (e.g. \"Check whether PR #123 CI is green\")"
                },
                "after_sec": {
                    "type": "integer",
                    "minimum": MIN_WAKEUP_SECS,
                    "maximum": MAX_WAKEUP_SECS,
                    "description": "Wake once after this many seconds. Omit it, or pass 0/null, when you do not want a one-shot wakeup"
                },
                "every_sec": {
                    "type": "integer",
                    "minimum": MIN_WAKEUP_SECS,
                    "maximum": MAX_WAKEUP_SECS,
                    "description": "Wake every this many seconds. Omit it, or pass 0/null, when you do not want a repeating wakeup"
                },
                "source": {
                    "type": "string",
                    "enum": ["agent_decided", "user_requested"],
                    "description": "agent_decided when you chose the schedule; user_requested when the user asked you to watch something"
                }
            },
            "required": ["prompt"]
        }),
    )
}

pub fn list_definition() -> ToolDefinition {
    definition(
        "wakeup_list",
        "List this session's scheduled wakeups with prompt, cadence, next fire time and status.",
        json!({ "type": "object", "properties": {} }),
    )
}

pub fn cancel_definition() -> ToolDefinition {
    definition(
        "wakeup_cancel",
        "Cancel a scheduled wakeup by wakeup_id so it never fires again.",
        json!({
            "type": "object",
            "properties": { "wakeup_id": { "type": "string" } },
            "required": ["wakeup_id"]
        }),
    )
}

fn session_id(ctx: &ExecCtx) -> std::result::Result<String, String> {
    ctx.session_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            "定时唤醒属于会话：当前上下文没有 session_id，无法创建或管理定时唤醒。".to_string()
        })
}

fn pool(ctx: &ExecCtx) -> std::result::Result<sqlx::SqlitePool, String> {
    ctx.db
        .clone()
        .ok_or_else(|| "定时唤醒需要应用数据库：当前上下文没有可用的 SQLite 连接。".to_string())
}

#[derive(Deserialize)]
struct ScheduleArgs {
    prompt: String,
    #[serde(default)]
    after_sec: Option<u64>,
    #[serde(default)]
    every_sec: Option<u64>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
struct CancelArgs {
    wakeup_id: String,
}

/// 唤醒时间的可读描述：一次性说"多久以后"，周期说"每多久"。
fn cadence_text(every_sec: Option<u64>, after_sec: Option<u64>) -> String {
    match (every_sec, after_sec) {
        (Some(every), _) => format!("每 {every} 秒"),
        (None, Some(after)) => format!("{after} 秒后（一次性）"),
        (None, None) => "未指定".to_string(),
    }
}

pub async fn execute_schedule(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let args: ScheduleArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(error) => {
            return Ok(ToolOutput::err(format!(
                "schedule_wakeup 参数错误：{error}"
            )))
        }
    };
    let session_id = match session_id(ctx) {
        Ok(id) => id,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let pool = match pool(ctx) {
        Ok(pool) => pool,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let source = match args.source.as_deref() {
        Some("user_requested") => TaskSource::UserRequested,
        _ => TaskSource::AgentDecided,
    };
    let request = WakeupRequest {
        session_id,
        root_turn_id: ctx.root_turn_id.clone(),
        prompt: args.prompt.clone(),
        source,
        after_sec: args.after_sec,
        every_sec: args.every_sec,
    };
    match background_tasks::schedule_wakeup(&pool, request).await {
        Ok(wakeup) => {
            let repeating = if wakeup.every_sec.is_some() {
                "到点后按周期继续，直到用 wakeup_cancel 取消。"
            } else {
                "触发一次后自动结束。"
            };
            Ok(ToolOutput::ok(format!(
                "已排好定时唤醒 {}\n节奏：{}｜来源：{}\n提示：{}\n\
                 到点会以这句话唤醒本会话。{}要提前取消用 wakeup_cancel wakeup_id={}；\
                 查看全部用 wakeup_list。",
                wakeup.id,
                cadence_text(wakeup.every_sec, wakeup.after_sec),
                wakeup.source.as_str(),
                wakeup.prompt,
                repeating,
                wakeup.id
            ))
            .with_metadata(json!({
                "wakeup_id": wakeup.id,
                "next_at": wakeup.next_at,
                "every_sec": wakeup.every_sec,
                "after_sec": wakeup.after_sec,
                "source": wakeup.source.as_str(),
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
    match background_tasks::list_wakeups(&pool, &session_id).await {
        Ok(wakeups) if wakeups.is_empty() => Ok(ToolOutput::ok("本会话没有定时唤醒。")),
        Ok(wakeups) => {
            let mut content = String::from("本会话的定时唤醒：\n");
            for wakeup in &wakeups {
                content.push_str(&format!(
                    "- {} [{}] {} | 提示 {} | 下次 {} | 已触发 {} | 来源 {}\n",
                    wakeup.id,
                    wakeup.status,
                    wakeup.cadence(),
                    wakeup.prompt,
                    wakeup.next_at,
                    wakeup.fire_count,
                    wakeup.source.as_str()
                ));
            }
            Ok(ToolOutput::ok(content))
        }
        Err(message) => Ok(ToolOutput::err(message)),
    }
}

pub async fn execute_cancel(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let args: CancelArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(error) => return Ok(ToolOutput::err(format!("wakeup_cancel 参数错误：{error}"))),
    };
    let session_id = match session_id(ctx) {
        Ok(id) => id,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let pool = match pool(ctx) {
        Ok(pool) => pool,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    match background_tasks::cancel_wakeup(&pool, &session_id, &args.wakeup_id).await {
        Ok(wakeup) => Ok(ToolOutput::ok(format!(
            "定时唤醒 {} 现在是 [{}]，不会再触发。",
            wakeup.id, wakeup.status
        ))),
        Err(message) => Ok(ToolOutput::err(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakeup_tools_are_advertised_with_bounded_intervals() {
        for definition in [
            schedule_definition(),
            list_definition(),
            cancel_definition(),
        ] {
            assert_eq!(definition.function.parameters["type"], "object");
        }
        let schedule = schedule_definition();
        assert!(schedule.function.name.eq("schedule_wakeup"));
        let properties = &schedule.function.parameters["properties"];
        assert_eq!(properties["after_sec"]["minimum"], MIN_WAKEUP_SECS);
        assert_eq!(properties["every_sec"]["maximum"], MAX_WAKEUP_SECS);
        assert!(schedule.function.description.contains("wakeup_cancel"));
    }

    #[test]
    fn cadence_text_distinguishes_one_shot_from_repeating() {
        assert_eq!(cadence_text(Some(300), None), "每 300 秒");
        assert_eq!(cadence_text(None, Some(300)), "300 秒后（一次性）");
    }

    #[tokio::test]
    async fn scheduling_without_a_session_is_refused_with_a_readable_sentence() {
        let dir = std::env::temp_dir().join(format!("cf-bg-wakeup-nosession-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = ExecCtx::new(dir.clone(), None);
        let out = execute_schedule(json!({ "prompt": "检查状态", "every_sec": 300 }), &ctx)
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("session_id"), "{}", out.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn m149_zero_every_sec_schedules_a_single_one_shot_wakeup() {
        let dir = std::env::temp_dir().join(format!("cf-bg-wakeup-m149-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let mut ctx = ExecCtx::new(dir.clone(), Some(pool.clone()));
        ctx.session_id = Some("s-m149".to_string());
        let out = execute_schedule(
            json!({"prompt": "检查 PR #1 的 CI 是否全绿", "after_sec": 180, "every_sec": 0}),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("180 秒后"), "{}", out.content);
        let rows = crate::agent::background_tasks::list_wakeups(&pool, "s-m149")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].after_sec, Some(180));
        assert_eq!(rows[0].every_sec, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
