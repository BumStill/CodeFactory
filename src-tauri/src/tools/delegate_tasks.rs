// SPDX-License-Identifier: Apache-2.0
//! Session-native task delegation. The chat agent uses this tool when a request
//! has multiple independently executable parts; users never need to leave the
//! conversation or operate a separate decomposition workflow.

#[cfg(not(test))]
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
#[cfg(not(test))]
use std::sync::Arc;
#[cfg(not(test))]
use tauri::Manager;
#[cfg(not(test))]
use uuid::Uuid;

use super::{ExecCtx, ToolOutput};
#[cfg(not(test))]
use crate::agent::scheduler::TaskScheduler;
#[cfg(not(test))]
use crate::commands::tasks::SchedulerHandles;
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};
#[cfg(not(test))]
use crate::storage::tasks::{self as task_storage, TaskRun};
#[cfg(not(test))]
use crate::AppState;

const MAX_DELEGATED_TASKS: usize = 8;

#[derive(Debug, Clone, Deserialize)]
struct DelegateTasksArgs {
    tasks: Vec<DelegatedTask>,
}

#[derive(Debug, Clone, Deserialize)]
struct DelegatedTask {
    id: String,
    title: String,
    description: String,
    #[serde(default)]
    dependencies: Vec<String>,
    acceptance_criteria: Vec<String>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "delegate_tasks".into(),
            description: "Delegate a complex request to parallel subagents inside the current conversation. Use this only when there are at least two independently executable work items; simple work should be completed directly. The task tree is created and execution starts automatically, so never ask the user to open a separate task-decomposition screen.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "minItems": 2,
                        "maxItems": MAX_DELEGATED_TASKS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Short unique id used by dependency references, e.g. backend" },
                                "title": { "type": "string" },
                                "description": { "type": "string", "description": "Self-contained implementation brief" },
                                "dependencies": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Ids of tasks that must finish first"
                                },
                                "acceptance_criteria": {
                                    "type": "array",
                                    "minItems": 1,
                                    "items": { "type": "string" },
                                    "description": "Machine-checkable or observable done conditions"
                                }
                            },
                            "required": ["id", "title", "description", "dependencies", "acceptance_criteria"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["tasks"],
                "additionalProperties": false
            }),
        },
    }
}

fn validate_tasks(tasks: &[DelegatedTask]) -> std::result::Result<(), String> {
    if !(2..=MAX_DELEGATED_TASKS).contains(&tasks.len()) {
        return Err(format!(
            "delegate_tasks requires 2-{MAX_DELEGATED_TASKS} tasks; complete simple work directly"
        ));
    }

    let mut ids = HashSet::new();
    for task in tasks {
        if task.id.trim().is_empty()
            || task.title.trim().is_empty()
            || task.description.trim().is_empty()
        {
            return Err("every delegated task needs a non-empty id, title, and description".into());
        }
        if task.acceptance_criteria.is_empty()
            || task
                .acceptance_criteria
                .iter()
                .any(|criterion| criterion.trim().is_empty())
        {
            return Err(format!(
                "task '{}' needs at least one non-empty acceptance criterion",
                task.id
            ));
        }
        if !ids.insert(task.id.clone()) {
            return Err(format!("duplicate delegated task id '{}'", task.id));
        }
    }

    for task in tasks {
        for dependency in &task.dependencies {
            if dependency == &task.id {
                return Err(format!("task '{}' cannot depend on itself", task.id));
            }
            if !ids.contains(dependency) {
                return Err(format!(
                    "task '{}' references unknown dependency '{}'",
                    task.id, dependency
                ));
            }
        }
    }

    fn visit(
        id: &str,
        by_id: &HashMap<&str, &DelegatedTask>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
    ) -> bool {
        if visited.contains(id) {
            return true;
        }
        if !visiting.insert(id.to_owned()) {
            return false;
        }
        let acyclic = by_id[id]
            .dependencies
            .iter()
            .all(|dependency| visit(dependency, by_id, visiting, visited));
        visiting.remove(id);
        if acyclic {
            visited.insert(id.to_owned());
        }
        acyclic
    }

    let by_id = tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<HashMap<_, _>>();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    if tasks
        .iter()
        .any(|task| !visit(&task.id, &by_id, &mut visiting, &mut visited))
    {
        return Err("delegated task dependencies contain a cycle".into());
    }
    Ok(())
}

#[cfg(not(test))]
pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let args: DelegateTasksArgs = serde_json::from_value(args)?;
    if let Err(message) = validate_tasks(&args.tasks) {
        return Ok(ToolOutput::err(message));
    }
    if ctx.task_id.is_some() {
        return Ok(ToolOutput::err(
            "subagents cannot recursively delegate more tasks",
        ));
    }

    let Some(db) = ctx.db.clone() else {
        return Ok(ToolOutput::err("delegate_tasks requires a persisted project session"));
    };
    let Some(session_id) = ctx.session_id.clone() else {
        return Ok(ToolOutput::err("delegate_tasks requires a current session"));
    };
    let Some(app) = ctx.app.clone() else {
        return Ok(ToolOutput::err("delegate_tasks is unavailable in this runtime"));
    };
    let Some(settings) = ctx.settings.clone() else {
        return Ok(ToolOutput::err("delegate_tasks requires project settings"));
    };

    let state = app.state::<AppState>();
    let handles = app.state::<SchedulerHandles>().inner().clone();
    if handles.lock().await.contains_key(&session_id) {
        return Ok(ToolOutput::err(
            "this session already has delegated work running; guide it in the conversation instead",
        ));
    }

    let persisted_cwd: Option<String> = sqlx::query_scalar(
        "SELECT cwd FROM sessions WHERE id = ? AND kind = 'project'",
    )
    .bind(&session_id)
    .fetch_optional(&db)
    .await?;
    if persisted_cwd.as_deref() != Some(ctx.cwd.to_string_lossy().as_ref()) {
        return Ok(ToolOutput::err(
            "delegate_tasks is available only in the current persisted project session",
        ));
    }

    let task_context_json = serde_json::to_string(
        &crate::knowledge::enabled_library_context(&db).await?,
    )?;
    let now = Utc::now().to_rfc3339();
    let mut tmp_to_real = HashMap::new();
    let mut ids = Vec::with_capacity(args.tasks.len());
    for task in &args.tasks {
        let id = Uuid::new_v4().to_string();
        tmp_to_real.insert(task.id.clone(), id.clone());
        task_storage::insert_task(
            &db,
            &TaskRun {
                id: id.clone(),
                session_id: session_id.clone(),
                title: task.title.clone(),
                description: task.description.clone(),
                status: "pending".into(),
                cwd: ctx.cwd.to_string_lossy().into_owned(),
                parent_task_id: None,
                sub_session_id: None,
                created_at: now.clone(),
                started_at: None,
                completed_at: None,
                result: None,
                error: None,
                attempt_count: 0,
                verification_results: None,
                task_context_json: Some(task_context_json.clone()),
                acceptance_criteria_json: Some(serde_json::to_string(
                    &task.acceptance_criteria,
                )?),
                spec_req_id: None,
                spec_title: None,
            },
        )
        .await?;
        ids.push(id);
    }
    for task in &args.tasks {
        for dependency in &task.dependencies {
            task_storage::add_dependency(
                &db,
                &tmp_to_real[&task.id],
                &tmp_to_real[dependency],
            )
            .await?;
        }
    }

    let scheduler = Arc::new(TaskScheduler::new(db.clone()));
    let cancel = scheduler.cancel_handle();
    handles
        .lock()
        .await
        .insert(session_id.clone(), cancel);
    crate::commands::tasks::spawn_delegated_session(
        scheduler,
        session_id.clone(),
        settings,
        app.clone(),
        state.pending_permissions.clone(),
        state.interjections.clone(),
        handles,
    );

    Ok(ToolOutput::ok(
        json!({
            "session_id": session_id,
            "task_count": ids.len(),
            "task_ids": ids,
            "status": "execution_started",
            "message": "Delegated tasks are running inside this session. Do not duplicate their implementation in the parent agent."
        })
        .to_string(),
    ))
}


// Keep validation and tool-dispatch coverage in unit-test builds without
// linking the desktop scheduler runtime into the standalone Windows test
// harness. Production builds use the implementation above unchanged.
#[cfg(test)]
pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let args: DelegateTasksArgs = serde_json::from_value(args)?;
    if let Err(message) = validate_tasks(&args.tasks) {
        return Ok(ToolOutput::err(message));
    }
    if ctx.task_id.is_some() {
        return Ok(ToolOutput::err(
            "subagents cannot recursively delegate more tasks",
        ));
    }
    Ok(ToolOutput::err(
        "delegate_tasks execution is unavailable in unit-test builds",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, dependencies: &[&str]) -> DelegatedTask {
        DelegatedTask {
            id: id.into(),
            title: format!("Task {id}"),
            description: format!("Implement {id}"),
            dependencies: dependencies.iter().map(|value| (*value).into()).collect(),
            acceptance_criteria: vec![format!("{id} test passes")],
        }
    }

    #[test]
    fn delegated_tasks_require_parallelizable_valid_dag() {
        assert!(validate_tasks(&[task("api", &[]), task("ui", &["api"])]).is_ok());
        assert!(validate_tasks(&[task("only", &[])]).is_err());
        assert!(validate_tasks(&[task("a", &["b"]), task("b", &["a"])]).is_err());
        assert!(validate_tasks(&[task("a", &["missing"]), task("b", &[])]).is_err());
    }

    #[test]
    fn definition_explains_session_native_automatic_execution() {
        let definition = definition();
        assert_eq!(definition.function.name, "delegate_tasks");
        assert!(definition.function.description.contains("current conversation"));
        assert!(definition.function.description.contains("starts automatically"));
        assert!(!definition.function.description.contains("open the task"));
    }
}
