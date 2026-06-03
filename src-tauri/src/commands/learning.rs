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
use tauri::{command, AppHandle, Emitter, State};
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
}

fn default_kind() -> String { "memory".into() }

// ── Queries ───────────────────────────────────────────────────────────────────

#[command]
pub async fn list_learning_events(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<LearningEvent>, AppError> {
    let pool = state.db.read().await;
    let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String, Option<String>, String, Option<String>, Option<String>)>(
        "SELECT id, session_id, cwd, observation, suggestion, status, created_at, decided_at, \
                kind, pref_key, pref_value \
         FROM learning_events WHERE cwd = ? ORDER BY created_at DESC LIMIT 50",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, session_id, cwd, observation, suggestion, status, created_at, decided_at, kind, pref_key, pref_value)| {
            LearningEvent {
                id, session_id, cwd, observation, suggestion, status,
                created_at, decided_at, kind, pref_key, pref_value,
            }
        })
        .collect())
}

/// Accept routes by kind. For 'memory' it appends to .codefactory/memory.md
/// (original behaviour, returns the updated memory blob). For 'preference'
/// it upserts pref_key→pref_value into user_preferences with source='ai'
/// (returns an empty ProjectMemory — caller should refresh preferences
/// instead). Either way the event is marked accepted in one transaction.
#[command]
pub async fn accept_learning_event(
    event_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ProjectMemory, AppError> {
    let pool = state.db.read().await;
    let row: (String, String, String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT cwd, suggestion, status, kind, pref_key, pref_value \
         FROM learning_events WHERE id = ?",
    )
    .bind(&event_id)
    .fetch_one(&*pool)
    .await?;
    let (cwd, suggestion, status, kind, pref_key, pref_value) = row;
    if status != "pending" {
        return Err(AppError::Other(format!(
            "learning event {} already {}",
            event_id, status
        )));
    }

    let memory = match kind.as_str() {
        "preference" => {
            let key = pref_key.ok_or_else(|| AppError::Other(
                "preference learning event missing pref_key".into(),
            ))?;
            let value = pref_value.unwrap_or_default();
            let now = Utc::now().to_rfc3339();
            sqlx::query(
                "INSERT INTO user_preferences (cwd, key, value, source, updated_at) \
                 VALUES (?,?,?,'ai',?) \
                 ON CONFLICT(cwd, key) DO UPDATE SET \
                   value = excluded.value, source = 'ai', updated_at = excluded.updated_at",
            )
            .bind(&cwd)
            .bind(&key)
            .bind(&value)
            .bind(&now)
            .execute(&*pool)
            .await?;
            // Return an empty memory blob — caller distinguishes by kind on UI side.
            ProjectMemory { path: String::new(), content: String::new(), exists: false }
        }
        _ => {
            // Drop the pool lock before re-acquiring inside append_project_memory.
            drop(pool);
            append_project_memory(cwd.clone(), suggestion.clone()).await?
        }
    };

    let pool = state.db.read().await;
    sqlx::query(
        "UPDATE learning_events SET status = 'accepted', decided_at = ? WHERE id = ?",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(&event_id)
    .execute(&*pool)
    .await?;
    // Tell other open panels (Workspace right rail, second Profile window)
    // to refresh — the row they're holding is now stale.
    let event = format!("learning_events_updated:{}", cwd);
    let _ = app.emit(&event, ());
    Ok(memory)
}

#[command]
pub async fn reject_learning_event(
    event_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await;
    // Capture cwd before update so we can emit the right per-cwd channel.
    let cwd: String = sqlx::query_scalar("SELECT cwd FROM learning_events WHERE id = ?")
        .bind(&event_id)
        .fetch_one(&*pool)
        .await
        .unwrap_or_default();
    sqlx::query(
        "UPDATE learning_events SET status = 'rejected', decided_at = ? WHERE id = ?",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(&event_id)
    .execute(&*pool)
    .await?;
    if !cwd.is_empty() {
        let event = format!("learning_events_updated:{}", cwd);
        let _ = app.emit(&event, ());
    }
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
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    pref_key: Option<String>,
    #[serde(default)]
    pref_value: Option<String>,
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
    app: AppHandle,
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

    let mut body = match serde_json::to_value(&req) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("postmortem serialize failed: {e}");
            return Ok(vec![]);
        }
    };

    let client = Client::new();
    // Send as-is; post_chat_completions reactively switches to
    // max_completion_tokens only if the server rejects max_tokens. Best-effort:
    // any failure just yields no learnings.
    let response = match crate::http_util::post_chat_completions(&client, &url, &api_key, &mut body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("postmortem request failed: {e}");
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
        // Resolve kind defensively: only honour 'preference' when the
        // structured payload is actually present, else fall back to memory.
        // This protects against the model returning kind="preference" with
        // a missing pref_key — the row would otherwise be unactionable.
        let raw_kind = e.kind.as_deref().unwrap_or("memory");
        let (kind, pref_key, pref_value): (&str, Option<String>, Option<String>) =
            if raw_kind == "preference" && e.pref_key.as_ref().map(|k| !k.trim().is_empty()).unwrap_or(false) {
                let key = e.pref_key.as_ref().unwrap().trim().to_string();
                let val = e.pref_value.unwrap_or_default().trim().to_string();
                ("preference", Some(key), Some(val))
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
        .bind(&session_id)
        .bind(&cwd)
        .bind(obs)
        .bind(sug)
        .bind(&now)
        .bind(kind)
        .bind(&pref_key)
        .bind(&pref_value)
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
            kind: kind.into(),
            pref_key,
            pref_value,
        });
    }

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
