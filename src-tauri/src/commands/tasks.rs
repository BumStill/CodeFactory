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
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agent::objective::{ObjectiveSnapshot, ObjectiveStatus};
use crate::agent::scheduler::{TaskMutationPermits, TaskScheduler};
use crate::agent::verification::{self, VerificationResult};
use crate::commands::evidence;
use crate::errors::AppError;
use crate::storage::tasks::{
    self, classify_task_failure, TaskAttempt, TaskConnectorContext, TaskFailureAttribution, TaskRun,
};
use crate::util::no_window::NoWindow;
use crate::AppState;

/// Map of session_id -> cancel flag for the running scheduler. When the user
/// hits Cancel we flip the flag and drop the entry. The actual scheduler task
/// is fire-and-forget — it polls the flag and exits gracefully.
#[derive(Clone)]
pub struct SchedulerHandle {
    pub cancel: Arc<AtomicBool>,
    pub mutation_permits: TaskMutationPermits,
}

impl SchedulerHandle {
    fn for_scheduler(scheduler: &TaskScheduler) -> Self {
        Self {
            cancel: scheduler.cancel_handle(),
            mutation_permits: scheduler.mutation_permits(),
        }
    }
}

pub type SchedulerHandles = Arc<Mutex<HashMap<String, SchedulerHandle>>>;

struct InjectedPermitGuard {
    permits: TaskMutationPermits,
    permit: codefactory_agent_loop::tool::MutationPermit,
}

impl Drop for InjectedPermitGuard {
    fn drop(&mut self) {
        let permits = self.permits.clone();
        let permit = self.permit.clone();
        tauri::async_runtime::spawn(async move {
            let mut permits = permits.write().await;
            let still_ours = permits.get(&permit.objective_id).is_some_and(|current| {
                current.remediation_id == permit.remediation_id
                    && current.owner == permit.owner
                    && current.claim_epoch == permit.claim_epoch
            });
            if still_ours {
                permits.remove(&permit.objective_id);
            }
        });
    }
}

/// Spawn one scheduler run behind a concrete `()` boundary. Keeping this in
/// the task module prevents the session-native delegation tool from embedding
/// `run_session`'s opaque future back into the AgentLoop tool future (which
/// would otherwise create a recursive async type through subagents).
pub fn spawn_delegated_session(
    scheduler: Arc<TaskScheduler>,
    session_id: String,
    settings: crate::config::settings::Settings,
    app: AppHandle,
    pending_permissions: crate::PendingPermissionMap,
    interjections: crate::commands::interjections::InterjectionQueue,
    handles: SchedulerHandles,
) {
    tokio::spawn(async move {
        if let Err(error) = scheduler
            .run_session(
                session_id.clone(),
                settings,
                app,
                pending_permissions,
                interjections,
                None,
            )
            .await
        {
            tracing::error!(
                "session-native delegated execution failed for {}: {error:#}",
                session_id
            );
        }
        handles.lock().await.remove(&session_id);
    });
}

/// Resume one due task Objective without bypassing the scheduler's durable
/// pending→running claim. The process-local handle map deduplicates runners;
/// the task CAS remains the cross-process execution owner.
pub(crate) async fn resume_task_objective(
    app: AppHandle,
    objective: ObjectiveSnapshot,
    mutation_permit: codefactory_agent_loop::tool::MutationPermit,
) -> Result<(), AppError> {
    if objective.status != ObjectiveStatus::WaitingSystem {
        return Err(AppError::Other(format!(
            "task objective {} is not waiting_system",
            objective.id
        )));
    }
    if mutation_permit.objective_id != objective.id {
        return Err(AppError::Other(
            "task recovery mutation permit objective mismatch".into(),
        ));
    }
    let task_id = objective
        .task_id
        .as_deref()
        .ok_or_else(|| AppError::Other("task objective is missing task_id".into()))?;
    let session_id = objective
        .session_id
        .as_deref()
        .ok_or_else(|| AppError::Other("task objective is missing session_id".into()))?;

    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| AppError::Other("application state is not ready".into()))?;
    if state
        .update_restart_reserved
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(AppError::Other(
            "应用更新已进入安全重启阶段，请等待自动恢复工作区".into(),
        ));
    }
    let handles = app
        .try_state::<SchedulerHandles>()
        .ok_or_else(|| AppError::Other("task scheduler handles are not ready".into()))?
        .inner()
        .clone();
    let pool = state.db.read().await.clone();
    // A process crash can leave the task projection `running` even though the
    // durable Objective is waiting. Reconcile only provably orphaned owners;
    // a live runner is never reset or silently treated as this claim's work.
    crate::agent::journal::recover_orphaned_tasks(
        &pool,
        crate::agent::journal::OrphanScope::Session(session_id.to_string()),
    )
    .await
    .map_err(|error| AppError::Other(format!("task orphan reconciliation failed: {error:#}")))?;
    let task = tasks::get_task(&pool, task_id)
        .await?
        .ok_or_else(|| AppError::Other(format!("task {task_id} no longer exists")))?;
    if task.session_id != session_id {
        return Err(AppError::Other(format!(
            "task {task_id} does not belong to objective session {session_id}"
        )));
    }
    if tasks::task_objective_id(&pool, task_id).await?.as_deref() != Some(objective.id.as_str()) {
        return Err(AppError::Other(format!(
            "task {task_id} is not bound to objective {}",
            objective.id
        )));
    }
    match task.status.as_str() {
        "running" => {
            return Err(AppError::Other(format!(
                "task {task_id} still has a proven live runner; defer recovery until it releases ownership"
            )))
        }
        "completed" | "cancelled" => {
            return Err(AppError::Other(format!(
                "task {task_id} is terminal while objective {} still waits",
                objective.id
            )))
        }
        "pending" => {}
        status => {
            return Err(AppError::Other(format!(
                "task {task_id} cannot resume from status {status}"
            )))
        }
    }

    let (handle, scheduler_to_spawn) = {
        let mut active = handles.lock().await;
        if let Some(handle) = active.get(session_id) {
            (handle.clone(), None)
        } else {
            if state
                .update_restart_reserved
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(AppError::Other(
                    "应用更新已进入安全重启阶段，请等待自动恢复工作区".into(),
                ));
            }
            let scheduler = Arc::new(TaskScheduler::new(pool.clone()));
            let handle = SchedulerHandle::for_scheduler(&scheduler);
            active.insert(session_id.to_string(), handle.clone());
            (handle, Some(scheduler))
        }
    };
    handle
        .mutation_permits
        .write()
        .await
        .insert(objective.id.clone(), mutation_permit.clone());
    let _permit_guard = InjectedPermitGuard {
        permits: handle.mutation_permits.clone(),
        permit: mutation_permit.clone(),
    };
    if !tasks::make_task_due_for_objective(&pool, task_id, &objective.id).await? {
        if scheduler_to_spawn.is_some() {
            handles.lock().await.remove(session_id);
        }
        return Err(AppError::Other(format!(
            "task {task_id} changed before objective resume"
        )));
    }

    if let Some(scheduler) = scheduler_to_spawn {
        let settings = state.settings.read().await.clone();
        let pending_permissions = state.pending_permissions.clone();
        let interjections = state.interjections.clone();
        spawn_delegated_session(
            scheduler,
            session_id.to_string(),
            settings,
            app,
            pending_permissions,
            interjections,
            handles.clone(),
        );
    }

    // Do not return while the task future still depends on this claim. The
    // caller owns the lease heartbeat and will keep it alive until settlement
    // supersedes this remediation (or a runner failure makes the handle vanish).
    loop {
        if !crate::agent::objective::ObjectiveStore::new(pool.clone())
            .claim_is_current(&mutation_permit)
            .await
            .map_err(|error| AppError::Other(format!("observe task claim: {error:#}")))?
        {
            return Ok(());
        }
        let active_handle = handles.lock().await.get(session_id).cloned();
        if active_handle.is_none() {
            return Err(AppError::Other(format!(
                "task scheduler for {session_id} stopped before Objective settlement"
            )));
        }
        let task_status = tasks::get_task(&pool, task_id)
            .await?
            .map(|task| task.status)
            .ok_or_else(|| AppError::Other(format!("task {task_id} disappeared during resume")))?;
        if matches!(task_status.as_str(), "completed" | "cancelled") {
            return Err(AppError::Other(format!(
                "task {task_id} became {task_status} before claimed Objective settlement"
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInput {
    pub tmp_id: String,
    pub title: String,
    pub description: String,
    pub cwd: String,
    /// One bullet per user-visible behavior that must hold for the task
    /// to count as done. Examples:
    ///   - "cargo test --package codefactory --lib settings::tests passes"
    ///   - "Opening the app shows the new dark-mode theme by default"
    /// The autonomous agent loop reads these from the SubagentBrief and
    /// MUST verify each before reporting completion. The scheduler also
    /// inspects them post-task and respawns the subagent if any are not
    /// evidenced in the result. Empty list is allowed (back-compat) but
    /// strongly discouraged — decompose commands now always populate it.
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDep {
    pub task_tmp_id: String,
    pub depends_on_tmp_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskRunView {
    #[serde(flatten)]
    pub task: TaskRun,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_attribution: Option<TaskFailureAttribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<TaskAttempt>,
}

impl From<TaskRun> for TaskRunView {
    fn from(task: TaskRun) -> Self {
        let failure_attribution = classify_task_failure(&task);
        Self {
            task,
            failure_attribution,
            attempts: Vec::new(),
        }
    }
}

/// Resolve the immutable knowledge snapshot persisted on every autonomous task.
/// An empty JSON context is intentional and must not collapse to SQL NULL,
/// because NULL denotes dynamic interactive scope while [] denies task access.
async fn resolve_task_context_json(
    pool: &sqlx::SqlitePool,
    context: Option<TaskConnectorContext>,
) -> Result<String, AppError> {
    let resolved_context = match context {
        Some(context) => context,
        None => crate::knowledge::enabled_library_context(pool).await?,
    };
    Ok(serde_json::to_string(&resolved_context)?)
}

/// Persist a task tree. `tmp_id`s are mapped to fresh UUIDs and then dependencies
/// are wired up. Returns the list of real DB ids in the same order as input tasks.
#[tauri::command]
pub async fn create_task_tree(
    session_id: String,
    tasks_in: Vec<TaskInput>,
    dependencies: Vec<TaskDep>,
    context: Option<TaskConnectorContext>,
    spec_req_id: Option<String>,
    spec_title: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<String>, AppError> {
    let pool = state.db.read().await;
    let mut tmp_to_real: HashMap<String, String> = HashMap::new();
    let mut real_ids: Vec<String> = Vec::with_capacity(tasks_in.len());
    let now = Utc::now().to_rfc3339();
    let task_context_json = resolve_task_context_json(&pool, context).await?;

    for t in &tasks_in {
        let id = Uuid::new_v4().to_string();
        tmp_to_real.insert(t.tmp_id.clone(), id.clone());
        let acceptance_json = if t.acceptance_criteria.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&t.acceptance_criteria).unwrap_or_else(|_| "[]".into()))
        };
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
            task_context_json: Some(task_context_json.clone()),
            acceptance_criteria_json: acceptance_json,
            spec_req_id: spec_req_id.clone(),
            spec_title: spec_title.clone(),
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
) -> Result<Vec<TaskRunView>, AppError> {
    let pool = state.db.read().await;
    let rows = tasks::list_session_tasks(&pool, &session_id).await?;
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        let mut view = TaskRunView::from(row);
        view.attempts = tasks::list_task_attempts(&pool, &view.task.id).await?;
        views.push(view);
    }
    Ok(views)
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
    if state
        .update_restart_reserved
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(AppError::Other(
            "应用更新已进入安全重启阶段，请等待自动恢复工作区".into(),
        ));
    }
    {
        let h = handles.lock().await;
        if h.contains_key(&session_id) {
            return Ok(()); // already running
        }
    }

    let pool = state.db.read().await.clone();
    let settings = state.settings.read().await.clone();
    let pending_perms = state.pending_permissions.clone();
    let interjections = state.interjections.clone();

    // ②-4: snapshot the working tree before the autonomous run so the user can
    // review the whole run's diff and revert it with one click — same mechanism
    // as the per-message chat checkpoint, surfaced in the CheckpointsPanel.
    // Best-effort: a non-git cwd or a git hiccup just means no safety net this
    // run, never a blocked start.
    let mut run_checkpoint_id: Option<String> = None;
    {
        use std::path::Path;
        let cwd: Option<String> = sqlx::query_scalar("SELECT cwd FROM sessions WHERE id = ?")
            .bind(&session_id)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten();
        if let Some(cwd) = cwd.filter(|c| !c.is_empty()) {
            let label = match &spec_title {
                Some(t) if !t.is_empty() => format!("自主执行：{t}"),
                _ => "自主执行".to_string(),
            };
            match crate::agent::checkpoint::create(Path::new(&cwd), &label) {
                Ok(Some(sha)) => {
                    let cp_id = Uuid::new_v4().to_string();
                    let now = Utc::now().to_rfc3339();
                    if let Err(e) = sqlx::query(
                        "INSERT INTO checkpoints (id, session_id, message_id, cwd, git_sha, label, created_at, reverted)
                         VALUES (?, ?, NULL, ?, ?, ?, ?, 0)",
                    )
                    .bind(&cp_id)
                    .bind(&session_id)
                    .bind(&cwd)
                    .bind(&sha)
                    .bind(&label)
                    .bind(&now)
                    .execute(&pool)
                    .await
                    {
                        tracing::warn!("autonomous checkpoint INSERT failed: {e}");
                    } else {
                        app.emit("checkpoint-created", &session_id).ok();
                        // The resume journal records which checkpoint's revert
                        // would undo each task completed in this run.
                        run_checkpoint_id = Some(cp_id.clone());
                    }
                }
                Ok(None) => {} // cwd not a git repo — skip
                Err(e) => tracing::warn!("autonomous checkpoint create failed: {e}"),
            }
        }
    }

    let scheduler = Arc::new(TaskScheduler::new(pool.clone()));
    let handle = SchedulerHandle::for_scheduler(&scheduler);

    {
        let mut active = handles.lock().await;
        if state
            .update_restart_reserved
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AppError::Other(
                "应用更新已进入安全重启阶段，请等待自动恢复工作区".into(),
            ));
        }
        active.insert(session_id.clone(), handle);
    }

    let session_id_clone = session_id.clone();
    let handles_clone = handles.inner().clone();
    tokio::spawn(async move {
        let result = scheduler
            .run_session(
                session_id_clone.clone(),
                settings.clone(),
                app.clone(),
                pending_perms,
                interjections,
                run_checkpoint_id,
            )
            .await;
        if let Err(e) = result {
            tracing::error!("scheduler error for session {}: {:#}", session_id_clone, e);
        }
        // Clean up the cancel handle.
        handles_clone.lock().await.remove(&session_id_clone);

        // Auto-collect evidence pack if spec info was provided.
        if let (Some(ref req_id), Some(ref title)) = (&spec_req_id, &spec_title) {
            // Gather all task ids for this session.
            let task_ids: Vec<String> =
                crate::storage::tasks::list_session_tasks(&pool, &session_id_clone)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| t.id)
                    .collect();

            // Get cwd from the session.
            let cwd: String =
                sqlx::query_as::<_, (String,)>("SELECT cwd FROM sessions WHERE id = ?")
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
                auto_create_pr_if_configured(&app, &settings, &session_id_clone, &cwd, title).await;
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
    if let Some(handle) = handles.lock().await.get(&session_id) {
        handle
            .cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub async fn retry_tasks(
    session_id: String,
    task_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<u64, AppError> {
    let pool = state.db.read().await;
    tasks::retry_selected_tasks(&pool, &session_id, &task_ids).await
}

#[tauri::command]
pub async fn retry_failed_tasks(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<u64, AppError> {
    let pool = state.db.read().await;
    tasks::retry_failed_tasks(&pool, &session_id).await
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
    let results = verification::run_verification(&plan, &app, &session_id, &task_id).await;

    // Persist.
    let json = serde_json::to_string(&results).map_err(|e| e.to_string())?;
    tasks::save_verification_results(&pool, &task_id, &json)
        .await
        .map_err(|e| e.to_string())?;

    Ok(results)
}

#[cfg(test)]
mod resource_context_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn active_scheduler_handle_accepts_only_objective_keyed_permit_injection() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let scheduler = TaskScheduler::new(pool);
        let handle = SchedulerHandle::for_scheduler(&scheduler);
        let permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: "objective-active-runner".into(),
            remediation_id: "remediation-active-runner".into(),
            owner: "supervisor-active-runner".into(),
            claim_epoch: 4,
            binding_id: Some("binding-active-runner".into()),
            resource_generation: Some(1),
        };
        handle
            .mutation_permits
            .write()
            .await
            .insert(permit.objective_id.clone(), permit.clone());

        assert_eq!(
            scheduler
                .mutation_permits()
                .read()
                .await
                .get("objective-active-runner"),
            Some(&permit)
        );
        assert!(scheduler
            .mutation_permits()
            .read()
            .await
            .get("another-objective")
            .is_none());
    }

    #[tokio::test]
    async fn missing_frontend_context_serializes_an_enabled_library_snapshot() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        crate::knowledge::ensure_schema(&pool)
            .await
            .expect("knowledge schema");

        for (id, enabled) in [("enabled-kb", 1_i64), ("disabled-kb", 0_i64)] {
            sqlx::query(
                "INSERT INTO knowledge_libraries
                 (id, name, root_path, enabled, created_at, scan_status)
                 VALUES (?, ?, ?, ?, '2026-01-01', 'completed')",
            )
            .bind(id)
            .bind(id)
            .bind(format!("/tmp/{id}"))
            .bind(enabled)
            .execute(&pool)
            .await
            .expect("insert library");
        }

        let json = super::resolve_task_context_json(&pool, None)
            .await
            .expect("resolve task context");
        let context = crate::storage::tasks::TaskConnectorContext::from_json(Some(&json))
            .expect("parse task context");

        assert_eq!(context.knowledge_library_ids(), vec!["enabled-kb"]);
    }

    #[tokio::test]
    async fn missing_frontend_context_serializes_an_explicit_empty_scope() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        crate::knowledge::ensure_schema(&pool)
            .await
            .expect("knowledge schema");

        let json = super::resolve_task_context_json(&pool, None)
            .await
            .expect("resolve empty task context");
        let context = crate::storage::tasks::TaskConnectorContext::from_json(Some(&json))
            .expect("empty context must still be persisted as JSON");

        assert!(context.knowledge_libraries.is_empty());
        assert!(json.contains("knowledge_libraries"));
    }
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
    let remote = match settings
        .git_remotes
        .iter()
        .find(|r| r.default_repo.is_some())
    {
        Some(r) => r.clone(),
        None => return,
    };
    let repo = remote.default_repo.as_deref().unwrap_or("").to_string();
    if repo.is_empty() {
        return;
    }

    // Get current branch.
    let branch_output = std::process::Command::new("git")
        .no_window()
        .current_dir(cwd)
        .args(["branch", "--show-current"])
        .output();

    let current_branch = match branch_output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => return,
    };

    if current_branch.is_empty() {
        return;
    }

    // Get default branch of the repo to use as base.
    let token = match crate::config::settings::resolve_git_remote_token(&remote) {
        Ok(token) => token,
        Err(e) => {
            tracing::warn!("Auto-create PR skipped: {}", e);
            return;
        }
    };
    let client = crate::git_remote::client::RemoteGitClient::new(
        &remote.base_url,
        &token,
        remote.provider.clone(),
    );

    let default_branch = match &remote.provider {
        crate::config::settings::GitProvider::Github => {
            match client.get(&format!("/repos/{}", repo)).await {
                Ok(v) => v
                    .get("default_branch")
                    .and_then(|x| x.as_str())
                    .unwrap_or("main")
                    .to_string(),
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
                &client,
                &repo,
                spec_title,
                &pr_body,
                &current_branch,
                &default_branch,
                true,
            )
            .await
        }
        crate::config::settings::GitProvider::Gitlab => {
            crate::git_remote::gitlab::create_pr(
                &client,
                &repo,
                spec_title,
                &pr_body,
                &current_branch,
                &default_branch,
                true,
            )
            .await
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
