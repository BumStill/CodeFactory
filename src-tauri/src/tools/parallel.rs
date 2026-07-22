// SPDX-License-Identifier: Apache-2.0
//! `dispatch_parallel_tasks` — the model-facing fan-out entry (WorkBuddy-gap
//! P1). The scheduler already runs tasks concurrently (semaphore-capped) and
//! re-queries pending rows every tick with a CAS claim, so all this tool has
//! to do is insert independent pending rows: a running scheduler picks them
//! up in parallel automatically. In plain chat (no scheduler running) the
//! rows appear in the task panel waiting for a start click — the tool's
//! return text tells the model which situation it is in.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

/// Hard cap per fan-out call: enough for real decomposition, small enough
/// that a runaway model cannot flood the queue.
pub const MAX_FANOUT: usize = 12;

#[derive(Deserialize)]
struct FanoutTask {
    title: String,
    description: String,
}

#[derive(Deserialize)]
struct Args {
    tasks: Vec<FanoutTask>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "dispatch_parallel_tasks".into(),
            description: "Fan out INDEPENDENT sub-tasks to run in parallel (each gets its own \
                autonomous agent, concurrency capped by the user's max_parallel_tasks setting). \
                Use for large decomposable work — auditing many modules, migrating many files, \
                researching several directions — where sub-tasks do not depend on each other and \
                do not edit the same files. Do NOT use for sequential steps of one change. Each \
                task needs a short title and a self-contained description (its agent sees nothing \
                of this conversation). Results land in the task panel as each finishes."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "description": { "type": "string", "description": "Self-contained brief: goal, scope, acceptance. The sub-agent sees ONLY this." }
                            },
                            "required": ["title", "description"]
                        }
                    }
                },
                "required": ["tasks"]
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let a: Args = match serde_json::from_value(args) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!("Invalid arguments: {e}"))),
    };
    if a.tasks.is_empty() {
        return Ok(ToolOutput::err(
            "tasks 不能为空;每个并行子任务需要 title 和自包含的 description。",
        ));
    }
    if a.tasks.len() > MAX_FANOUT {
        return Ok(ToolOutput::err(format!(
            "一次最多派发 {MAX_FANOUT} 个并行任务(收到 {});请合并或分批。",
            a.tasks.len()
        )));
    }
    let Some(db) = ctx.db.as_ref() else {
        return Ok(ToolOutput::err("此上下文没有数据库,无法派发任务。"));
    };
    let Some(session_id) = ctx.session_id.as_deref() else {
        return Ok(ToolOutput::err("此上下文没有会话,无法派发任务。"));
    };

    let now = chrono::Utc::now().to_rfc3339();
    let cwd = ctx.cwd.to_string_lossy().to_string();
    let mut ids = Vec::with_capacity(a.tasks.len());
    for t in &a.tasks {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO task_runs (id, session_id, title, description, status, cwd, created_at, attempt_count) \
             VALUES (?,?,?,?,'pending',?,?,0)",
        )
        .bind(&id)
        .bind(session_id)
        .bind(&t.title)
        .bind(&t.description)
        .bind(&cwd)
        .bind(&now)
        .execute(db)
        .await?;
        ids.push(id);
    }

    // Inside an autonomous run (ctx.task_id set) the session scheduler is
    // live and will claim these rows on its next tick; in plain chat the
    // rows wait in the task panel for a start click.
    let dispatch_note = if ctx.task_id.is_some() {
        "调度器运行中,将按并发上限自动并行执行。"
    } else {
        "已进入任务面板;在任务面板点击开始即可并行执行。"
    };
    Ok(ToolOutput::ok(format!(
        "已创建 {} 个并行子任务(互相独立,无依赖)。{}\n任务:{}",
        ids.len(),
        dispatch_note,
        a.tasks
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>()
            .join("、"),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool_with_schema() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, title TEXT NOT NULL,
                description TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
                cwd TEXT NOT NULL, parent_task_id TEXT, sub_session_id TEXT,
                created_at TEXT NOT NULL, started_at TEXT, completed_at TEXT,
                result TEXT, error TEXT, attempt_count INTEGER NOT NULL DEFAULT 0,
                verification_results TEXT, task_context_json TEXT,
                acceptance_criteria_json TEXT, spec_req_id TEXT, spec_title TEXT,
                owner_pid INTEGER, owner_start_token TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn ctx(pool: sqlx::SqlitePool, in_autonomous_run: bool) -> ExecCtx {
        let mut ctx = ExecCtx::new(std::path::PathBuf::from("/proj"), Some(pool));
        ctx.session_id = Some("session-1".into());
        if in_autonomous_run {
            ctx.task_id = Some("parent-task".into());
        }
        ctx
    }

    #[tokio::test]
    async fn fanout_inserts_independent_pending_rows_for_the_session() {
        let pool = pool_with_schema().await;
        let out = execute(
            json!({"tasks": [
                {"title": "审计模块 A", "description": "检查 A 的错误处理"},
                {"title": "审计模块 B", "description": "检查 B 的错误处理"},
                {"title": "审计模块 C", "description": "检查 C 的错误处理"},
            ]}),
            &ctx(pool.clone(), true),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("3 个并行子任务"));
        assert!(out.content.contains("调度器运行中"));

        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT session_id, status, cwd FROM task_runs ORDER BY created_at",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 3);
        for (session_id, status, cwd) in rows {
            assert_eq!(session_id, "session-1");
            assert_eq!(status, "pending");
            assert_eq!(cwd, "/proj");
        }
    }

    #[tokio::test]
    async fn plain_chat_fanout_tells_the_model_to_point_at_the_task_panel() {
        let pool = pool_with_schema().await;
        let out = execute(
            json!({"tasks": [{"title": "T", "description": "D"}]}),
            &ctx(pool, false),
        )
        .await
        .unwrap();
        assert!(out.content.contains("任务面板"));
    }

    #[tokio::test]
    async fn fanout_rejects_empty_and_oversized_batches() {
        let pool = pool_with_schema().await;
        let empty = execute(json!({"tasks": []}), &ctx(pool.clone(), true))
            .await
            .unwrap();
        assert!(empty.is_error);

        let too_many: Vec<_> = (0..13)
            .map(|i| json!({"title": format!("t{i}"), "description": "d"}))
            .collect();
        let over = execute(json!({ "tasks": too_many }), &ctx(pool, true))
            .await
            .unwrap();
        assert!(over.is_error);
        assert!(over.content.contains("最多派发"));
    }
}
