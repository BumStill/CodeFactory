// SPDX-License-Identifier: Apache-2.0
//! Parallel task scheduler — Phase 3 extended with retries + verification.
//!
//! Polls the DB for ready tasks (all dependencies completed) and dispatches
//! them to subagents up to `max_parallel` concurrent runs. Emits Tauri events
//! the dashboard subscribes to.
//!
//! # Retry strategy
//! Each task is attempted up to [`MAX_ATTEMPTS`] times. On every attempt:
//! 1. The subagent brief is (re)built, optionally enriched with the previous
//!    attempt's error / verification failure.
//! 2. The subagent runs to completion.
//! 3. Verification is run against the cwd. If any check fails AND attempts
//!    remain, the task is retried with the verification output appended to
//!    the brief.
//! 4. If the acceptance check in [`SubagentResult`] fails, it is treated as a
//!    soft failure and triggers a retry.
//!
//! # Concurrency model
//! A `Semaphore(max_parallel)` gates how many subagent futures can run at
//! once (`Settings::max_parallel_tasks`, clamped to 1..=8). Each spawned
//! future holds its own permit until it finishes. The scheduler loop is
//! single-threaded — it owns the DB read for "what's ready next?" and the
//! event emission. Cancellation is cooperative via an `AtomicBool` checked
//! at the top of each iteration; in-flight subagents are NOT killed
//! mid-flight (they always finish their current iteration).
//!
//! # Disk isolation
//! With `Settings::subagent_isolation == Worktree`, each task runs in its
//! own git worktree (see [`crate::agent::worktree`]): the brief cwd and
//! verification both point into the worktree, and the diff is applied back
//! onto the user's tree only after verification passes. Merge-backs are
//! serialized per session so concurrent tasks never interleave `git apply`
//! on the same checkout. Non-git cwds fall back to today's shared-cwd mode.

use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

use crate::agent::hooks::{HookEvent, HookRunner};
use crate::agent::journal;
use crate::agent::objective::{
    CompletionArbiter, DecisionRouter, EvidenceKind, ObjectiveEvidence, ObjectiveSnapshot,
    ObjectiveStatus, ObjectiveStore, RecoveryDomain, RouteSignal,
};
use crate::agent::subagent::{self, SubagentBrief, SubagentResult};
use crate::agent::verification;
use crate::agent::worktree;
use crate::config::settings::{Settings, SubagentIsolation};
use crate::errors::AppError;
use crate::storage::tasks;
use crate::PendingPermissionMap;

/// Default max concurrent subagents; the live cap comes from
/// `Settings::max_parallel_tasks` in [`TaskScheduler::run_session`].
pub const MAX_PARALLEL: usize = 3;

/// Max retry attempts per task (including the first attempt).
const MAX_ATTEMPTS: u32 = 3;

/// The tool allow-list every subagent brief advertises. Shared with the
/// journal's dispatch-input hashing so the two can never drift.
pub const SUBAGENT_ALLOWED_TOOLS: &[&str] = &[
    "read_file",
    "glob",
    "grep",
    "kb_search",
    "kb_get_chunk",
    "write_file",
    "edit_file",
    "bash",
    "browser_session",
];

/// The EFFECTIVE dispatch inputs for this session, resolved the same way at
/// dispatch time and at resume so keys compare like-for-like.
fn dispatch_inputs(settings: &Settings) -> journal::DispatchInputs {
    journal::DispatchInputs {
        resolved_model: settings.default_model.clone(),
        resolved_tools: SUBAGENT_ALLOWED_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        isolation: match settings.subagent_isolation {
            SubagentIsolation::Shared => "shared".to_string(),
            SubagentIsolation::Worktree => "worktree".to_string(),
        },
    }
}

/// Poll interval when no new tasks can be dispatched but some are still in flight.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const TASK_RECOVERY_BACKOFF_MS: i64 = 5_000;

#[derive(Debug)]
enum VerificationAttemptDecision {
    Retry { error: String },
    Finish(SubagentResult),
}

fn failed_verification_message(verif_results: &[verification::VerificationResult]) -> String {
    let mut verif_msg = String::from("Previous attempt failed verification:\n");
    for r in verif_results {
        if !r.passed {
            verif_msg.push_str(&format!(
                "- {}: FAILED\n  {}\n",
                r.check,
                r.output.lines().take(20).collect::<Vec<_>>().join("\n  ")
            ));
        }
    }
    verif_msg.push_str("Please fix the above errors.");
    verif_msg
}

fn settle_result_after_verification(
    mut result: SubagentResult,
    verif_results: &[verification::VerificationResult],
    attempt: u32,
    max_attempts: u32,
) -> VerificationAttemptDecision {
    if !verif_results.iter().any(|r| !r.passed) {
        return VerificationAttemptDecision::Finish(result);
    }

    let error = failed_verification_message(verif_results);
    // The local attempt ceiling limits one approach, not the business
    // objective. The caller journals this attempt and hands exhaustion to the
    // durable remediation queue; it must never manufacture terminal `failed`.
    let _ = (&mut result, attempt, max_attempts);
    VerificationAttemptDecision::Retry { error }
}

fn requested_task_acceptance(task: &tasks::TaskRun) -> String {
    task.acceptance_criteria_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .filter(|criteria| !criteria.is_empty())
        .map(|criteria| criteria.join("\n"))
        .unwrap_or_else(|| task.description.clone())
}

async fn objective_for_task(
    pool: &SqlitePool,
    task: &tasks::TaskRun,
) -> Result<ObjectiveSnapshot, AppError> {
    let store = ObjectiveStore::new(pool.clone());
    if let Some(objective_id) = tasks::task_objective_id(pool, &task.id).await? {
        if let Some(objective) = store
            .get(&objective_id)
            .await
            .map_err(|error| AppError::Other(format!("load task objective: {error:#}")))?
        {
            if objective.task_id.as_deref() != Some(task.id.as_str())
                || objective.session_id.as_deref() != Some(task.session_id.as_str())
            {
                return Err(AppError::Other(format!(
                    "task {} objective binding identity mismatch",
                    task.id
                )));
            }
            return Ok(objective);
        }
    }
    store
        .ensure_task_objective(&task.session_id, &task.id, &requested_task_acceptance(task))
        .await
        .map_err(|error| AppError::Other(format!("ensure task objective: {error:#}")))
}

fn task_completion_evidence(
    objective: &ObjectiveSnapshot,
    result: &SubagentResult,
    verification_results: &[verification::VerificationResult],
) -> Vec<ObjectiveEvidence> {
    if verification_results.is_empty() || verification_results.iter().any(|item| !item.passed) {
        return Vec::new();
    }
    let now = Utc::now().timestamp_millis();
    let evidence_ref = format!(
        "task-scheduler:{}:r{}",
        objective.id,
        objective.revision + 1
    );
    let digest = |material: &str| format!("sha256:{:x}", Sha256::digest(material.as_bytes()));
    let mut evidence = Vec::new();
    if result.files_changed.is_empty() {
        evidence.push(ObjectiveEvidence {
            id: Uuid::new_v4().to_string(),
            kind: EvidenceKind::CurrentStateAcceptance,
            scope: objective
                .task_id
                .clone()
                .unwrap_or_else(|| objective.id.clone()),
            digest: digest(&result.summary),
            evidence_ref: evidence_ref.clone(),
            observed_at: now,
            reached_acceptance: objective.requested_acceptance.clone(),
        });
    } else {
        let mut files = result.files_changed.clone();
        files.sort();
        evidence.push(ObjectiveEvidence {
            id: Uuid::new_v4().to_string(),
            kind: EvidenceKind::ChangeSet,
            scope: objective
                .task_id
                .clone()
                .unwrap_or_else(|| objective.id.clone()),
            digest: digest(&files.join("\n")),
            evidence_ref: evidence_ref.clone(),
            observed_at: now,
            reached_acceptance: objective.requested_acceptance.clone(),
        });
    }
    let verification_json = serde_json::to_string(verification_results).unwrap_or_default();
    evidence.push(ObjectiveEvidence {
        id: Uuid::new_v4().to_string(),
        kind: EvidenceKind::PostChangeValidation,
        scope: objective
            .task_id
            .clone()
            .unwrap_or_else(|| objective.id.clone()),
        digest: digest(&verification_json),
        evidence_ref,
        observed_at: now,
        reached_acceptance: objective.requested_acceptance.clone(),
    });
    evidence
}

async fn complete_verified_task_objective(
    pool: &SqlitePool,
    objective: &ObjectiveSnapshot,
    result: &SubagentResult,
    verification_results: &[verification::VerificationResult],
) -> Result<ObjectiveSnapshot, AppError> {
    let evidence = task_completion_evidence(objective, result, verification_results);
    let decision = CompletionArbiter::decide(objective, &evidence).map_err(|error| {
        AppError::Other(format!("task completion evidence rejected: {error:#}"))
    })?;
    ObjectiveStore::new(pool.clone())
        .apply_decision(objective.revision, decision)
        .await
        .map_err(|error| AppError::Other(format!("complete task objective: {error:#}")))
}

async fn handoff_task_to_system_recovery(
    pool: &SqlitePool,
    objective: &ObjectiveSnapshot,
    task_id: &str,
    failure_code: &str,
    error: &str,
) -> Result<ObjectiveSnapshot, AppError> {
    let next_observation_at = Utc::now().timestamp_millis() + TASK_RECOVERY_BACKOFF_MS;
    let failure_signature = format!(
        "sha256:{:x}",
        Sha256::digest(format!("{task_id}:{failure_code}:{error}").as_bytes())
    );
    let decision = DecisionRouter::route(
        objective,
        RouteSignal::TechnicalFailure {
            domain: RecoveryDomain::Task,
            failure_code: failure_code.to_string(),
            failure_signature,
            next_observation_at,
            resume_cursor: Some(task_id.to_string()),
        },
    )
    .map_err(|route_error| {
        AppError::Other(format!("route task recovery decision: {route_error:#}"))
    })?;
    let waiting = ObjectiveStore::new(pool.clone())
        .apply_decision(objective.revision, decision)
        .await
        .map_err(|apply_error| {
            AppError::Other(format!("persist task recovery decision: {apply_error:#}"))
        })?;
    let projected =
        tasks::mark_task_waiting_system(pool, task_id, &waiting.id, error, next_observation_at)
            .await?;
    if !projected {
        return Err(AppError::Other(format!(
            "task {task_id} changed before waiting_system projection"
        )));
    }
    Ok(waiting)
}

/// RAII net under a dispatched task future: on drop without `settled`, the
/// task row resets to `pending` (journal marked stale) and the in-memory
/// running slot is freed — turning an intra-process panic from a permanent
/// wedge into a normal re-dispatch on the next scheduler tick.
struct DispatchGuard {
    pool: SqlitePool,
    running: Arc<Mutex<HashSet<String>>>,
    task_id: String,
    settled: bool,
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        let id = self.task_id.clone();
        let running = self.running.clone();
        let pool = self.pool.clone();
        let settled = self.settled;
        // Drop is sync; hand the async cleanup to the runtime (present on
        // every scheduler path — this future runs inside tokio::spawn).
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if !settled {
                    let has_journal = journal::journal_get(&pool, &id)
                        .await
                        .ok()
                        .flatten()
                        .is_some();
                    if tasks::reset_running_task_to_pending(
                        &pool,
                        &id,
                        "resume: re-running after interrupted dispatch",
                    )
                    .await
                    .unwrap_or(false)
                        && has_journal
                    {
                        let _ = journal::journal_mark_stale(&pool, &id).await;
                    }
                }
                running.lock().await.remove(&id);
            });
        }
    }
}

#[derive(Clone)]
pub struct TaskScheduler {
    pub pool: SqlitePool,
    pub max_parallel: usize,
    pub running: Arc<Mutex<HashSet<String>>>,
    pub cancel_flag: Arc<AtomicBool>,
}

impl TaskScheduler {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            max_parallel: MAX_PARALLEL,
            running: Arc::new(Mutex::new(HashSet::new())),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    // Scaffolding: convenience in-process canceller. The live path hands out
    // `cancel_handle()` and trips the flag itself, so this symmetric helper is
    // unused for now — kept alongside `cancel_handle`.
    #[allow(dead_code)]
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }

    /// Drives the scheduler loop until every task is in a terminal state
    /// (`completed`, `failed`, or `cancelled`) or cancellation is requested.
    pub async fn run_session(
        self: Arc<Self>,
        session_id: String,
        settings: Settings,
        app_handle: AppHandle,
        pending_perms: PendingPermissionMap,
        interjections: crate::commands::interjections::InterjectionQueue,
        run_checkpoint_id: Option<String>,
    ) -> Result<(), AppError> {
        let max_parallel = usize::from(settings.max_parallel_tasks.clamp(1, 8));
        let semaphore = Arc::new(Semaphore::new(max_parallel));

        // ── Resume (content-addressed journal) ─────────────────────────────
        // Phase A: heal crash-orphaned 'running' rows for THIS session (boot
        // recovery in ensure_schema already handled the session-agnostic
        // sweep; this catches a peer-process crash without a restart).
        // Phase B: revalidate completed tasks against their journal — restore
        // matches, reset input-changed/output-lost tasks to pending so the
        // unchanged pending/ready logic below re-runs exactly what's needed.
        // Best-effort by design: a resume hiccup degrades to today's coarse
        // "skip completed" behavior, never blocks the run.
        {
            let inputs = dispatch_inputs(&settings);
            let mut report = journal::ResumeReport::default();
            match journal::recover_orphaned_tasks(
                &self.pool,
                journal::OrphanScope::Session(session_id.clone()),
            )
            .await
            {
                Ok(recovered) => report.recovered = recovered,
                Err(e) => tracing::warn!("scheduler: orphan recovery failed (non-fatal): {e}"),
            }
            match journal::plan_resume(&self.pool, &session_id, &inputs).await {
                Ok(mut planned) => {
                    planned.recovered = std::mem::take(&mut report.recovered);
                    report = planned;
                }
                Err(e) => tracing::warn!("scheduler: plan_resume failed (non-fatal): {e}"),
            }
            if !report.restored.is_empty()
                || !report.invalidated.is_empty()
                || !report.recovered.is_empty()
            {
                let event = format!("resume_summary:{}", session_id);
                if let Err(e) = app_handle.emit(&event, &report) {
                    tracing::warn!("failed to emit {}: {}", event, e);
                }
                for restored in &report.restored {
                    let event = format!("task_restored:{}", session_id);
                    let _ = app_handle.emit(&event, restored);
                }
            }
        }

        // Worktree isolation: resolve the container dir once. Worktrees live
        // under the app data dir — never inside the user's project — so
        // nothing ever shows up in their `git status`.
        let worktree_container: Option<PathBuf> =
            if settings.subagent_isolation == SubagentIsolation::Worktree {
                Some(
                    app_handle
                        .path()
                        .app_data_dir()
                        .unwrap_or_else(|_| std::env::temp_dir().join("codefactory"))
                        .join("task-worktrees"),
                )
            } else {
                None
            };
        // Serializes merge-backs within this session: concurrent tasks must
        // not interleave `git apply` on the same user checkout.
        let merge_lock = Arc::new(Mutex::new(()));
        // Durable home for worktree merge-back patches: worktree cleanup reaps
        // the original after a successful merge, but the journal's presence
        // gate needs the patch at every later resume.
        let journal_patch_dir: PathBuf = app_handle
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("codefactory"))
            .join("task-journal-patches");

        // Write shared brief listing all tasks for parallel subagents to read
        {
            let all_tasks = tasks::list_all_tasks_for_session(&self.pool, &session_id)
                .await
                .unwrap_or_default();
            if !all_tasks.is_empty() {
                // Determine a common cwd (use first task's cwd)
                let cwd = all_tasks[0].cwd.clone();
                let brief_path = format!("{}/_codefactory_brief.md", cwd);
                let mut brief_content = format!(
                    "# CodeFactory Shared Brief\n\
                     Session: {}\n\n\
                     ## Parallel Tasks\n\n",
                    session_id
                );
                for (i, t) in all_tasks.iter().enumerate() {
                    brief_content.push_str(&format!(
                        "### Task {} — {}\n{}\n\n",
                        i + 1,
                        t.title,
                        t.description
                    ));
                }
                if let Some(context) = tasks::TaskConnectorContext::from_json(
                    all_tasks[0].task_context_json.as_deref(),
                ) {
                    let rendered = context.render_markdown();
                    if !rendered.is_empty() {
                        brief_content.push_str(&rendered);
                        brief_content.push('\n');
                    }
                }
                brief_content
                    .push_str("## Task Results\n\n_(will be updated as tasks complete)_\n");
                let _ = std::fs::write(&brief_path, &brief_content);
            }
        }

        loop {
            // 1. Honour cancellation (after letting in-flight tasks drain).
            if self.cancel_flag.load(Ordering::SeqCst) {
                self.mark_remaining_cancelled(&session_id, &app_handle)
                    .await
                    .ok();
                self.wait_for_running_to_drain().await;
                tracing::info!(
                    "scheduler: session {} cancelled, all in-flight tasks settled",
                    session_id
                );
                break;
            }

            // 2. Find pending tasks whose dependencies are all completed.
            let pending = tasks::list_pending_tasks_for_session(&self.pool, &session_id).await?;
            let mut ready: Vec<_> = Vec::new();
            for t in pending {
                if tasks::is_task_ready(&self.pool, &t.id).await? {
                    ready.push(t);
                }
            }

            // 3. Dispatch as many as the parallelism budget allows.
            let mut dispatched_any = false;
            for task in ready {
                if self.running.lock().await.contains(&task.id) {
                    continue;
                }
                let task_objective = objective_for_task(&self.pool, &task).await?;
                match task_objective.status {
                    ObjectiveStatus::Completed => {
                        tasks::mark_task_completed(
                            &self.pool,
                            &task.id,
                            r#"{"summary":"reconciled from completed objective evidence"}"#,
                        )
                        .await?;
                        continue;
                    }
                    ObjectiveStatus::Cancelled => {
                        tasks::mark_task_cancelled(&self.pool, &task.id).await?;
                        continue;
                    }
                    ObjectiveStatus::WaitingSystem
                        if task_objective
                            .next_observation_at
                            .is_some_and(|due| due > Utc::now().timestamp_millis()) =>
                    {
                        let due = task_objective.next_observation_at.unwrap_or_default();
                        let reason = task_objective
                            .failure_code
                            .as_deref()
                            .unwrap_or("waiting_system");
                        tasks::mark_task_waiting_system(
                            &self.pool,
                            &task.id,
                            &task_objective.id,
                            reason,
                            due,
                        )
                        .await?;
                        continue;
                    }
                    _ => {}
                }
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => break, // cap reached, try again next tick
                };

                // CAS claim: pending → running with THIS process's identity.
                // Another scheduler (possibly another process sharing the DB)
                // may have claimed it between the ready query and here — skip
                // instead of double-dispatching.
                match journal::claim_task(&self.pool, &task.id).await {
                    Ok(true) => {}
                    Ok(false) => {
                        drop(permit);
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!("scheduler: claim_task({}) failed: {e}", task.id);
                        drop(permit);
                        continue;
                    }
                }
                if !tasks::clear_task_recovery_after_claim(&self.pool, &task.id, &task_objective.id)
                    .await?
                {
                    tasks::reset_running_task_to_pending(
                        &self.pool,
                        &task.id,
                        "resume: task objective claim reconciliation",
                    )
                    .await?;
                    drop(permit);
                    continue;
                }
                self.running.lock().await.insert(task.id.clone());
                emit_task(
                    &app_handle,
                    &session_id,
                    "task_started",
                    &TaskEventPayload {
                        task_id: &task.id,
                        title: Some(&task.title),
                        message: None,
                        result: None,
                        error: None,
                        files_changed: None,
                        cwd: None,
                    },
                );
                dispatched_any = true;

                // Clone everything needed inside the spawned future.
                let pool = self.pool.clone();
                let settings = settings.clone();
                let app = app_handle.clone();
                let perms = pending_perms.clone();
                let interjections_clone = interjections.clone();
                let session_id_for_task = session_id.clone();
                let running = self.running.clone();
                let worktree_container = worktree_container.clone();
                let merge_lock = merge_lock.clone();
                let journal_patch_dir = journal_patch_dir.clone();
                let run_checkpoint_id = run_checkpoint_id.clone();
                let task_row = task.clone();
                let task_id = task.id.clone();
                let task_cwd = task.cwd.clone();
                let task_title = task.title.clone();
                let task_description = task.description.clone();
                let task_objective = task_objective.clone();
                // Parse acceptance_criteria_json (stored as JSON array) into a
                // human-readable bullet block for the subagent brief. The
                // autonomous prompt instructs the model to verify each one
                // before reporting done.
                let task_acceptance: Option<String> = task
                    .acceptance_criteria_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                    .filter(|v| !v.is_empty())
                    .map(|v| {
                        v.iter()
                            .map(|c| format!("- {}", c))
                            .collect::<Vec<_>>()
                            .join("\n")
                    });

                tokio::spawn(async move {
                    let _permit = permit; // released on drop

                    // RAII guard: a panic anywhere in this future would
                    // otherwise leak the in-memory running slot AND leave the
                    // DB row 'running' under a live owner PID that orphan
                    // recovery rightly refuses to reset. On unwind the guard
                    // resets the row to pending so the next scheduler tick
                    // simply re-dispatches it.
                    let mut dispatch_guard = DispatchGuard {
                        pool: pool.clone(),
                        running: running.clone(),
                        task_id: task_id.clone(),
                        settled: false,
                    };

                    // ── Pre-task hook ────────────────────────────────────────
                    let hook_runner = HookRunner::from_settings(&settings, app.clone());
                    hook_runner
                        .fire(HookEvent::PreTask {
                            task_id: task_id.clone(),
                            title: task_title.clone(),
                        })
                        .await;

                    // ── Worktree isolation (optional) ───────────────────────
                    // One worktree per task, reused across attempts so partial
                    // progress carries over exactly like shared mode. Non-git
                    // cwds (or repos without a commit) fall back to shared.
                    let mut task_worktree: Option<worktree::TaskWorktree> = None;
                    let mut effective_cwd = task_cwd.clone();
                    if let Some(ref container) = worktree_container {
                        match worktree::create(std::path::Path::new(&task_cwd), &task_id, container)
                        {
                            Ok(wt) => {
                                effective_cwd = wt.effective_cwd.display().to_string();
                                emit_task(
                                    &app,
                                    &session_id_for_task,
                                    "task_progress",
                                    &TaskEventPayload {
                                        task_id: &task_id,
                                        title: None,
                                        message: Some(&format!(
                                            "Running isolated on branch {}",
                                            wt.branch
                                        )),
                                        result: None,
                                        error: None,
                                        files_changed: None,
                                        cwd: None,
                                    },
                                );
                                task_worktree = Some(wt);
                            }
                            Err(e) => {
                                tracing::info!(
                                    "scheduler: task {} worktree isolation unavailable ({e}); \
                                     falling back to shared cwd",
                                    task_id
                                );
                                emit_task(
                                    &app,
                                    &session_id_for_task,
                                    "task_progress",
                                    &TaskEventPayload {
                                        task_id: &task_id,
                                        title: None,
                                        message: Some(&format!(
                                            "Worktree isolation unavailable ({e}); \
                                             running in shared directory"
                                        )),
                                        result: None,
                                        error: None,
                                        files_changed: None,
                                        cwd: None,
                                    },
                                );
                            }
                        }
                    }

                    // ── Retry loop ──────────────────────────────────────────
                    let mut prev_error: Option<String> = None;
                    let mut final_outcome: Option<SubagentResult> = None;
                    let mut final_verification_results = Vec::new();
                    let mut recovery_failure_code = "task_attempts_exhausted";

                    for attempt in 1..=MAX_ATTEMPTS {
                        // Emit retry event on attempts 2+.
                        if attempt > 1 {
                            tasks::increment_attempt(&pool, &task_id).await.ok();
                            emit_retry(&app, &session_id_for_task, &task_id, attempt);
                        }

                        // Build the brief, enriched on retries.
                        let description = if let Some(ref prev) = prev_error {
                            format!(
                                "{}\n\nPrevious attempt failed. Error: {}. \
                                 Please try a different approach.",
                                task_description, prev
                            )
                        } else {
                            task_description.clone()
                        };

                        // Drain pending user interjections — they redirect
                        // execution before this task starts. Picked up as
                        // parent_summary so the subagent sees them as
                        // session context, not as a direct task override.
                        let pending_interjections =
                            crate::commands::interjections::drain_for_session(
                                &interjections_clone,
                                &session_id_for_task,
                            )
                            .await;
                        let parent_summary = if pending_interjections.is_empty() {
                            None
                        } else {
                            let notes: Vec<String> = pending_interjections
                                .iter()
                                .map(|i| format!("- {}", i.message))
                                .collect();
                            Some(format!(
                                "User redirections received before this task started:\n{}",
                                notes.join("\n")
                            ))
                        };

                        let brief = SubagentBrief {
                            task_id: task_id.clone(),
                            title: task_title.clone(),
                            description,
                            cwd: effective_cwd.clone(),
                            parent_summary,
                            allowed_tools: SUBAGENT_ALLOWED_TOOLS
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
                            acceptance_criteria: task_acceptance.clone(),
                            connector_context: tasks::TaskConnectorContext::from_json(
                                task.task_context_json.as_deref(),
                            ),
                        };

                        // Emit progress so the dashboard knows we're retrying.
                        if attempt > 1 {
                            emit_task(
                                &app,
                                &session_id_for_task,
                                "task_progress",
                                &TaskEventPayload {
                                    task_id: &task_id,
                                    title: None,
                                    message: Some(&format!("Attempt {attempt}/{MAX_ATTEMPTS}…")),
                                    result: None,
                                    error: None,
                                    files_changed: None,
                                    cwd: None,
                                },
                            );
                        }

                        let attempt_id = Uuid::new_v4().to_string();
                        let attempt_index = task_row.attempt_count + attempt as i32;
                        if let Err(error) = tasks::start_task_objective_attempt(
                            &pool,
                            &attempt_id,
                            &task_id,
                            &task_objective.id,
                            attempt_index,
                        )
                        .await
                        {
                            let message = format!("attempt journal start failed: {error}");
                            tracing::error!("scheduler: task {} {message}", task_id);
                            prev_error = Some(message);
                            recovery_failure_code = "attempt_journal_error";
                            continue;
                        }

                        let subagent_result = subagent::run_subagent(
                            brief,
                            &attempt_id,
                            &pool,
                            &session_id_for_task,
                            &settings,
                            &app,
                            &perms,
                        )
                        .await;

                        match subagent_result {
                            Err(e) => {
                                let msg = e.to_string();
                                tasks::finish_task_attempt(
                                    &pool,
                                    &attempt_id,
                                    "failed",
                                    Some("subagent_error"),
                                    Some(&msg),
                                    None,
                                    None,
                                )
                                .await
                                .ok();
                                tracing::warn!(
                                    "scheduler: task {} attempt {attempt} error: {msg}",
                                    task_id
                                );
                                prev_error = Some(msg.clone());
                                recovery_failure_code = "subagent_error";
                                emit_task(
                                    &app,
                                    &session_id_for_task,
                                    "task_progress",
                                    &TaskEventPayload {
                                        task_id: &task_id,
                                        title: None,
                                        message: Some(&format!(
                                            "Attempt {attempt} failed: {msg}, retrying…"
                                        )),
                                        result: None,
                                        error: None,
                                        files_changed: None,
                                        cwd: None,
                                    },
                                );
                                // Continue to next attempt.
                            }
                            Ok(result) => {
                                // Persist sub-session link.
                                tasks::set_sub_session(&pool, &task_id, &result.sub_session_id)
                                    .await
                                    .ok();

                                // ── Acceptance check ────────────────────────
                                if let Some(ref ac) = result.acceptance_check {
                                    if !ac.passed {
                                        let msg = format!("Acceptance check failed: {}", ac.reason);
                                        tasks::finish_task_attempt(
                                            &pool,
                                            &attempt_id,
                                            "failed",
                                            Some("acceptance_failed"),
                                            Some(&msg),
                                            Some(&result.summary),
                                            None,
                                        )
                                        .await
                                        .ok();
                                        tracing::warn!(
                                            "scheduler: task {} attempt {attempt} acceptance failed: {}",
                                            task_id,
                                            ac.reason
                                        );
                                        prev_error = Some(msg.clone());
                                        recovery_failure_code = "acceptance_failed";
                                        emit_task(
                                            &app,
                                            &session_id_for_task,
                                            "task_progress",
                                            &TaskEventPayload {
                                                task_id: &task_id,
                                                title: None,
                                                message: Some(&format!(
                                                    "Attempt {attempt} acceptance check failed, retrying…"
                                                )),
                                                result: None,
                                                error: None,
                                                files_changed: None,
                                                cwd: None,
                                            },
                                        );
                                        if attempt < MAX_ATTEMPTS {
                                            continue;
                                        }
                                        // All attempts exhausted.
                                        final_outcome = Some(SubagentResult {
                                            completed: false,
                                            summary: format!(
                                                "Failed after {MAX_ATTEMPTS} attempts (acceptance)"
                                            ),
                                            ..result
                                        });
                                        break;
                                    }
                                }

                                // ── Verification ─────────────────────────────
                                // Runs against the effective cwd: in worktree
                                // mode that's the isolated checkout, so only
                                // verified work ever merges back.
                                let verif_plan =
                                    verification::detect_verification_plan(&effective_cwd);
                                let verif_results = verification::run_verification(
                                    &verif_plan,
                                    &app,
                                    &session_id_for_task,
                                    &task_id,
                                )
                                .await;
                                let verif_json =
                                    serde_json::to_string(&verif_results).unwrap_or_default();

                                // Persist verification results.
                                if let Ok(json) = serde_json::to_string(&verif_results) {
                                    tasks::save_verification_results(&pool, &task_id, &json)
                                        .await
                                        .ok();
                                }

                                match settle_result_after_verification(
                                    result,
                                    &verif_results,
                                    attempt,
                                    MAX_ATTEMPTS,
                                ) {
                                    VerificationAttemptDecision::Retry { error } => {
                                        tasks::finish_task_attempt(
                                            &pool,
                                            &attempt_id,
                                            "failed",
                                            Some("verification_failed"),
                                            Some(&error),
                                            None,
                                            Some(&verif_json),
                                        )
                                        .await
                                        .ok();
                                        tracing::warn!(
                                            "scheduler: task {} attempt {attempt} failed verification",
                                            task_id
                                        );
                                        prev_error = Some(error);
                                        recovery_failure_code = "verification_failed";
                                        emit_task(
                                            &app,
                                            &session_id_for_task,
                                            "task_progress",
                                            &TaskEventPayload {
                                                task_id: &task_id,
                                                title: None,
                                                message: Some(&format!(
                                                    "Attempt {attempt} incomplete, retrying…"
                                                )),
                                                result: None,
                                                error: None,
                                                files_changed: None,
                                                cwd: None,
                                            },
                                        );
                                        continue;
                                    }
                                    VerificationAttemptDecision::Finish(outcome) => {
                                        let status = if outcome.completed {
                                            "completed"
                                        } else {
                                            "failed"
                                        };
                                        tasks::finish_task_attempt(
                                            &pool,
                                            &attempt_id,
                                            status,
                                            (!outcome.completed).then_some("verification_failed"),
                                            (!outcome.completed)
                                                .then_some(outcome.summary.as_str()),
                                            Some(&outcome.summary),
                                            Some(&verif_json),
                                        )
                                        .await
                                        .ok();
                                        final_verification_results = verif_results;
                                        final_outcome = Some(outcome);
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    // ── Merge back isolated work (verify-then-merge) ────────
                    // Only verified, completed outcomes merge; a conflict is
                    // all-or-nothing (user tree untouched) and downgrades the
                    // task to failed with the branch + patch preserved for
                    // manual recovery. Failed tasks keep their worktree.
                    // Materialization evidence for the resume journal: how this
                    // task's output landed on disk. Shared mode edits in place.
                    let mut journal_material = journal::Materialization {
                        kind: "shared_inplace",
                        merge_applied: true,
                        patch_path: None,
                        repo_root: None,
                        base_sha: None,
                    };
                    let journal_inputs = dispatch_inputs(&settings);
                    let journal_dep_keys = journal::dep_keys_for(&pool, &task_id)
                        .await
                        .unwrap_or_default();

                    if let Some(ref wt) = task_worktree {
                        let completed_ok = final_outcome.as_ref().map_or(false, |r| r.completed);
                        if completed_ok {
                            let _merge_guard = merge_lock.lock().await;
                            // Durable 'merging' intent BEFORE the side effect:
                            // a crash between "diff applied" and "row completed"
                            // is then resolved exactly-once by orphan recovery
                            // (reverse-apply-check decides finalize vs reset).
                            let intent_result_json = final_outcome
                                .as_ref()
                                .and_then(|r| serde_json::to_string(r).ok())
                                .unwrap_or_else(|| "{}".into());
                            let intent = journal::Materialization {
                                kind: "applied",
                                merge_applied: false,
                                patch_path: Some(wt.patch_path.display().to_string()),
                                repo_root: Some(wt.repo_root.display().to_string()),
                                base_sha: Some(wt.base_sha.clone()),
                            };
                            if let Err(e) = journal::record_merging_intent(
                                &pool,
                                &task_row,
                                &journal_inputs,
                                &journal_dep_keys,
                                run_checkpoint_id.as_deref(),
                                &intent,
                                &intent_result_json,
                            )
                            .await
                            {
                                tracing::warn!(
                                    "scheduler: task {} merging intent write failed: {e}",
                                    task_id
                                );
                            }
                            match worktree::merge_back(wt) {
                                Ok(worktree::MergeOutcome::Applied) => {
                                    if let Some(ref mut r) = final_outcome {
                                        r.files_changed = worktree::remap_paths(
                                            std::mem::take(&mut r.files_changed),
                                            wt,
                                        );
                                    }
                                    // Copy the merge-back patch to its durable
                                    // home BEFORE cleanup reaps the original —
                                    // it is the presence-gate evidence at every
                                    // later resume.
                                    let durable =
                                        journal_patch_dir.join(format!("task-{}.patch", task_id));
                                    let _ = std::fs::create_dir_all(&journal_patch_dir);
                                    let copied = std::fs::copy(&wt.patch_path, &durable).is_ok();
                                    journal_material = journal::Materialization {
                                        kind: "applied",
                                        merge_applied: true,
                                        patch_path: copied.then(|| durable.display().to_string()),
                                        repo_root: Some(wt.repo_root.display().to_string()),
                                        base_sha: Some(wt.base_sha.clone()),
                                    };
                                    worktree::cleanup(wt, true);
                                }
                                Ok(worktree::MergeOutcome::NoChanges) => {
                                    if let Some(ref mut r) = final_outcome {
                                        r.files_changed = worktree::remap_paths(
                                            std::mem::take(&mut r.files_changed),
                                            wt,
                                        );
                                    }
                                    journal_material = journal::Materialization {
                                        kind: "no_changes",
                                        merge_applied: true,
                                        patch_path: None,
                                        repo_root: Some(wt.repo_root.display().to_string()),
                                        base_sha: Some(wt.base_sha.clone()),
                                    };
                                    worktree::cleanup(wt, true);
                                }
                                Ok(worktree::MergeOutcome::Conflict { message }) => {
                                    if let Err(e) =
                                        journal::journal_mark_stale(&pool, &task_id).await
                                    {
                                        tracing::warn!(
                                            "scheduler: task {} journal_mark_stale failed after \
                                             merge conflict ({e})",
                                            task_id
                                        );
                                    }
                                    let note = format!(
                                        "Merge-back conflict: {message}. Work preserved on \
                                         branch '{}' with patch at {}.",
                                        wt.branch,
                                        wt.patch_path.display()
                                    );
                                    tracing::warn!("scheduler: task {} {}", task_id, note);
                                    prev_error = Some(note.clone());
                                    recovery_failure_code = "merge_back_conflict";
                                    if let Some(ref mut r) = final_outcome {
                                        r.completed = false;
                                        r.summary =
                                            format!("{note}\n\nOriginal summary:\n{}", r.summary);
                                    }
                                }
                                Err(e) => {
                                    if let Err(mark_stale_err) =
                                        journal::journal_mark_stale(&pool, &task_id).await
                                    {
                                        tracing::warn!(
                                            "scheduler: task {} journal_mark_stale failed after \
                                             merge-back error ({mark_stale_err})",
                                            task_id
                                        );
                                    }
                                    let note = format!(
                                        "Merge-back failed: {e}. Work preserved on branch '{}' \
                                         at {}.",
                                        wt.branch,
                                        wt.worktree_root.display()
                                    );
                                    tracing::warn!("scheduler: task {} {}", task_id, note);
                                    prev_error = Some(note.clone());
                                    recovery_failure_code = "merge_back_failed";
                                    if let Some(ref mut r) = final_outcome {
                                        r.completed = false;
                                        r.summary =
                                            format!("{note}\n\nOriginal summary:\n{}", r.summary);
                                    }
                                }
                            }
                        }
                    }

                    // ── Settle the Objective, then its task projection ──────
                    let mut projection_settled = false;
                    let mut objective_completed = false;
                    let mut post_status = "waiting_system";
                    let mut post_summary = final_outcome
                        .as_ref()
                        .map(|result| result.summary.clone())
                        .unwrap_or_else(|| prev_error.clone().unwrap_or_default());

                    if let Some(result) = final_outcome.as_ref().filter(|result| result.completed) {
                        match complete_verified_task_objective(
                            &pool,
                            &task_objective,
                            result,
                            &final_verification_results,
                        )
                        .await
                        {
                            Ok(completed) if completed.status == ObjectiveStatus::Completed => {
                                objective_completed = true;
                                let result_json =
                                    serde_json::to_string(result).unwrap_or_else(|_| "{}".into());
                                let task_completed = match journal::record_completion(
                                    &pool,
                                    &task_row,
                                    &journal_inputs,
                                    &journal_dep_keys,
                                    run_checkpoint_id.as_deref(),
                                    &journal_material,
                                    &result_json,
                                )
                                .await
                                {
                                    Ok(()) => true,
                                    Err(error) => {
                                        tracing::warn!(
                                            "scheduler: task {} journal completion failed ({error}); \
                                             reconciling the projection from completed Objective",
                                            task_id
                                        );
                                        tasks::mark_task_completed(&pool, &task_id, &result_json)
                                            .await
                                            .is_ok()
                                    }
                                };
                                if task_completed {
                                    projection_settled = true;
                                    post_status = "completed";
                                    crate::notify::send(
                                        &settings,
                                        crate::notify::NotifyEvent::TaskCompleted,
                                        format!(
                                            "{task_title}\n{}",
                                            result.summary.chars().take(200).collect::<String>()
                                        ),
                                    );
                                    let brief_path = format!("{}/_codefactory_brief.md", task_cwd);
                                    if std::path::Path::new(&brief_path).exists() {
                                        let result_entry = format!(
                                            "\n### \u{2705} {} \u{2014} done\n{}\n",
                                            task_title,
                                            result.summary.chars().take(500).collect::<String>()
                                        );
                                        if let Ok(mut existing) =
                                            std::fs::read_to_string(&brief_path)
                                        {
                                            existing = existing.replace(
                                                "_(will be updated as tasks complete)_",
                                                "",
                                            );
                                            existing.push_str(&result_entry);
                                            let _ = std::fs::write(&brief_path, &existing);
                                        }
                                    }
                                    emit_task(
                                        &app,
                                        &session_id_for_task,
                                        "task_completed",
                                        &TaskEventPayload {
                                            task_id: &task_id,
                                            title: None,
                                            message: None,
                                            result: Some(&result_json),
                                            error: None,
                                            files_changed: if result.files_changed.is_empty() {
                                                None
                                            } else {
                                                Some(result.files_changed.as_slice())
                                            },
                                            cwd: Some(&task_cwd),
                                        },
                                    );
                                }
                            }
                            Ok(_) => {
                                prev_error =
                                    Some("completion arbiter returned nonterminal state".into());
                                recovery_failure_code = "completion_evidence_incomplete";
                            }
                            Err(error) => {
                                prev_error = Some(error.to_string());
                                recovery_failure_code = "completion_evidence_incomplete";
                            }
                        }
                    }

                    if !projection_settled && !objective_completed {
                        let error = prev_error.clone().unwrap_or_else(|| {
                            final_outcome
                                .as_ref()
                                .map(|result| result.summary.clone())
                                .unwrap_or_else(|| {
                                    format!("attempt budget {MAX_ATTEMPTS} exhausted")
                                })
                        });
                        post_summary = error.clone();
                        match handoff_task_to_system_recovery(
                            &pool,
                            &task_objective,
                            &task_id,
                            recovery_failure_code,
                            &error,
                        )
                        .await
                        {
                            Ok(waiting) => {
                                projection_settled = true;
                                emit_task(
                                    &app,
                                    &session_id_for_task,
                                    "task_progress",
                                    &TaskEventPayload {
                                        task_id: &task_id,
                                        title: None,
                                        message: Some("技术恢复已排队，系统将自动继续"),
                                        result: None,
                                        error: None,
                                        files_changed: None,
                                        cwd: None,
                                    },
                                );
                                tracing::info!(
                                    task_id = %task_id,
                                    objective_id = %waiting.id,
                                    failure_code = recovery_failure_code,
                                    "task attempt exhausted into durable system recovery"
                                );
                            }
                            Err(handoff_error) => {
                                tracing::error!(
                                    task_id = %task_id,
                                    objective_id = %task_objective.id,
                                    %handoff_error,
                                    "task recovery handoff failed; dispatch guard will restore pending"
                                );
                            }
                        }
                    }
                    if objective_completed && !projection_settled {
                        post_status = "reconciling";
                        post_summary =
                            "Objective completed; task projection reconciliation pending"
                                .to_string();
                    }

                    // ── Post-task hook ───────────────────────────────────────
                    hook_runner
                        .fire(HookEvent::PostTask {
                            task_id: task_id.clone(),
                            status: post_status.to_string(),
                            summary: post_summary,
                        })
                        .await;
                    let reclaimed = crate::tools::browser_session::close_for_task(&task_id).await;
                    if reclaimed > 0 {
                        tracing::info!(
                            "scheduler: reclaimed {reclaimed} browser session(s) for task {task_id}"
                        );
                    }

                    dispatch_guard.settled = projection_settled;
                    drop(dispatch_guard); // frees the running slot
                });
            }

            // 4. If nothing is in flight and no new tasks were dispatched and
            //    no pending tasks remain, we're done.
            let remaining_pending =
                tasks::count_all_pending_tasks_for_session(&self.pool, &session_id).await?;
            let in_flight = self.running.lock().await.len();
            if !dispatched_any && in_flight == 0 && remaining_pending == 0 {
                tracing::info!("scheduler: session {} drained", session_id);
                break;
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }

        Ok(())
    }

    async fn mark_remaining_cancelled(
        &self,
        session_id: &str,
        app_handle: &AppHandle,
    ) -> Result<(), AppError> {
        let pending = tasks::list_all_pending_tasks_for_session(&self.pool, session_id).await?;
        for t in pending {
            match objective_for_task(&self.pool, &t).await {
                Ok(objective) if !objective.status.is_terminal() => {
                    match DecisionRouter::route(
                        &objective,
                        RouteSignal::Cancelled {
                            domain: RecoveryDomain::Task,
                            provenance: "explicit_cancel".into(),
                        },
                    ) {
                        Ok(decision) => {
                            if let Err(error) = ObjectiveStore::new(self.pool.clone())
                                .apply_decision(objective.revision, decision)
                                .await
                            {
                                tracing::warn!(
                                    task_id = %t.id,
                                    objective_id = %objective.id,
                                    %error,
                                    "failed to project explicit task cancellation to Objective"
                                );
                            }
                        }
                        Err(error) => {
                            tracing::warn!(task_id = %t.id, %error, "failed to route task cancellation")
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(task_id = %t.id, %error, "failed to load task Objective during cancellation")
                }
            }
            tasks::mark_task_cancelled(&self.pool, &t.id).await.ok();
            emit_task(
                app_handle,
                session_id,
                "task_failed",
                &TaskEventPayload {
                    task_id: &t.id,
                    title: None,
                    message: None,
                    result: None,
                    error: Some("Cancelled by user before starting"),
                    files_changed: None,
                    cwd: None,
                },
            );
        }
        Ok(())
    }

    async fn wait_for_running_to_drain(&self) {
        loop {
            if self.running.lock().await.is_empty() {
                return;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

// ── Event types ───────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
struct TaskEventPayload<'a> {
    task_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    /// Paths the sub-agent touched during this task — emitted on
    /// task_completed so the UI can surface "AI changed N files" with a
    /// drill-down to git diff. Empty/absent for other event kinds.
    #[serde(skip_serializing_if = "Option::is_none")]
    files_changed: Option<&'a [String]>,
    /// Working directory of the task — surfaced alongside files_changed
    /// so the UI knows where to run git diff against.
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<&'a str>,
}

#[derive(Serialize, Clone)]
struct RetryEventPayload<'a> {
    task_id: &'a str,
    attempt: u32,
}

fn emit_task(app: &AppHandle, session_id: &str, kind: &str, payload: &TaskEventPayload<'_>) {
    let event = format!("{}:{}", kind, session_id);
    if let Err(e) = app.emit(&event, payload) {
        tracing::warn!("failed to emit {} event: {}", event, e);
    }
}

fn emit_retry(app: &AppHandle, session_id: &str, task_id: &str, attempt: u32) {
    let event = format!("task_retry:{}", session_id);
    if let Err(e) = app.emit(&event, RetryEventPayload { task_id, attempt }) {
        tracing::warn!("failed to emit retry event: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::objective::ObjectiveKind;
    use crate::agent::subagent::SubagentResult;
    use crate::agent::verification::VerificationResult;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn task_objective_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, title TEXT NOT NULL,
                description TEXT NOT NULL, status TEXT NOT NULL, cwd TEXT NOT NULL,
                parent_task_id TEXT, sub_session_id TEXT, created_at TEXT NOT NULL,
                started_at TEXT, completed_at TEXT, result TEXT, error TEXT,
                attempt_count INTEGER NOT NULL DEFAULT 0, verification_results TEXT,
                task_context_json TEXT, acceptance_criteria_json TEXT,
                spec_req_id TEXT, spec_title TEXT, owner_pid INTEGER,
                owner_start_token TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE task_attempts (
                id TEXT PRIMARY KEY, task_id TEXT NOT NULL, attempt_index INTEGER NOT NULL,
                sub_session_id TEXT, status TEXT NOT NULL, failure_code TEXT,
                started_at TEXT NOT NULL, completed_at TEXT, error TEXT, result TEXT,
                verification_results TEXT, UNIQUE(task_id, attempt_index)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        pool
    }

    fn task_row(id: &str, status: &str) -> tasks::TaskRun {
        tasks::TaskRun {
            id: id.into(),
            session_id: "session-task-objective".into(),
            title: "Implement durable task recovery".into(),
            description: "change code and verify it".into(),
            status: status.into(),
            cwd: "/tmp/project".into(),
            parent_task_id: None,
            sub_session_id: None,
            created_at: "2026-08-11T00:00:00Z".into(),
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
            attempt_count: 0,
            verification_results: None,
            task_context_json: None,
            acceptance_criteria_json: Some(r#"["change is present","focused test passes"]"#.into()),
            spec_req_id: None,
            spec_title: None,
        }
    }

    fn completed_subagent_result() -> SubagentResult {
        SubagentResult {
            completed: true,
            summary: "implementation done".into(),
            sub_session_id: "sub-1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn final_failed_verification_hands_off_to_system_remediation_instead_of_finishing() {
        let results = vec![VerificationResult {
            check: "npm test".into(),
            passed: false,
            output: "expected red test".into(),
            duration_ms: 12,
        }];

        let decision = settle_result_after_verification(
            completed_subagent_result(),
            &results,
            MAX_ATTEMPTS,
            MAX_ATTEMPTS,
        );

        match decision {
            VerificationAttemptDecision::Retry { error } => {
                assert!(error.contains("npm test"));
                assert!(error.contains("expected red test"));
            }
            VerificationAttemptDecision::Finish(outcome) => panic!(
                "attempt exhaustion is still system-owned work and needs a durable remediation handoff; it must not Finish into terminal failed: {}",
                outcome.summary
            ),
        }
    }

    #[test]
    fn non_final_failed_verification_requests_retry() {
        let results = vec![VerificationResult {
            check: "cargo test".into(),
            passed: false,
            output: "compiler error".into(),
            duration_ms: 7,
        }];

        let decision = settle_result_after_verification(
            completed_subagent_result(),
            &results,
            1,
            MAX_ATTEMPTS,
        );

        match decision {
            VerificationAttemptDecision::Retry { error } => {
                assert!(error.contains("cargo test"));
                assert!(error.contains("compiler error"));
            }
            VerificationAttemptDecision::Finish(_) => {
                panic!("non-final failed verification should retry")
            }
        }
    }

    #[tokio::test]
    async fn dispatch_and_legacy_resume_bind_one_task_objective_idempotently() {
        let pool = task_objective_pool().await;
        let task = task_row("task-idempotent", "pending");
        tasks::insert_task(&pool, &task).await.unwrap();

        let first = objective_for_task(&pool, &task).await.unwrap();
        let resumed = objective_for_task(&pool, &task).await.unwrap();
        assert_eq!(first.id, resumed.id);
        assert_eq!(first.task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(
            tasks::task_objective_id(&pool, &task.id)
                .await
                .unwrap()
                .as_deref(),
            Some(first.id.as_str())
        );
        let objective_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE task_id=?")
                .bind(&task.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(objective_count, 1);

        let result = SubagentResult {
            completed: true,
            summary: "completed before legacy projection settled".into(),
            files_changed: vec!["src/lib.rs".into()],
            ..Default::default()
        };
        let verification = vec![VerificationResult {
            check: "focused test".into(),
            passed: true,
            output: "ok".into(),
            duration_ms: 1,
        }];
        let completed = complete_verified_task_objective(&pool, &first, &result, &verification)
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let crash_resumed = objective_for_task(&pool, &task).await.unwrap();
        assert_eq!(crash_resumed.id, first.id);
        assert_eq!(crash_resumed.status, ObjectiveStatus::Completed);
        let objective_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE task_id=?")
                .bind(&task.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            objective_count, 1,
            "projection crash recovery must not create a second Objective"
        );
    }

    #[test]
    fn verified_task_completion_requires_arbiter_accepted_evidence() {
        let mut objective = ObjectiveSnapshot::new(
            "objective-completion",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Task,
            "change is present and focused test passes",
        );
        objective.task_id = Some("task-completion".into());
        let result = SubagentResult {
            completed: true,
            summary: "implemented and verified".into(),
            files_changed: vec!["src/lib.rs".into()],
            ..Default::default()
        };
        let verification = vec![VerificationResult {
            check: "cargo test focused".into(),
            passed: true,
            output: "ok".into(),
            duration_ms: 12,
        }];

        let evidence = task_completion_evidence(&objective, &result, &verification);
        assert!(evidence
            .iter()
            .any(|item| item.kind == EvidenceKind::ChangeSet));
        assert!(evidence
            .iter()
            .any(|item| item.kind == EvidenceKind::PostChangeValidation));
        let decision = CompletionArbiter::decide(&objective, &evidence).unwrap();
        assert_eq!(decision.status, ObjectiveStatus::Completed);
        assert!(CompletionArbiter::decide(
            &objective,
            &task_completion_evidence(&objective, &result, &[]),
        )
        .is_err());
    }

    #[tokio::test]
    async fn exhausted_task_is_durable_system_wait_not_terminal_failure() {
        let pool = task_objective_pool().await;
        let task = task_row("task-recoverable", "running");
        tasks::insert_task(&pool, &task).await.unwrap();
        let objective = objective_for_task(&pool, &task).await.unwrap();

        let waiting = handoff_task_to_system_recovery(
            &pool,
            &objective,
            &task.id,
            "verification_failed",
            "focused test is still red",
        )
        .await
        .unwrap();
        assert_eq!(waiting.status, ObjectiveStatus::WaitingSystem);
        assert!(!waiting.requires_user_action);
        assert!(waiting.next_observation_at.is_some());
        let projection: (String, Option<String>, Option<i64>, Option<String>) = sqlx::query_as(
            "SELECT status, recovery_state, next_observation_at, completed_at \
             FROM task_runs WHERE id=?",
        )
        .bind(&task.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(projection.0, "pending");
        assert_eq!(projection.1.as_deref(), Some("waiting_system"));
        assert!(projection.2.is_some());
        assert_eq!(projection.3, None);
    }
}
