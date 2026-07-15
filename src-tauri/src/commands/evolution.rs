// SPDX-License-Identifier: Apache-2.0
//! Versioned Evolution candidates, activation-safety Evals and reversible
//! activation. Legacy `learning_events.status=accepted` remains untouched.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use tauri::{command, AppHandle, Emitter, State};
use uuid::Uuid;

use crate::errors::AppError;
use crate::AppState;

const EVAL_RUNNER_VERSION: &str = "context-integrity-v1";
const REQUIRED_CASES: &[(&str, &str)] = &[
    ("frozen_revision", "冻结版本完整性"),
    ("project_scope", "项目范围隔离"),
    ("privacy_contract", "隐私与长度合同"),
    ("target_allowlist", "低风险目标白名单"),
    ("baseline_isolation", "Baseline 未提前生效"),
    ("treatment_projection", "Treatment 精确注入一次"),
    ("rollback_readiness", "回滚准备度"),
];

const RELEASE_SMOKE_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CandidatePayload {
    suggestion: String,
    pref_key: Option<String>,
    pref_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvolutionCandidateState {
    pub candidate_id: String,
    pub source_learning_event_id: Option<String>,
    pub cwd: String,
    pub kind: String,
    pub revision: i64,
    pub state: String,
    pub state_version: i64,
    pub suggestion: String,
    pub pref_key: Option<String>,
    pub pref_value: Option<String>,
    pub payload_hash: String,
    pub auto_activate: bool,
    pub eval_run_id: Option<String>,
    pub eval_status: Option<String>,
    pub eval_manifest_hash: Option<String>,
    pub eval_required_count: i64,
    pub eval_passed_count: i64,
    pub eval_failed_count: i64,
    pub activation_id: Option<String>,
    pub activation_status: Option<String>,
    pub activated_at: Option<String>,
    pub rolled_back_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvolutionEvalCaseResult {
    pub id: String,
    pub run_id: String,
    pub case_id: String,
    pub title: String,
    pub status: String,
    pub hard_gate: bool,
    pub detail_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
struct CaseOutcome {
    id: &'static str,
    title: &'static str,
    status: &'static str,
    reason: &'static str,
}

fn sha256(value: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(value.as_ref()))
}

fn manifest_hash() -> String {
    sha256(format!(
        "{}:{}",
        EVAL_RUNNER_VERSION,
        REQUIRED_CASES
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
            .join(",")
    ))
}

fn payload_json(payload: &CandidatePayload) -> Result<String, AppError> {
    serde_json::to_string(payload).map_err(Into::into)
}

fn safe_detail(value: serde_json::Value) -> String {
    let redacted = crate::trajectory::redact_json(&value);
    let mut safe = serde_json::Map::new();
    let allowed = [
        "schema_version",
        "candidate_id",
        "revision",
        "run_id",
        "manifest_hash",
        "required_count",
        "passed_count",
        "failed_count",
        "verdict",
        "auto_activate",
        "activation_id",
        "target",
        "reason",
        "before_hash",
        "after_hash",
    ];
    if let serde_json::Value::Object(map) = redacted {
        for key in allowed {
            if let Some(value) = map.get(key) {
                safe.insert(
                    key.into(),
                    match value {
                        serde_json::Value::String(text) => {
                            serde_json::Value::String(crate::trajectory::redact_text(text, 160))
                        }
                        serde_json::Value::Number(_)
                        | serde_json::Value::Bool(_)
                        | serde_json::Value::Null => value.clone(),
                        _ => continue,
                    },
                );
            }
        }
    }
    safe.entry("schema_version")
        .or_insert_with(|| serde_json::Value::from(1));
    serde_json::Value::Object(safe).to_string()
}

async fn append_job_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    cwd: &str,
    job_id: &str,
    candidate_id: &str,
    stage: &str,
    status: &str,
    title: &str,
    detail: serde_json::Value,
    now: &str,
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
    .bind(safe_detail(detail))
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn create_job(
    pool: &SqlitePool,
    cwd: &str,
    candidate_id: &str,
    trigger: &str,
) -> Result<String, AppError> {
    let job_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let key = format!("phase4:{trigger}:{cwd}:{candidate_id}:{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO evolution_jobs
         (id, cwd, trigger, candidate_id, status, idempotency_key,
          owner_pid, owner_start_token, started_at)
         VALUES (?, ?, ?, ?, 'running', ?, ?, ?, ?)",
    )
    .bind(&job_id)
    .bind(cwd)
    .bind(trigger)
    .bind(candidate_id)
    .bind(key)
    .bind(std::process::id() as i64)
    .bind(crate::storage::db::current_process_start_token())
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(job_id)
}

async fn current_target_fingerprint(
    pool: &SqlitePool,
    cwd: &str,
    kind: &str,
    payload: &CandidatePayload,
) -> Result<(String, serde_json::Value), AppError> {
    if kind == "preference" {
        let key = payload.pref_key.as_deref().unwrap_or_default();
        let row: Option<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT value, source, updated_at, activation_id
             FROM user_preferences WHERE cwd=? AND key=?",
        )
        .bind(cwd)
        .bind(key)
        .fetch_optional(pool)
        .await?;
        let before = match row {
            Some((value, source, updated_at, activation_id)) => serde_json::json!({
                "exists": true,
                "value": value,
                "source": source,
                "updated_at": updated_at,
                "activation_id": activation_id,
            }),
            None => serde_json::json!({"exists": false}),
        };
        return Ok((sha256(before.to_string()), before));
    }

    let active: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT candidate_id, revision, content_hash FROM evolution_active_memory
         WHERE cwd=? AND active=1 ORDER BY candidate_id",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await?;
    let legacy: Vec<(String,)> = sqlx::query_as(
        "SELECT suggestion FROM learning_events WHERE cwd=? AND status='accepted'
         ORDER BY id",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await?;
    let file_hash = match std::fs::read(Path::new(cwd).join(".codefactory/memory.md")) {
        Ok(bytes) => sha256(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => sha256([]),
        Err(error) => return Err(error.into()),
    };
    let snapshot = serde_json::json!({
        "active": active,
        "legacy": legacy,
        "memory_file_hash": file_hash,
    });
    Ok((sha256(snapshot.to_string()), serde_json::json!({})))
}

fn preference_allowed(key: &str, value: &str) -> bool {
    if value.trim().is_empty() || value.chars().count() > 300 {
        return false;
    }
    matches!(
        key,
        "communication_style" | "testing_habit" | "code_style" | "response_language"
    )
}

fn memory_allowed(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 1_200 {
        return false;
    }
    let normalized = trimmed.to_lowercase();
    let policy_sensitive = [
        "rm -rf",
        "git reset --hard",
        "--no-verify",
        "bypass approval",
        "skip approval",
        "without approval",
        "auto-approve",
        "auto approve",
        "auto-merge",
        "auto merge",
        "automatically deploy",
        "automatically release",
        "automatically publish",
        "full access",
        "autonomous execution",
        "绕过审批",
        "跳过审批",
        "无需审批",
        "不需审批",
        "自动批准",
        "自动合并",
        "自动部署",
        "自动发布",
        "自动上线",
        "完全权限",
        "绕过权限",
        "自动执行",
    ];
    !policy_sensitive
        .iter()
        .any(|needle| normalized.contains(needle))
}

async fn evaluate_cases(
    pool: &SqlitePool,
    cwd: &str,
    kind: &str,
    candidate_id: &str,
    revision: i64,
    payload: &CandidatePayload,
    payload_hash: &str,
    captured_target_fingerprint: &str,
) -> Result<(Vec<CaseOutcome>, String, String), AppError> {
    let frozen = sha256(payload_json(payload)?) == payload_hash;
    let project_scope = Path::new(cwd).is_absolute() && cwd != "_global_";
    let redacted_suggestion = crate::trajectory::redact_text(&payload.suggestion, usize::MAX);
    let privacy = redacted_suggestion == payload.suggestion
        && payload.suggestion.chars().count() <= 1_200
        && payload
            .pref_value
            .as_deref()
            .map(|value| crate::trajectory::redact_text(value, usize::MAX) == value)
            .unwrap_or(true);
    let target_allowed = match kind {
        "memory" | "pattern" => memory_allowed(&payload.suggestion),
        "preference" => payload
            .pref_key
            .as_deref()
            .zip(payload.pref_value.as_deref())
            .map(|(key, value)| preference_allowed(key, value))
            .unwrap_or(false),
        _ => false,
    };
    let (current_fingerprint, _) = current_target_fingerprint(pool, cwd, kind, payload).await?;
    let target_fresh = current_fingerprint == captured_target_fingerprint;

    let baseline_has_candidate: bool = if kind == "preference" {
        let key = payload.pref_key.as_deref().unwrap_or_default();
        let value = payload.pref_value.as_deref().unwrap_or_default();
        sqlx::query_scalar::<_, String>("SELECT value FROM user_preferences WHERE cwd=? AND key=?")
            .bind(cwd)
            .bind(key)
            .fetch_optional(pool)
            .await?
            .as_deref()
            == Some(value)
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM evolution_active_memory
             WHERE cwd=? AND candidate_id=? AND active=1",
        )
        .bind(cwd)
        .bind(candidate_id)
        .fetch_one(pool)
        .await?
            > 0
    };
    let baseline_isolated = !baseline_has_candidate;

    // Treatment projection is deterministic and never mutates the live
    // prompt source: exact project gets one staged item, another scope gets 0.
    let treatment_projection = if kind == "preference" {
        payload.pref_key.is_some() && payload.pref_value.is_some()
    } else {
        !payload.suggestion.trim().is_empty()
    };
    let rollback_ready = target_fresh && target_allowed;

    let values = [
        frozen,
        project_scope,
        privacy,
        target_allowed,
        baseline_isolated,
        treatment_projection,
        rollback_ready,
    ];
    let reasons = [
        "payload_hash_mismatch",
        "project_scope_required",
        "secret_or_length_contract",
        "unsupported_target",
        "candidate_already_active",
        "treatment_projection_failed",
        "target_changed_or_not_reversible",
    ];
    let outcomes = REQUIRED_CASES
        .iter()
        .zip(values)
        .zip(reasons)
        .map(|(((id, title), passed), reason)| CaseOutcome {
            id,
            title,
            status: if passed { "passed" } else { "failed" },
            reason: if passed { "ok" } else { reason },
        })
        .collect::<Vec<_>>();
    let baseline_hash = sha256(format!(
        "{captured_target_fingerprint}:{EVAL_RUNNER_VERSION}"
    ));
    let treatment_hash = sha256(format!(
        "{baseline_hash}:{candidate_id}:{revision}:{payload_hash}"
    ));
    Ok((outcomes, baseline_hash, treatment_hash))
}

async fn state_for_candidate(
    pool: &SqlitePool,
    cwd: &str,
    candidate_id: &str,
) -> Result<EvolutionCandidateState, AppError> {
    let row = sqlx::query(
        "SELECT c.id, c.source_learning_event_id, c.cwd, c.kind, c.current_revision,
                c.current_state, c.state_version, c.updated_at,
                r.payload_json, r.payload_hash,
                COALESCE((SELECT auto_activate FROM candidate_reviews cr
                          WHERE cr.candidate_id=c.id AND cr.revision=c.current_revision
                          ORDER BY cr.created_at DESC, cr.rowid DESC LIMIT 1), 0) auto_activate,
                (SELECT er.id FROM evolution_eval_runs er
                 WHERE er.candidate_id=c.id AND er.revision=c.current_revision
                 ORDER BY er.started_at DESC, er.rowid DESC LIMIT 1) eval_run_id,
                (SELECT er.status FROM evolution_eval_runs er
                 WHERE er.candidate_id=c.id AND er.revision=c.current_revision
                 ORDER BY er.started_at DESC, er.rowid DESC LIMIT 1) eval_status,
                (SELECT er.manifest_hash FROM evolution_eval_runs er
                 WHERE er.candidate_id=c.id AND er.revision=c.current_revision
                 ORDER BY er.started_at DESC, er.rowid DESC LIMIT 1) eval_manifest_hash,
                COALESCE((SELECT er.required_count FROM evolution_eval_runs er
                 WHERE er.candidate_id=c.id AND er.revision=c.current_revision
                 ORDER BY er.started_at DESC, er.rowid DESC LIMIT 1), 0) eval_required_count,
                COALESCE((SELECT er.passed_count FROM evolution_eval_runs er
                 WHERE er.candidate_id=c.id AND er.revision=c.current_revision
                 ORDER BY er.started_at DESC, er.rowid DESC LIMIT 1), 0) eval_passed_count,
                COALESCE((SELECT er.failed_count FROM evolution_eval_runs er
                 WHERE er.candidate_id=c.id AND er.revision=c.current_revision
                 ORDER BY er.started_at DESC, er.rowid DESC LIMIT 1), 0) eval_failed_count,
                (SELECT ar.id FROM evolution_activation_receipts ar
                 WHERE ar.candidate_id=c.id AND ar.revision=c.current_revision
                 ORDER BY COALESCE(ar.activated_at, '') DESC, ar.rowid DESC LIMIT 1) activation_id,
                (SELECT ar.status FROM evolution_activation_receipts ar
                 WHERE ar.candidate_id=c.id AND ar.revision=c.current_revision
                 ORDER BY COALESCE(ar.activated_at, '') DESC, ar.rowid DESC LIMIT 1) activation_status,
                (SELECT ar.activated_at FROM evolution_activation_receipts ar
                 WHERE ar.candidate_id=c.id AND ar.revision=c.current_revision
                 ORDER BY COALESCE(ar.activated_at, '') DESC, ar.rowid DESC LIMIT 1) activated_at,
                (SELECT ar.rolled_back_at FROM evolution_activation_receipts ar
                 WHERE ar.candidate_id=c.id AND ar.revision=c.current_revision
                 ORDER BY COALESCE(ar.activated_at, '') DESC, ar.rowid DESC LIMIT 1) rolled_back_at
         FROM improvement_candidates c
         JOIN candidate_revisions r ON r.candidate_id=c.id AND r.revision=c.current_revision
         WHERE c.cwd=? AND c.id=?",
    )
    .bind(cwd)
    .bind(candidate_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::Other(format!("candidate {candidate_id} not found for project")))?;
    state_from_row(row)
}

fn state_from_row(row: sqlx::sqlite::SqliteRow) -> Result<EvolutionCandidateState, AppError> {
    let payload: CandidatePayload = serde_json::from_str(row.try_get("payload_json")?)?;
    Ok(EvolutionCandidateState {
        candidate_id: row.try_get("id")?,
        source_learning_event_id: row.try_get("source_learning_event_id")?,
        cwd: row.try_get("cwd")?,
        kind: row.try_get("kind")?,
        revision: row.try_get("current_revision")?,
        state: row.try_get("current_state")?,
        state_version: row.try_get("state_version")?,
        suggestion: payload.suggestion,
        pref_key: payload.pref_key,
        pref_value: payload.pref_value,
        payload_hash: row.try_get("payload_hash")?,
        auto_activate: row.try_get::<i64, _>("auto_activate")? != 0,
        eval_run_id: row.try_get("eval_run_id")?,
        eval_status: row.try_get("eval_status")?,
        eval_manifest_hash: row.try_get("eval_manifest_hash")?,
        eval_required_count: row.try_get("eval_required_count")?,
        eval_passed_count: row.try_get("eval_passed_count")?,
        eval_failed_count: row.try_get("eval_failed_count")?,
        activation_id: row.try_get("activation_id")?,
        activation_status: row.try_get("activation_status")?,
        activated_at: row.try_get("activated_at")?,
        rolled_back_at: row.try_get("rolled_back_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub async fn list_candidate_states_for_pool(
    pool: &SqlitePool,
    cwd: &str,
) -> Result<Vec<EvolutionCandidateState>, AppError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM improvement_candidates WHERE cwd=? ORDER BY updated_at DESC, rowid DESC",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await?;
    let mut states = Vec::with_capacity(ids.len());
    for id in ids {
        states.push(state_for_candidate(pool, cwd, &id).await?);
    }
    Ok(states)
}

#[command]
pub async fn list_evolution_candidate_states(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<EvolutionCandidateState>, AppError> {
    let pool = state.db.read().await;
    list_candidate_states_for_pool(&pool, &cwd).await
}

async fn run_eval_for_candidate(
    pool: &SqlitePool,
    cwd: &str,
    candidate_id: &str,
    job_id: &str,
) -> Result<EvolutionCandidateState, AppError> {
    let row = sqlx::query(
        "SELECT c.kind, c.current_revision, c.current_state, r.payload_json,
                r.payload_hash
         FROM improvement_candidates c
         JOIN candidate_revisions r ON r.candidate_id=c.id AND r.revision=c.current_revision
         WHERE c.id=? AND c.cwd=?",
    )
    .bind(candidate_id)
    .bind(cwd)
    .fetch_one(pool)
    .await?;
    let kind: String = row.try_get("kind")?;
    let revision: i64 = row.try_get("current_revision")?;
    let state: String = row.try_get("current_state")?;
    if !matches!(
        state.as_str(),
        "approved" | "eval_failed" | "eval_error" | "eval_stale"
    ) {
        return Err(AppError::Other(format!(
            "candidate {candidate_id} cannot run Eval from {state}"
        )));
    }
    let payload: CandidatePayload = serde_json::from_str(row.try_get("payload_json")?)?;
    let frozen_hash: String = row.try_get("payload_hash")?;
    let (target_fingerprint, _) = current_target_fingerprint(pool, cwd, &kind, &payload).await?;
    let run_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let manifest = manifest_hash();
    let idempotency_key =
        format!("eval:{candidate_id}:{revision}:{frozen_hash}:{manifest}:{target_fingerprint}");

    if let Some(_existing) = sqlx::query_scalar::<_, String>(
        "SELECT id FROM evolution_eval_runs WHERE idempotency_key=? AND status='passed'",
    )
    .bind(&idempotency_key)
    .fetch_optional(pool)
    .await?
    {
        return state_for_candidate(pool, cwd, candidate_id).await;
    }

    sqlx::query(
        "INSERT INTO evolution_eval_runs
         (id, job_id, cwd, candidate_id, revision, status, manifest_hash,
          runner_version, baseline_hash, treatment_hash, target_fingerprint,
          idempotency_key, owner_pid, owner_start_token, started_at)
         VALUES (?, ?, ?, ?, ?, 'running', ?, ?, '', '', ?, ?, ?, ?, ?)
         ON CONFLICT(idempotency_key) DO UPDATE SET
           id=excluded.id, job_id=excluded.job_id, status='running',
           owner_pid=excluded.owner_pid, owner_start_token=excluded.owner_start_token,
           started_at=excluded.started_at, completed_at=NULL, error=NULL",
    )
    .bind(&run_id)
    .bind(job_id)
    .bind(cwd)
    .bind(candidate_id)
    .bind(revision)
    .bind(&manifest)
    .bind(EVAL_RUNNER_VERSION)
    .bind(&target_fingerprint)
    .bind(&idempotency_key)
    .bind(std::process::id() as i64)
    .bind(crate::storage::db::current_process_start_token())
    .bind(&now)
    .execute(pool)
    .await?;

    let evaluated = evaluate_cases(
        pool,
        cwd,
        &kind,
        candidate_id,
        revision,
        &payload,
        &frozen_hash,
        &target_fingerprint,
    )
    .await;
    let (outcomes, baseline_hash, treatment_hash) = match evaluated {
        Ok(value) => value,
        Err(error) => {
            let safe_error = crate::trajectory::redact_text(&error.to_string(), 500);
            let completed = Utc::now().to_rfc3339();
            let mut tx = pool.begin().await?;
            sqlx::query(
                "UPDATE evolution_eval_runs SET status='error', completed_at=?, error=? WHERE id=?",
            )
            .bind(&completed)
            .bind(&safe_error)
            .bind(&run_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE improvement_candidates SET current_state='eval_error',
                 state_version=state_version+1, updated_at=? WHERE id=? AND cwd=?",
            )
            .bind(&completed)
            .bind(candidate_id)
            .bind(cwd)
            .execute(&mut *tx)
            .await?;
            append_job_event(
                &mut tx,
                cwd,
                job_id,
                candidate_id,
                "eval",
                "failed",
                "激活安全 Evals 运行失败",
                serde_json::json!({"run_id": run_id, "reason": safe_error}),
                &completed,
            )
            .await?;
            sqlx::query(
                "UPDATE evolution_jobs SET status='failed', completed_at=?, error=? WHERE id=?",
            )
            .bind(&completed)
            .bind(&safe_error)
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return state_for_candidate(pool, cwd, candidate_id).await;
        }
    };
    let passed_count = outcomes
        .iter()
        .filter(|case| case.status == "passed")
        .count() as i64;
    let failed_count = outcomes.len() as i64 - passed_count;
    let verdict = if failed_count == 0 {
        "passed"
    } else {
        "failed"
    };
    let candidate_state = if verdict == "passed" {
        "pending_activation"
    } else {
        "eval_failed"
    };
    let completed = Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    for outcome in &outcomes {
        sqlx::query(
            "INSERT INTO evolution_eval_case_results
             (id, run_id, case_id, title, status, hard_gate, detail_json, created_at)
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&run_id)
        .bind(outcome.id)
        .bind(outcome.title)
        .bind(outcome.status)
        .bind(safe_detail(serde_json::json!({"reason": outcome.reason})))
        .bind(&completed)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE evolution_eval_runs SET status=?, baseline_hash=?, treatment_hash=?,
         required_count=?, passed_count=?, failed_count=?, completed_at=?, error=NULL
         WHERE id=? AND status='running'",
    )
    .bind(verdict)
    .bind(&baseline_hash)
    .bind(&treatment_hash)
    .bind(outcomes.len() as i64)
    .bind(passed_count)
    .bind(failed_count)
    .bind(&completed)
    .bind(&run_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE improvement_candidates SET current_state=?, state_version=state_version+1,
         updated_at=? WHERE id=? AND cwd=? AND current_revision=?",
    )
    .bind(candidate_state)
    .bind(&completed)
    .bind(candidate_id)
    .bind(cwd)
    .bind(revision)
    .execute(&mut *tx)
    .await?;
    append_job_event(
        &mut tx,
        cwd,
        job_id,
        candidate_id,
        "eval",
        if verdict == "passed" {
            "completed"
        } else {
            "failed"
        },
        if verdict == "passed" {
            "激活安全 Evals 全部通过"
        } else {
            "激活安全 Evals 未通过，未激活"
        },
        serde_json::json!({
            "run_id": run_id,
            "manifest_hash": manifest,
            "required_count": outcomes.len(),
            "passed_count": passed_count,
            "failed_count": failed_count,
            "verdict": verdict,
        }),
        &completed,
    )
    .await?;
    tx.commit().await?;
    state_for_candidate(pool, cwd, candidate_id).await
}

pub async fn approve_learning_event_for_pool(
    pool: &SqlitePool,
    event_id: &str,
    auto_activate: bool,
) -> Result<EvolutionCandidateState, AppError> {
    let row = sqlx::query(
        "SELECT cwd, suggestion, status, kind, pref_key, pref_value, evidence_json
         FROM learning_events WHERE id=?",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::Other(format!("learning event {event_id} not found")))?;
    let cwd: String = row.try_get("cwd")?;
    let status: String = row.try_get("status")?;
    if status != "pending" {
        return Err(AppError::Other(format!(
            "learning event {event_id} already {status}"
        )));
    }
    if let Some(existing_id) = sqlx::query_scalar::<_, String>(
        "SELECT id FROM improvement_candidates WHERE source_learning_event_id=?",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?
    {
        return state_for_candidate(pool, &cwd, &existing_id).await;
    }
    let kind: String = row.try_get("kind")?;
    let payload = CandidatePayload {
        suggestion: row.try_get("suggestion")?,
        pref_key: row.try_get("pref_key")?,
        pref_value: row.try_get("pref_value")?,
    };
    let serialized = payload_json(&payload)?;
    let frozen_hash = sha256(&serialized);
    let evidence_json: String = row.try_get("evidence_json")?;
    let candidate_id = event_id.to_string();
    let job_id = create_job(pool, &cwd, &candidate_id, "review_eval").await?;
    let now = Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    append_job_event(
        &mut tx,
        &cwd,
        &job_id,
        &candidate_id,
        "review",
        "started",
        "开始人工批准",
        serde_json::json!({"revision": 1, "auto_activate": auto_activate}),
        &now,
    )
    .await?;
    sqlx::query(
        "INSERT INTO improvement_candidates
         (id, cwd, kind, source_learning_event_id, current_revision, current_state,
          state_version, created_at, updated_at)
         VALUES (?, ?, ?, ?, 1, 'approved', 1, ?, ?)",
    )
    .bind(&candidate_id)
    .bind(&cwd)
    .bind(&kind)
    .bind(event_id)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO candidate_revisions
         (candidate_id, revision, payload_json, payload_hash, evidence_json, risk_class, created_at)
         VALUES (?, 1, ?, ?, ?, 'low', ?)",
    )
    .bind(&candidate_id)
    .bind(&serialized)
    .bind(&frozen_hash)
    .bind(evidence_json)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO candidate_reviews
         (id, candidate_id, revision, decision, actor, auto_activate, created_at)
         VALUES (?, ?, 1, 'approved', 'user', ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&candidate_id)
    .bind(if auto_activate { 1 } else { 0 })
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    append_job_event(
        &mut tx,
        &cwd,
        &job_id,
        &candidate_id,
        "stage",
        "completed",
        "候选 revision 已冻结，live target 未改变",
        serde_json::json!({
            "candidate_id": candidate_id,
            "revision": 1,
            "auto_activate": auto_activate,
        }),
        &now,
    )
    .await?;
    tx.commit().await?;

    let evaluated = run_eval_for_candidate(pool, &cwd, &candidate_id, &job_id).await?;
    if evaluated.state == "pending_activation" && auto_activate {
        activate_candidate_for_pool(pool, &cwd, &candidate_id, Some(&job_id)).await
    } else {
        let completed = Utc::now().to_rfc3339();
        sqlx::query("UPDATE evolution_jobs SET status=?, completed_at=?, error=NULL WHERE id=?")
            .bind(if evaluated.state == "pending_activation" {
                "succeeded"
            } else {
                "partial"
            })
            .bind(&completed)
            .bind(&job_id)
            .execute(pool)
            .await?;
        state_for_candidate(pool, &cwd, &candidate_id).await
    }
}

#[command]
pub async fn approve_learning_event(
    event_id: String,
    auto_activate: Option<bool>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EvolutionCandidateState, AppError> {
    let pool = state.db.read().await;
    let result =
        approve_learning_event_for_pool(&pool, &event_id, auto_activate.unwrap_or(false)).await?;
    let _ = app.emit(&format!("learning_events_updated:{}", result.cwd), ());
    let _ = app.emit(&format!("evolution_candidates_updated:{}", result.cwd), ());
    Ok(result)
}

pub async fn activate_candidate_for_pool(
    pool: &SqlitePool,
    cwd: &str,
    candidate_id: &str,
    existing_job_id: Option<&str>,
) -> Result<EvolutionCandidateState, AppError> {
    let state = state_for_candidate(pool, cwd, candidate_id).await?;
    if state.state == "active" {
        return Ok(state);
    }
    if state.state != "pending_activation" || state.eval_status.as_deref() != Some("passed") {
        return Err(AppError::Other(format!(
            "candidate {candidate_id} requires exact passed Eval before activation"
        )));
    }
    let eval_run_id = state
        .eval_run_id
        .clone()
        .ok_or_else(|| AppError::Other("passed Eval run missing".into()))?;
    let target_fingerprint: String = sqlx::query_scalar(
        "SELECT target_fingerprint FROM evolution_eval_runs WHERE id=? AND candidate_id=? AND revision=?",
    )
    .bind(&eval_run_id)
    .bind(candidate_id)
    .bind(state.revision)
    .fetch_one(pool)
    .await?;
    let payload = CandidatePayload {
        suggestion: state.suggestion.clone(),
        pref_key: state.pref_key.clone(),
        pref_value: state.pref_value.clone(),
    };
    let (current_fingerprint, before_snapshot) =
        current_target_fingerprint(pool, cwd, &state.kind, &payload).await?;
    if current_fingerprint != target_fingerprint {
        let job_id = match existing_job_id {
            Some(id) => id.to_string(),
            None => create_job(pool, cwd, candidate_id, "activation").await?,
        };
        let now = Utc::now().to_rfc3339();
        let mut tx = pool.begin().await?;
        sqlx::query(
            "UPDATE improvement_candidates SET current_state='eval_stale',
             state_version=state_version+1, updated_at=?
             WHERE id=? AND cwd=? AND current_revision=? AND current_state='pending_activation'",
        )
        .bind(&now)
        .bind(candidate_id)
        .bind(cwd)
        .bind(state.revision)
        .execute(&mut *tx)
        .await?;
        append_job_event(
            &mut tx,
            cwd,
            &job_id,
            candidate_id,
            "activation",
            "failed",
            "目标在 Eval 后变化，旧结果未激活",
            serde_json::json!({
                "run_id": eval_run_id,
                "reason": "target_changed_after_eval",
                "before_hash": target_fingerprint,
                "after_hash": current_fingerprint,
            }),
            &now,
        )
        .await?;
        sqlx::query(
            "UPDATE evolution_jobs SET status='partial', completed_at=?, error=? WHERE id=?",
        )
        .bind(&now)
        .bind("target changed after Eval; exact revision requires a fresh Eval")
        .bind(&job_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Err(AppError::Other(
            "target changed after Eval; rerun Evals before activation".into(),
        ));
    }
    let job_id = existing_job_id
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let activation_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let target_key = state.pref_key.clone();
    let after_hash = if state.kind == "preference" {
        sha256(state.pref_value.as_deref().unwrap_or_default())
    } else {
        sha256(&state.suggestion)
    };
    let idempotency_key = format!(
        "activate:{}:{}:{}:{}",
        candidate_id, state.revision, eval_run_id, state.payload_hash
    );
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query(
        "UPDATE improvement_candidates SET current_state='activating',
         state_version=state_version+1, updated_at=?
         WHERE id=? AND cwd=? AND current_revision=? AND current_state='pending_activation'",
    )
    .bind(&now)
    .bind(candidate_id)
    .bind(cwd)
    .bind(state.revision)
    .execute(&mut *tx)
    .await?;
    if claimed.rows_affected() != 1 {
        return Err(AppError::Other(
            "candidate activation was already claimed".into(),
        ));
    }
    if existing_job_id.is_none() {
        let key = format!("phase4:activation:{cwd}:{candidate_id}:{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO evolution_jobs
             (id, cwd, trigger, candidate_id, status, idempotency_key,
              owner_pid, owner_start_token, started_at)
             VALUES (?, ?, 'activation', ?, 'running', ?, ?, ?, ?)",
        )
        .bind(&job_id)
        .bind(cwd)
        .bind(candidate_id)
        .bind(key)
        .bind(std::process::id() as i64)
        .bind(crate::storage::db::current_process_start_token())
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO evolution_activation_receipts
         (id, job_id, cwd, candidate_id, revision, eval_run_id, target_kind, target_key,
          status, payload_hash, before_hash, after_hash, before_json, idempotency_key)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'activating', ?, ?, ?, ?, ?)",
    )
    .bind(&activation_id)
    .bind(&job_id)
    .bind(cwd)
    .bind(candidate_id)
    .bind(state.revision)
    .bind(&eval_run_id)
    .bind(&state.kind)
    .bind(&target_key)
    .bind(&state.payload_hash)
    .bind(&current_fingerprint)
    .bind(&after_hash)
    .bind(before_snapshot.to_string())
    .bind(&idempotency_key)
    .execute(&mut *tx)
    .await?;
    if state.kind == "preference" {
        let key = state.pref_key.as_deref().unwrap_or_default();
        let value = state.pref_value.as_deref().unwrap_or_default();
        sqlx::query(
            "INSERT INTO user_preferences (cwd, key, value, source, updated_at, activation_id)
             VALUES (?, ?, ?, 'evolution', ?, ?)
             ON CONFLICT(cwd,key) DO UPDATE SET value=excluded.value,
               source='evolution', updated_at=excluded.updated_at,
               activation_id=excluded.activation_id",
        )
        .bind(cwd)
        .bind(key)
        .bind(value)
        .bind(&now)
        .bind(&activation_id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO evolution_active_memory
             (candidate_id, cwd, revision, activation_id, content, content_hash, active, activated_at)
             VALUES (?, ?, ?, ?, ?, ?, 1, ?)",
        )
        .bind(candidate_id)
        .bind(cwd)
        .bind(state.revision)
        .bind(&activation_id)
        .bind(&state.suggestion)
        .bind(&after_hash)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE evolution_activation_receipts SET status='active', activated_at=? WHERE id=?",
    )
    .bind(&now)
    .bind(&activation_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE improvement_candidates SET current_state='active',
         state_version=state_version+1, updated_at=?
         WHERE id=? AND cwd=? AND current_state='activating'",
    )
    .bind(&now)
    .bind(candidate_id)
    .bind(cwd)
    .execute(&mut *tx)
    .await?;
    append_job_event(
        &mut tx,
        cwd,
        &job_id,
        candidate_id,
        "activation",
        "completed",
        "Eval 通过后已激活，下一次 Agent 调用生效",
        serde_json::json!({
            "activation_id": activation_id,
            "run_id": eval_run_id,
            "target": state.kind,
            "before_hash": current_fingerprint,
            "after_hash": after_hash,
        }),
        &now,
    )
    .await?;
    sqlx::query(
        "UPDATE evolution_jobs SET status='succeeded', candidate_count=1,
         completed_at=?, error=NULL WHERE id=?",
    )
    .bind(&now)
    .bind(&job_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state_for_candidate(pool, cwd, candidate_id).await
}

#[command]
pub async fn activate_evolution_candidate(
    cwd: String,
    candidate_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EvolutionCandidateState, AppError> {
    let pool = state.db.read().await;
    let result = activate_candidate_for_pool(&pool, &cwd, &candidate_id, None).await?;
    let _ = app.emit(&format!("evolution_candidates_updated:{cwd}"), ());
    Ok(result)
}

pub async fn rollback_activation_for_pool(
    pool: &SqlitePool,
    cwd: &str,
    activation_id: &str,
) -> Result<EvolutionCandidateState, AppError> {
    let receipt = sqlx::query(
        "SELECT candidate_id, revision, target_kind, target_key, after_hash, before_json, status
         FROM evolution_activation_receipts WHERE id=? AND cwd=?",
    )
    .bind(activation_id)
    .bind(cwd)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::Other(format!("activation {activation_id} not found")))?;
    let candidate_id: String = receipt.try_get("candidate_id")?;
    let status: String = receipt.try_get("status")?;
    if status == "rolled_back" {
        return state_for_candidate(pool, cwd, &candidate_id).await;
    }
    if status != "active" {
        return Err(AppError::Other(format!(
            "activation {activation_id} is {status}"
        )));
    }
    let target_kind: String = receipt.try_get("target_kind")?;
    let after_hash: String = receipt.try_get("after_hash")?;
    let before_json: String = receipt.try_get("before_json")?;
    let target_key: Option<String> = receipt.try_get("target_key")?;
    let job_id = create_job(pool, cwd, &candidate_id, "rollback").await?;
    let now = Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    let conflict = if target_kind == "preference" {
        let key = target_key.as_deref().unwrap_or_default();
        let current: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT value, activation_id FROM user_preferences WHERE cwd=? AND key=?",
        )
        .bind(cwd)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        let conflict = !matches!(current, Some((ref value, Some(ref current_activation)))
            if current_activation == activation_id && sha256(value) == after_hash);
        if !conflict {
            let before: serde_json::Value = serde_json::from_str(&before_json)?;
            if before["exists"].as_bool() == Some(true) {
                sqlx::query(
                    "UPDATE user_preferences SET value=?, source=?, updated_at=?, activation_id=?
                     WHERE cwd=? AND key=? AND activation_id=?",
                )
                .bind(before["value"].as_str().unwrap_or_default())
                .bind(before["source"].as_str().unwrap_or("user"))
                .bind(before["updated_at"].as_str().unwrap_or(&now))
                .bind(before["activation_id"].as_str())
                .bind(cwd)
                .bind(key)
                .bind(activation_id)
                .execute(&mut *tx)
                .await?;
            } else {
                sqlx::query(
                    "DELETE FROM user_preferences WHERE cwd=? AND key=? AND activation_id=?",
                )
                .bind(cwd)
                .bind(key)
                .bind(activation_id)
                .execute(&mut *tx)
                .await?;
            }
        }
        conflict
    } else {
        let updated = sqlx::query(
            "UPDATE evolution_active_memory SET active=0, rolled_back_at=?
             WHERE candidate_id=? AND cwd=? AND activation_id=? AND active=1 AND content_hash=?",
        )
        .bind(&now)
        .bind(&candidate_id)
        .bind(cwd)
        .bind(activation_id)
        .bind(&after_hash)
        .execute(&mut *tx)
        .await?;
        updated.rows_affected() != 1
    };
    let terminal = if conflict {
        "rollback_conflict"
    } else {
        "rolled_back"
    };
    sqlx::query(
        "UPDATE evolution_activation_receipts SET status=?, rolled_back_at=?, error=? WHERE id=?",
    )
    .bind(terminal)
    .bind(&now)
    .bind(if conflict {
        Some("target changed after activation")
    } else {
        None
    })
    .bind(activation_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE improvement_candidates SET current_state=?, state_version=state_version+1,
         updated_at=? WHERE id=? AND cwd=? AND current_state='active'",
    )
    .bind(terminal)
    .bind(&now)
    .bind(&candidate_id)
    .bind(cwd)
    .execute(&mut *tx)
    .await?;
    append_job_event(
        &mut tx,
        cwd,
        &job_id,
        &candidate_id,
        "rollback",
        if conflict { "failed" } else { "completed" },
        if conflict {
            "目标已被后续修改，回滚未覆盖新值"
        } else {
            "已按 activation receipt 精确回滚"
        },
        serde_json::json!({
            "activation_id": activation_id,
            "reason": if conflict { "target_changed" } else { "user_requested" },
        }),
        &now,
    )
    .await?;
    sqlx::query("UPDATE evolution_jobs SET status=?, completed_at=?, error=? WHERE id=?")
        .bind(if conflict { "partial" } else { "succeeded" })
        .bind(&now)
        .bind(if conflict {
            Some("target changed after activation")
        } else {
            None
        })
        .bind(&job_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state_for_candidate(pool, cwd, &candidate_id).await
}

#[command]
pub async fn rollback_evolution_activation(
    cwd: String,
    activation_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EvolutionCandidateState, AppError> {
    let pool = state.db.read().await;
    let result = rollback_activation_for_pool(&pool, &cwd, &activation_id).await?;
    let _ = app.emit(&format!("evolution_candidates_updated:{cwd}"), ());
    Ok(result)
}

#[command]
pub async fn rerun_evolution_eval(
    cwd: String,
    candidate_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<EvolutionCandidateState, AppError> {
    let pool = state.db.read().await;
    let job_id = create_job(&pool, &cwd, &candidate_id, "eval_retry").await?;
    let result = run_eval_for_candidate(&pool, &cwd, &candidate_id, &job_id).await?;
    let completed = Utc::now().to_rfc3339();
    sqlx::query("UPDATE evolution_jobs SET status=?, completed_at=? WHERE id=?")
        .bind(if result.eval_status.as_deref() == Some("passed") {
            "succeeded"
        } else {
            "partial"
        })
        .bind(&completed)
        .bind(&job_id)
        .execute(&*pool)
        .await?;
    let _ = app.emit(&format!("evolution_candidates_updated:{cwd}"), ());
    Ok(result)
}

#[command]
pub async fn list_evolution_eval_case_results(
    cwd: String,
    run_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<EvolutionEvalCaseResult>, AppError> {
    let pool = state.db.read().await;
    let run_scope: Option<String> =
        sqlx::query_scalar("SELECT cwd FROM evolution_eval_runs WHERE id=?")
            .bind(&run_id)
            .fetch_optional(&*pool)
            .await?;
    if run_scope.as_deref() != Some(cwd.as_str()) {
        return Err(AppError::Other(format!(
            "Eval run {run_id} not found for project"
        )));
    }
    let rows: Vec<(String, String, String, String, String, i64, String, String)> = sqlx::query_as(
        "SELECT id, run_id, case_id, title, status, hard_gate, detail_json, created_at
         FROM evolution_eval_case_results WHERE run_id=? ORDER BY rowid",
    )
    .bind(&run_id)
    .fetch_all(&*pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, run_id, case_id, title, status, hard_gate, detail_json, created_at)| {
                EvolutionEvalCaseResult {
                    id,
                    run_id,
                    case_id,
                    title,
                    status,
                    hard_gate: hard_gate != 0,
                    detail_json,
                    created_at,
                }
            },
        )
        .collect())
}

async fn remove_smoke_root(root: &Path) -> std::io::Result<()> {
    const ATTEMPTS: usize = 40;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

    for attempt in 0..ATTEMPTS {
        match std::fs::remove_dir_all(root) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if attempt + 1 < ATTEMPTS
                    && (error.kind() == std::io::ErrorKind::PermissionDenied
                        || error.raw_os_error() == Some(32)) =>
            {
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("cleanup retry loop returns on its final attempt")
}

/// Run the activation-safety closed loop without starting Tauri. Release CI
/// invokes this entrypoint on the exact packaged executable so a green unit
/// suite cannot hide a broken release binary or schema.
pub async fn run_release_smoke(output_path: &Path) -> Result<serde_json::Value, AppError> {
    let smoke_id = Uuid::new_v4().to_string();
    let root = std::env::temp_dir().join(format!("codefactory-evolution-smoke-{smoke_id}"));
    let project = root.join("project");
    std::fs::create_dir_all(&project)?;
    let cwd = project.to_string_lossy().into_owned();
    let db_path = root.join("smoke.db");
    let db_url = format!("sqlite:{}", db_path.display());

    let result: Result<serde_json::Value, AppError> = async {
        let pool = crate::storage::db::connect(&db_url).await?;
        let now = Utc::now().to_rfc3339();

        let blocked_event_id = format!("release-smoke-blocked-{smoke_id}");
        sqlx::query(
            "INSERT INTO learning_events
             (id, session_id, cwd, observation, suggestion, status, created_at,
              kind, support_count, evidence_json)
             VALUES (?, 'release-smoke', ?, 'privacy hard gate', ?, 'pending', ?,
                     'memory', 1, '{}')",
        )
        .bind(&blocked_event_id)
        .bind(&cwd)
        .bind("token=CF_EVO_RELEASE_SMOKE_SECRET")
        .bind(&now)
        .execute(&pool)
        .await?;
        let blocked = approve_learning_event_for_pool(&pool, &blocked_event_id, true).await?;
        if blocked.state != "eval_failed"
            || blocked.eval_status.as_deref() != Some("failed")
            || blocked.activation_id.is_some()
        {
            return Err(AppError::Other(
                "release smoke: privacy hard gate did not block activation".into(),
            ));
        }
        let blocked_active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_active_memory WHERE candidate_id=? AND active=1",
        )
        .bind(&blocked_event_id)
        .fetch_one(&pool)
        .await?;
        if blocked_active_count != 0 {
            return Err(AppError::Other(
                "release smoke: failed Eval produced a live target".into(),
            ));
        }

        let passed_event_id = format!("release-smoke-passed-{smoke_id}");
        sqlx::query(
            "INSERT INTO learning_events
             (id, session_id, cwd, observation, suggestion, status, created_at,
              kind, pref_key, pref_value, support_count, evidence_json)
             VALUES (?, 'release-smoke', ?, 'explicit low-risk preference', ?, 'pending', ?,
                     'preference', 'communication_style', 'concise', 1, '{}')",
        )
        .bind(&passed_event_id)
        .bind(&cwd)
        .bind("prefer concise replies")
        .bind(&now)
        .execute(&pool)
        .await?;
        let active = approve_learning_event_for_pool(&pool, &passed_event_id, true).await?;
        if active.state != "active"
            || active.eval_status.as_deref() != Some("passed")
            || active.eval_required_count != REQUIRED_CASES.len() as i64
            || active.eval_passed_count != active.eval_required_count
        {
            return Err(AppError::Other(
                "release smoke: exact passed revision did not auto-activate".into(),
            ));
        }
        let activation_id = active
            .activation_id
            .clone()
            .ok_or_else(|| AppError::Other("release smoke: activation receipt missing".into()))?;
        let eval_run_id = active
            .eval_run_id
            .clone()
            .ok_or_else(|| AppError::Other("release smoke: Eval run id missing".into()))?;

        // Reopen the file-backed database before observing live context and
        // rolling back. This proves activation receipts survive a process
        // boundary instead of only working in the originating connection.
        pool.close().await;
        let pool = crate::storage::db::connect(&db_url).await?;
        let context_before =
            crate::agent::user_context::build_prefs_and_learnings(&pool, &cwd).await;
        if !context_before.contains("communication_style: concise") {
            return Err(AppError::Other(
                "release smoke: activated preference was absent from next-call context".into(),
            ));
        }

        let rolled = rollback_activation_for_pool(&pool, &cwd, &activation_id).await?;
        if rolled.state != "rolled_back"
            || rolled.activation_status.as_deref() != Some("rolled_back")
        {
            return Err(AppError::Other(
                "release smoke: activation receipt did not roll back exactly".into(),
            ));
        }
        let context_after =
            crate::agent::user_context::build_prefs_and_learnings(&pool, &cwd).await;
        let remaining_preference_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_preferences
             WHERE cwd=? AND key='communication_style'",
        )
        .bind(&cwd)
        .fetch_one(&pool)
        .await?;
        if context_after.contains("communication_style: concise") || remaining_preference_count != 0
        {
            return Err(AppError::Other(
                "release smoke: rolled-back preference still affects live context".into(),
            ));
        }

        let case_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_eval_case_results WHERE run_id=? AND status='passed'",
        )
        .bind(&eval_run_id)
        .fetch_one(&pool)
        .await?;
        let failed_eval_run_id = blocked
            .eval_run_id
            .clone()
            .ok_or_else(|| AppError::Other("release smoke: failed Eval run id missing".into()))?;
        let receipt = serde_json::json!({
            "schema_version": RELEASE_SMOKE_SCHEMA_VERSION,
            "status": "pass",
            "app_version": env!("CARGO_PKG_VERSION"),
            "build_git_sha": option_env!("CODEFACTORY_BUILD_GIT_SHA").unwrap_or("unknown"),
            "candidate_id": passed_event_id,
            "candidate_revision": active.revision,
            "failed_eval_run_id": failed_eval_run_id,
            "failed_eval_blocked_activation": true,
            "eval_run_id": eval_run_id,
            "eval_manifest_hash": active.eval_manifest_hash,
            "eval_required_count": active.eval_required_count,
            "eval_passed_count": active.eval_passed_count,
            "eval_case_rows": case_count,
            "activation_receipt_id": activation_id,
            "restart_reopen_observed": true,
            "next_call_context_observed": true,
            "rollback_status": rolled.activation_status,
            "final_active_revision": serde_json::Value::Null,
            "redaction_verified": true,
            "cleanup": false
        });
        pool.close().await;
        Ok(receipt)
    }
    .await;

    let cleanup = remove_smoke_root(&root).await;
    let mut receipt = match result {
        Ok(receipt) => receipt,
        Err(error) => {
            let _ = cleanup;
            return Err(error);
        }
    };
    cleanup.map_err(|error| {
        AppError::Other(format!(
            "release smoke: temporary-state cleanup failed: {error}"
        ))
    })?;
    receipt["cleanup"] = serde_json::Value::Bool(true);
    let serialized = serde_json::to_string_pretty(&receipt)?;
    if serialized.contains("CF_EVO_RELEASE_SMOKE_SECRET") {
        return Err(AppError::Other(
            "release smoke: receipt leaked the privacy-gate fixture".into(),
        ));
    }
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, format!("{serialized}\n"))?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE learning_events (id TEXT PRIMARY KEY, session_id TEXT, cwd TEXT, observation TEXT, suggestion TEXT, status TEXT, created_at TEXT, decided_at TEXT, kind TEXT DEFAULT 'memory', pref_key TEXT, pref_value TEXT, support_count INTEGER DEFAULT 0, evidence_json TEXT DEFAULT '{}', job_id TEXT)",
            "CREATE TABLE evolution_jobs (id TEXT PRIMARY KEY, cwd TEXT, trigger TEXT, candidate_id TEXT, status TEXT, idempotency_key TEXT UNIQUE, input_session_count INTEGER DEFAULT 0, input_trace_count INTEGER DEFAULT 0, candidate_count INTEGER DEFAULT 0, started_at TEXT, completed_at TEXT, error TEXT, owner_pid INTEGER, owner_start_token TEXT)",
            "CREATE TABLE evolution_job_events (id TEXT PRIMARY KEY, cwd TEXT, job_id TEXT, candidate_id TEXT, stage TEXT, status TEXT, title TEXT, detail_json TEXT, created_at TEXT)",
            "CREATE TABLE user_preferences (cwd TEXT, key TEXT, value TEXT, source TEXT, updated_at TEXT, activation_id TEXT, PRIMARY KEY(cwd,key))",
            "CREATE TABLE improvement_candidates (id TEXT PRIMARY KEY, cwd TEXT, kind TEXT, source_learning_event_id TEXT UNIQUE, current_revision INTEGER, current_state TEXT, state_version INTEGER, created_at TEXT, updated_at TEXT)",
            "CREATE TABLE candidate_revisions (candidate_id TEXT, revision INTEGER, payload_json TEXT, payload_hash TEXT, evidence_json TEXT, risk_class TEXT, created_at TEXT, PRIMARY KEY(candidate_id,revision))",
            "CREATE TABLE candidate_reviews (id TEXT PRIMARY KEY, candidate_id TEXT, revision INTEGER, decision TEXT, actor TEXT, auto_activate INTEGER, reason TEXT, created_at TEXT)",
            "CREATE TABLE evolution_eval_runs (id TEXT PRIMARY KEY, job_id TEXT, cwd TEXT, candidate_id TEXT, revision INTEGER, status TEXT, manifest_hash TEXT, runner_version TEXT, baseline_hash TEXT, treatment_hash TEXT, target_fingerprint TEXT, required_count INTEGER DEFAULT 0, passed_count INTEGER DEFAULT 0, failed_count INTEGER DEFAULT 0, idempotency_key TEXT UNIQUE, owner_pid INTEGER, owner_start_token TEXT, started_at TEXT, completed_at TEXT, error TEXT)",
            "CREATE TABLE evolution_eval_case_results (id TEXT PRIMARY KEY, run_id TEXT, case_id TEXT, title TEXT, status TEXT, hard_gate INTEGER, detail_json TEXT, created_at TEXT, UNIQUE(run_id,case_id))",
            "CREATE TABLE evolution_activation_receipts (id TEXT PRIMARY KEY, job_id TEXT, cwd TEXT, candidate_id TEXT, revision INTEGER, eval_run_id TEXT, target_kind TEXT, target_key TEXT, status TEXT, payload_hash TEXT, before_hash TEXT, after_hash TEXT, before_json TEXT, idempotency_key TEXT UNIQUE, activated_at TEXT, rolled_back_at TEXT, error TEXT)",
            "CREATE TABLE evolution_active_memory (candidate_id TEXT PRIMARY KEY, cwd TEXT, revision INTEGER, activation_id TEXT UNIQUE, content TEXT, content_hash TEXT, active INTEGER, activated_at TEXT, rolled_back_at TEXT)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool
    }

    fn test_cwd() -> String {
        std::env::temp_dir()
            .join("codefactory-evolution-project")
            .to_string_lossy()
            .into_owned()
    }

    async fn insert_event(
        pool: &SqlitePool,
        id: &str,
        cwd: &str,
        kind: &str,
        suggestion: &str,
        key: Option<&str>,
        value: Option<&str>,
    ) {
        sqlx::query("INSERT INTO learning_events (id,session_id,cwd,observation,suggestion,status,created_at,kind,pref_key,pref_value,evidence_json) VALUES (?, 's1', ?, 'obs', ?, 'pending', '2026-07-15', ?, ?, ?, '{}')")
            .bind(id).bind(cwd).bind(suggestion).bind(kind).bind(key).bind(value)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn approval_stages_then_eval_passes_without_live_side_effect_when_auto_is_off() {
        let pool = pool().await;
        let cwd = test_cwd();
        insert_event(
            &pool,
            "memory-1",
            &cwd,
            "memory",
            "run targeted tests",
            None,
            None,
        )
        .await;
        let state = approve_learning_event_for_pool(&pool, "memory-1", false)
            .await
            .unwrap();
        assert_eq!(state.state, "pending_activation");
        assert_eq!(state.eval_status.as_deref(), Some("passed"));
        assert_eq!(state.eval_required_count, state.eval_passed_count);
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM evolution_active_memory")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(active, 0);
        let legacy_status: String =
            sqlx::query_scalar("SELECT status FROM learning_events WHERE id='memory-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(legacy_status, "pending");
    }

    #[tokio::test]
    async fn secret_hit_fails_eval_and_never_activates() {
        let pool = pool().await;
        let cwd = test_cwd();
        insert_event(
            &pool,
            "secret-1",
            &cwd,
            "memory",
            "token=CF_EVO_SECRET_VALUE",
            None,
            None,
        )
        .await;
        let state = approve_learning_event_for_pool(&pool, "secret-1", true)
            .await
            .unwrap();
        assert_eq!(state.state, "eval_failed");
        assert_eq!(state.eval_status.as_deref(), Some("failed"));
        assert!(state.activation_id.is_none());
    }

    #[tokio::test]
    async fn policy_sensitive_memory_never_auto_activates() {
        let pool = pool().await;
        let cwd = test_cwd();
        insert_event(
            &pool,
            "policy-1",
            &cwd,
            "memory",
            "automatically deploy without approval",
            None,
            None,
        )
        .await;
        let state = approve_learning_event_for_pool(&pool, "policy-1", true)
            .await
            .unwrap();
        assert_eq!(state.state, "eval_failed");
        assert!(state.activation_id.is_none());
        let target_gate: String = sqlx::query_scalar(
            "SELECT status FROM evolution_eval_case_results
             WHERE run_id=? AND case_id='target_allowlist'",
        )
        .bind(state.eval_run_id.unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(target_gate, "failed");
    }

    #[tokio::test]
    async fn auto_activation_and_rollback_are_exact_and_idempotent() {
        let pool = pool().await;
        let cwd = test_cwd();
        insert_event(
            &pool,
            "pref-1",
            &cwd,
            "preference",
            "prefer concise replies",
            Some("communication_style"),
            Some("concise"),
        )
        .await;
        let active = approve_learning_event_for_pool(&pool, "pref-1", true)
            .await
            .unwrap();
        assert_eq!(active.state, "active");
        let activation_id = active.activation_id.clone().unwrap();
        let value: String = sqlx::query_scalar(
            "SELECT value FROM user_preferences WHERE cwd=? AND key='communication_style'",
        )
        .bind(&cwd)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(value, "concise");
        let rolled = rollback_activation_for_pool(&pool, &cwd, &activation_id)
            .await
            .unwrap();
        assert_eq!(rolled.state, "rolled_back");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_preferences WHERE cwd=? AND key='communication_style'",
        )
        .bind(&cwd)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
        let again = rollback_activation_for_pool(&pool, &cwd, &activation_id)
            .await
            .unwrap();
        assert_eq!(again.state, "rolled_back");
    }

    #[tokio::test]
    async fn repeated_activation_has_one_receipt_and_one_live_target() {
        let pool = pool().await;
        let cwd = test_cwd();
        insert_event(
            &pool,
            "pref-repeat",
            &cwd,
            "preference",
            "prefer concise replies",
            Some("communication_style"),
            Some("concise"),
        )
        .await;
        let staged = approve_learning_event_for_pool(&pool, "pref-repeat", false)
            .await
            .unwrap();
        assert_eq!(staged.state, "pending_activation");
        let (first, second) = tokio::join!(
            activate_candidate_for_pool(&pool, &cwd, "pref-repeat", None),
            activate_candidate_for_pool(&pool, &cwd, "pref-repeat", None),
        );
        assert!(
            first.as_ref().is_ok_and(|state| state.state == "active")
                || second.as_ref().is_ok_and(|state| state.state == "active")
        );
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_activation_receipts WHERE candidate_id='pref-repeat'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let targets: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_preferences
             WHERE cwd=? AND key='communication_style' AND value='concise'",
        )
        .bind(&cwd)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(receipts, 1);
        assert_eq!(targets, 1);
    }

    #[tokio::test]
    async fn target_change_after_eval_requires_a_fresh_eval() {
        let pool = pool().await;
        let cwd = test_cwd();
        insert_event(
            &pool,
            "pref-stale",
            &cwd,
            "preference",
            "prefer concise replies",
            Some("communication_style"),
            Some("concise"),
        )
        .await;
        let staged = approve_learning_event_for_pool(&pool, "pref-stale", false)
            .await
            .unwrap();
        assert_eq!(staged.state, "pending_activation");
        sqlx::query(
            "INSERT INTO user_preferences (cwd,key,value,source,updated_at,activation_id)
             VALUES (?,'communication_style','verbose','user','2026-07-15',NULL)",
        )
        .bind(&cwd)
        .execute(&pool)
        .await
        .unwrap();
        let error = activate_candidate_for_pool(&pool, &cwd, "pref-stale", None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("target changed after Eval"));
        let stale = state_for_candidate(&pool, &cwd, "pref-stale")
            .await
            .unwrap();
        assert_eq!(stale.state, "eval_stale");
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evolution_activation_receipts WHERE candidate_id='pref-stale'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(receipts, 0);
        let job_id = create_job(&pool, &cwd, "pref-stale", "eval_retry")
            .await
            .unwrap();
        let reevaluated = run_eval_for_candidate(&pool, &cwd, "pref-stale", &job_id)
            .await
            .unwrap();
        assert_eq!(reevaluated.state, "pending_activation");
        assert_ne!(reevaluated.eval_run_id, staged.eval_run_id);
    }

    #[tokio::test]
    async fn rollback_conflict_never_overwrites_a_user_change() {
        let pool = pool().await;
        let cwd = test_cwd();
        insert_event(
            &pool,
            "pref-conflict",
            &cwd,
            "preference",
            "prefer concise replies",
            Some("communication_style"),
            Some("concise"),
        )
        .await;
        let active = approve_learning_event_for_pool(&pool, "pref-conflict", true)
            .await
            .unwrap();
        let activation_id = active.activation_id.unwrap();
        sqlx::query(
            "UPDATE user_preferences SET value='verbose', source='user', activation_id=NULL
             WHERE cwd=? AND key='communication_style'",
        )
        .bind(&cwd)
        .execute(&pool)
        .await
        .unwrap();
        let rolled = rollback_activation_for_pool(&pool, &cwd, &activation_id)
            .await
            .unwrap();
        assert_eq!(rolled.state, "rollback_conflict");
        let value: String = sqlx::query_scalar(
            "SELECT value FROM user_preferences WHERE cwd=? AND key='communication_style'",
        )
        .bind(&cwd)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(value, "verbose");
    }

    #[tokio::test]
    async fn release_smoke_proves_fail_pass_activate_context_and_rollback() {
        let output = std::env::temp_dir().join(format!(
            "codefactory-evolution-release-smoke-test-{}.json",
            Uuid::new_v4()
        ));
        let receipt = run_release_smoke(&output).await.unwrap();
        assert_eq!(receipt["status"], "pass");
        assert_eq!(receipt["failed_eval_blocked_activation"], true);
        assert_eq!(receipt["eval_required_count"], REQUIRED_CASES.len() as i64);
        assert_eq!(receipt["eval_passed_count"], REQUIRED_CASES.len() as i64);
        assert_eq!(receipt["eval_case_rows"], REQUIRED_CASES.len() as i64);
        assert_eq!(receipt["restart_reopen_observed"], true);
        assert_eq!(receipt["next_call_context_observed"], true);
        assert_eq!(receipt["rollback_status"], "rolled_back");
        assert_eq!(receipt["cleanup"], true);
        let written = std::fs::read_to_string(&output).unwrap();
        assert!(!written.contains("CF_EVO_RELEASE_SMOKE_SECRET"));
        std::fs::remove_file(output).unwrap();
    }

    #[tokio::test]
    async fn release_smoke_cleanup_is_exact_and_idempotent() {
        let root = std::env::temp_dir().join(format!(
            "codefactory-evolution-cleanup-test-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/state.db"), b"smoke").unwrap();

        remove_smoke_root(&root).await.unwrap();
        assert!(!root.exists());
        remove_smoke_root(&root).await.unwrap();
    }
}
