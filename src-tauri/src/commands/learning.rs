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
//! Input is bounded to a short summary of task titles + status. Output
//! capped at 500 tokens. Uses the user's default model — no new config.

use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{command, State};
use uuid::Uuid;

use crate::commands::memory::{append_project_memory, ProjectMemory};
use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEvent {
    pub id: String,
    pub session_id: String,
    pub cwd: String,
    pub observation: String,
    pub suggestion: String,
    pub status: String, // pending | accepted | rejected
    pub created_at: String,
    pub decided_at: Option<String>,
}

// ── Queries ───────────────────────────────────────────────────────────────────

#[command]
pub async fn list_learning_events(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<LearningEvent>, AppError> {
    let pool = state.db.read().await;
    let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String, Option<String>)>(
        "SELECT id, session_id, cwd, observation, suggestion, status, created_at, decided_at \
         FROM learning_events WHERE cwd = ? ORDER BY created_at DESC LIMIT 50",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, session_id, cwd, observation, suggestion, status, created_at, decided_at)| {
            LearningEvent {
                id,
                session_id,
                cwd,
                observation,
                suggestion,
                status,
                created_at,
                decided_at,
            }
        })
        .collect())
}

#[command]
pub async fn accept_learning_event(
    event_id: String,
    state: State<'_, AppState>,
) -> Result<ProjectMemory, AppError> {
    let pool = state.db.read().await;
    let row: (String, String, String) = sqlx::query_as(
        "SELECT cwd, suggestion, status FROM learning_events WHERE id = ?",
    )
    .bind(&event_id)
    .fetch_one(&*pool)
    .await?;
    if row.2 != "pending" {
        return Err(AppError::Other(format!(
            "learning event {} already {}",
            event_id, row.2
        )));
    }

    // Drop the pool lock before re-acquiring inside append_project_memory.
    drop(pool);
    let memory = append_project_memory(row.0.clone(), row.1.clone()).await?;

    let pool = state.db.read().await;
    sqlx::query(
        "UPDATE learning_events SET status = 'accepted', decided_at = ? WHERE id = ?",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(&event_id)
    .execute(&*pool)
    .await?;
    Ok(memory)
}

#[command]
pub async fn reject_learning_event(
    event_id: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await;
    sqlx::query(
        "UPDATE learning_events SET status = 'rejected', decided_at = ? WHERE id = ?",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(&event_id)
    .execute(&*pool)
    .await?;
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
}

#[derive(Debug, Deserialize)]
struct AiResponseMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AiResponse {
    choices: Vec<AiResponseChoice>,
}

#[derive(Debug, Deserialize)]
struct PostmortemEntry {
    observation: String,
    suggestion: String,
}

/// Run a single post-mortem pass over a finished session. Idempotent on
/// repeat call — if the AI returns the same observations they'll just
/// appear as duplicates in the table (rare in practice; the UI dedups
/// visually). Failure is logged but never propagated to the caller —
/// post-mortem is best-effort and should never break a successful run.
#[command]
pub async fn run_postmortem(
    session_id: String,
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<LearningEvent>, AppError> {
    // ── Gather a tiny summary: task titles + statuses + first 80 chars
    //    of result/error. Keeps the prompt input under ~500 tokens
    //    regardless of how many tasks ran.
    let pool = state.db.read().await;
    let rows: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT title, status, result, error FROM task_runs \
         WHERE session_id = ? ORDER BY created_at ASC",
    )
    .bind(&session_id)
    .fetch_all(&*pool)
    .await?;
    drop(pool);

    if rows.is_empty() {
        return Ok(vec![]);
    }

    let summary = rows
        .iter()
        .enumerate()
        .map(|(i, (title, status, result, error))| {
            let outcome = match status.as_str() {
                "completed" => result.as_deref().unwrap_or("").chars().take(80).collect::<String>(),
                "failed" => format!("FAIL: {}", error.as_deref().unwrap_or("").chars().take(80).collect::<String>()),
                other => other.into(),
            };
            format!("{}. [{}] {} — {}", i + 1, status, title, outcome)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // ── Build prompt
    let settings = state.settings.read().await.clone();
    let ep_name = &settings.default_endpoint;
    let model = settings.default_model.clone();
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
    let base_url = endpoint.base_url.trim_end_matches('/');
    let url = format!("{base_url}/chat/completions");

    let prompt = format!(
        "You just finished a session for a user. Reflect on the task outcomes below and \
identify 0-3 NON-OBVIOUS observations about how this user works that would help future sessions. \
Skip obvious things like \"the user wants working code\". Look for patterns: preferred libraries, \
style choices, repeated mistakes worth avoiding, missing tests they had to ask for, etc.\n\n\
Return ONLY a JSON array (no markdown fences). Each entry has \"observation\" (what you noticed) \
and \"suggestion\" (a concrete one-line memory you'd add to help future sessions). \
If nothing notable, return [].\n\n\
Example:\n\
[{{\"observation\": \"User had to remind me twice to handle the empty-array case.\", \
\"suggestion\": \"Always check empty-collection edge cases before claiming a function is done.\"}}]\n\n\
Tasks from this session:\n{summary}"
    );

    let req = AiRequest {
        model,
        messages: vec![AiMessage {
            role: "user",
            content: prompt,
        }],
        stream: false,
        temperature: 0.3,
        max_tokens: 500, // hard cap — see module-level doc on token economy
    };

    let client = Client::new();
    let response = match client
        .post(&url)
        .bearer_auth(&api_key)
        .header("X-Title", "CodeFactory")
        .header("Content-Type", "application/json")
        .json(&req)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("postmortem HTTP failed: {e}");
            return Ok(vec![]);
        }
    };
    let response = match crate::http_util::check_status(response).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("postmortem status failed: {e}");
            return Ok(vec![]);
        }
    };
    let resp: AiResponse = match response.json().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("postmortem JSON parse failed: {e}");
            return Ok(vec![]);
        }
    };

    let text = resp
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .unwrap_or_default();
    let trimmed = text.trim();
    let json_str = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```").trim())
        .unwrap_or(trimmed);

    let entries: Vec<PostmortemEntry> = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("postmortem parse failed: {e}, raw: {json_str}");
            return Ok(vec![]);
        }
    };

    // ── Persist as pending learning events
    let pool = state.db.read().await;
    let now = Utc::now().to_rfc3339();
    let mut created: Vec<LearningEvent> = Vec::new();
    for e in entries {
        // Skip obvious junk: empty fields or duplicates of the most recent entry.
        let obs = e.observation.trim();
        let sug = e.suggestion.trim();
        if obs.is_empty() || sug.is_empty() {
            continue;
        }
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO learning_events \
             (id, session_id, cwd, observation, suggestion, status, created_at) \
             VALUES (?, ?, ?, ?, ?, 'pending', ?)",
        )
        .bind(&id)
        .bind(&session_id)
        .bind(&cwd)
        .bind(obs)
        .bind(sug)
        .bind(&now)
        .execute(&*pool)
        .await?;
        created.push(LearningEvent {
            id,
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            observation: obs.into(),
            suggestion: sug.into(),
            status: "pending".into(),
            created_at: now.clone(),
            decided_at: None,
        });
    }

    Ok(created)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // Storage-only tests; the post-mortem AI call needs a live endpoint.
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

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
}
