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
//! once. Each spawned future holds its own permit until it finishes. The
//! scheduler loop is single-threaded — it owns the DB read for "what's
//! ready next?" and the event emission. Cancellation is cooperative via an
//! `AtomicBool` checked at the top of each iteration; in-flight subagents
//! are NOT killed mid-flight (they always finish their current iteration).

use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Semaphore};

use crate::agent::hooks::{HookEvent, HookRunner};
use crate::agent::subagent::{self, SubagentBrief, SubagentResult};
use crate::agent::verification;
use crate::config::settings::Settings;
use crate::errors::AppError;
use crate::storage::tasks;
use crate::PendingPermissionMap;

/// Max concurrent subagents per scheduler instance.
pub const MAX_PARALLEL: usize = 3;

/// Max retry attempts per task (including the first attempt).
const MAX_ATTEMPTS: u32 = 3;

/// Poll interval when no new tasks can be dispatched but some are still in flight.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

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
    ) -> Result<(), AppError> {
        let semaphore = Arc::new(Semaphore::new(self.max_parallel));

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
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => break, // cap reached, try again next tick
                };

                tasks::mark_task_started(&self.pool, &task.id).await.ok();
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
                    },
                );
                dispatched_any = true;

                // Clone everything needed inside the spawned future.
                let pool = self.pool.clone();
                let settings = settings.clone();
                let app = app_handle.clone();
                let perms = pending_perms.clone();
                let session_id_for_task = session_id.clone();
                let running = self.running.clone();
                let task_id = task.id.clone();
                let task_cwd = task.cwd.clone();
                let task_title = task.title.clone();
                let task_description = task.description.clone();

                tokio::spawn(async move {
                    let _permit = permit; // released on drop

                    // ── Pre-task hook ────────────────────────────────────────
                    let hook_runner = HookRunner::from_settings(&settings, app.clone());
                    hook_runner
                        .fire(HookEvent::PreTask {
                            task_id: task_id.clone(),
                            title: task_title.clone(),
                        })
                        .await;

                    // ── Retry loop ──────────────────────────────────────────
                    let mut prev_error: Option<String> = None;
                    let mut final_outcome: Option<SubagentResult> = None;

                    for attempt in 1..=MAX_ATTEMPTS {
                        // Emit retry event on attempts 2+.
                        if attempt > 1 {
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

                        let brief = SubagentBrief {
                            task_id: task_id.clone(),
                            title: task_title.clone(),
                            description,
                            cwd: task_cwd.clone(),
                            parent_summary: None,
                            allowed_tools: vec![
                                "read_file".into(),
                                "glob".into(),
                                "grep".into(),
                                "write_file".into(),
                                "edit_file".into(),
                                "bash".into(),
                            ],
                            acceptance_criteria: None,
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
                                    message: Some(&format!(
                                        "Attempt {attempt}/{MAX_ATTEMPTS}…"
                                    )),
                                    result: None,
                                    error: None,
                                },
                            );
                        }

                        let subagent_result = subagent::run_subagent(
                            brief,
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
                                tracing::warn!(
                                    "scheduler: task {} attempt {attempt} error: {msg}",
                                    task_id
                                );
                                prev_error = Some(msg.clone());
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
                                    },
                                );
                                // Continue to next attempt.
                            }
                            Ok(result) => {
                                // Persist sub-session link.
                                tasks::set_sub_session(
                                    &pool,
                                    &task_id,
                                    &result.sub_session_id,
                                )
                                .await
                                .ok();

                                // ── Acceptance check ────────────────────────
                                if let Some(ref ac) = result.acceptance_check {
                                    if !ac.passed {
                                        let msg = format!(
                                            "Acceptance check failed: {}",
                                            ac.reason
                                        );
                                        tracing::warn!(
                                            "scheduler: task {} attempt {attempt} acceptance failed: {}",
                                            task_id,
                                            ac.reason
                                        );
                                        prev_error = Some(msg.clone());
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
                                let verif_plan =
                                    verification::detect_verification_plan(&task_cwd);
                                let verif_results = verification::run_verification(
                                    &verif_plan,
                                    &app,
                                    &session_id_for_task,
                                    &task_id,
                                )
                                .await;

                                // Persist verification results.
                                if let Ok(json) =
                                    serde_json::to_string(&verif_results)
                                {
                                    tasks::save_verification_results(
                                        &pool,
                                        &task_id,
                                        &json,
                                    )
                                    .await
                                    .ok();
                                }

                                let any_failed = verif_results.iter().any(|r| !r.passed);

                                if any_failed && attempt < MAX_ATTEMPTS {
                                    // Build a rich error message for the next brief.
                                    let mut verif_msg = String::from(
                                        "Previous attempt failed verification:\n",
                                    );
                                    for r in &verif_results {
                                        if !r.passed {
                                            verif_msg.push_str(&format!(
                                                "- {}: FAILED\n  {}\n",
                                                r.check,
                                                r.output.lines().take(20).collect::<Vec<_>>().join("\n  ")
                                            ));
                                        }
                                    }
                                    verif_msg
                                        .push_str("Please fix the above errors.");

                                    tracing::warn!(
                                        "scheduler: task {} attempt {attempt} failed verification",
                                        task_id
                                    );
                                    prev_error = Some(verif_msg.clone());
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
                                        },
                                    );
                                    continue;
                                }

                                // Success (or exhausted but verification is the stopper).
                                final_outcome = Some(result);
                                break;
                            }
                        }
                    }

                    // ── Settle the task ──────────────────────────────────────
                    // Capture hook data before match consumes final_outcome.
                    let hook_completed = final_outcome.as_ref().map_or(false, |r| r.completed);
                    let hook_summary = final_outcome
                        .as_ref()
                        .map(|r| r.summary.clone())
                        .unwrap_or_else(|| prev_error.clone().unwrap_or_default());

                    match final_outcome {
                        Some(result) if result.completed => {
                            let result_json = serde_json::to_string(&result)
                                .unwrap_or_else(|_| "{}".into());
                            tasks::mark_task_completed(&pool, &task_id, &result_json)
                                .await
                                .ok();
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
                                },
                            );
                        }
                        Some(result) => {
                            // Completed=false after retries (e.g. acceptance failure).
                            let err = prev_error
                                .clone()
                                .unwrap_or_else(|| result.summary.clone());
                            tasks::mark_task_failed(&pool, &task_id, &err)
                                .await
                                .ok();
                            emit_task(
                                &app,
                                &session_id_for_task,
                                "task_failed",
                                &TaskEventPayload {
                                    task_id: &task_id,
                                    title: None,
                                    message: None,
                                    result: None,
                                    error: Some(&err),
                                },
                            );
                        }
                        None => {
                            // All attempts returned Err.
                            let err = prev_error.clone().unwrap_or_else(|| {
                                format!("Failed after {MAX_ATTEMPTS} attempts")
                            });
                            tasks::mark_task_failed(&pool, &task_id, &err)
                                .await
                                .ok();
                            emit_task(
                                &app,
                                &session_id_for_task,
                                "task_failed",
                                &TaskEventPayload {
                                    task_id: &task_id,
                                    title: None,
                                    message: None,
                                    result: None,
                                    error: Some(&err),
                                },
                            );
                        }
                    }

                    // ── Post-task hook ───────────────────────────────────────
                    let post_status = if hook_completed { "completed" } else { "failed" };
                    let post_summary = hook_summary;
                    hook_runner
                        .fire(HookEvent::PostTask {
                            task_id: task_id.clone(),
                            status: post_status.to_string(),
                            summary: post_summary,
                        })
                        .await;

                    running.lock().await.remove(&task_id);
                });
            }

            // 4. If nothing is in flight and no new tasks were dispatched and
            //    no pending tasks remain, we're done.
            let remaining_pending =
                tasks::list_pending_tasks_for_session(&self.pool, &session_id)
                    .await?
                    .len();
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
        let pending =
            tasks::list_pending_tasks_for_session(&self.pool, session_id).await?;
        for t in pending {
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
}

#[derive(Serialize, Clone)]
struct RetryEventPayload<'a> {
    task_id: &'a str,
    attempt: u32,
}

fn emit_task(
    app: &AppHandle,
    session_id: &str,
    kind: &str,
    payload: &TaskEventPayload<'_>,
) {
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
