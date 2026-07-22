// SPDX-License-Identifier: Apache-2.0
//! Learning events — the self-evolution loop.
//!
//! After a task session completes, a single cheap post-mortem pass
//! produces 0-3 observations about the user (e.g. "the user kept asking
//! me to add tests after implementation"). Each observation pairs with a
//! suggestion ("auto-add tests for new functions"). These land in the
//! `learning_events` table as `status=pending`.
//!
//! The Profile page surfaces pending events; the user clicks Accept
//! (suggestion gets appended to `.codefactory/memory.md` so it influences
//! future sessions) or Reject (event marked rejected, not shown again).
//!
//! Token economy: post-mortem runs **once per session**, not per task.
//! Input is bounded to a short summary of task outcomes when available,
//! otherwise to redacted chat/tool evidence. Output is capped at 500 tokens
//! and uses the user's default model — no new config.

use chrono::Utc;
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path;
use tauri::{command, AppHandle, Emitter, State};
use uuid::Uuid;

use crate::commands::memory::ProjectMemory;
use crate::ai_text::{run_one_shot_text, AiMessage as OneShotAiMessage};
use crate::config::settings::ApiStyle;
use crate::errors::AppError;
use crate::AppState;

static LEARNING_DECISION_LOCK: Lazy<tokio::sync::Mutex<()>> =
    Lazy::new(|| tokio::sync::Mutex::new(()));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEvent {
    pub id: String,
    pub session_id: String,
    pub cwd: String,
    pub observation: String,
    pub suggestion: String,
    /// pending | accepted | rejected
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
    /// 'memory' (default) appends to .codefactory/memory.md on accept.
    /// 'preference' upserts pref_key→pref_value into user_preferences.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Only populated when kind == 'preference'.
    #[serde(default)]
    pub pref_key: Option<String>,
    /// Only populated when kind == 'preference'.
    #[serde(default)]
    pub pref_value: Option<String>,
    /// Self-evolution P1: support count behind a mined insight. The exact unit
    /// is declared by evidence_json.support_unit (sessions or decisions).
    /// Per-session post-mortem rows keep this at 0.
    #[serde(default)]
    pub support_count: i64,
    /// Raw metrics behind a mined insight, as JSON ("{}" for non-mined rows).
    #[serde(default = "default_evidence")]
    pub evidence_json: String,
    /// Analysis job that first produced this candidate. Legacy and per-session
    /// post-mortem rows intentionally keep this NULL.
    #[serde(default)]
    pub job_id: Option<String>,
}

fn default_kind() -> String {
    "memory".into()
}
fn default_evidence() -> String {
    "{}".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvolutionJob {
    pub id: String,
    pub cwd: String,
    pub trigger: String,
    pub candidate_id: Option<String>,
    pub status: String,
    pub input_session_count: i64,
    pub input_trace_count: i64,
    pub candidate_count: i64,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvolutionJobEvent {
    pub id: String,
    pub cwd: String,
    pub job_id: String,
    pub candidate_id: Option<String>,
    pub stage: String,
    pub status: String,
    pub title: String,
    pub detail_json: String,
    pub created_at: String,
}

type JobRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    i64,
    i64,
    i64,
    String,
    Option<String>,
    Option<String>,
);

fn job_from_row(row: JobRow) -> EvolutionJob {
    let (
        id,
        cwd,
        trigger,
        candidate_id,
        status,
        input_session_count,
        input_trace_count,
        candidate_count,
        started_at,
        completed_at,
        error,
    ) = row;
    EvolutionJob {
        id,
        cwd,
        trigger,
        candidate_id,
        status,
        input_session_count,
        input_trace_count,
        candidate_count,
        started_at,
        completed_at,
        error,
    }
}

type JobEventRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
);

fn job_event_from_row(row: JobEventRow) -> EvolutionJobEvent {
    let (id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at) = row;
    EvolutionJobEvent {
        id,
        cwd,
        job_id,
        candidate_id,
        stage,
        status,
        title,
        detail_json,
        created_at,
    }
}

fn redacted_job_detail(detail: serde_json::Value) -> String {
    const ALLOWED_KEYS: &[&str] = &[
        "schema_version",
        "session_count",
        "trace_count",
        "tool_call_count",
        "task_run_count",
        "decision_count",
        "candidate_count",
        "extracted_count",
        "duplicate_count",
        "pending_count",
        "support_count",
        "status",
        "terminal_status",
        "candidate_status",
        "candidate_kind",
        "decision",
        "target",
        "trigger",
        "reason",
        "error",
        "aggregate_only",
        "redactor",
        "reasoning_included",
        "raw_prompt_included",
        "already_present",
        "candidate_marker_present",
        "value_persisted",
        "materialization_started",
    ];
    let redacted = crate::trajectory::redact_json(&detail);
    let mut allowed = serde_json::Map::new();
    if let serde_json::Value::Object(map) = redacted {
        for key in ALLOWED_KEYS {
            let Some(value) = map.get(*key) else {
                continue;
            };
            let bounded = match value {
                serde_json::Value::String(value) => {
                    let redacted = crate::trajectory::redact_text(value, 160);
                    serde_json::Value::String(redacted.chars().take(160).collect())
                }
                serde_json::Value::Number(_)
                | serde_json::Value::Bool(_)
                | serde_json::Value::Null => value.clone(),
                serde_json::Value::Array(_) | serde_json::Value::Object(_) => continue,
            };
            allowed.insert((*key).to_string(), bounded);
        }
    }
    allowed
        .entry("schema_version".to_string())
        .or_insert_with(|| serde_json::Value::from(1));
    serde_json::Value::Object(allowed).to_string()
}

async fn append_evolution_job_event(
    pool: &SqlitePool,
    cwd: &str,
    job_id: &str,
    candidate_id: Option<&str>,
    stage: &str,
    status: &str,
    title: &str,
    detail: serde_json::Value,
) -> Result<EvolutionJobEvent, AppError> {
    let event = EvolutionJobEvent {
        id: Uuid::new_v4().to_string(),
        cwd: cwd.to_string(),
        job_id: job_id.to_string(),
        candidate_id: candidate_id.map(str::to_string),
        stage: stage.to_string(),
        status: status.to_string(),
        title: title.to_string(),
        detail_json: redacted_job_detail(detail),
        created_at: Utc::now().to_rfc3339(),
    };
    sqlx::query(
        "INSERT INTO evolution_job_events
         (id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&event.id)
    .bind(&event.cwd)
    .bind(&event.job_id)
    .bind(&event.candidate_id)
    .bind(&event.stage)
    .bind(&event.status)
    .bind(&event.title)
    .bind(&event.detail_json)
    .bind(&event.created_at)
    .execute(pool)
    .await?;
    Ok(event)
}

async fn append_evolution_job_event_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    cwd: &str,
    job_id: &str,
    candidate_id: Option<&str>,
    stage: &str,
    status: &str,
    title: &str,
    detail: serde_json::Value,
    created_at: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO evolution_job_events
         (id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(cwd)
    .bind(job_id)
    .bind(candidate_id)
    .bind(stage)
    .bind(status)
    .bind(title)
    .bind(redacted_job_detail(detail))
    .bind(created_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ── Queries ───────────────────────────────────────────────────────────────────

type LearningEventRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
    Option<String>,
);

async fn list_learning_events_for_pool(
    cwd: &str,
    pool: &SqlitePool,
) -> Result<Vec<LearningEvent>, AppError> {
    let rows = sqlx::query_as::<_, LearningEventRow>(
        "SELECT id, session_id, cwd, observation, suggestion, status, created_at, decided_at, \
                kind, pref_key, pref_value, support_count, evidence_json, job_id \
         FROM learning_events
         WHERE cwd = ? AND (
           (status = 'pending' AND id NOT IN (
             SELECT source_learning_event_id FROM improvement_candidates
             WHERE source_learning_event_id IS NOT NULL
           )) OR id IN (
             SELECT id FROM learning_events
             WHERE cwd = ? AND status <> 'pending'
             ORDER BY COALESCE(decided_at, created_at) DESC, rowid DESC LIMIT 100
           )
         )
         ORDER BY CASE WHEN status = 'pending' THEN 0 ELSE 1 END,
                  CASE WHEN status = 'pending' THEN support_count END DESC,
                  CASE WHEN status = 'pending' THEN created_at END DESC,
                  CASE WHEN status <> 'pending' THEN COALESCE(decided_at, created_at) END DESC,
                  rowid DESC",
    )
    .bind(cwd)
    .bind(cwd)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                session_id,
                cwd,
                observation,
                suggestion,
                status,
                created_at,
                decided_at,
                kind,
                pref_key,
                pref_value,
                support_count,
                evidence_json,
                job_id,
            )| {
                LearningEvent {
                    id,
                    session_id,
                    cwd,
                    observation,
                    suggestion,
                    status,
                    created_at,
                    decided_at,
                    kind,
                    pref_key,
                    pref_value,
                    support_count,
                    evidence_json,
                    job_id,
                }
            },
        )
        .collect())
}

#[command]
pub async fn list_learning_events(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<LearningEvent>, AppError> {
    let pool = state.db.read().await;
    list_learning_events_for_pool(&cwd, &pool).await
}

async fn list_evolution_jobs_for_pool(
    cwd: &str,
    pool: &SqlitePool,
) -> Result<Vec<EvolutionJob>, AppError> {
    let rows: Vec<JobRow> = sqlx::query_as(
        "SELECT id, cwd, trigger, candidate_id, status, input_session_count,
                input_trace_count, candidate_count, started_at, completed_at, error
         FROM evolution_jobs WHERE cwd = ?
         ORDER BY started_at DESC, rowid DESC LIMIT 100",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(job_from_row).collect())
}

async fn list_evolution_decision_jobs_for_pool(
    cwd: &str,
    pool: &SqlitePool,
) -> Result<Vec<EvolutionJob>, AppError> {
    let rows: Vec<JobRow> = sqlx::query_as(
        "SELECT id, cwd, trigger, candidate_id, status, input_session_count,
                input_trace_count, candidate_count, started_at, completed_at, error
         FROM evolution_jobs
         WHERE cwd = ? AND trigger IN ('review_accept', 'review_reject')
           AND candidate_id IN (
             SELECT id FROM learning_events
             WHERE cwd = ? AND status <> 'pending'
             ORDER BY COALESCE(decided_at, created_at) DESC, rowid DESC LIMIT 100
           )
         ORDER BY started_at DESC, rowid DESC",
    )
    .bind(cwd)
    .bind(cwd)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(job_from_row).collect())
}

async fn get_evolution_job_for_pool(
    cwd: &str,
    job_id: &str,
    pool: &SqlitePool,
) -> Result<EvolutionJob, AppError> {
    let row: Option<JobRow> = sqlx::query_as(
        "SELECT id, cwd, trigger, candidate_id, status, input_session_count,
                input_trace_count, candidate_count, started_at, completed_at, error
         FROM evolution_jobs WHERE cwd = ? AND id = ?",
    )
    .bind(cwd)
    .bind(job_id)
    .fetch_optional(pool)
    .await?;
    row.map(job_from_row)
        .ok_or_else(|| AppError::Other(format!("evolution job {job_id} not found for project")))
}

#[command]
pub async fn get_evolution_job(
    cwd: String,
    job_id: String,
    state: State<'_, AppState>,
) -> Result<EvolutionJob, AppError> {
    let pool = state.db.read().await;
    get_evolution_job_for_pool(&cwd, &job_id, &pool).await
}

#[command]
pub async fn list_evolution_jobs(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<EvolutionJob>, AppError> {
    let pool = state.db.read().await;
    list_evolution_jobs_for_pool(&cwd, &pool).await
}

#[command]
pub async fn list_evolution_decision_jobs(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<EvolutionJob>, AppError> {
    let pool = state.db.read().await;
    list_evolution_decision_jobs_for_pool(&cwd, &pool).await
}

async fn list_evolution_job_events_for_pool(
    cwd: &str,
    job_id: Option<&str>,
    pool: &SqlitePool,
) -> Result<Vec<EvolutionJobEvent>, AppError> {
    let rows: Vec<JobEventRow> = if let Some(job_id) = job_id {
        sqlx::query_as(
            "SELECT id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at
             FROM (
               SELECT rowid AS event_rowid, id, cwd, job_id, candidate_id, stage, status,
                      title, detail_json, created_at
               FROM evolution_job_events WHERE cwd = ? AND job_id = ?
               ORDER BY created_at DESC, rowid DESC LIMIT 500
             )
             ORDER BY created_at ASC, event_rowid ASC",
        )
        .bind(cwd)
        .bind(job_id)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at
             FROM (
               SELECT rowid AS event_rowid, id, cwd, job_id, candidate_id, stage, status,
                      title, detail_json, created_at
               FROM evolution_job_events WHERE cwd = ?
               ORDER BY created_at DESC, rowid DESC LIMIT 500
             )
             ORDER BY created_at ASC, event_rowid ASC",
        )
        .bind(cwd)
        .fetch_all(pool)
        .await?
    };
    Ok(rows.into_iter().map(job_event_from_row).collect())
}

#[command]
pub async fn list_evolution_job_events(
    cwd: String,
    job_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<EvolutionJobEvent>, AppError> {
    let pool = state.db.read().await;
    list_evolution_job_events_for_pool(&cwd, job_id.as_deref(), &pool).await
}

type DecisionCandidateRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

async fn load_pending_decision_candidate(
    pool: &SqlitePool,
    event_id: &str,
) -> Result<DecisionCandidateRow, AppError> {
    let row: Option<DecisionCandidateRow> = sqlx::query_as(
        "SELECT cwd, suggestion, status, kind, pref_key, pref_value
         FROM learning_events WHERE id = ?",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?;
    let row = row.ok_or_else(|| AppError::Other(format!("learning event {event_id} not found")))?;
    if row.2 != "pending" {
        return Err(AppError::Other(format!(
            "learning event {event_id} already {}",
            row.2
        )));
    }
    Ok(row)
}

async fn begin_decision_job(
    pool: &SqlitePool,
    cwd: &str,
    candidate_id: &str,
    trigger: &str,
) -> Result<String, AppError> {
    let idempotency_key = format!("{trigger}:{cwd}:{candidate_id}");
    let existing: Option<(String, String)> =
        sqlx::query_as("SELECT id, status FROM evolution_jobs WHERE idempotency_key = ?")
            .bind(&idempotency_key)
            .fetch_optional(pool)
            .await?;
    let now = Utc::now().to_rfc3339();
    let owner_pid = std::process::id() as i64;
    let owner_start_token = crate::storage::db::current_process_start_token();
    let job_id = if let Some((job_id, status)) = existing {
        match status.as_str() {
            "succeeded" => {
                return Err(AppError::Other(format!(
                    "decision job for learning event {candidate_id} already succeeded"
                )));
            }
            "running" | "queued" => {
                return Err(AppError::Other(format!(
                    "decision job for learning event {candidate_id} is already running"
                )));
            }
            _ => {}
        }
        sqlx::query(
            "UPDATE evolution_jobs
             SET status='running', owner_pid=?, owner_start_token=?, started_at=?, completed_at=NULL, error=NULL
             WHERE id=? AND cwd=? AND candidate_id=?",
        )
        .bind(owner_pid)
        .bind(&owner_start_token)
        .bind(&now)
        .bind(&job_id)
        .bind(cwd)
        .bind(candidate_id)
        .execute(pool)
        .await?;
        job_id
    } else {
        let job_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, candidate_id, status, idempotency_key, owner_pid, owner_start_token, started_at)
             VALUES (?, ?, ?, ?, 'running', ?, ?, ?, ?)",
        )
        .bind(&job_id)
        .bind(cwd)
        .bind(trigger)
        .bind(candidate_id)
        .bind(&idempotency_key)
        .bind(owner_pid)
        .bind(&owner_start_token)
        .bind(&now)
        .execute(pool)
        .await?;
        job_id
    };
    if let Err(error) = append_evolution_job_event(
        pool,
        cwd,
        &job_id,
        Some(candidate_id),
        "job",
        "started",
        "审核作业开始",
        serde_json::json!({"trigger": trigger}),
    )
    .await
    {
        record_job_failure(
            pool,
            cwd,
            &job_id,
            Some(candidate_id),
            "job",
            "审核作业启动失败",
            &error,
        )
        .await;
        return Err(error);
    }
    Ok(job_id)
}

async fn record_job_failure(
    pool: &SqlitePool,
    cwd: &str,
    job_id: &str,
    candidate_id: Option<&str>,
    stage: &str,
    title: &str,
    error: &AppError,
) {
    if let Err(persist_error) = persist_job_failure(
        pool,
        cwd,
        job_id,
        candidate_id,
        stage,
        title,
        error,
    )
    .await
    {
        tracing::warn!("failed to persist atomic evolution job failure: {persist_error}");
    }
}

async fn persist_job_failure(
    pool: &SqlitePool,
    cwd: &str,
    job_id: &str,
    candidate_id: Option<&str>,
    stage: &str,
    title: &str,
    error: &AppError,
) -> Result<(), AppError> {
    let safe_error = crate::trajectory::redact_text(&error.to_string(), 500);
    let now = Utc::now().to_rfc3339();
    let mut transaction = pool.begin().await?;
    append_evolution_job_event_in_transaction(
        &mut transaction,
        cwd,
        job_id,
        candidate_id,
        stage,
        "failed",
        title,
        serde_json::json!({
            "error": safe_error,
            "raw_prompt_included": false,
            "reasoning_included": false,
        }),
        &now,
    )
    .await?;
    let updated = sqlx::query(
        "UPDATE evolution_jobs SET status='failed', completed_at=?, error=? WHERE id=? AND cwd=?",
    )
    .bind(&now)
    .bind(&safe_error)
    .bind(job_id)
    .bind(cwd)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Other(format!(
            "evolution job {job_id} missing while recording failure"
        )));
    }
    transaction.commit().await?;
    Ok(())
}

async fn append_learning_memory_once(
    cwd: &str,
    suggestion: &str,
    candidate_id: &str,
) -> Result<(ProjectMemory, bool), AppError> {
    let suggestion = suggestion.trim();
    if suggestion.is_empty() {
        return Err(AppError::Other("Cannot save empty fact to memory".into()));
    }
    let path = learning_memory_path(cwd);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let marker = format!("<!-- codefactory-learning-event:{candidate_id} -->");
    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    if existing.contains(&marker) {
        return Ok((
            ProjectMemory {
                path: path.to_string_lossy().into_owned(),
                content: existing,
                exists: true,
            },
            true,
        ));
    }

    let date = Utc::now().format("%Y-%m-%d");
    let mut combined = existing;
    if combined.is_empty() {
        combined.push_str("# Project memory\n\n");
        combined.push_str("Auto-injected into every chat session in this repo.\n");
        combined.push_str("Use the Remember button in the chat UI to add new entries.\n\n");
    } else {
        combined.push_str("\n\n");
    }
    combined.push_str(&format!("- ({date}) {suggestion}\n{marker}"));
    tokio::fs::write(&path, &combined).await?;
    Ok((
        ProjectMemory {
            path: path.to_string_lossy().into_owned(),
            content: combined,
            exists: true,
        },
        false,
    ))
}

fn learning_memory_path(cwd: &str) -> std::path::PathBuf {
    Path::new(cwd).join(".codefactory").join("memory.md")
}

async fn learning_memory_marker_present(cwd: &str, candidate_id: &str) -> Result<bool, AppError> {
    let marker = format!("<!-- codefactory-learning-event:{candidate_id} -->");
    match tokio::fs::read_to_string(learning_memory_path(cwd)).await {
        Ok(content) => Ok(content.contains(&marker)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

async fn accept_learning_event_for_pool(
    event_id: &str,
    pool: &SqlitePool,
) -> Result<(ProjectMemory, String), AppError> {
    let _decision_guard = LEARNING_DECISION_LOCK.lock().await;
    let (cwd, suggestion, _status, kind, pref_key, pref_value) =
        load_pending_decision_candidate(pool, event_id).await?;
    let job_id = begin_decision_job(pool, &cwd, event_id, "review_accept").await?;
    let prepare = async {
        append_evolution_job_event(
            pool,
            &cwd,
            &job_id,
            Some(event_id),
            "review",
            "started",
            "开始人工审核",
            serde_json::json!({"candidate_kind": kind}),
        )
        .await?;
        append_evolution_job_event(
            pool,
            &cwd,
            &job_id,
            Some(event_id),
            "review",
            "completed",
            "人工审核通过，准备物化",
            serde_json::json!({"candidate_kind": kind}),
        )
        .await?;
        append_evolution_job_event(
            pool,
            &cwd,
            &job_id,
            Some(event_id),
            "materialize",
            "started",
            "开始应用候选",
            serde_json::json!({"target": kind}),
        )
        .await?;
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(error) = prepare {
        record_job_failure(
            pool,
            &cwd,
            &job_id,
            Some(event_id),
            "job",
            "审核作业日志写入失败",
            &error,
        )
        .await;
        return Err(error);
    }

    let materialized: Result<
        (ProjectMemory, serde_json::Value, Option<(String, String)>),
        AppError,
    > = match kind.as_str() {
        "preference" => {
            let key = pref_key.ok_or_else(|| {
                AppError::Other("preference learning event missing pref_key".into())
            });
            key.map(|key| {
                (
                    ProjectMemory {
                        path: String::new(),
                        content: String::new(),
                        exists: false,
                    },
                    serde_json::json!({"target": "preference", "value_persisted": true}),
                    Some((key, pref_value.unwrap_or_default())),
                )
            })
        }
        _ => append_learning_memory_once(&cwd, &suggestion, event_id)
            .await
            .map(|(memory, already_present)| {
                (
                    memory,
                    serde_json::json!({
                        "target": "memory",
                        "candidate_marker_present": true,
                        "already_present": already_present,
                    }),
                    None,
                )
            }),
    };
    let (memory, receipt, preference_write) = match materialized {
        Ok(result) => result,
        Err(error) => {
            record_job_failure(
                pool,
                &cwd,
                &job_id,
                Some(event_id),
                "materialize",
                "候选物化失败",
                &error,
            )
            .await;
            return Err(error);
        }
    };

    let finalized: Result<(), AppError> = async {
        let now = Utc::now().to_rfc3339();
        let mut transaction = pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE learning_events SET status='accepted', decided_at=?
             WHERE id=? AND cwd=? AND status='pending'",
        )
        .bind(&now)
        .bind(event_id)
        .bind(&cwd)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::Other(format!(
                "learning event {event_id} changed before materialization could be committed"
            )));
        }
        if let Some((key, value)) = &preference_write {
            sqlx::query(
                "INSERT INTO user_preferences (cwd, key, value, source, updated_at)
                 VALUES (?,?,?,'ai',?)
                 ON CONFLICT(cwd, key) DO UPDATE SET
                   value = excluded.value, source = 'ai', updated_at = excluded.updated_at",
            )
            .bind(&cwd)
            .bind(key)
            .bind(value)
            .bind(&now)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "INSERT INTO evolution_job_events
             (id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at)
             VALUES (?, ?, ?, ?, 'materialize', 'completed', '候选已物化并生效', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&cwd)
        .bind(&job_id)
        .bind(event_id)
        .bind(redacted_job_detail(receipt))
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO evolution_job_events
             (id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at)
             VALUES (?, ?, ?, ?, 'job', 'completed', '审核与物化完成', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&cwd)
        .bind(&job_id)
        .bind(event_id)
        .bind(redacted_job_detail(serde_json::json!({
            "terminal_status": "succeeded",
            "candidate_status": "accepted",
        })))
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE evolution_jobs
             SET status='succeeded', candidate_count=1, completed_at=?, error=NULL
             WHERE id=? AND cwd=?",
        )
        .bind(&now)
        .bind(&job_id)
        .bind(&cwd)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }
    .await;
    if let Err(error) = finalized {
        record_job_failure(
            pool,
            &cwd,
            &job_id,
            Some(event_id),
            "materialize",
            "物化终态提交失败",
            &error,
        )
        .await;
        return Err(error);
    }
    Ok((memory, cwd))
}

/// Accept is one user action but two audited stages: review then materialize.
/// The learning row becomes accepted only after the side effect succeeds.
#[command]
pub async fn accept_learning_event(
    event_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ProjectMemory, AppError> {
    let pool = state.db.read().await;
    let (memory, cwd) = accept_learning_event_for_pool(&event_id, &pool).await?;
    let event = format!("learning_events_updated:{cwd}");
    let _ = app.emit(&event, ());
    Ok(memory)
}

async fn reject_learning_event_for_pool(
    event_id: &str,
    pool: &SqlitePool,
) -> Result<String, AppError> {
    let _decision_guard = LEARNING_DECISION_LOCK.lock().await;
    let (cwd, _suggestion, _status, kind, _pref_key, _pref_value) =
        load_pending_decision_candidate(pool, event_id).await?;
    if kind != "preference" && learning_memory_marker_present(&cwd, event_id).await? {
        return Err(AppError::Other(format!(
            "learning event {event_id} was already written to project memory; retry accept to reconcile its audit state"
        )));
    }
    let job_id = begin_decision_job(pool, &cwd, event_id, "review_reject").await?;
    if let Err(error) = append_evolution_job_event(
        pool,
        &cwd,
        &job_id,
        Some(event_id),
        "review",
        "started",
        "开始人工审核",
        serde_json::json!({"candidate_kind": kind}),
    )
    .await
    {
        record_job_failure(
            pool,
            &cwd,
            &job_id,
            Some(event_id),
            "review",
            "审核日志写入失败",
            &error,
        )
        .await;
        return Err(error);
    }

    let finalized: Result<(), AppError> = async {
        let now = Utc::now().to_rfc3339();
        let mut transaction = pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE learning_events SET status='rejected', decided_at=?
             WHERE id=? AND cwd=? AND status='pending'",
        )
        .bind(&now)
        .bind(event_id)
        .bind(&cwd)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::Other(format!(
                "learning event {event_id} changed before rejection could be committed"
            )));
        }
        sqlx::query(
            "INSERT INTO evolution_job_events
             (id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at)
             VALUES (?, ?, ?, ?, 'review', 'completed', '人工已拒绝候选', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&cwd)
        .bind(&job_id)
        .bind(event_id)
        .bind(redacted_job_detail(serde_json::json!({
            "decision": "rejected",
            "materialization_started": false,
        })))
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO evolution_job_events
             (id, cwd, job_id, candidate_id, stage, status, title, detail_json, created_at)
             VALUES (?, ?, ?, ?, 'job', 'completed', '拒绝决定已保存', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&cwd)
        .bind(&job_id)
        .bind(event_id)
        .bind(redacted_job_detail(serde_json::json!({
            "terminal_status": "succeeded",
            "candidate_status": "rejected",
        })))
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE evolution_jobs SET status='succeeded', completed_at=?, error=NULL
             WHERE id=? AND cwd=?",
        )
        .bind(&now)
        .bind(&job_id)
        .bind(&cwd)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }
    .await;
    if let Err(error) = finalized {
        record_job_failure(
            pool,
            &cwd,
            &job_id,
            Some(event_id),
            "review",
            "拒绝候选失败",
            &error,
        )
        .await;
        return Err(error);
    }
    Ok(cwd)
}

#[command]
pub async fn reject_learning_event(
    event_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await;
    let cwd = reject_learning_event_for_pool(&event_id, &pool).await?;
    let event = format!("learning_events_updated:{cwd}");
    let _ = app.emit(&event, ());
    Ok(())
}

// ── Post-mortem (after-session AI pass) ──────────────────────────────────────

#[derive(Debug, Serialize)]
struct AiMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Debug, Serialize)]
struct AiRequest<'a> {
    model: String,
    messages: Vec<AiMessage<'a>>,
    stream: bool,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct AiResponseChoice {
    message: AiResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AiResponseMessage {
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AiResponse {
    choices: Vec<AiResponseChoice>,
}

struct PostmortemCompletion {
    text: String,
    reasoning_present: bool,
    finish_reason: Option<String>,
}

fn extract_postmortem_completion(response: AiResponse) -> PostmortemCompletion {
    let Some(choice) = response.choices.into_iter().next() else {
        return PostmortemCompletion {
            text: String::new(),
            reasoning_present: false,
            finish_reason: None,
        };
    };
    PostmortemCompletion {
        text: choice.message.content.unwrap_or_default(),
        reasoning_present: choice
            .message
            .reasoning_content
            .as_deref()
            .map(str::trim)
            .is_some_and(|reasoning| !reasoning.is_empty()),
        finish_reason: choice.finish_reason,
    }
}

fn expand_postmortem_completion_budget(body: &mut serde_json::Value) {
    let field = if body.get("max_completion_tokens").is_some() {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    body[field] = serde_json::Value::from(2_000);
}

fn resolve_postmortem_model(settings: &crate::config::Settings) -> Option<String> {
    settings.resolve_model_for_endpoint(&settings.default_endpoint, &settings.default_model)
}

#[derive(Debug, Deserialize)]
struct PostmortemEntry {
    observation: String,
    suggestion: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    pref_key: Option<String>,
    #[serde(default)]
    pref_value: Option<String>,
}

fn valid_preference_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase())
        && chars.clone().count() < 64
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn sanitize_postmortem_entry(mut entry: PostmortemEntry) -> Option<PostmortemEntry> {
    entry.observation = crate::trajectory::redact_text(entry.observation.trim(), 2_000);
    entry.suggestion = crate::trajectory::redact_text(entry.suggestion.trim(), 1_000);
    entry.pref_key = entry
        .pref_key
        .map(|value| crate::trajectory::redact_text(value.trim(), 120));
    entry.pref_value = entry
        .pref_value
        .map(|value| crate::trajectory::redact_text(value.trim(), 500));
    if entry.kind.as_deref() == Some("preference")
        && !entry
            .pref_key
            .as_deref()
            .map(valid_preference_key)
            .unwrap_or(false)
    {
        entry.kind = Some("memory".into());
        entry.pref_key = None;
        entry.pref_value = None;
    }
    if entry.observation.is_empty() || entry.suggestion.is_empty() {
        None
    } else {
        Some(entry)
    }
}

async fn persist_postmortem_entries(
    pool: &SqlitePool,
    session_id: &str,
    cwd: &str,
    entries: Vec<PostmortemEntry>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<Vec<LearningEvent>, AppError> {
    let now = Utc::now().to_rfc3339();
    let mut created = Vec::new();
    for entry in entries.into_iter().filter_map(sanitize_postmortem_entry) {
        if !seen.insert(norm_suggestion(&entry.suggestion)) {
            continue;
        }

        // Resolve kind defensively: only honour 'preference' when the
        // structured payload is present. Invalid payloads remain reviewable as
        // memory candidates instead of creating unusable preference rows.
        let raw_kind = entry.kind.as_deref().unwrap_or("memory");
        let (kind, pref_key, pref_value): (&str, Option<String>, Option<String>) = if raw_kind
            == "preference"
            && entry
                .pref_key
                .as_ref()
                .map(|key| !key.trim().is_empty())
                .unwrap_or(false)
        {
            let key = entry.pref_key.as_ref().unwrap().trim().to_string();
            let value = entry.pref_value.unwrap_or_default().trim().to_string();
            ("preference", Some(key), Some(value))
        } else {
            ("memory", None, None)
        };

        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO learning_events \
             (id, session_id, cwd, observation, suggestion, status, created_at, kind, pref_key, pref_value) \
             VALUES (?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(session_id)
        .bind(cwd)
        .bind(&entry.observation)
        .bind(&entry.suggestion)
        .bind(&now)
        .bind(kind)
        .bind(&pref_key)
        .bind(&pref_value)
        .execute(pool)
        .await?;
        created.push(LearningEvent {
            id,
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            observation: entry.observation,
            suggestion: entry.suggestion,
            status: "pending".into(),
            created_at: now.clone(),
            decided_at: None,
            kind: kind.into(),
            pref_key,
            pref_value,
            support_count: 0,
            evidence_json: "{}".into(),
            job_id: None,
        });
    }
    Ok(created)
}

/// Normalize a suggestion for duplicate detection: trim, lowercase, and
/// collapse internal whitespace. Cheap exact-ish matching — semantic dedup
/// (catching reworded-but-equivalent facts) is a later vector-search concern.
fn norm_suggestion(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// P3 self-tuning: turn the user's accept/reject history per learning kind into
/// an advisory line for the post-mortem prompt, so the proposer offers fewer of
/// a kind they reliably reject (and leans into ones they accept). Pure. Empty
/// unless a kind hits an extreme accept-rate with enough decisions. It only
/// shapes what the proposer *suggests* — the user still reviews every one.
fn calibration_hint(decisions: &[(String, String)]) -> String {
    use std::collections::HashMap;
    let mut by_kind: HashMap<&str, (i64, i64)> = HashMap::new(); // (accepted, total)
    for (kind, status) in decisions {
        let e = by_kind.entry(kind.as_str()).or_insert((0, 0));
        e.1 += 1;
        if status == "accepted" {
            e.0 += 1;
        }
    }
    let mut kinds: Vec<&str> = by_kind.keys().copied().collect();
    kinds.sort(); // deterministic output
    let mut lines: Vec<String> = Vec::new();
    for k in kinds {
        let (acc, tot) = by_kind[k];
        if tot < 4 {
            continue;
        }
        let rate = acc * 100 / tot;
        if rate <= 25 {
            lines.push(format!(
                "- The user has rejected most \"{k}\" suggestions ({acc}/{tot} accepted) — only propose a \"{k}\" when highly confident."
            ));
        } else if rate >= 80 {
            lines.push(format!(
                "- The user accepts most \"{k}\" suggestions ({acc}/{tot}) — those are welcome."
            ));
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!(
            "Calibration from this user's past decisions:\n{}\n\n",
            lines.join("\n")
        )
    }
}

async fn validated_postmortem_cwd(
    pool: &SqlitePool,
    session_id: &str,
    requested_cwd: &str,
) -> Result<Option<String>, AppError> {
    let session: Option<(String, String)> =
        sqlx::query_as("SELECT cwd, kind FROM sessions WHERE id = ?")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;
    let (canonical_cwd, kind) = session.ok_or_else(|| {
        AppError::Other(format!("session {session_id} not found for post-mortem"))
    })?;
    if kind == "anonymous" {
        return Ok(None);
    }
    if canonical_cwd != requested_cwd {
        return Err(AppError::Other(format!(
            "post-mortem scope mismatch for session {session_id}"
        )));
    }
    Ok(Some(canonical_cwd))
}

/// Run a single post-mortem pass over a finished session. The model is given
/// what's already known so it won't repeat it, exact-duplicate proposals are
/// dropped on insert, and contradictions are flagged in the suggestion text.
/// Failure is logged but never propagated to the caller — post-mortem is
/// best-effort and should never break a successful run.
#[command]
pub async fn run_postmortem(
    session_id: String,
    cwd: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<LearningEvent>, AppError> {
    // Remote model-based post-mortem is privacy-sensitive and opt-in. Keep the
    // command itself gated so direct invocations cannot bypass the frontend
    // trigger checks while settings are still loading or after they change.
    if !state.settings.read().await.remote_postmortem_enabled {
        return Ok(vec![]);
    }

    let pool = state.db.read().await;
    let Some(cwd) = validated_postmortem_cwd(&pool, &session_id, &cwd).await? else {
        return Ok(vec![]);
    };

    // ── Gather a tiny summary: task titles + statuses + first 80 chars
    //    of result/error. Keeps the prompt input under ~500 tokens
    //    regardless of how many tasks ran.
    let rows: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT title, status, result, error FROM task_runs \
         WHERE session_id = ? ORDER BY created_at ASC",
    )
    .bind(&session_id)
    .fetch_all(&*pool)
    .await?;
    // A normal project/Quick chat usually has no task_runs. Preserve a small,
    // redacted conversation sample so the chat-end trigger has real evidence
    // instead of returning an unconditional empty result.
    let messages: Vec<(String, String)> = sqlx::query_as(
        "SELECT role, content FROM (\
            SELECT role, content, created_at, rowid AS message_rowid FROM messages \
            WHERE session_id = ? AND role IN ('user','assistant') \
            ORDER BY created_at DESC, rowid DESC LIMIT 12\
         ) ORDER BY created_at ASC, message_rowid ASC",
    )
    .bind(&session_id)
    .fetch_all(&*pool)
    .await
    .unwrap_or_default();
    let tools: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT tc.tool_name, tc.status, tc.error FROM tool_calls tc \
         JOIN messages m ON m.id = tc.message_id \
         WHERE m.session_id = ? AND tc.status IN ('done','error','denied','cancelled') \
         ORDER BY tc.created_at ASC LIMIT 100",
    )
    .bind(&session_id)
    .fetch_all(&*pool)
    .await
    .unwrap_or_default();
    // Existing learnings for this project — lets the model avoid repeating what
    // it already knows (folded into the prompt below) and lets us drop exact
    // duplicates defensively on insert.
    let existing: Vec<(String,)> = sqlx::query_as(
        "SELECT suggestion FROM learning_events \
         WHERE cwd = ? AND status IN ('accepted', 'pending') \
         ORDER BY decided_at DESC, created_at DESC LIMIT 40",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await
    .unwrap_or_default();
    // P3 self-tuning: the user's accept/reject history per kind, to calibrate
    // what the proposer offers (fewer of a kind they reliably reject).
    let decisions: Vec<(String, String)> = sqlx::query_as(
        "SELECT kind, status FROM learning_events \
         WHERE cwd = ? AND status IN ('accepted', 'rejected')",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await
    .unwrap_or_default();
    drop(pool);

    let summary = build_postmortem_summary(&rows, &messages, &tools);
    if summary.is_empty() {
        return Ok(vec![]);
    }

    // What we already know — folded into the prompt (avoid repeats / flag
    // contradictions) and into a dedup set that guards the insert below.
    let known_suggestions: Vec<String> = existing
        .iter()
        .map(|(s,)| crate::trajectory::redact_text(s.trim(), 1_000))
        .filter(|s| !s.is_empty())
        .collect();
    let mut seen: std::collections::HashSet<String> = known_suggestions
        .iter()
        .map(|s| norm_suggestion(s))
        .collect();
    let known_block = if known_suggestions.is_empty() {
        String::new()
    } else {
        format!(
            "Already known about this user — do NOT repeat any of these. If a new \
observation CONTRADICTS one, still report it but prefix its suggestion with \
\"⚠️ 与现有冲突: <the conflicting fact>\":\n{}\n\n",
            known_suggestions
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let calibration = calibration_hint(&decisions);

    // ── Build prompt
    let settings = state.settings.read().await.clone();
    let ep_name = &settings.default_endpoint;
    let model = resolve_postmortem_model(&settings).ok_or_else(|| {
        AppError::Other(format!(
            "No post-mortem model configured for endpoint '{ep_name}'"
        ))
    })?;
    let endpoint = settings
        .endpoints
        .get(ep_name)
        .ok_or_else(|| AppError::Other(format!("Endpoint '{}' not configured", ep_name)))?;
    let api_key = if let Some(ref key_ref) = endpoint.key_ref {
        crate::secrets::get_key(key_ref)
            .map_err(|e| AppError::Other(format!("Failed to load API key: {e}")))?
            .unwrap_or_default()
    } else {
        String::new()
    };
    let prompt = format!(
        "You just finished a session for a user. Reflect on the task outcomes below and \
identify 0-3 NON-OBVIOUS observations about how this user works that would help future sessions. \
Skip obvious things like \"the user wants working code\". Look for patterns: preferred libraries, \
style choices, repeated mistakes worth avoiding, missing tests they had to ask for, etc.\n\n\
Each observation should be classified as ONE of:\n\
  - \"memory\"     — a free-form fact / rule of thumb to remember (e.g. \"this project uses pnpm not npm\")\n\
  - \"preference\" — a STRUCTURED user preference. Use this for stable per-user behavioural \
    choices like autonomy level, communication style, testing habit, code style. Pick a snake_case \
    pref_key (reuse existing if applicable: autonomy_level, communication_style, testing_habit, \
    code_style) and a short pref_value (e.g. \"high\", \"verbose\", \"tdd\", \"prefer arrow fns\").\n\n\
Return ONLY a JSON array (no markdown fences). Each entry has:\n\
  - observation  (what you noticed)\n\
  - suggestion   (one-line human-readable summary, shown in the UI)\n\
  - kind         (\"memory\" or \"preference\")\n\
  - pref_key     (snake_case key, REQUIRED when kind=\"preference\")\n\
  - pref_value   (short value string, REQUIRED when kind=\"preference\")\n\n\
If nothing notable, return [].\n\n\
Examples:\n\
[\n\
  {{\"observation\": \"User asked me to add tests after every implementation.\", \
\"suggestion\": \"Use TDD by default.\", \"kind\": \"preference\", \
\"pref_key\": \"testing_habit\", \"pref_value\": \"tdd\"}},\n\
  {{\"observation\": \"This project uses pnpm not npm.\", \
\"suggestion\": \"This project uses pnpm — never run npm commands here.\", \"kind\": \"memory\"}}\n\
]\n\n\
{calibration}{known_block}Evidence from this session:\n{summary}"
    );
    let text = match endpoint.api_style {
        ApiStyle::Openai => {
            let base_url = endpoint.base_url.trim_end_matches('/');
            let url = format!("{base_url}/chat/completions");
            let req = AiRequest {
                model: model.clone(),
                messages: vec![AiMessage {
                    role: "user",
                    content: prompt.clone(),
                }],
                stream: false,
                temperature: 0.3,
                max_tokens: 500,
            };
            let mut body = match serde_json::to_value(&req) {
                Ok(body) => body,
                Err(error) => {
                    tracing::warn!("postmortem serialize failed: {error}");
                    return Ok(vec![]);
                }
            };
            let client = Client::new();
            let response = match crate::http_util::post_chat_completions(
                &client,
                &url,
                &api_key,
                &mut body,
            )
            .await
            {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!("postmortem request failed: {error}");
                    return Ok(vec![]);
                }
            };
            let response: AiResponse = match response.json().await {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!("postmortem JSON parse failed: {error}");
                    return Ok(vec![]);
                }
            };
            let mut completion = extract_postmortem_completion(response);
            if completion.text.trim().is_empty() {
                tracing::warn!(
                    "postmortem returned no final content (finish_reason={:?}, reasoning_present={}); retrying with expanded completion budget",
                    completion.finish_reason,
                    completion.reasoning_present
                );
                expand_postmortem_completion_budget(&mut body);
                let retry_response = match crate::http_util::post_chat_completions(
                    &client,
                    &url,
                    &api_key,
                    &mut body,
                )
                .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!("postmortem expanded-budget retry failed: {error}");
                        return Ok(vec![]);
                    }
                };
                let retry_response: AiResponse = match retry_response.json().await {
                    Ok(response) => response,
                    Err(error) => {
                        tracing::warn!("postmortem expanded-budget JSON parse failed: {error}");
                        return Ok(vec![]);
                    }
                };
                completion = extract_postmortem_completion(retry_response);
            }
            if completion.text.trim().is_empty() {
                tracing::warn!(
                    "postmortem produced no final content after bounded retry (finish_reason={:?}, reasoning_present={})",
                    completion.finish_reason,
                    completion.reasoning_present
                );
                return Ok(vec![]);
            }
            completion.text
        }
        ApiStyle::Anthropic => {
            let messages = || {
                vec![OneShotAiMessage {
                    role: "user".into(),
                    content: prompt.clone(),
                }]
            };
            let mut completion = match run_one_shot_text(
                &endpoint.base_url,
                &api_key,
                &model,
                &endpoint.api_style,
                messages(),
                500,
                0.3,
            )
            .await
            {
                Ok(completion) => completion,
                Err(error) => {
                    tracing::warn!("postmortem request failed: {error}");
                    return Ok(vec![]);
                }
            };
            if completion.trim().is_empty() {
                completion = match run_one_shot_text(
                    &endpoint.base_url,
                    &api_key,
                    &model,
                    &endpoint.api_style,
                    messages(),
                    2_000,
                    0.3,
                )
                .await
                {
                    Ok(completion) => completion,
                    Err(error) => {
                        tracing::warn!("postmortem expanded-budget retry failed: {error}");
                        return Ok(vec![]);
                    }
                };
            }
            if completion.trim().is_empty() {
                tracing::warn!("postmortem produced no final content after bounded retry");
                return Ok(vec![]);
            }
            completion
        }
        ApiStyle::Chatgpt => {
            tracing::warn!("postmortem does not support ChatGPT endpoints");
            return Ok(vec![]);
        }
    };
    let trimmed = text.trim();
    let json_str = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```").trim())
        .unwrap_or(trimmed);

    let entries: Vec<PostmortemEntry> = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            let preview = crate::trajectory::redact_text(json_str, 500);
            tracing::warn!("postmortem parse failed: {e}, redacted preview: {preview}");
            return Ok(vec![]);
        }
    };

    // ── Persist as pending learning events
    let pool = state.db.read().await;
    let created = persist_postmortem_entries(&pool, &session_id, &cwd, entries, &mut seen).await?;

    // Notify UI so the Profile page + Workspace "记忆增量" panel can
    // refresh without polling. Per-cwd channel so two open projects
    // don't interfere. Best-effort — emit failures are non-fatal.
    if !created.is_empty() {
        let event = format!("learning_events_updated:{}", cwd);
        if let Err(e) = app.emit(&event, &created) {
            tracing::warn!("emit {} failed: {}", event, e);
        }
    }

    Ok(created)
}

fn build_postmortem_summary(
    tasks: &[(String, String, Option<String>, Option<String>)],
    messages: &[(String, String)],
    tools: &[(String, String, Option<String>)],
) -> String {
    let mut lines = Vec::new();
    if !tasks.is_empty() {
        lines.push("Task outcomes:".to_string());
        lines.extend(
            tasks
                .iter()
                .enumerate()
                .map(|(index, (title, status, result, error))| {
                    let raw_outcome = match status.as_str() {
                        "completed" => result.as_deref().unwrap_or(""),
                        "failed" => error.as_deref().unwrap_or(""),
                        other => other,
                    };
                    let outcome = crate::trajectory::redact_text(raw_outcome, 80);
                    let prefix = if status == "failed" { "FAIL: " } else { "" };
                    format!(
                        "{}. [{}] {} — {prefix}{outcome}",
                        index + 1,
                        status,
                        crate::trajectory::redact_text(title, 100),
                    )
                }),
        );
    } else if !messages.is_empty() {
        lines.push("Conversation turns (bounded, redacted):".to_string());
        lines.extend(messages.iter().enumerate().map(|(index, (role, content))| {
            format!(
                "{}. [{}] {}",
                index + 1,
                role,
                crate::trajectory::redact_text(content, 160)
            )
        }));
    }

    if !tools.is_empty() {
        use std::collections::BTreeMap;
        let mut counts: BTreeMap<(&str, &str), i64> = BTreeMap::new();
        for (name, status, _) in tools {
            *counts.entry((name.as_str(), status.as_str())).or_default() += 1;
        }
        lines.push("Tool outcomes:".to_string());
        lines.extend(
            counts
                .into_iter()
                .map(|((name, status), count)| format!("- {name}: {status} × {count}")),
        );
        if let Some((name, _, Some(error))) = tools.iter().find(|(_, status, _)| status == "error")
        {
            lines.push(format!(
                "- sample error from {name}: {}",
                crate::trajectory::redact_text(error, 100)
            ));
        }
    }

    lines.join("\n")
}

// ── Self-evolution P1: cross-session pattern mining ───────────────────────────
//
// Per-session post-mortem reflects on ONE session. The miner aggregates the
// outcome data the app already records across MANY sessions for a cwd and turns
// recurring, evidence-backed patterns into kind='pattern' learnings that flow to
// chat via the same A1–A3 pipeline once accepted.
// See docs/self-evolution/P1-cross-session-pattern-mining.md.

/// A mined pattern, before it becomes a learning_event row. Detectors are pure
/// functions producing these, so they unit-test without a DB or model.
#[derive(Debug, Clone)]
pub struct PatternInsight {
    pub observation: String,
    pub suggestion: String,
    pub support_count: i64,
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ToolCallRow {
    pub session_id: String,
    pub tool_name: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskRow {
    pub session_id: String,
    // Fetched by the task queries but not yet consumed by the retry/pattern
    // detectors (which key off attempt_count + error); kept to match the row shape.
    #[allow(dead_code)]
    pub status: String,
    pub attempt_count: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LearningDecisionRow {
    pub kind: String,
    pub status: String,
}

fn pct(n: i64, d: i64) -> i64 {
    if d == 0 {
        0
    } else {
        (n * 100) / d
    }
}

/// Tools whose recent failure rate is high enough to warn about.
fn detect_tool_reliability(rows: &[ToolCallRow]) -> Vec<PatternInsight> {
    use std::collections::{HashMap, HashSet};
    let mut total: HashMap<&str, i64> = HashMap::new();
    let mut errs: HashMap<&str, i64> = HashMap::new();
    let mut sample: HashMap<&str, String> = HashMap::new();
    let mut sessions: HashMap<&str, HashSet<&str>> = HashMap::new();
    for r in rows {
        if !matches!(r.status.as_str(), "done" | "error") {
            continue;
        }
        *total.entry(r.tool_name.as_str()).or_default() += 1;
        sessions
            .entry(r.tool_name.as_str())
            .or_default()
            .insert(r.session_id.as_str());
        if r.status == "error" {
            *errs.entry(r.tool_name.as_str()).or_default() += 1;
            if let Some(e) = &r.error {
                sample
                    .entry(r.tool_name.as_str())
                    .or_insert_with(|| crate::trajectory::redact_text(e, 80));
            }
        }
    }
    let mut out = Vec::new();
    for (tool, &t) in &total {
        let e = errs.get(tool).copied().unwrap_or(0);
        let rate = pct(e, t);
        let session_count = sessions.get(tool).map_or(0, |items| items.len() as i64);
        if t >= 8 && rate >= 25 && session_count >= 2 {
            let ex = sample.get(tool).cloned().unwrap_or_default();
            let tail = if ex.is_empty() {
                String::new()
            } else {
                format!("，最常见：{ex}")
            };
            out.push(PatternInsight {
                observation: format!("工具 `{tool}` 跨 {session_count} 个 session 的最近 {t} 次调用失败 {e} 次（{rate}%）{tail}。"),
                suggestion: format!(
                    "`{tool}` 近期失败率偏高（{e}/{t}，{rate}%）——调用前先核对前置条件，或考虑替代方案。"
                ),
                support_count: session_count,
                evidence: serde_json::json!({
                    "detector":"tool_reliability",
                    "support_unit":"sessions",
                    "tool":tool,
                    "total":t,
                    "total_calls":t,
                    "errors":e,
                    "rate":rate,
                    "session_count":session_count
                }),
            });
        }
    }
    out.sort_by(|a, b| b.support_count.cmp(&a.support_count));
    out
}

/// A failure that keeps forcing retries across tasks.
fn detect_retry_prone(rows: &[TaskRow]) -> Vec<PatternInsight> {
    use std::collections::{HashMap, HashSet};
    let mut by_err: HashMap<String, (i64, String, HashSet<String>)> = HashMap::new();
    for r in rows {
        if r.attempt_count < 2 {
            continue;
        }
        let raw = crate::trajectory::redact_text(r.error.as_deref().unwrap_or_default(), 120);
        let key = norm_suggestion(&raw.chars().take(50).collect::<String>());
        if key.is_empty() {
            continue;
        }
        let entry =
            by_err
                .entry(key)
                .or_insert((0, raw.chars().take(60).collect(), HashSet::new()));
        entry.0 += 1;
        entry.2.insert(r.session_id.clone());
    }
    let mut out: Vec<PatternInsight> = by_err
        .into_iter()
        .filter(|(_, (count, _, sessions))| *count >= 3 && sessions.len() >= 2)
        .map(|(_, (count, sample, sessions))| PatternInsight {
            observation: format!(
                "跨 {} 个 session 有 {count} 个任务因「{sample}」反复重试。",
                sessions.len()
            ),
            suggestion: format!(
                "反复踩坑：「{sample}」导致多次重试——值得加一道前置检查或固定解法。"
            ),
            support_count: sessions.len() as i64,
            evidence: serde_json::json!({
                "detector":"retry_prone",
                "support_unit":"sessions",
                "task_count":count,
                "session_count":sessions.len(),
                "sample":sample
            }),
        })
        .collect();
    out.sort_by(|a, b| b.support_count.cmp(&a.support_count));
    out
}

/// Calibrate the proposer from the user's accept/reject history.
fn detect_learning_calibration(rows: &[LearningDecisionRow]) -> Vec<PatternInsight> {
    use std::collections::HashMap;
    let mut acc: HashMap<&str, i64> = HashMap::new();
    let mut dec: HashMap<&str, i64> = HashMap::new();
    for r in rows {
        if r.status != "accepted" && r.status != "rejected" {
            continue;
        }
        *dec.entry(r.kind.as_str()).or_default() += 1;
        if r.status == "accepted" {
            *acc.entry(r.kind.as_str()).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for (kind, &d) in &dec {
        if d < 5 {
            continue;
        }
        let a = acc.get(kind).copied().unwrap_or(0);
        let rate = pct(a, d);
        let (obs, sug) = if rate <= 20 {
            (
                format!("你几乎总是拒绝『{kind}』类学习建议（接受 {a}/{d}）。"),
                format!("校准：少提『{kind}』类学习（接受率仅 {rate}%）——除非很有把握。"),
            )
        } else if rate >= 80 {
            (
                format!("你几乎总是接受『{kind}』类学习（接受 {a}/{d}）。"),
                format!("校准：『{kind}』类学习接受率高（{rate}%）——可以多提。"),
            )
        } else {
            continue;
        };
        out.push(PatternInsight {
            observation: obs,
            suggestion: sug,
            support_count: d,
            evidence: serde_json::json!({
                "detector":"learning_calibration",
                "support_unit":"decisions",
                "kind":kind,
                "decided":d,
                "decision_count":d,
                "accepted":a,
                "accept_rate":rate
            }),
        });
    }
    out.sort_by(|a, b| b.support_count.cmp(&a.support_count));
    out
}

/// Run every detector over the supplied rows. Pure; the command wires SQL +
/// persistence around it.
fn run_detectors(
    tools: &[ToolCallRow],
    tasks: &[TaskRow],
    decisions: &[LearningDecisionRow],
) -> Vec<PatternInsight> {
    let mut out = Vec::new();
    out.extend(detect_tool_reliability(tools));
    out.extend(detect_retry_prone(tasks));
    out.extend(detect_learning_calibration(decisions));
    out
}

struct MiningFailure {
    stage: &'static str,
    error: AppError,
}

impl MiningFailure {
    fn new(stage: &'static str, error: impl Into<AppError>) -> Self {
        Self {
            stage,
            error: error.into(),
        }
    }
}

async fn create_analysis_job(pool: &SqlitePool, cwd: &str) -> Result<String, AppError> {
    let job_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO evolution_jobs (id, cwd, trigger, status, owner_pid, owner_start_token, started_at)
         VALUES (?, ?, 'cross_session', 'running', ?, ?, ?)",
    )
    .bind(&job_id)
    .bind(cwd)
    .bind(std::process::id() as i64)
    .bind(crate::storage::db::current_process_start_token())
    .bind(&now)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO evolution_job_events
         (id, cwd, job_id, stage, status, title, detail_json, created_at)
         VALUES (?, ?, ?, 'job', 'started', '跨会话分析开始', ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(cwd)
    .bind(&job_id)
    .bind(redacted_job_detail(serde_json::json!({
        "trigger": "cross_session",
        "raw_prompt_included": false,
        "reasoning_included": false,
    })))
    .bind(&now)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(job_id)
}

async fn mine_cross_session_patterns_in_job(
    cwd: &str,
    job_id: &str,
    pool: &SqlitePool,
) -> Result<Vec<LearningEvent>, MiningFailure> {
    let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE cwd = ?")
        .bind(cwd)
        .fetch_one(pool)
        .await
        .map_err(|error| MiningFailure::new("scope", error))?;
    append_evolution_job_event(
        pool,
        cwd,
        job_id,
        None,
        "scope",
        "completed",
        "分析范围已确定",
        serde_json::json!({"session_count": session_count}),
    )
    .await
    .map_err(|error| MiningFailure::new("scope", error))?;

    // Tool calls in this project (tool_calls → messages → sessions.cwd).
    let tools: Vec<ToolCallRow> = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT s.id, tc.tool_name, tc.status, tc.error
             FROM tool_calls tc
             JOIN messages m ON tc.message_id = m.id
             JOIN sessions s ON m.session_id = s.id
             WHERE s.cwd = ? AND tc.status IN ('done','error')
             ORDER BY tc.created_at DESC LIMIT 4000",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await
    .map_err(|error| MiningFailure::new("trace_read", error))?
    .into_iter()
    .map(|(session_id, tool_name, status, error)| ToolCallRow {
        session_id,
        tool_name,
        status,
        error,
    })
    .collect();

    // Task runs in this project (task_runs → sessions.cwd).
    let tasks: Vec<TaskRow> = sqlx::query_as::<_, (String, String, i64, Option<String>)>(
        "SELECT s.id, t.status, t.attempt_count, t.error
             FROM task_runs t JOIN sessions s ON t.session_id = s.id
             WHERE s.cwd = ? ORDER BY t.created_at DESC LIMIT 2000",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await
    .map_err(|error| MiningFailure::new("trace_read", error))?
    .into_iter()
    .map(|(session_id, status, attempt_count, error)| TaskRow {
        session_id,
        status,
        attempt_count,
        error,
    })
    .collect();

    let decisions: Vec<LearningDecisionRow> = sqlx::query_as::<_, (String, String)>(
        "SELECT kind, status FROM learning_events
             WHERE cwd = ? AND status IN ('accepted','rejected')",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await
    .map_err(|error| MiningFailure::new("trace_read", error))?
    .into_iter()
    .map(|(kind, status)| LearningDecisionRow { kind, status })
    .collect();
    let trace_count = (tools.len() + tasks.len()) as i64;
    sqlx::query(
        "UPDATE evolution_jobs
         SET input_session_count=?, input_trace_count=? WHERE id=? AND cwd=?",
    )
    .bind(session_count)
    .bind(trace_count)
    .bind(job_id)
    .bind(cwd)
    .execute(pool)
    .await
    .map_err(|error| MiningFailure::new("trace_read", error))?;
    append_evolution_job_event(
        pool,
        cwd,
        job_id,
        None,
        "trace_read",
        "completed",
        "轨迹读取完成",
        serde_json::json!({
            "session_count": session_count,
            "trace_count": trace_count,
            "tool_call_count": tools.len(),
            "task_run_count": tasks.len(),
            "decision_count": decisions.len(),
        }),
    )
    .await
    .map_err(|error| MiningFailure::new("trace_read", error))?;
    append_evolution_job_event(
        pool,
        cwd,
        job_id,
        None,
        "privacy",
        "completed",
        "隐私处理完成",
        serde_json::json!({
            "redactor": "trajectory",
            "aggregate_only": true,
            "raw_prompt_included": false,
            "reasoning_included": false,
        }),
    )
    .await
    .map_err(|error| MiningFailure::new("privacy", error))?;

    let existing: Vec<(String,)> = sqlx::query_as(
        "SELECT suggestion FROM learning_events
         WHERE cwd = ? AND status IN ('accepted','pending')",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await
    .map_err(|error| MiningFailure::new("deduplicate", error))?;
    let mut seen: std::collections::HashSet<String> = existing
        .iter()
        .map(|(suggestion,)| norm_suggestion(suggestion))
        .collect();
    let insights = run_detectors(&tools, &tasks, &decisions);
    let extracted_count = insights.len() as i64;
    append_evolution_job_event(
        pool,
        cwd,
        job_id,
        None,
        "extract",
        "completed",
        "候选提取完成",
        serde_json::json!({"extracted_count": extracted_count}),
    )
    .await
    .map_err(|error| MiningFailure::new("extract", error))?;

    let now = Utc::now().to_rfc3339();
    let mut created: Vec<LearningEvent> = Vec::new();
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| MiningFailure::new("deduplicate", error))?;
    for insight in insights {
        if !seen.insert(norm_suggestion(&insight.suggestion)) {
            continue;
        }
        let id = Uuid::new_v4().to_string();
        let evidence = crate::trajectory::redact_json(&insight.evidence).to_string();
        sqlx::query(
            "INSERT INTO learning_events
             (id, session_id, cwd, observation, suggestion, status, created_at, kind,
              support_count, evidence_json, job_id)
             VALUES (?, '', ?, ?, ?, 'pending', ?, 'pattern', ?, ?, ?)",
        )
        .bind(&id)
        .bind(cwd)
        .bind(&insight.observation)
        .bind(&insight.suggestion)
        .bind(&now)
        .bind(insight.support_count)
        .bind(&evidence)
        .bind(job_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| MiningFailure::new("deduplicate", error))?;
        created.push(LearningEvent {
            id,
            session_id: String::new(),
            cwd: cwd.to_string(),
            observation: insight.observation,
            suggestion: insight.suggestion,
            status: "pending".into(),
            created_at: now.clone(),
            decided_at: None,
            kind: "pattern".into(),
            pref_key: None,
            pref_value: None,
            support_count: insight.support_count,
            evidence_json: evidence,
            job_id: Some(job_id.to_string()),
        });
    }
    let candidate_count = created.len() as i64;
    let duplicate_count = extracted_count - candidate_count;
    append_evolution_job_event_in_transaction(
        &mut transaction,
        cwd,
        job_id,
        None,
        "deduplicate",
        "completed",
        "候选去重完成",
        serde_json::json!({
            "extracted_count": extracted_count,
            "duplicate_count": duplicate_count,
            "candidate_count": candidate_count,
        }),
        &now,
    )
    .await
    .map_err(|error| MiningFailure::new("deduplicate", error))?;

    if created.is_empty() {
        append_evolution_job_event_in_transaction(
            &mut transaction,
            cwd,
            job_id,
            None,
            "review",
            "completed",
            "没有新候选需要审核",
            serde_json::json!({"candidate_count": 0}),
            &now,
        )
        .await
        .map_err(|error| MiningFailure::new("review", error))?;
    } else {
        for candidate in &created {
            append_evolution_job_event_in_transaction(
                &mut transaction,
                cwd,
                job_id,
                Some(&candidate.id),
                "review",
                "waiting",
                "等待人工审核",
                serde_json::json!({
                    "candidate_kind": candidate.kind,
                    "support_count": candidate.support_count,
                }),
                &now,
            )
            .await
            .map_err(|error| MiningFailure::new("review", error))?;
        }
    }

    let terminal_status = if created.is_empty() {
        "no_candidates"
    } else {
        "succeeded"
    };
    let completed_at = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE evolution_jobs
         SET status=?, input_session_count=?, input_trace_count=?, candidate_count=?,
             completed_at=?, error=NULL
         WHERE id=? AND cwd=?",
    )
    .bind(terminal_status)
    .bind(session_count)
    .bind(trace_count)
    .bind(candidate_count)
    .bind(&completed_at)
    .bind(job_id)
    .bind(cwd)
    .execute(&mut *transaction)
    .await
    .map_err(|error| MiningFailure::new("job", error))?;
    append_evolution_job_event_in_transaction(
        &mut transaction,
        cwd,
        job_id,
        None,
        "job",
        "completed",
        if created.is_empty() {
            "分析完成，暂无新候选"
        } else {
            "分析完成"
        },
        serde_json::json!({
            "terminal_status": terminal_status,
            "candidate_count": candidate_count,
        }),
        &completed_at,
    )
    .await
    .map_err(|error| MiningFailure::new("job", error))?;
    transaction
        .commit()
        .await
        .map_err(|error| MiningFailure::new("job", error))?;
    Ok(created)
}

async fn mine_cross_session_patterns_for_pool(
    cwd: &str,
    pool: &SqlitePool,
) -> Result<Vec<LearningEvent>, AppError> {
    let job_id = create_analysis_job(pool, cwd).await?;
    match mine_cross_session_patterns_in_job(cwd, &job_id, pool).await {
        Ok(created) => Ok(created),
        Err(failure) => {
            record_job_failure(
                pool,
                cwd,
                &job_id,
                None,
                failure.stage,
                "跨会话分析失败",
                &failure.error,
            )
            .await;
            Err(failure.error)
        }
    }
}

/// Cross-session pattern miner. Aggregates the cwd's recent outcome data, runs
/// the detectors, dedups against existing learnings (A3's norm_suggestion), and
/// inserts the survivors as kind='pattern' pending rows — which accept-route
/// like memory, so an accepted insight reaches chat via A1's injection.
#[command]
pub async fn mine_cross_session_patterns(
    cwd: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<LearningEvent>, AppError> {
    let pool = state.db.read().await;
    let created = mine_cross_session_patterns_for_pool(&cwd, &pool).await?;
    drop(pool);

    if !created.is_empty() {
        let event = format!("learning_events_updated:{}", cwd);
        if let Err(e) = app.emit(&event, &created) {
            tracing::warn!("emit {} failed: {}", event, e);
        }
    }
    Ok(created)
}

// ── Self-evolution P4: self-modification (SAFE foundation only) ────────────────
//
// P4 is "the factory improves its own code" — the boldest, highest-risk phase.
// v1 ships ONLY the read-only foundation: aggregate friction globally and render
// a PROPOSAL for the human. It writes no code, opens no PR, ships nothing. The
// autonomous draft→branch→implement→verify→PR pipeline is deliberately gated and
// NOT built here. See docs/self-evolution/P4-self-modification.md.

/// Render a self-improvement proposal (markdown) from global friction insights.
/// Pure. Its header makes the human-gate explicit: it changes nothing.
fn build_improvement_proposal(
    tool_insights: &[PatternInsight],
    retry_insights: &[PatternInsight],
) -> String {
    let mut md = String::from(
        "# CodeFactory 自我改进提案\n\n\
> 本提案由系统从你的使用数据**只读聚合**生成。它**不修改任何代码、不开 PR、不发布任何版本**\
——一切改动由你决定并经人工审批。\n\n",
    );
    if tool_insights.is_empty() && retry_insights.is_empty() {
        md.push_str("暂未发现明显的反复摩擦点。继续用着，数据多了再来看。\n");
        return md;
    }
    if !tool_insights.is_empty() {
        md.push_str("## 工具可靠性\n");
        for i in tool_insights.iter().take(5) {
            md.push_str(&format!(
                "- {}\n  - 可考虑：在该工具实现里加前置检查 / 更稳的错误处理。\n",
                i.suggestion
            ));
        }
        md.push('\n');
    }
    if !retry_insights.is_empty() {
        md.push_str("## 反复重试的失败\n");
        for i in retry_insights.iter().take(5) {
            md.push_str(&format!(
                "- {}\n  - 可考虑：为这个失败加一道前置检查或固定解法。\n",
                i.suggestion
            ));
        }
        md.push('\n');
    }
    md.push_str(
        "---\n采纳方式：你（或在你审批下的 agent）据此开分支实现、verify、提 PR——系统不会自己动手。\n",
    );
    md
}

/// P4 v1: a read-only self-improvement proposal. Aggregates friction GLOBALLY
/// (all projects) via P1's detectors and renders a markdown proposal for the
/// human. Writes no code, opens no PR, ships nothing.
#[command]
pub async fn self_improvement_proposal(state: State<'_, AppState>) -> Result<String, AppError> {
    let pool = state.db.read().await;
    let tools: Vec<ToolCallRow> = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT m.session_id, tc.tool_name, tc.status, tc.error FROM tool_calls tc \
         JOIN messages m ON m.id = tc.message_id \
         WHERE tc.status IN ('done','error') ORDER BY tc.created_at DESC LIMIT 8000",
    )
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(session_id, tool_name, status, error)| ToolCallRow {
        session_id,
        tool_name,
        status,
        error,
    })
    .collect();
    let tasks: Vec<TaskRow> = sqlx::query_as::<_, (String, String, i64, Option<String>)>(
        "SELECT session_id, status, attempt_count, error FROM task_runs ORDER BY created_at DESC LIMIT 4000",
    )
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(session_id, status, attempt_count, error)| TaskRow {
        session_id,
        status,
        attempt_count,
        error,
    })
    .collect();
    drop(pool);

    Ok(build_improvement_proposal(
        &detect_tool_reliability(&tools),
        &detect_retry_prone(&tasks),
    ))
}

// ── P3 tool-policy: flaky-tool gating proposals ───────────────────────────────
//
// P1 already mines which tools fail a lot. This turns that signal into a SAFE,
// human-gated tweak to the permission policy: propose moving a flaky tool from
// `allow` to `ask` so the agent confirms before running it. It rides the
// existing `decide_permission` — no new enforcement. See
// docs/self-evolution/P3-tool-policy.md.

/// A proposal to gate a flaky tool behind a confirmation prompt. Surfaced
/// read-only; applied only when the human clicks (`apply_tool_gate`).
#[derive(Debug, Clone, Serialize)]
pub struct ToolGateProposal {
    pub tool: String,
    pub total: i64,
    pub errors: i64,
    pub rate: i64,
    pub observation: String,
}

/// Pure: from flaky-tool insights + the current permission allow-list, propose
/// gating the tools that are *currently auto-allowed* — so accepting actually
/// changes behavior (auto-run → confirm). Tools already gated (absent from
/// `allow`) or special-cased (`bash`, which already asks; `skill_*`, always
/// allowed) are skipped. Order follows the detector's worst-first sort.
fn tool_gate_proposals(insights: &[PatternInsight], allow: &[String]) -> Vec<ToolGateProposal> {
    use std::collections::HashSet;
    let allowed: HashSet<&str> = allow.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for ins in insights {
        // Only tool_reliability insights carry a tool name in evidence.
        let Some(tool) = ins.evidence.get("tool").and_then(|v| v.as_str()) else {
            continue;
        };
        if tool == "bash" || tool.starts_with("skill_") {
            continue;
        }
        if !allowed.contains(tool) {
            continue; // already gated — nothing to propose
        }
        let g = |k: &str| {
            ins.evidence
                .get(k)
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
        };
        out.push(ToolGateProposal {
            tool: tool.to_string(),
            total: g("total"),
            errors: g("errors"),
            rate: g("rate"),
            observation: ins.observation.clone(),
        });
    }
    out
}

/// P3 tool-policy v1: read-only. Find flaky tools (P1 detector, global) that are
/// currently auto-allowed and propose gating them. Mutates nothing.
#[command]
pub async fn propose_tool_gates(
    state: State<'_, AppState>,
) -> Result<Vec<ToolGateProposal>, AppError> {
    let pool = state.db.read().await;
    let tools: Vec<ToolCallRow> = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT m.session_id, tc.tool_name, tc.status, tc.error FROM tool_calls tc \
         JOIN messages m ON m.id = tc.message_id \
         WHERE tc.status IN ('done','error') ORDER BY tc.created_at DESC LIMIT 8000",
    )
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(session_id, tool_name, status, error)| ToolCallRow {
        session_id,
        tool_name,
        status,
        error,
    })
    .collect();
    drop(pool);
    let allow = state.settings.read().await.permissions.allow.clone();
    Ok(tool_gate_proposals(
        &detect_tool_reliability(&tools),
        &allow,
    ))
}

/// P3 tool-policy v1: the human-gated enable. Moves `tool` from the permission
/// `allow` list to `ask`, so the existing `decide_permission` now confirms
/// before running it. Persists like `save_settings` (disk + in-memory). Only
/// ever tightens (auto-run → confirm); never grants new access. Idempotent.
#[command]
pub async fn apply_tool_gate(tool: String, state: State<'_, AppState>) -> Result<(), AppError> {
    let mut s = state.settings.read().await.clone();
    let before = s.permissions.allow.len();
    s.permissions.allow.retain(|t| t != &tool);
    let removed = s.permissions.allow.len() != before;
    let added_ask = if s.permissions.ask.iter().any(|t| t == &tool) {
        false
    } else {
        s.permissions.ask.push(tool.clone());
        true
    };
    if removed || added_ask {
        crate::config::settings::save(&s)?;
        *state.settings.write().await = s;
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // Storage-only tests; the post-mortem AI call needs a live endpoint.
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[test]
    fn postmortem_uses_the_active_model_for_the_selected_endpoint() {
        let mut settings = crate::config::Settings::default();
        settings.default_endpoint = "deepseek".into();
        settings.default_model = "gpt-5.5".into();
        settings.endpoints.insert(
            "deepseek".into(),
            crate::config::settings::Endpoint {
                base_url: "https://api.deepseek.com".into(),
                key_ref: Some("codefactory.endpoint.deepseek".into()),
                api_style: crate::config::settings::ApiStyle::Openai,
                custom_models: vec![],
                active_model: Some("deepseek-v4-pro".into()),
            },
        );

        assert_eq!(
            resolve_postmortem_model(&settings).as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn postmortem_response_never_uses_reasoning_as_candidate_content() {
        let response: AiResponse = serde_json::from_value(serde_json::json!({
            "choices": [{
                "finish_reason": "length",
                "message": {
                    "content": null,
                    "reasoning_content": "private chain of thought"
                }
            }]
        }))
        .unwrap();

        let completion = extract_postmortem_completion(response);

        assert!(completion.text.is_empty());
        assert!(completion.reasoning_present);
        assert_eq!(completion.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn postmortem_retry_expands_whichever_completion_budget_field_is_active() {
        let mut max_tokens = serde_json::json!({"max_tokens": 500});
        expand_postmortem_completion_budget(&mut max_tokens);
        assert_eq!(max_tokens["max_tokens"], 2_000);
        assert!(max_tokens.get("max_completion_tokens").is_none());

        let mut max_completion_tokens = serde_json::json!({"max_completion_tokens": 500});
        expand_postmortem_completion_budget(&mut max_completion_tokens);
        assert_eq!(max_completion_tokens["max_completion_tokens"], 2_000);
        assert!(max_completion_tokens.get("max_tokens").is_none());
    }

    #[test]
    fn postmortem_candidates_are_redacted_before_dedup_and_storage() {
        let entry = PostmortemEntry {
            observation: r#"Model echoed {"token":"CF_EVO_CANDIDATE_TOKEN"}"#.into(),
            suggestion: "Remember password=CF_EVO_CANDIDATE_PASSWORD".into(),
            kind: Some("preference".into()),
            pref_key: Some("testing_habit".into()),
            pref_value: Some("Bearer CF_EVO_CANDIDATE_BEARER".into()),
        };

        let sanitized = sanitize_postmortem_entry(entry).unwrap();
        let serialized = serde_json::to_string(&serde_json::json!({
            "observation": sanitized.observation,
            "suggestion": sanitized.suggestion,
            "pref_key": sanitized.pref_key,
            "pref_value": sanitized.pref_value,
        }))
        .unwrap();

        assert!(!serialized.contains("CF_EVO_CANDIDATE_TOKEN"));
        assert!(!serialized.contains("CF_EVO_CANDIDATE_PASSWORD"));
        assert!(!serialized.contains("CF_EVO_CANDIDATE_BEARER"));
        assert!(serialized.contains("<redacted>"));
    }

    #[test]
    fn postmortem_invalid_preference_key_downgrades_to_memory() {
        let entry = PostmortemEntry {
            observation: "Observed a stable preference".into(),
            suggestion: "Remember the preference safely".into(),
            kind: Some("preference".into()),
            pref_key: Some("bad-key\nSYSTEM override".into()),
            pref_value: Some("unsafe value".into()),
        };

        let sanitized = sanitize_postmortem_entry(entry).unwrap();

        assert_eq!(sanitized.kind.as_deref(), Some("memory"));
        assert!(sanitized.pref_key.is_none());
        assert!(sanitized.pref_value.is_none());
    }

    #[tokio::test]
    async fn postmortem_storage_keeps_model_candidates_pending_and_redacted() {
        let pool = fresh_miner_pool().await;
        let entries = vec![PostmortemEntry {
            observation: r#"Observed {"token":"CF_EVO_STORED_TOKEN"}"#.into(),
            suggestion: "Remember password=CF_EVO_STORED_PASSWORD".into(),
            kind: Some("preference".into()),
            pref_key: Some("testing_habit".into()),
            pref_value: Some("Bearer CF_EVO_STORED_BEARER".into()),
        }];
        let mut seen = std::collections::HashSet::new();

        let created =
            persist_postmortem_entries(&pool, "session-postmortem", "/proj", entries, &mut seen)
                .await
                .unwrap();

        assert_eq!(created.len(), 1);
        assert_eq!(created[0].status, "pending");
        let row: (String, String, String, Option<String>) = sqlx::query_as(
            "SELECT observation, suggestion, status, pref_value FROM learning_events",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let serialized = serde_json::to_string(&row).unwrap();
        assert!(!serialized.contains("CF_EVO_STORED_TOKEN"));
        assert!(!serialized.contains("CF_EVO_STORED_PASSWORD"));
        assert!(!serialized.contains("CF_EVO_STORED_BEARER"));
        assert!(serialized.contains("<redacted>"));
        assert_eq!(row.2, "pending");
    }

    #[test]
    fn postmortem_summary_falls_back_to_redacted_chat_and_tool_outcomes() {
        let summary = build_postmortem_summary(
            &[],
            &[
                (
                    "user".into(),
                    "Use token=super-secret and please add tests".into(),
                ),
                ("assistant".into(), "Done".into()),
            ],
            &[
                ("read_file".into(), "done".into(), None),
                (
                    "bash".into(),
                    "error".into(),
                    Some("Bearer error-secret failed".into()),
                ),
            ],
        );
        assert!(summary.contains("Conversation turns"));
        assert!(summary.contains("please add tests"));
        assert!(summary.contains("bash: error × 1"));
        assert!(!summary.contains("super-secret"));
        assert!(!summary.contains("error-secret"));
    }

    #[test]
    fn postmortem_summary_prefers_task_outcomes_when_available() {
        let summary = build_postmortem_summary(
            &[(
                "Run release".into(),
                "failed".into(),
                None,
                Some("password=hunter2".into()),
            )],
            &[("user".into(), "chat fallback should not appear".into())],
            &[],
        );
        assert!(summary.contains("Task outcomes"));
        assert!(summary.contains("Run release"));
        assert!(summary.contains("FAIL"));
        assert!(!summary.contains("hunter2"));
        assert!(!summary.contains("chat fallback should not appear"));
    }

    #[test]
    fn improvement_proposal_is_read_only_and_lists_friction() {
        // Empty → states no friction, still carries the no-mutation header.
        let empty = build_improvement_proposal(&[], &[]);
        assert!(
            empty.contains("不修改任何代码"),
            "must state it changes nothing"
        );
        assert!(empty.contains("暂未发现"));
        // With friction → lists it + keeps the human-gate footer.
        let tool = PatternInsight {
            observation: "o".into(),
            suggestion: "工具 `bash` 失败率偏高".into(),
            support_count: 10,
            evidence: serde_json::json!({}),
        };
        let md = build_improvement_proposal(&[tool], &[]);
        assert!(md.contains("## 工具可靠性"));
        assert!(md.contains("工具 `bash` 失败率偏高"));
        assert!(md.contains("系统不会自己动手"));
    }

    #[test]
    fn tool_gate_only_proposes_currently_allowed_flaky_tools() {
        // edit_file: 10 calls, 4 errors (40%) → flaky; and it's in `allow`.
        // flaky_gated: 9 calls, all errors → flaky, but NOT in `allow` (already gated).
        // bash: 10 calls, all errors → flaky + "allowed", but special-cased (already asks).
        let mut rows = Vec::new();
        for i in 0..6 {
            rows.push(tc(
                if i % 2 == 0 { "s1" } else { "s2" },
                "edit_file",
                "done",
                None,
            ));
        }
        for i in 0..4 {
            rows.push(tc(
                if i % 2 == 0 { "s1" } else { "s2" },
                "edit_file",
                "error",
                Some("boom"),
            ));
        }
        for i in 0..9 {
            rows.push(tc(
                if i % 2 == 0 { "s1" } else { "s2" },
                "flaky_gated",
                "error",
                Some("x"),
            ));
        }
        for i in 0..10 {
            rows.push(tc(
                if i % 2 == 0 { "s1" } else { "s2" },
                "bash",
                "error",
                Some("e"),
            ));
        }
        let insights = detect_tool_reliability(&rows);
        let allow = vec![
            "edit_file".to_string(),
            "bash".to_string(),
            "read_file".to_string(),
        ];

        let proposals = tool_gate_proposals(&insights, &allow);

        // Only edit_file: flaky AND currently allowed AND not special-cased.
        assert_eq!(
            proposals.len(),
            1,
            "only currently-allowed, non-special flaky tools"
        );
        let p = &proposals[0];
        assert_eq!(p.tool, "edit_file");
        assert_eq!(p.total, 10);
        assert_eq!(p.errors, 4);
        assert_eq!(p.rate, 40);
        // flaky_gated is flaky but already gated (absent from `allow`) → skipped.
        assert!(proposals.iter().all(|q| q.tool != "flaky_gated"));
        // bash is flaky + "allowed" but already asks → never proposed.
        assert!(proposals.iter().all(|q| q.tool != "bash"));
    }

    #[test]
    fn norm_suggestion_folds_case_and_whitespace_for_dedup() {
        // Trivial rewordings normalize to the same key…
        assert_eq!(
            norm_suggestion("  Use  pnpm  "),
            norm_suggestion("use pnpm")
        );
        assert_eq!(
            norm_suggestion("Use TDD by default."),
            "use tdd by default."
        );
        // …but genuinely different facts do not collide.
        assert_ne!(norm_suggestion("use pnpm"), norm_suggestion("use npm"));
    }

    #[test]
    fn dedup_set_drops_repeats_keeps_new() {
        let existing = ["Use pnpm not npm.", "Prefer TDD."];
        let mut seen: std::collections::HashSet<String> =
            existing.iter().map(|s| norm_suggestion(s)).collect();
        // A reworded duplicate of an existing learning is rejected.
        assert!(!seen.insert(norm_suggestion("use   pnpm not npm.")));
        // A brand-new learning is accepted (and now itself guards repeats).
        assert!(seen.insert(norm_suggestion("This project deploys via GitHub Actions.")));
        assert!(!seen.insert(norm_suggestion("this project deploys via github actions.")));
    }

    fn decs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, s)| (k.to_string(), s.to_string()))
            .collect()
    }

    #[test]
    fn calibration_hint_fires_only_at_extremes() {
        let d = decs(&[
            // preference 1/5 = 20% → reject hint
            ("preference", "rejected"),
            ("preference", "rejected"),
            ("preference", "rejected"),
            ("preference", "rejected"),
            ("preference", "accepted"),
            // memory 5/6 = 83% → welcome hint
            ("memory", "accepted"),
            ("memory", "accepted"),
            ("memory", "accepted"),
            ("memory", "accepted"),
            ("memory", "accepted"),
            ("memory", "rejected"),
            // pattern 2/3 → below 4-decision threshold → silent
            ("pattern", "accepted"),
            ("pattern", "accepted"),
            ("pattern", "rejected"),
        ]);
        let hint = calibration_hint(&d);
        assert!(hint.contains("rejected most \"preference\""), "got: {hint}");
        assert!(hint.contains("accepts most \"memory\""), "got: {hint}");
        assert!(
            !hint.contains("pattern"),
            "below-threshold kind stays silent: {hint}"
        );
    }

    #[test]
    fn calibration_hint_empty_when_no_extreme_or_too_few() {
        assert_eq!(
            calibration_hint(&decs(&[("memory", "accepted"), ("memory", "rejected")])),
            ""
        );
        // 50/50 with enough decisions is not an extreme → still empty.
        assert_eq!(
            calibration_hint(&decs(&[
                ("memory", "accepted"),
                ("memory", "rejected"),
                ("memory", "accepted"),
                ("memory", "rejected"),
            ])),
            ""
        );
    }

    fn tc(session_id: &str, name: &str, status: &str, err: Option<&str>) -> ToolCallRow {
        ToolCallRow {
            session_id: session_id.into(),
            tool_name: name.into(),
            status: status.into(),
            error: err.map(Into::into),
        }
    }

    #[test]
    fn tool_reliability_flags_only_high_volume_high_error_tools() {
        let mut rows = Vec::new();
        // flaky: 10 calls, 4 errors (40%) → flagged.
        for i in 0..10 {
            rows.push(tc(
                if i % 2 == 0 { "s1" } else { "s2" },
                "bash",
                if i < 4 { "error" } else { "done" },
                Some("pwsh not found"),
            ));
        }
        // reliable: 12 calls, 1 error (8%) → not flagged.
        for i in 0..12 {
            rows.push(tc(
                if i % 2 == 0 { "s1" } else { "s2" },
                "read_file",
                if i < 1 { "error" } else { "done" },
                None,
            ));
        }
        // flaky but low-volume: 5 calls, 3 errors → not flagged (< 8 calls).
        for i in 0..5 {
            rows.push(tc(
                if i % 2 == 0 { "s1" } else { "s2" },
                "write_xlsx",
                if i < 3 { "error" } else { "done" },
                None,
            ));
        }

        let out = detect_tool_reliability(&rows);
        assert_eq!(out.len(), 1, "only the high-volume flaky tool is flagged");
        assert!(out[0].suggestion.contains("bash"));
        assert_eq!(out[0].support_count, 2);
        assert!(out[0].evidence.get("rate").and_then(|v| v.as_i64()) == Some(40));
    }

    #[test]
    fn retry_prone_groups_by_error_and_needs_three() {
        let rows = vec![
            // Same recurring failure (case/whitespace fold to one key) on retries.
            TaskRow {
                session_id: "s1".into(),
                status: "completed".into(),
                attempt_count: 3,
                error: Some("schannel: server closed abruptly".into()),
            },
            TaskRow {
                session_id: "s2".into(),
                status: "completed".into(),
                attempt_count: 2,
                error: Some("schannel: server closed abruptly".into()),
            },
            TaskRow {
                session_id: "s3".into(),
                status: "failed".into(),
                attempt_count: 4,
                error: Some("Schannel:  server  closed  abruptly".into()),
            },
            // single-attempt → ignored even though same error.
            TaskRow {
                session_id: "s1".into(),
                status: "completed".into(),
                attempt_count: 1,
                error: Some("schannel: server closed abruptly".into()),
            },
            // a different one-off retry error → its own group, below threshold.
            TaskRow {
                session_id: "s4".into(),
                status: "failed".into(),
                attempt_count: 2,
                error: Some("totally different".into()),
            },
        ];
        let out = detect_retry_prone(&rows);
        assert_eq!(
            out.len(),
            1,
            "only the 3x recurring retry error is surfaced"
        );
        assert_eq!(out[0].support_count, 3);
    }

    #[test]
    fn learning_calibration_emits_at_extremes_only() {
        let mut rows = Vec::new();
        // memory: 6 decided, 5 accepted (83%) → "propose more".
        for i in 0..6 {
            rows.push(LearningDecisionRow {
                kind: "memory".into(),
                status: if i < 5 { "accepted" } else { "rejected" }.into(),
            });
        }
        // preference: 6 decided, 1 accepted (17%) → "propose less".
        for i in 0..6 {
            rows.push(LearningDecisionRow {
                kind: "preference".into(),
                status: if i < 1 { "accepted" } else { "rejected" }.into(),
            });
        }
        // pattern: only 4 decided → below threshold, no insight.
        for _ in 0..4 {
            rows.push(LearningDecisionRow {
                kind: "pattern".into(),
                status: "accepted".into(),
            });
        }

        let out = detect_learning_calibration(&rows);
        assert_eq!(out.len(), 2);
        assert!(out
            .iter()
            .any(|p| p.suggestion.contains("memory") && p.suggestion.contains("可以多提")));
        assert!(out
            .iter()
            .any(|p| p.suggestion.contains("preference") && p.suggestion.contains("少提")));
    }

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE learning_events (
                id TEXT PRIMARY KEY, session_id TEXT, cwd TEXT,
                observation TEXT, suggestion TEXT, status TEXT,
                created_at TEXT, decided_at TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn insert_and_filter_pending_only() {
        let pool = fresh_pool().await;
        let now = Utc::now().to_rfc3339();
        for (id, status) in [("a", "pending"), ("b", "accepted"), ("c", "pending")] {
            sqlx::query(
                "INSERT INTO learning_events (id, session_id, cwd, observation, suggestion, status, created_at) \
                 VALUES (?, 's1', '/proj', 'obs', 'sug', ?, ?)",
            )
            .bind(id).bind(status).bind(&now)
            .execute(&pool).await.unwrap();
        }
        let pending: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM learning_events WHERE cwd = '/proj' AND status = 'pending'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.iter().any(|(id,)| id == "a"));
        assert!(pending.iter().any(|(id,)| id == "c"));
    }

    async fn fresh_miner_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, cwd TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'project'
            )",
            "CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT NOT NULL)",
            "CREATE TABLE tool_calls (
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL, tool_name TEXT NOT NULL,
                status TEXT NOT NULL, error TEXT, created_at INTEGER NOT NULL
            )",
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, status TEXT NOT NULL,
                attempt_count INTEGER NOT NULL, error TEXT, created_at TEXT NOT NULL
            )",
            "CREATE TABLE learning_events (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, cwd TEXT NOT NULL,
                observation TEXT NOT NULL, suggestion TEXT NOT NULL, status TEXT NOT NULL,
                created_at TEXT NOT NULL, decided_at TEXT, kind TEXT NOT NULL DEFAULT 'memory',
                pref_key TEXT, pref_value TEXT, support_count INTEGER NOT NULL DEFAULT 0,
                evidence_json TEXT NOT NULL DEFAULT '{}', job_id TEXT
            )",
            "CREATE TABLE evolution_jobs (
                id TEXT PRIMARY KEY, cwd TEXT NOT NULL, trigger TEXT NOT NULL,
                candidate_id TEXT, status TEXT NOT NULL, idempotency_key TEXT,
                input_session_count INTEGER NOT NULL DEFAULT 0,
                input_trace_count INTEGER NOT NULL DEFAULT 0,
                candidate_count INTEGER NOT NULL DEFAULT 0,
                started_at TEXT NOT NULL, completed_at TEXT, error TEXT,
                owner_pid INTEGER, owner_start_token TEXT
            )",
            "CREATE TABLE evolution_job_events (
                id TEXT PRIMARY KEY, cwd TEXT NOT NULL, job_id TEXT NOT NULL,
                candidate_id TEXT, stage TEXT NOT NULL, status TEXT NOT NULL,
                title TEXT NOT NULL, detail_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            )",
            "CREATE TABLE user_preferences (
                cwd TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'user', updated_at TEXT NOT NULL,
                PRIMARY KEY (cwd, key)
            )",
            "CREATE TABLE improvement_candidates (
                id TEXT PRIMARY KEY, cwd TEXT NOT NULL, kind TEXT NOT NULL,
                source_learning_event_id TEXT UNIQUE, current_revision INTEGER NOT NULL,
                current_state TEXT NOT NULL, state_version INTEGER NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            )",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "CREATE UNIQUE INDEX idx_evolution_jobs_candidate_running
             ON evolution_jobs(candidate_id)
             WHERE candidate_id IS NOT NULL AND status = 'running'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE UNIQUE INDEX idx_evolution_jobs_scope_analysis_running
             ON evolution_jobs(cwd)
             WHERE trigger = 'cross_session' AND status = 'running'",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn learning_list_keeps_every_pending_candidate_and_bounds_decision_history() {
        let pool = fresh_miner_pool().await;
        for index in 0..55 {
            sqlx::query(
                "INSERT INTO learning_events
                 (id, session_id, cwd, observation, suggestion, status, created_at)
                 VALUES (?, '', '/proj', 'pending observation', 'pending suggestion', 'pending', ?)",
            )
            .bind(format!("pending-{index}"))
            .bind(format!("pending-{index:03}"))
            .execute(&pool)
            .await
            .unwrap();
        }
        for index in 0..120 {
            sqlx::query(
                "INSERT INTO learning_events
                 (id, session_id, cwd, observation, suggestion, status, created_at, decided_at)
                 VALUES (?, '', '/proj', 'decided observation', 'decided suggestion', 'accepted', ?, ?)",
            )
            .bind(format!("decided-{index}"))
            .bind(format!("decided-{index:03}"))
            .bind(format!("decided-{index:03}"))
            .execute(&pool)
            .await
            .unwrap();
        }

        let events = list_learning_events_for_pool("/proj", &pool).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.status == "pending")
                .count(),
            55
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.status != "pending")
                .count(),
            100
        );
        assert!(events
            .iter()
            .take(55)
            .all(|event| event.status == "pending"));
    }

    #[test]
    fn job_detail_drops_unknown_fields_and_bounds_allowed_errors() {
        let detail = redacted_job_detail(serde_json::json!({
            "reason": "process_restart",
            "error": format!("password=CF_EVO_DETAIL_SECRET {}", "x".repeat(500)),
            "unknown_private_field": "must not persist",
        }));
        assert!(detail.contains("process_restart"));
        assert!(detail.contains("<redacted>"));
        assert!(!detail.contains("CF_EVO_DETAIL_SECRET"));
        assert!(!detail.contains("unknown_private_field"));
        let parsed: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert!(parsed["error"].as_str().unwrap().chars().count() <= 160);
    }

    #[tokio::test]
    async fn postmortem_scope_comes_from_the_session_and_rejects_cross_project_input() {
        let pool = fresh_miner_pool().await;
        sqlx::query("INSERT INTO sessions (id, cwd) VALUES ('session-a', '/project-a')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, cwd, kind)
             VALUES ('anonymous-a', '/anonymous', 'anonymous')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            validated_postmortem_cwd(&pool, "session-a", "/project-a")
                .await
                .unwrap()
                .as_deref(),
            Some("/project-a")
        );
        assert!(validated_postmortem_cwd(&pool, "session-a", "/project-b")
            .await
            .is_err());
        assert_eq!(
            validated_postmortem_cwd(&pool, "anonymous-a", "/anonymous")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn miner_requires_real_cross_session_support_and_ignores_non_terminal_rows() {
        let pool = fresh_miner_pool().await;
        for (session_id, cwd) in [("s1", "/proj"), ("s2", "/proj"), ("other", "/other")] {
            sqlx::query("INSERT INTO sessions (id, cwd) VALUES (?, ?)")
                .bind(session_id)
                .bind(cwd)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO messages (id, session_id) VALUES (?, ?)")
                .bind(format!("m-{session_id}"))
                .bind(session_id)
                .execute(&pool)
                .await
                .unwrap();
        }

        let mut created_at = 1_i64;
        for (session_id, status, error) in [
            ("s1", "done", None),
            ("s1", "done", None),
            ("s1", "done", None),
            ("s1", "error", Some("password=CF_EVO_MINER_SECRET")),
            ("s2", "done", None),
            ("s2", "done", None),
            ("s2", "done", None),
            ("s2", "error", Some("password=CF_EVO_MINER_SECRET")),
            ("s1", "denied", Some("user denied")),
            ("s2", "pending", None),
        ] {
            sqlx::query(
                "INSERT INTO tool_calls (id, message_id, tool_name, status, error, created_at) \
                 VALUES (?, ?, 'edit_file', ?, ?, ?)",
            )
            .bind(format!("tc-{created_at}"))
            .bind(format!("m-{session_id}"))
            .bind(status)
            .bind(error)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
            created_at += 1;
        }

        // Eight errors in one session must not be called a cross-session pattern.
        for index in 0..8 {
            sqlx::query(
                "INSERT INTO tool_calls (id, message_id, tool_name, status, error, created_at) \
                 VALUES (?, 'm-s1', 'single_session_tool', 'error', 'boom', ?)",
            )
            .bind(format!("single-{index}"))
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
            created_at += 1;
        }

        // A different cwd must never influence this project's denominator.
        for index in 0..8 {
            sqlx::query(
                "INSERT INTO tool_calls (id, message_id, tool_name, status, error, created_at) \
                 VALUES (?, 'm-other', 'edit_file', 'error', 'other cwd', ?)",
            )
            .bind(format!("other-{index}"))
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
            created_at += 1;
        }

        let created = mine_cross_session_patterns_for_pool("/proj", &pool)
            .await
            .unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].support_count, 2);
        assert!(!created[0].observation.contains("CF_EVO_MINER_SECRET"));
        assert!(!created[0].suggestion.contains("CF_EVO_MINER_SECRET"));
        let evidence: serde_json::Value = serde_json::from_str(&created[0].evidence_json).unwrap();
        assert_eq!(evidence["total_calls"], 8);
        assert_eq!(evidence["errors"], 2);
        assert_eq!(evidence["rate"], 25);
        assert_eq!(evidence["session_count"], 2);
        let candidate_job_id: Option<String> =
            sqlx::query_scalar("SELECT job_id FROM learning_events WHERE id = ?")
                .bind(&created[0].id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let job_id = candidate_job_id.expect("new candidate must reference its analysis job");
        let job: (String, i64, i64, i64) = sqlx::query_as(
            "SELECT status, input_session_count, input_trace_count, candidate_count
             FROM evolution_jobs WHERE id = ?",
        )
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(job, ("succeeded".into(), 2, 16, 1));

        let stages: Vec<String> = sqlx::query_scalar(
            "SELECT stage FROM evolution_job_events WHERE job_id = ? ORDER BY rowid",
        )
        .bind(&job_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        for required in [
            "scope",
            "trace_read",
            "privacy",
            "extract",
            "deduplicate",
            "review",
        ] {
            assert!(
                stages.iter().any(|stage| stage == required),
                "missing stage {required}: {stages:?}"
            );
        }
        let details: Vec<String> =
            sqlx::query_scalar("SELECT detail_json FROM evolution_job_events WHERE job_id = ?")
                .bind(&job_id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(!details.join("\n").contains("CF_EVO_MINER_SECRET"));

        let second = mine_cross_session_patterns_for_pool("/proj", &pool)
            .await
            .unwrap();
        assert!(
            second.is_empty(),
            "second scan must deduplicate the same pattern"
        );

        let latest_status: String = sqlx::query_scalar(
            "SELECT status FROM evolution_jobs WHERE cwd='/proj' ORDER BY started_at DESC, rowid DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(latest_status, "no_candidates");
    }

    fn temp_project() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("cf-evolution-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    async fn insert_candidate(
        pool: &SqlitePool,
        id: &str,
        cwd: &str,
        kind: &str,
        suggestion: &str,
        pref_key: Option<&str>,
        pref_value: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO learning_events
             (id, session_id, cwd, observation, suggestion, status, created_at, kind,
              pref_key, pref_value)
             VALUES (?, '', ?, 'observation', ?, 'pending', ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(cwd)
        .bind(suggestion)
        .bind(Utc::now().to_rfc3339())
        .bind(kind)
        .bind(pref_key)
        .bind(pref_value)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn concurrent_accept_materializes_once_and_only_then_marks_accepted() {
        let pool = fresh_miner_pool().await;
        let project = temp_project();
        let cwd = project.to_string_lossy().into_owned();
        insert_candidate(
            &pool,
            "candidate-accept",
            &cwd,
            "memory",
            "always run the targeted Rust tests",
            None,
            None,
        )
        .await;

        let first_pool = pool.clone();
        let second_pool = pool.clone();
        let first = tokio::spawn(async move {
            accept_learning_event_for_pool("candidate-accept", &first_pool).await
        });
        let second = tokio::spawn(async move {
            accept_learning_event_for_pool("candidate-accept", &second_pool).await
        });
        let (first, second) = tokio::join!(first, second);
        let outcomes = [first.unwrap(), second.unwrap()];
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(outcomes.iter().filter(|result| result.is_err()).count(), 1);

        let status: String =
            sqlx::query_scalar("SELECT status FROM learning_events WHERE id='candidate-accept'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "accepted");
        let content = std::fs::read_to_string(project.join(".codefactory/memory.md")).unwrap();
        assert_eq!(
            content
                .matches("always run the targeted Rust tests")
                .count(),
            1
        );
        assert_eq!(
            content
                .matches("codefactory-learning-event:candidate-accept")
                .count(),
            1
        );

        let job: (String, i64) = sqlx::query_as(
            "SELECT status, candidate_count FROM evolution_jobs
             WHERE candidate_id='candidate-accept' AND trigger='review_accept'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(job, ("succeeded".into(), 1));
        let stages: Vec<(String, String)> = sqlx::query_as(
            "SELECT stage, status FROM evolution_job_events
             WHERE candidate_id='candidate-accept' ORDER BY rowid",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(stages.contains(&("review".into(), "completed".into())));
        assert!(stages.contains(&("materialize".into(), "completed".into())));
        let _ = std::fs::remove_dir_all(project);
    }

    #[tokio::test]
    async fn running_decision_cannot_be_reentered_or_raced_by_opposite_decision() {
        let pool = fresh_miner_pool().await;
        insert_candidate(
            &pool,
            "candidate-running",
            "/proj",
            "memory",
            "one decision owner",
            None,
            None,
        )
        .await;

        begin_decision_job(&pool, "/proj", "candidate-running", "review_accept")
            .await
            .unwrap();
        let same = begin_decision_job(&pool, "/proj", "candidate-running", "review_accept")
            .await
            .unwrap_err();
        assert!(same.to_string().contains("already running"));
        assert!(
            begin_decision_job(&pool, "/proj", "candidate-running", "review_reject",)
                .await
                .is_err()
        );
        let running: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_jobs
             WHERE candidate_id='candidate-running' AND status='running'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(running, 1);
    }

    #[tokio::test]
    async fn a_project_cannot_start_two_analysis_jobs_at_the_same_time() {
        let pool = fresh_miner_pool().await;
        create_analysis_job(&pool, "/proj").await.unwrap();

        let second = create_analysis_job(&pool, "/proj")
            .await
            .expect_err("a second running project analysis must be rejected");
        assert!(second.to_string().contains("UNIQUE constraint failed"));
        let running: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_jobs
             WHERE cwd='/proj' AND trigger='cross_session' AND status='running'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(running, 1);
    }

    #[tokio::test]
    async fn preference_write_rolls_back_when_accept_terminal_transaction_fails() {
        let pool = fresh_miner_pool().await;
        insert_candidate(
            &pool,
            "candidate-pref-rollback",
            "/proj",
            "preference",
            "set concise output",
            Some("response_style"),
            Some("concise"),
        )
        .await;
        sqlx::query(
            "CREATE TRIGGER force_accept_rollback
             BEFORE UPDATE OF status ON learning_events
             WHEN NEW.status = 'accepted'
             BEGIN SELECT RAISE(ABORT, 'forced accept rollback'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            accept_learning_event_for_pool("candidate-pref-rollback", &pool)
                .await
                .is_err()
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM learning_events WHERE id='candidate-pref-rollback'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let preference_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_preferences
             WHERE cwd='/proj' AND key='response_style'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "pending");
        assert_eq!(preference_count, 0);
    }

    #[tokio::test]
    async fn reject_refuses_a_memory_candidate_with_an_existing_materialization_marker() {
        let pool = fresh_miner_pool().await;
        let project = temp_project();
        let cwd = project.to_string_lossy().into_owned();
        insert_candidate(
            &pool,
            "candidate-reconcile",
            &cwd,
            "memory",
            "already written",
            None,
            None,
        )
        .await;
        std::fs::create_dir_all(project.join(".codefactory")).unwrap();
        std::fs::write(
            project.join(".codefactory/memory.md"),
            "<!-- codefactory-learning-event:candidate-reconcile -->",
        )
        .unwrap();

        let error = reject_learning_event_for_pool("candidate-reconcile", &pool)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("retry accept to reconcile"));
        let status: String =
            sqlx::query_scalar("SELECT status FROM learning_events WHERE id='candidate-reconcile'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "pending");
        let _ = std::fs::remove_dir_all(project);
    }

    #[tokio::test]
    async fn failed_materialization_stays_pending_and_persists_redacted_failure() {
        let pool = fresh_miner_pool().await;
        insert_candidate(
            &pool,
            "candidate-fail",
            "/proj",
            "preference",
            "preference without a key",
            None,
            Some("password=CF_EVO_FAILURE_SECRET"),
        )
        .await;

        let error = accept_learning_event_for_pool("candidate-fail", &pool)
            .await
            .expect_err("missing pref_key must fail materialization");
        assert!(error.to_string().contains("missing pref_key"));
        let status: String =
            sqlx::query_scalar("SELECT status FROM learning_events WHERE id='candidate-fail'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "pending");
        let job: (String, Option<String>) = sqlx::query_as(
            "SELECT status, error FROM evolution_jobs WHERE candidate_id='candidate-fail'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(job.0, "failed");
        assert!(!job.1.unwrap_or_default().contains("CF_EVO_FAILURE_SECRET"));
        let materialize_status: String = sqlx::query_scalar(
            "SELECT status FROM evolution_job_events
             WHERE candidate_id='candidate-fail' AND stage='materialize'
             ORDER BY rowid DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(materialize_status, "failed");
    }

    #[tokio::test]
    async fn reject_uses_pending_cas_and_never_materializes() {
        let pool = fresh_miner_pool().await;
        let project = temp_project();
        let cwd = project.to_string_lossy().into_owned();
        insert_candidate(
            &pool,
            "candidate-reject",
            &cwd,
            "memory",
            "must not be written",
            None,
            None,
        )
        .await;

        reject_learning_event_for_pool("candidate-reject", &pool)
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM learning_events WHERE id='candidate-reject'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "rejected");
        assert!(!project.join(".codefactory/memory.md").exists());
        let materialize_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_job_events
             WHERE candidate_id='candidate-reject' AND stage='materialize'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(materialize_count, 0);
        assert!(
            reject_learning_event_for_pool("candidate-reject", &pool)
                .await
                .is_err(),
            "a second decision must lose the pending CAS"
        );
        let _ = std::fs::remove_dir_all(project);
    }

    #[tokio::test]
    async fn job_queries_are_scoped_by_cwd_and_optional_job_id() {
        let pool = fresh_miner_pool().await;
        mine_cross_session_patterns_for_pool("/proj", &pool)
            .await
            .unwrap();
        let jobs = list_evolution_jobs_for_pool("/proj", &pool).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, "no_candidates");
        assert_eq!(
            get_evolution_job_for_pool("/proj", &jobs[0].id, &pool)
                .await
                .unwrap(),
            jobs[0]
        );
        assert!(get_evolution_job_for_pool("/other", &jobs[0].id, &pool)
            .await
            .is_err());
        assert!(list_evolution_jobs_for_pool("/other", &pool)
            .await
            .unwrap()
            .is_empty());

        let events = list_evolution_job_events_for_pool("/proj", Some(&jobs[0].id), &pool)
            .await
            .unwrap();
        assert!(!events.is_empty());
        assert!(events.iter().all(|event| event.cwd == "/proj"));
        assert!(events.iter().all(|event| event.job_id == jobs[0].id));
        assert!(events.iter().all(|event| {
            !event.detail_json.contains("reasoning_content")
                && !event.detail_json.contains("raw_prompt\"")
        }));
    }

    #[tokio::test]
    async fn decision_job_query_keeps_exact_receipts_for_bounded_history() {
        let pool = fresh_miner_pool().await;
        for index in 0..105 {
            let candidate_id = format!("decision-candidate-{index:03}");
            let decided_at = format!("2026-07-15T{:02}:{:02}:00Z", index / 60, index % 60);
            sqlx::query(
                "INSERT INTO learning_events
                 (id, session_id, cwd, observation, suggestion, status, created_at, decided_at)
                 VALUES (?, '', '/proj', 'observation', 'suggestion', 'accepted', ?, ?)",
            )
            .bind(&candidate_id)
            .bind(&decided_at)
            .bind(&decided_at)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO evolution_jobs
                 (id, cwd, trigger, candidate_id, status, started_at, completed_at)
                 VALUES (?, '/proj', 'review_accept', ?, 'succeeded', ?, ?)",
            )
            .bind(format!("decision-job-{index:03}"))
            .bind(&candidate_id)
            .bind(&decided_at)
            .bind(&decided_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, candidate_id, status, started_at, completed_at)
             VALUES ('other-project-job', '/other', 'review_accept', 'decision-candidate-104',
                     'succeeded', '2026-07-15T23:00:00Z', '2026-07-15T23:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let jobs = list_evolution_decision_jobs_for_pool("/proj", &pool)
            .await
            .unwrap();
        assert_eq!(jobs.len(), 100);
        assert!(jobs.iter().all(|job| job.cwd == "/proj"));
        assert!(jobs.iter().any(|job| job.id == "decision-job-104"));
        assert!(!jobs.iter().any(|job| job.id == "decision-job-000"));
    }

    #[tokio::test]
    async fn specific_job_event_query_keeps_the_latest_terminal_event_when_bounded() {
        let pool = fresh_miner_pool().await;
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, status, owner_pid, started_at)
             VALUES ('long-job', '/proj', 'cross_session', 'succeeded', ?, '2026-07-15')",
        )
        .bind(std::process::id() as i64)
        .execute(&pool)
        .await
        .unwrap();
        for index in 0..505 {
            sqlx::query(
                "INSERT INTO evolution_job_events
                 (id, cwd, job_id, stage, status, title, detail_json, created_at)
                 VALUES (?, '/proj', 'long-job', 'extract', 'completed', ?, '{}', ?)",
            )
            .bind(format!("long-event-{index:03}"))
            .bind(if index == 504 { "terminal" } else { "progress" })
            .bind(format!(
                "2026-07-15T00:{:02}:{:02}Z",
                index / 60,
                index % 60
            ))
            .execute(&pool)
            .await
            .unwrap();
        }

        let events = list_evolution_job_events_for_pool("/proj", Some("long-job"), &pool)
            .await
            .unwrap();
        assert_eq!(events.len(), 500);
        assert_eq!(events.last().unwrap().title, "terminal");
        assert!(!events.iter().any(|event| event.id == "long-event-000"));
    }

    #[tokio::test]
    async fn miner_rolls_back_candidates_when_final_job_ledger_commit_fails() {
        let pool = fresh_miner_pool().await;
        for index in 0..2 {
            let session_id = format!("atomic-session-{index}");
            let message_id = format!("atomic-message-{index}");
            sqlx::query("INSERT INTO sessions (id, cwd) VALUES (?, '/proj')")
                .bind(&session_id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO messages (id, session_id) VALUES (?, ?)")
                .bind(&message_id)
                .bind(&session_id)
                .execute(&pool)
                .await
                .unwrap();
            for call in 0..4 {
                sqlx::query(
                    "INSERT INTO tool_calls
                     (id, message_id, tool_name, status, error, created_at)
                     VALUES (?, ?, 'bash', ?, ?, ?)",
                )
                .bind(format!("atomic-call-{index}-{call}"))
                .bind(&message_id)
                .bind(if call == 0 { "error" } else { "done" })
                .bind(if call == 0 {
                    Some("bounded failure")
                } else {
                    None
                })
                .bind(index * 10 + call)
                .execute(&pool)
                .await
                .unwrap();
            }
        }
        sqlx::query(
            "CREATE TRIGGER fail_final_mining_ledger
             BEFORE INSERT ON evolution_job_events
             WHEN NEW.stage = 'deduplicate'
             BEGIN SELECT RAISE(ABORT, 'forced final ledger failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = mine_cross_session_patterns_for_pool("/proj", &pool)
            .await
            .expect_err("final ledger failure must fail the analysis");
        assert!(error.to_string().contains("forced final ledger failure"));
        let candidates: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM learning_events WHERE cwd='/proj' AND status='pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            candidates, 0,
            "failed analysis must not leave adoptable candidates"
        );
    }

    #[tokio::test]
    async fn job_failure_redacts_error_in_summary_and_append_only_event() {
        let pool = fresh_miner_pool().await;
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, status, started_at)
             VALUES ('job-failure', '/proj', 'cross_session', 'running', ?)",
        )
        .bind(Utc::now().to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

        record_job_failure(
            &pool,
            "/proj",
            "job-failure",
            None,
            "extract",
            "提取失败",
            &AppError::Other("password=CF_EVO_JOB_SECRET".into()),
        )
        .await;

        let job: (String, String) =
            sqlx::query_as("SELECT status, error FROM evolution_jobs WHERE id='job-failure'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(job.0, "failed");
        assert!(!job.1.contains("CF_EVO_JOB_SECRET"));
        assert!(job.1.contains("<redacted>"));
        let detail: String = sqlx::query_scalar(
            "SELECT detail_json FROM evolution_job_events
             WHERE job_id='job-failure' ORDER BY rowid DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!detail.contains("CF_EVO_JOB_SECRET"));
        assert!(detail.contains("<redacted>"));
    }

    #[tokio::test]
    async fn job_failure_rolls_back_terminal_when_failed_event_cannot_be_written() {
        let pool = fresh_miner_pool().await;
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, status, started_at)
             VALUES ('event-half-failure', '/proj', 'cross_session', 'running', '2026-07-15')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER block_failed_event
             BEFORE INSERT ON evolution_job_events
             WHEN NEW.status = 'failed'
             BEGIN SELECT RAISE(ABORT, 'blocked failed event'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        record_job_failure(
            &pool,
            "/proj",
            "event-half-failure",
            None,
            "extract",
            "提取失败",
            &AppError::Other("forced".into()),
        )
        .await;

        let status: String = sqlx::query_scalar(
            "SELECT status FROM evolution_jobs WHERE id='event-half-failure'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "running", "terminal update must roll back with its event");
        let failed_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_job_events
             WHERE job_id='event-half-failure' AND status='failed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(failed_events, 0);
    }

    #[tokio::test]
    async fn job_failure_rolls_back_event_when_terminal_update_cannot_be_written() {
        let pool = fresh_miner_pool().await;
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, status, started_at)
             VALUES ('terminal-half-failure', '/proj', 'cross_session', 'running', '2026-07-15')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER block_failed_terminal
             BEFORE UPDATE ON evolution_jobs
             WHEN NEW.status = 'failed'
             BEGIN SELECT RAISE(ABORT, 'blocked failed terminal'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        record_job_failure(
            &pool,
            "/proj",
            "terminal-half-failure",
            None,
            "extract",
            "提取失败",
            &AppError::Other("forced".into()),
        )
        .await;

        let status: String = sqlx::query_scalar(
            "SELECT status FROM evolution_jobs WHERE id='terminal-half-failure'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "running");
        let failed_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_job_events
             WHERE job_id='terminal-half-failure' AND status='failed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(failed_events, 0, "failed event must roll back with terminal update");
    }

    #[tokio::test]
    async fn miner_failure_keeps_a_failed_job_and_structured_stage_log() {
        let pool = fresh_miner_pool().await;
        sqlx::query("DROP TABLE tool_calls")
            .execute(&pool)
            .await
            .unwrap();

        let error = mine_cross_session_patterns_for_pool("/proj", &pool)
            .await
            .expect_err("missing trace table must fail the real analysis job");
        assert!(error.to_string().contains("tool_calls"));
        let job: (String, Option<String>) = sqlx::query_as(
            "SELECT status, error FROM evolution_jobs
             WHERE cwd='/proj' ORDER BY rowid DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(job.0, "failed");
        assert!(job.1.is_some());
        let event: (String, String) = sqlx::query_as(
            "SELECT stage, status FROM evolution_job_events
             WHERE cwd='/proj' ORDER BY rowid DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event, ("trace_read".into(), "failed".into()));
    }
}
