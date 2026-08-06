// SPDX-License-Identifier: Apache-2.0
//! Evidence Pack auto-collection — Phase 6.
//!
//! Collects all artefacts produced during a spec implementation run and
//! writes them to `.codefactory/evidence/{spec_req_id}/{timestamp}/`.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::process::Command;
use tauri::{AppHandle, Emitter};

use crate::errors::AppError;
use crate::storage::tasks;
use crate::util::no_window::NoWindow;

// ── Data structures ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePackMeta {
    pub spec_req_id: String,
    pub spec_title: String,
    pub task_run_ids: Vec<String>,
    pub session_id: String,
    pub created_at: String,
    pub completed_at: String,
    pub status: String, // "passed" | "failed" | "partial"
    pub total_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
    pub total_tool_calls: usize,
    pub files_changed: usize,
    pub verification_passed: bool,
    pub total_tokens: i64,
    pub duration_minutes: f64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePack {
    pub manifest: EvidencePackMeta,
    pub summary_md: String,
    pub tool_calls: Vec<serde_json::Value>,
    pub knowledge_refs: Vec<serde_json::Value>,
    pub files_changed: Vec<serde_json::Value>,
    pub verification: Vec<serde_json::Value>,
    pub git_commits: Vec<serde_json::Value>,
    pub ai_collaboration: serde_json::Value,
}

// ── Core collection ───────────────────────────────────────────────────────────

pub async fn collect_evidence_pack(
    cwd: &str,
    spec_req_id: &str,
    spec_title: &str,
    session_id: &str,
    task_run_ids: &[String],
    pool: &SqlitePool,
) -> Result<String, AppError> {
    let created_at = Utc::now();
    let timestamp = created_at.format("%Y-%m-%dT%H-%M-%S").to_string();

    // 1. Create evidence directory
    let evidence_dir = format!(
        "{}/.codefactory/evidence/{}/{}",
        cwd, spec_req_id, timestamp
    );
    std::fs::create_dir_all(&evidence_dir)?;

    // 2. Query all task_runs by id
    let mut task_runs = Vec::new();
    for id in task_run_ids {
        if let Some(t) = tasks::get_task(pool, id).await? {
            task_runs.push(t);
        }
    }

    // 3. Query all messages for the session
    let messages: Vec<(String, String, String, Option<String>, Option<i64>, Option<i64>, i64)> =
        sqlx::query_as(
            "SELECT id, role, content, tool_calls, input_tokens, output_tokens, created_at \
             FROM messages WHERE session_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(session_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    // 4. Extract tool calls from messages
    let mut tool_calls_out: Vec<serde_json::Value> = Vec::new();
    let mut total_tokens: i64 = 0;

    for (msg_id, role, _content, tool_calls_json, input_tok, output_tok, ts_ms) in &messages {
        total_tokens += input_tok.unwrap_or(0) + output_tok.unwrap_or(0);

        if role == "assistant" {
            if let Some(ref tc_json) = tool_calls_json {
                if let Ok(tc_arr) = serde_json::from_str::<serde_json::Value>(tc_json) {
                    if let Some(arr) = tc_arr.as_array() {
                        let ts = chrono::DateTime::<Utc>::from_timestamp_millis(*ts_ms)
                            .map(|d| d.to_rfc3339())
                            .unwrap_or_default();
                        for tc in arr {
                            let tool_name = tc
                                .get("name")
                                .or_else(|| tc.get("function").and_then(|f| f.get("name")))
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let args = tc
                                .get("input")
                                .cloned()
                                .or_else(|| {
                                    tc.get("function")
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|a| {
                                            if let Some(s) = a.as_str() {
                                                serde_json::from_str(s).ok()
                                            } else {
                                                Some(a.clone())
                                            }
                                        })
                                })
                                .unwrap_or(serde_json::Value::Null);
                            let args = crate::trajectory::redact_json(&args);

                            tool_calls_out.push(serde_json::json!({
                                "tool_name": tool_name,
                                "args": args,
                                "result_preview": "",
                                "timestamp": ts,
                                "message_id": msg_id,
                                "task_id": null,
                            }));
                        }
                    }
                }
            }
        }
    }

    // Prefer the normalized lifecycle table when available. It is populated by
    // the real AgentLoop and already contains bounded, redacted payloads.
    let tc_records: Vec<(
        String,
        String,
        Option<String>,
        String,
        Option<i64>,
        Option<String>,
        String,
        i64,
    )> = sqlx::query_as(
        "SELECT tc.tool_name, tc.arguments, tc.result, tc.status, tc.duration_ms, \
                    tc.error, tc.message_id, tc.created_at \
             FROM tool_calls tc \
             JOIN messages m ON m.id = tc.message_id \
             WHERE m.session_id = ? ORDER BY tc.created_at ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if !tc_records.is_empty() {
        // Prefer richer normalized records if available.
        tool_calls_out.clear();
        for (tool_name, arguments, result, status, duration_ms, error, message_id, created_at) in
            &tc_records
        {
            let args = serde_json::from_str(arguments)
                .unwrap_or(serde_json::Value::String(arguments.clone()));
            let args = crate::trajectory::redact_json(&args);
            let result_preview =
                crate::trajectory::redact_text(result.as_deref().unwrap_or(""), 200);
            let error_preview = error
                .as_deref()
                .map(|value| crate::trajectory::redact_text(value, 200));
            let timestamp = chrono::DateTime::<Utc>::from_timestamp_millis(*created_at)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default();
            tool_calls_out.push(serde_json::json!({
                "tool_name": tool_name,
                "args": args,
                "result_preview": result_preview,
                "status": status,
                "error": error_preview,
                "timestamp": timestamp,
                "duration_ms": duration_ms,
                "message_id": message_id,
                "task_id": null,
            }));
        }
    }

    let total_tool_calls = tool_calls_out.len();

    // 5. Write tool_calls.jsonl
    {
        let mut jsonl = String::new();
        for tc in &tool_calls_out {
            jsonl.push_str(&serde_json::to_string(tc)?);
            jsonl.push('\n');
        }
        std::fs::write(format!("{}/tool_calls.jsonl", evidence_dir), jsonl)?;
    }

    // 6. Collect knowledge retrieval events. These are the field-level proof
    // that a task used a scoped personal knowledge library instead of an
    // invisible prompt injection.
    let mut seen_retrieval_ids: HashSet<String> = HashSet::new();
    let mut knowledge_refs_out: Vec<serde_json::Value> = Vec::new();
    let mut retrieval_rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
        String,
        i64,
    )> = sqlx::query_as(
        "SELECT id, session_id, task_id, query, filters_json, result_refs_json, created_at, latency_ms
         FROM retrieval_events
         WHERE session_id = ?
         ORDER BY created_at ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for task in &task_runs {
        let mut task_rows: Vec<(
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
            String,
            i64,
        )> = sqlx::query_as(
            "SELECT id, session_id, task_id, query, filters_json, result_refs_json, created_at, latency_ms
             FROM retrieval_events
             WHERE task_id = ?
             ORDER BY created_at ASC",
        )
        .bind(&task.id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        retrieval_rows.append(&mut task_rows);
    }
    for (id, event_session_id, task_id, query, filters_json, refs_json, created_at, latency_ms) in
        retrieval_rows
    {
        if !seen_retrieval_ids.insert(id.clone()) {
            continue;
        }
        let filters: serde_json::Value =
            serde_json::from_str(&filters_json).unwrap_or(serde_json::Value::Null);
        let refs: serde_json::Value =
            serde_json::from_str(&refs_json).unwrap_or(serde_json::Value::Array(Vec::new()));
        knowledge_refs_out.push(serde_json::json!({
            "id": id,
            "session_id": event_session_id,
            "task_id": task_id,
            "query": query,
            "filters": filters,
            "result_refs": refs,
            "created_at": created_at,
            "latency_ms": latency_ms,
        }));
    }
    std::fs::write(
        format!("{}/knowledge_refs.json", evidence_dir),
        serde_json::to_string_pretty(&knowledge_refs_out)?,
    )?;

    // 7. Collect files changed by scanning tool calls for write_file/edit_file
    let mut changed_paths: Vec<String> = Vec::new();
    for tc in &tool_calls_out {
        let name = tc.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(name, "write_file" | "edit_file" | "WriteFile" | "EditFile") {
            let args = tc.get("args");
            if let Some(path) = args
                .and_then(|a| a.get("path").or_else(|| a.get("file_path")))
                .and_then(|v| v.as_str())
            {
                if !changed_paths.contains(&path.to_string()) {
                    changed_paths.push(path.to_string());
                }
            }
        }
    }

    let mut files_changed_out: Vec<serde_json::Value> = Vec::new();
    for file_path in &changed_paths {
        // Try to get git diff for this file
        let diff = Command::new("git").no_window()
            .current_dir(cwd)
            .args(["diff", "HEAD~1", "--", file_path])
            .output()
            .ok()
            .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
            .unwrap_or_default();

        files_changed_out.push(serde_json::json!({
            "path": file_path,
            "diff": diff,
        }));
    }

    let files_changed_count = files_changed_out.len();
    std::fs::write(
        format!("{}/files_changed.json", evidence_dir),
        serde_json::to_string_pretty(&files_changed_out)?,
    )?;

    // 8. Collect verification results
    let mut verification_out: Vec<serde_json::Value> = Vec::new();
    let mut all_verif_passed = true;
    for task in &task_runs {
        if let Some(ref vr_json) = task.verification_results {
            if let Ok(vr_arr) = serde_json::from_str::<serde_json::Value>(vr_json) {
                if let Some(arr) = vr_arr.as_array() {
                    for item in arr {
                        if let Some(false) = item.get("passed").and_then(|v| v.as_bool()) {
                            all_verif_passed = false;
                        }
                        let mut entry = item.clone();
                        if let Some(obj) = entry.as_object_mut() {
                            obj.insert("task_id".to_string(), serde_json::json!(task.id));
                            obj.insert("task_title".to_string(), serde_json::json!(task.title));
                        }
                        verification_out.push(entry);
                    }
                }
            }
        }
    }
    if verification_out.is_empty() {
        all_verif_passed = false; // no results = not verified
    }

    std::fs::write(
        format!("{}/verification.json", evidence_dir),
        serde_json::to_string_pretty(&verification_out)?,
    )?;

    // 9. Git commits since session was created
    let session_created: Option<(i64,)> =
        sqlx::query_as("SELECT created_at FROM sessions WHERE id = ?")
            .bind(session_id)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    let mut git_commits_out: Vec<serde_json::Value> = Vec::new();
    if let Some((created_ms,)) = session_created {
        // Convert ms epoch to seconds for git --after
        let secs = created_ms / 1000;
        let dt = chrono::DateTime::<Utc>::from_timestamp(secs, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();

        if !dt.is_empty() {
            let git_log_out = Command::new("git").no_window()
                .current_dir(cwd)
                .args([
                    "log",
                    &format!("--after={}", dt),
                    "--format=%H|%s|%an|%ae|%ai",
                ])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .unwrap_or_default();

            for line in git_log_out.lines() {
                let parts: Vec<&str> = line.splitn(5, '|').collect();
                if parts.len() == 5 {
                    git_commits_out.push(serde_json::json!({
                        "hash": parts[0],
                        "short_hash": &parts[0][..7.min(parts[0].len())],
                        "message": parts[1],
                        "author": parts[2],
                        "email": parts[3],
                        "timestamp": parts[4],
                    }));
                }
            }
        }
    }

    std::fs::write(
        format!("{}/git_commits.json", evidence_dir),
        serde_json::to_string_pretty(&git_commits_out)?,
    )?;

    // 10. AI collaboration metadata
    let model: String = messages
        .iter()
        .find(|(_, role, _, _, _, _, _)| role == "assistant")
        .and_then(|_| None) // model_id not in this query
        .unwrap_or_else(|| "unknown".to_string());

    // Gather assumptions from task descriptions
    let assumptions: Vec<String> = task_runs
        .iter()
        .filter(|t| {
            t.description.contains("assume") || t.description.contains("default")
        })
        .map(|t| format!("[{}] {}", t.title, t.description.chars().take(120).collect::<String>()))
        .collect();

    let review_points: Vec<String> = task_runs
        .iter()
        .filter(|t| t.status == "failed")
        .map(|t| {
            format!(
                "[{}] Failed: {}",
                t.title,
                t.error.as_deref().unwrap_or("unknown error").chars().take(120).collect::<String>()
            )
        })
        .collect();

    // Get the real model_id from the DB
    let model_from_db: String = sqlx::query_as::<_, (String,)>(
        "SELECT model_id FROM sessions WHERE id = ?",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|(m,)| m)
    .unwrap_or(model);

    let ai_collab = serde_json::json!({
        "model": model_from_db,
        "total_tokens": total_tokens,
        "assumptions": assumptions,
        "review_points": review_points,
    });

    std::fs::write(
        format!("{}/ai_collaboration.json", evidence_dir),
        serde_json::to_string_pretty(&ai_collab)?,
    )?;

    // 11. Compute stats for manifest
    let completed_tasks = task_runs.iter().filter(|t| t.status == "completed").count();
    let failed_tasks = task_runs.iter().filter(|t| t.status == "failed").count();
    let total_tasks = task_runs.len();

    let status = if failed_tasks == 0 && completed_tasks == total_tasks && total_tasks > 0 {
        "passed"
    } else if completed_tasks == 0 {
        "failed"
    } else {
        "partial"
    };

    // Duration: from first task started_at to last completed_at
    let first_started = task_runs
        .iter()
        .filter_map(|t| t.started_at.as_deref())
        .min()
        .map(str::to_string);
    let last_completed = task_runs
        .iter()
        .filter_map(|t| t.completed_at.as_deref())
        .max()
        .map(str::to_string);

    let duration_minutes = match (&first_started, &last_completed) {
        (Some(s), Some(e)) => {
            let start = chrono::DateTime::parse_from_rfc3339(s).ok();
            let end = chrono::DateTime::parse_from_rfc3339(e).ok();
            match (start, end) {
                (Some(s), Some(e)) => {
                    let diff = e.signed_duration_since(s);
                    diff.num_seconds() as f64 / 60.0
                }
                _ => 0.0,
            }
        }
        _ => 0.0,
    };

    let completed_at_str = Utc::now().to_rfc3339();

    // 12. Write summary.md following the evidence-pack template
    let summary_md = build_summary_md(
        spec_req_id,
        spec_title,
        session_id,
        status,
        total_tasks,
        completed_tasks,
        failed_tasks,
        total_tool_calls,
        files_changed_count,
        all_verif_passed,
        total_tokens,
        duration_minutes,
        &task_runs,
        &git_commits_out,
        &ai_collab,
        knowledge_refs_out.len(),
    );
    std::fs::write(format!("{}/summary.md", evidence_dir), &summary_md)?;

    // 13. Write manifest.json
    let manifest = EvidencePackMeta {
        spec_req_id: spec_req_id.to_string(),
        spec_title: spec_title.to_string(),
        task_run_ids: task_run_ids.to_vec(),
        session_id: session_id.to_string(),
        created_at: created_at.to_rfc3339(),
        completed_at: completed_at_str,
        status: status.to_string(),
        total_tasks,
        completed_tasks,
        failed_tasks,
        total_tool_calls,
        files_changed: files_changed_count,
        verification_passed: all_verif_passed,
        total_tokens,
        duration_minutes,
        path: evidence_dir.clone(),
    };

    std::fs::write(
        format!("{}/manifest.json", evidence_dir),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    Ok(evidence_dir)
}

fn build_summary_md(
    spec_req_id: &str,
    spec_title: &str,
    session_id: &str,
    status: &str,
    total_tasks: usize,
    completed_tasks: usize,
    failed_tasks: usize,
    total_tool_calls: usize,
    files_changed: usize,
    verification_passed: bool,
    total_tokens: i64,
    duration_minutes: f64,
    task_runs: &[crate::storage::tasks::TaskRun],
    git_commits: &[serde_json::Value],
    ai_collab: &serde_json::Value,
    knowledge_refs_count: usize,
) -> String {
    let verif_str = if verification_passed { "PASSED" } else { "FAILED / NOT RUN" };
    let model = ai_collab.get("model").and_then(|v| v.as_str()).unwrap_or("unknown");
    let assumptions = ai_collab.get("assumptions")
        .and_then(|v| v.as_array())
        .map(|a| a.iter()
            .filter_map(|v| v.as_str())
            .map(|s| format!("- {}", s))
            .collect::<Vec<_>>()
            .join("\n"))
        .unwrap_or_default();
    let review_points = ai_collab.get("review_points")
        .and_then(|v| v.as_array())
        .map(|a| a.iter()
            .filter_map(|v| v.as_str())
            .map(|s| format!("- {}", s))
            .collect::<Vec<_>>()
            .join("\n"))
        .unwrap_or_default();

    let tasks_section = task_runs.iter().map(|t| {
        let verif = t.verification_results.as_deref()
            .map(|_| "verified")
            .unwrap_or("not verified");
        format!(
            "- **[{}]** {} — status: `{}` ({})",
            t.id.chars().take(8).collect::<String>(),
            t.title,
            t.status,
            verif
        )
    }).collect::<Vec<_>>().join("\n");

    let commits_section = git_commits.iter().map(|c| {
        let hash = c.get("short_hash").and_then(|v| v.as_str()).unwrap_or("");
        let msg = c.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let author = c.get("author").and_then(|v| v.as_str()).unwrap_or("");
        format!("- `{}` {} ({})", hash, msg, author)
    }).collect::<Vec<_>>().join("\n");

    format!(
r#"# Evidence Pack — {spec_req_id}: {spec_title}

## Primary User Path

- Spec: `{spec_req_id}` — {spec_title}
- Session: `{session_id}`
- Status: **{status}**
- Duration: {duration_minutes:.1} minutes
- Total tool calls: {total_tool_calls}
- Files changed: {files_changed}
- Knowledge retrieval events: {knowledge_refs_count}

## Task Execution

{tasks_section}

## Request and Response Evidence

- Total tasks: {total_tasks}
- Completed: {completed_tasks}
- Failed: {failed_tasks}
- Total tokens consumed: {total_tokens}

## QA Conclusion

- Verification result: **{verif_str}**
- Overall status: `{status}`

## Live Verification

- Verification passed: {verification_passed}
- health: N/A
- deployment: N/A

## Git History

{commits_section}

## AI Collaboration

- context scope: session `{session_id}`
- model: {model}
- total tokens: {total_tokens}
- assumptions:
{assumptions}
- review points:
{review_points}
- validation result: {verif_str}
"#,
        spec_req_id = spec_req_id,
        spec_title = spec_title,
        session_id = session_id,
        status = status,
        duration_minutes = duration_minutes,
        total_tool_calls = total_tool_calls,
        files_changed = files_changed,
        knowledge_refs_count = knowledge_refs_count,
        tasks_section = tasks_section,
        total_tasks = total_tasks,
        completed_tasks = completed_tasks,
        failed_tasks = failed_tasks,
        total_tokens = total_tokens,
        verif_str = verif_str,
        verification_passed = verification_passed,
        commits_section = if commits_section.is_empty() { "No commits recorded.".to_string() } else { commits_section },
        model = model,
        assumptions = if assumptions.is_empty() { "(none recorded)".to_string() } else { assumptions },
        review_points = if review_points.is_empty() { "(none)".to_string() } else { review_points },
    )
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_evidence_packs(cwd: String) -> Result<Vec<EvidencePackMeta>, String> {
    let evidence_root = format!("{}/.codefactory/evidence", cwd);
    let root = std::path::Path::new(&evidence_root);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut packs: Vec<EvidencePackMeta> = Vec::new();

    // Walk: evidence_root/{spec_req_id}/{timestamp}/manifest.json
    let spec_dirs = std::fs::read_dir(root).map_err(|e| e.to_string())?;
    for spec_entry in spec_dirs.flatten() {
        let spec_path = spec_entry.path();
        if !spec_path.is_dir() {
            continue;
        }
        let ts_dirs = std::fs::read_dir(&spec_path).map_err(|e| e.to_string())?;
        for ts_entry in ts_dirs.flatten() {
            let ts_path = ts_entry.path();
            if !ts_path.is_dir() {
                continue;
            }
            let manifest_path = ts_path.join("manifest.json");
            if manifest_path.exists() {
                let raw = std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
                if let Ok(mut meta) = serde_json::from_str::<EvidencePackMeta>(&raw) {
                    meta.path = ts_path.to_string_lossy().to_string();
                    packs.push(meta);
                }
            }
        }
    }

    // Sort by created_at descending
    packs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(packs)
}

#[tauri::command]
pub async fn get_evidence_pack(path: String) -> Result<EvidencePack, String> {
    let dir = std::path::Path::new(&path);

    let read_file = |name: &str| -> String {
        std::fs::read_to_string(dir.join(name)).unwrap_or_default()
    };

    let manifest_raw = read_file("manifest.json");
    let mut manifest: EvidencePackMeta =
        serde_json::from_str(&manifest_raw).map_err(|e| e.to_string())?;
    manifest.path = path.clone();

    let summary_md = read_file("summary.md");

    // Parse tool_calls.jsonl
    let tool_calls: Vec<serde_json::Value> = read_file("tool_calls.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let parse_json_array = |name: &str| -> Vec<serde_json::Value> {
        let raw = read_file(name);
        if raw.trim().is_empty() {
            return Vec::new();
        }
        serde_json::from_str::<Vec<serde_json::Value>>(&raw).unwrap_or_default()
    };

    let files_changed = parse_json_array("files_changed.json");
    let knowledge_refs = parse_json_array("knowledge_refs.json");
    let verification = parse_json_array("verification.json");
    let git_commits = parse_json_array("git_commits.json");

    let ai_collaboration: serde_json::Value = {
        let raw = read_file("ai_collaboration.json");
        if raw.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null)
        }
    };

    Ok(EvidencePack {
        manifest,
        summary_md,
        tool_calls,
        knowledge_refs,
        files_changed,
        verification,
        git_commits,
        ai_collaboration,
    })
}

#[tauri::command]
pub async fn open_evidence_pack_dir(path: String) -> Result<(), String> {
    Command::new("explorer").no_window()
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Auto-collect helper (called from scheduler after session completion) ───────

/// Called by the scheduler when all tasks for a session are done and
/// spec info was provided. Collects the evidence pack and emits a
/// `evidence_pack_ready:{session_id}` event.
pub async fn auto_collect_and_emit(
    app: &AppHandle,
    pool: &SqlitePool,
    session_id: &str,
    cwd: &str,
    spec_req_id: &str,
    spec_title: &str,
    task_run_ids: &[String],
) {
    match collect_evidence_pack(cwd, spec_req_id, spec_title, session_id, task_run_ids, pool).await {
        Ok(pack_path) => {
            let event = format!("evidence_pack_ready:{}", session_id);
            let payload = serde_json::json!({
                "spec_req_id": spec_req_id,
                "spec_title": spec_title,
                "path": pack_path,
            });
            if let Err(e) = app.emit(&event, payload) {
                tracing::warn!("failed to emit evidence_pack_ready event: {}", e);
            }
        }
        Err(e) => {
            tracing::error!("auto_collect_evidence_pack failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn evidence_pack_prefers_normalized_redacted_tool_lifecycle() {
        let root = std::env::temp_dir().join(format!(
            "codefactory-evidence-normalized-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db_path = root.join("test.db");
        let db_url = format!("sqlite:{}", db_path.display());
        let pool = crate::storage::db::connect(&db_url).await.unwrap();

        sqlx::query(
            "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at) \
             VALUES ('session-1', 'Evidence test', ?, 'test-model', 1, 1)",
        )
        .bind(root.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, tool_calls, created_at) \
             VALUES ('message-1', 'session-1', 'assistant', '', '[]', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls \
             (id, message_id, tool_name, arguments, result, status, duration_ms, created_at) \
             VALUES ('tool-1', 'message-1', 'bash', ?, ?, 'done', 42, 1)",
        )
        .bind(r#"{"command":"printf token=CF_EVO_EVIDENCE_SECRET"}"#)
        .bind(r#"{"token":"CF_EVO_EVIDENCE_SECRET","safe":"visible"}"#)
        .execute(&pool)
        .await
        .unwrap();

        let pack_path = collect_evidence_pack(
            root.to_string_lossy().as_ref(),
            "CF-EVO-R5",
            "normalized evidence",
            "session-1",
            &[],
            &pool,
        )
        .await
        .unwrap();
        let tool_calls =
            std::fs::read_to_string(format!("{pack_path}/tool_calls.jsonl")).unwrap();

        assert!(tool_calls.contains(r#""status":"done""#));
        assert!(tool_calls.contains(r#""duration_ms":42"#));
        assert!(tool_calls.contains("<redacted>"));
        assert!(tool_calls.contains("visible"));
        assert!(!tool_calls.contains("CF_EVO_EVIDENCE_SECRET"));

        // Releases the WAL sidecars too — a plain `pool.close()` leaves a
        // memory-mapped `-shm` file behind most of the time, which is what used
        // to make the cleanup below fail on Windows with os error 32.
        crate::storage::db::close_and_release_files(pool).await;
        crate::util::fs_cleanup::remove_fixture_dir(&root).await;
    }
}
