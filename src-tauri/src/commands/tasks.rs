// SPDX-License-Identifier: Apache-2.0
//! Tauri commands for the task system.
//!
//! Wires the frontend dashboard to the [`scheduler`](crate::agent::scheduler)
//! and persists task trees with client-side temporary ids that get resolved
//! to real DB ids on insert.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agent::scheduler::TaskScheduler;
use crate::agent::verification::{self, VerificationResult};
use crate::commands::evidence;
use crate::errors::AppError;
use crate::storage::tasks::{self, TaskRun};
use crate::AppState;
use crate::util::no_window::NoWindow;

/// Map of session_id -> cancel flag for the running scheduler. When the user
/// hits Cancel we flip the flag and drop the entry. The actual scheduler task
/// is fire-and-forget — it polls the flag and exits gracefully.
pub type SchedulerHandles = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInput {
    pub tmp_id: String,
    pub title: String,
    pub description: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDep {
    pub task_tmp_id: String,
    pub depends_on_tmp_id: String,
}

/// Persist a task tree. `tmp_id`s are mapped to fresh UUIDs and then dependencies
/// are wired up. Returns the list of real DB ids in the same order as input tasks.
#[tauri::command]
pub async fn create_task_tree(
    session_id: String,
    tasks_in: Vec<TaskInput>,
    dependencies: Vec<TaskDep>,
    state: State<'_, AppState>,
) -> Result<Vec<String>, AppError> {
    let pool = state.db.read().await;
    let mut tmp_to_real: HashMap<String, String> = HashMap::new();
    let mut real_ids: Vec<String> = Vec::with_capacity(tasks_in.len());
    let now = Utc::now().to_rfc3339();

    for t in &tasks_in {
        let id = Uuid::new_v4().to_string();
        tmp_to_real.insert(t.tmp_id.clone(), id.clone());
        let row = TaskRun {
            id: id.clone(),
            session_id: session_id.clone(),
            title: t.title.clone(),
            description: t.description.clone(),
            status: "pending".into(),
            cwd: t.cwd.clone(),
            parent_task_id: None,
            sub_session_id: None,
            created_at: now.clone(),
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
            attempt_count: 0,
            verification_results: None,
        };
        tasks::insert_task(&pool, &row).await?;
        real_ids.push(id);
    }

    for dep in &dependencies {
        let task_id = tmp_to_real.get(&dep.task_tmp_id).ok_or_else(|| {
            AppError::Other(format!("Unknown tmp_id in dep: {}", dep.task_tmp_id))
        })?;
        let depends_on = tmp_to_real.get(&dep.depends_on_tmp_id).ok_or_else(|| {
            AppError::Other(format!(
                "Unknown depends_on_tmp_id: {}",
                dep.depends_on_tmp_id
            ))
        })?;
        tasks::add_dependency(&pool, task_id, depends_on).await?;
    }

    Ok(real_ids)
}

#[tauri::command]
pub async fn list_tasks(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<TaskRun>, AppError> {
    let pool = state.db.read().await;
    let rows = tasks::list_session_tasks(&pool, &session_id).await?;
    Ok(rows)
}

#[tauri::command]
pub async fn get_task_detail(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<TaskRun, String> {
    let pool = state.db.read().await;
    let row = tasks::get_task(&pool, &task_id)
        .await
        .map_err(|e| e.to_string())?;
    row.ok_or_else(|| format!("Task '{}' not found", task_id))
}

/// Returns the dependency edges for a task (real DB ids of tasks it depends on).
#[tauri::command]
pub async fn get_task_dependencies(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, AppError> {
    let pool = state.db.read().await;
    let deps = tasks::get_dependencies(&pool, &task_id).await?;
    Ok(deps)
}

/// Spawn the scheduler in a background tokio task and stash its cancel handle.
/// Idempotent: if a scheduler is already running for the session it returns Ok
/// without starting a second one.
///
/// Optional `spec_req_id` and `spec_title` enable auto-collection of an
/// Evidence Pack once all tasks for the session are done.
#[tauri::command]
pub async fn start_implementation(
    session_id: String,
    spec_req_id: Option<String>,
    spec_title: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
    handles: State<'_, SchedulerHandles>,
) -> Result<(), AppError> {
    {
        let h = handles.lock().await;
        if h.contains_key(&session_id) {
            return Ok(()); // already running
        }
    }

    let pool = state.db.read().await.clone();
    let settings = state.settings.read().await.clone();
    let pending_perms = state.pending_permissions.clone();

    let scheduler = Arc::new(TaskScheduler::new(pool.clone()));
    let cancel = scheduler.cancel_handle();

    handles
        .lock()
        .await
        .insert(session_id.clone(), cancel.clone());

    let session_id_clone = session_id.clone();
    let handles_clone = handles.inner().clone();
    tokio::spawn(async move {
        let result = scheduler
            .run_session(session_id_clone.clone(), settings.clone(), app.clone(), pending_perms)
            .await;
        if let Err(e) = result {
            tracing::error!("scheduler error for session {}: {:#}", session_id_clone, e);
        }
        // Clean up the cancel handle.
        handles_clone.lock().await.remove(&session_id_clone);

        // Auto-collect evidence pack if spec info was provided.
        if let (Some(ref req_id), Some(ref title)) = (&spec_req_id, &spec_title) {
            // Gather all task ids for this session.
            let task_ids: Vec<String> = crate::storage::tasks::list_session_tasks(&pool, &session_id_clone)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.id)
                .collect();

            // Get cwd from the session.
            let cwd: String = sqlx::query_as::<_, (String,)>(
                "SELECT cwd FROM sessions WHERE id = ?",
            )
            .bind(&session_id_clone)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .map(|(c,)| c)
            .unwrap_or_default();

            if !cwd.is_empty() && !task_ids.is_empty() {
                evidence::auto_collect_and_emit(
                    &app,
                    &pool,
                    &session_id_clone,
                    &cwd,
                    req_id,
                    title,
                    &task_ids,
                )
                .await;

                // Auto-create draft PR if configured.
                auto_create_pr_if_configured(
                    &app,
                    &settings,
                    &session_id_clone,
                    &cwd,
                    title,
                )
                .await;
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn cancel_implementation(
    session_id: String,
    handles: State<'_, SchedulerHandles>,
) -> Result<(), AppError> {
    if let Some(flag) = handles.lock().await.get(&session_id) {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(())
}

/// Return the persisted verification results for a task.
/// Returns an empty Vec when the task hasn't been verified yet.
#[tauri::command]
pub async fn get_verification_results(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<VerificationResult>, String> {
    let pool = state.db.read().await;
    let raw = tasks::get_verification_results(&pool, &task_id)
        .await
        .map_err(|e| e.to_string())?;
    match raw {
        None => Ok(Vec::new()),
        Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string()),
    }
}

/// Auto-detect a verification plan for the session's cwd, run it
/// synchronously, persist the results, and return them.
#[tauri::command]
pub async fn run_verification_now(
    session_id: String,
    task_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<VerificationResult>, String> {
    // Determine the cwd from the task row.
    let pool = state.db.read().await.clone();
    let task = tasks::get_task(&pool, &task_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task '{}' not found", task_id))?;

    let plan = verification::detect_verification_plan(&task.cwd);
    let results =
        verification::run_verification(&plan, &app, &session_id, &task_id).await;

    // Persist.
    let json = serde_json::to_string(&results).map_err(|e| e.to_string())?;
    tasks::save_verification_results(&pool, &task_id, &json)
        .await
        .map_err(|e| e.to_string())?;

    Ok(results)
}

// ── Auto PR creation helper ───────────────────────────────────────────────────

async fn auto_create_pr_if_configured(
    app: &AppHandle,
    settings: &crate::config::Settings,
    session_id: &str,
    cwd: &str,
    spec_title: &str,
) {
    if !settings.auto_create_pr || settings.git_remotes.is_empty() {
        return;
    }

    // Find a remote with a default_repo set (use first match).
    let remote = match settings.git_remotes.iter().find(|r| r.default_repo.is_some()) {
        Some(r) => r.clone(),
        None => return,
    };
    let repo = remote.default_repo.as_deref().unwrap_or("").to_string();
    if repo.is_empty() {
        return;
    }

    // Get current branch.
    let branch_output = std::process::Command::new("git").no_window()
        .current_dir(cwd)
        .args(["branch", "--show-current"])
        .output();

    let current_branch = match branch_output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        _ => return,
    };

    if current_branch.is_empty() {
        return;
    }

    // Get default branch of the repo to use as base.
    let client = crate::git_remote::client::RemoteGitClient::new(
        &remote.base_url,
        &remote.token,
        remote.provider.clone(),
    );

    let default_branch = match &remote.provider {
        crate::config::settings::GitProvider::Github => {
            match client.get(&format!("/repos/{}", repo)).await {
                Ok(v) => v.get("default_branch").and_then(|x| x.as_str()).unwrap_or("main").to_string(),
                Err(_) => "main".to_string(),
            }
        }
        crate::config::settings::GitProvider::Gitlab => "main".to_string(),
    };

    if current_branch == default_branch {
        return; // Don't create PR from default branch to itself.
    }

    let pr_body = format!(
        "Auto-created draft PR for spec: **{}**\n\nSession: `{}`",
        spec_title, session_id
    );

    let pr_result = match &remote.provider {
        crate::config::settings::GitProvider::Github => {
            crate::git_remote::github::create_pr(
                &client, &repo, spec_title, &pr_body,
                &current_branch, &default_branch, true,
            ).await
        }
        crate::config::settings::GitProvider::Gitlab => {
            crate::git_remote::gitlab::create_pr(
                &client, &repo, spec_title, &pr_body,
                &current_branch, &default_branch, true,
            ).await
        }
    };

    match pr_result {
        Ok(pr) => {
            tracing::info!("Auto-created draft PR: {}", pr.url);
            let _ = app.emit(&format!("pr_created:{}", session_id), &pr.url);
        }
        Err(e) => {
            tracing::warn!("Auto-create PR failed: {}", e);
        }
    }
}
