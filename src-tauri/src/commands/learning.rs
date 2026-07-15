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
use serde::{Deserialize, Serialize};
#[cfg(test)]
use sqlx::SqlitePool;
use tauri::{command, AppHandle, Emitter, State};
use uuid::Uuid;

use crate::commands::memory::{append_project_memory, ProjectMemory};
use crate::commands::specs::{run_one_shot_text, AiMessage};
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
    /// Self-evolution P1: sessions of evidence behind a mined insight
    /// (0 for per-session post-mortem rows). kind == 'pattern' rows set it.
    #[serde(default)]
    pub support_count: i64,
    /// Raw metrics behind a mined insight, as JSON ("{}" for non-mined rows).
    #[serde(default = "default_evidence")]
    pub evidence_json: String,
}

fn default_kind() -> String {
    "memory".into()
}
fn default_evidence() -> String {
    "{}".into()
}

// ── Queries ───────────────────────────────────────────────────────────────────

#[command]
pub async fn list_learning_events(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<Vec<LearningEvent>, AppError> {
    let pool = state.db.read().await;
    let rows = sqlx::query_as::<
        _,
        (
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
        ),
    >(
        "SELECT id, session_id, cwd, observation, suggestion, status, created_at, decided_at, \
                kind, pref_key, pref_value, support_count, evidence_json \
         FROM learning_events WHERE cwd = ? ORDER BY support_count DESC, created_at DESC LIMIT 50",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
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
                }
            },
        )
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
    let row: (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
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
            let key = pref_key.ok_or_else(|| {
                AppError::Other("preference learning event missing pref_key".into())
            })?;
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
            ProjectMemory {
                path: String::new(),
                content: String::new(),
                exists: false,
            }
        }
        _ => {
            // Drop the pool lock before re-acquiring inside append_project_memory.
            drop(pool);
            append_project_memory(cwd.clone(), suggestion.clone()).await?
        }
    };

    let pool = state.db.read().await;
    sqlx::query("UPDATE learning_events SET status = 'accepted', decided_at = ? WHERE id = ?")
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
    sqlx::query("UPDATE learning_events SET status = 'rejected', decided_at = ? WHERE id = ?")
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

    if rows.is_empty() {
        return Ok(vec![]);
    }

    let summary = rows
        .iter()
        .enumerate()
        .map(|(i, (title, status, result, error))| {
            let outcome = match status.as_str() {
                "completed" => result
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>(),
                "failed" => format!(
                    "FAIL: {}",
                    error
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect::<String>()
                ),
                other => other.into(),
            };
            format!("{}. [{}] {} — {}", i + 1, status, title, outcome)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // What we already know — folded into the prompt (avoid repeats / flag
    // contradictions) and into a dedup set that guards the insert below.
    let known_suggestions: Vec<String> = existing
        .iter()
        .map(|(s,)| s.trim().to_string())
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
    let model = settings.resolved_default_model().ok_or_else(|| {
        AppError::Other(format!(
            "No model configured for endpoint '{}'. Please choose a model in the picker.",
            ep_name
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
{calibration}{known_block}Tasks from this session:\n{summary}"
    );

    let text = match run_one_shot_text(
        &endpoint.base_url,
        &api_key,
        &model,
        &endpoint.api_style,
        vec![AiMessage {
            role: "user".into(),
            content: prompt,
        }],
        500, // hard cap — see module-level doc on token economy
        0.3,
    )
    .await
    {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!("postmortem request failed: {error}");
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
            tracing::warn!("postmortem parse failed: {e}, raw: {json_str}");
            return Ok(vec![]);
        }
    };

    // ── Persist as pending learning events
    let pool = state.db.read().await;
    let now = Utc::now().to_rfc3339();
    let mut created: Vec<LearningEvent> = Vec::new();
    for e in entries {
        // Skip empty fields.
        let obs = e.observation.trim();
        let sug = e.suggestion.trim();
        if obs.is_empty() || sug.is_empty() {
            continue;
        }
        // Drop exact duplicates — of an already-known learning OR one we just
        // emitted this round. norm_suggestion folds case + whitespace so trivial
        // rewordings of the same fact don't pile up now that learnings are
        // injected into every chat.
        if !seen.insert(norm_suggestion(sug)) {
            continue;
        }
        // Resolve kind defensively: only honour 'preference' when the
        // structured payload is actually present, else fall back to memory.
        // This protects against the model returning kind="preference" with
        // a missing pref_key — the row would otherwise be unactionable.
        let raw_kind = e.kind.as_deref().unwrap_or("memory");
        let (kind, pref_key, pref_value): (&str, Option<String>, Option<String>) = if raw_kind
            == "preference"
            && e.pref_key
                .as_ref()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false)
        {
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
            support_count: 0,
            evidence_json: "{}".into(),
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
    pub tool_name: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskRow {
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
    use std::collections::HashMap;
    let mut total: HashMap<&str, i64> = HashMap::new();
    let mut errs: HashMap<&str, i64> = HashMap::new();
    let mut sample: HashMap<&str, String> = HashMap::new();
    for r in rows {
        *total.entry(r.tool_name.as_str()).or_default() += 1;
        if r.status == "error" {
            *errs.entry(r.tool_name.as_str()).or_default() += 1;
            if let Some(e) = &r.error {
                sample
                    .entry(r.tool_name.as_str())
                    .or_insert_with(|| e.chars().take(80).collect());
            }
        }
    }
    let mut out = Vec::new();
    for (tool, &t) in &total {
        let e = errs.get(tool).copied().unwrap_or(0);
        let rate = pct(e, t);
        if t >= 8 && rate >= 25 {
            let ex = sample.get(tool).cloned().unwrap_or_default();
            let tail = if ex.is_empty() {
                String::new()
            } else {
                format!("，最常见：{ex}")
            };
            out.push(PatternInsight {
                observation: format!("工具 `{tool}` 最近 {t} 次调用失败 {e} 次（{rate}%）{tail}。"),
                suggestion: format!(
                    "`{tool}` 近期失败率偏高（{e}/{t}，{rate}%）——调用前先核对前置条件，或考虑替代方案。"
                ),
                support_count: t,
                evidence: serde_json::json!({"detector":"tool_reliability","tool":tool,"total":t,"errors":e,"rate":rate}),
            });
        }
    }
    out.sort_by(|a, b| b.support_count.cmp(&a.support_count));
    out
}

/// A failure that keeps forcing retries across tasks.
fn detect_retry_prone(rows: &[TaskRow]) -> Vec<PatternInsight> {
    use std::collections::HashMap;
    let mut by_err: HashMap<String, (i64, String)> = HashMap::new();
    for r in rows {
        if r.attempt_count < 2 {
            continue;
        }
        let raw = r.error.clone().unwrap_or_default();
        let key = norm_suggestion(&raw.chars().take(50).collect::<String>());
        if key.is_empty() {
            continue;
        }
        let entry = by_err
            .entry(key)
            .or_insert((0, raw.chars().take(60).collect()));
        entry.0 += 1;
    }
    let mut out: Vec<PatternInsight> = by_err
        .into_iter()
        .filter(|(_, (count, _))| *count >= 3)
        .map(|(_, (count, sample))| PatternInsight {
            observation: format!("有 {count} 个任务因「{sample}」反复重试。"),
            suggestion: format!(
                "反复踩坑：「{sample}」导致多次重试——值得加一道前置检查或固定解法。"
            ),
            support_count: count,
            evidence: serde_json::json!({"detector":"retry_prone","count":count,"sample":sample}),
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
            evidence: serde_json::json!({"detector":"learning_calibration","kind":kind,"decided":d,"accepted":a,"accept_rate":rate}),
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

    // Tool calls in this project (tool_calls → messages → sessions.cwd).
    let tools: Vec<ToolCallRow> = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT tc.tool_name, tc.status, tc.error \
         FROM tool_calls tc \
         JOIN messages m ON tc.message_id = m.id \
         JOIN sessions s ON m.session_id = s.id \
         WHERE s.cwd = ? ORDER BY tc.created_at DESC LIMIT 4000",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(tool_name, status, error)| ToolCallRow {
        tool_name,
        status,
        error,
    })
    .collect();

    // Task runs in this project (task_runs → sessions.cwd).
    let tasks: Vec<TaskRow> = sqlx::query_as::<_, (String, i64, Option<String>)>(
        "SELECT t.status, t.attempt_count, t.error \
         FROM task_runs t JOIN sessions s ON t.session_id = s.id \
         WHERE s.cwd = ? ORDER BY t.created_at DESC LIMIT 2000",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(status, attempt_count, error)| TaskRow {
        status,
        attempt_count,
        error,
    })
    .collect();

    // Decided learnings (accept/reject calibration).
    let decisions: Vec<LearningDecisionRow> = sqlx::query_as::<_, (String, String)>(
        "SELECT kind, status FROM learning_events \
         WHERE cwd = ? AND status IN ('accepted','rejected')",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(kind, status)| LearningDecisionRow { kind, status })
    .collect();

    // Existing suggestions (accepted + pending) for dedup — same guard as A3.
    let existing: Vec<(String,)> = sqlx::query_as(
        "SELECT suggestion FROM learning_events WHERE cwd = ? AND status IN ('accepted','pending')",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await
    .unwrap_or_default();
    let mut seen: std::collections::HashSet<String> =
        existing.iter().map(|(s,)| norm_suggestion(s)).collect();

    let insights = run_detectors(&tools, &tasks, &decisions);

    let now = Utc::now().to_rfc3339();
    let mut created: Vec<LearningEvent> = Vec::new();
    for ins in insights {
        if !seen.insert(norm_suggestion(&ins.suggestion)) {
            continue;
        }
        let id = Uuid::new_v4().to_string();
        let evidence = ins.evidence.to_string();
        sqlx::query(
            "INSERT INTO learning_events \
             (id, session_id, cwd, observation, suggestion, status, created_at, kind, \
              support_count, evidence_json) \
             VALUES (?, '', ?, ?, ?, 'pending', ?, 'pattern', ?, ?)",
        )
        .bind(&id)
        .bind(&cwd)
        .bind(&ins.observation)
        .bind(&ins.suggestion)
        .bind(&now)
        .bind(ins.support_count)
        .bind(&evidence)
        .execute(&*pool)
        .await?;
        created.push(LearningEvent {
            id,
            session_id: String::new(),
            cwd: cwd.clone(),
            observation: ins.observation,
            suggestion: ins.suggestion,
            status: "pending".into(),
            created_at: now.clone(),
            decided_at: None,
            kind: "pattern".into(),
            pref_key: None,
            pref_value: None,
            support_count: ins.support_count,
            evidence_json: evidence,
        });
    }
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
    let tools: Vec<ToolCallRow> = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT tool_name, status, error FROM tool_calls ORDER BY created_at DESC LIMIT 8000",
    )
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(tool_name, status, error)| ToolCallRow {
        tool_name,
        status,
        error,
    })
    .collect();
    let tasks: Vec<TaskRow> = sqlx::query_as::<_, (String, i64, Option<String>)>(
        "SELECT status, attempt_count, error FROM task_runs ORDER BY created_at DESC LIMIT 4000",
    )
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(status, attempt_count, error)| TaskRow {
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
    let tools: Vec<ToolCallRow> = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT tool_name, status, error FROM tool_calls ORDER BY created_at DESC LIMIT 8000",
    )
    .fetch_all(&*pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(tool_name, status, error)| ToolCallRow {
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
        for _ in 0..6 {
            rows.push(tc("edit_file", "ok", None));
        }
        for _ in 0..4 {
            rows.push(tc("edit_file", "error", Some("boom")));
        }
        for _ in 0..9 {
            rows.push(tc("flaky_gated", "error", Some("x")));
        }
        for _ in 0..10 {
            rows.push(tc("bash", "error", Some("e")));
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

    fn tc(name: &str, status: &str, err: Option<&str>) -> ToolCallRow {
        ToolCallRow {
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
                "bash",
                if i < 4 { "error" } else { "done" },
                Some("pwsh not found"),
            ));
        }
        // reliable: 12 calls, 1 error (8%) → not flagged.
        for i in 0..12 {
            rows.push(tc("read_file", if i < 1 { "error" } else { "done" }, None));
        }
        // flaky but low-volume: 5 calls, 3 errors → not flagged (< 8 calls).
        for i in 0..5 {
            rows.push(tc("write_xlsx", if i < 3 { "error" } else { "done" }, None));
        }

        let out = detect_tool_reliability(&rows);
        assert_eq!(out.len(), 1, "only the high-volume flaky tool is flagged");
        assert!(out[0].suggestion.contains("bash"));
        assert_eq!(out[0].support_count, 10);
        assert!(out[0].evidence.get("rate").and_then(|v| v.as_i64()) == Some(40));
    }

    #[test]
    fn retry_prone_groups_by_error_and_needs_three() {
        let rows = vec![
            // Same recurring failure (case/whitespace fold to one key) on retries.
            TaskRow {
                status: "completed".into(),
                attempt_count: 3,
                error: Some("schannel: server closed abruptly".into()),
            },
            TaskRow {
                status: "completed".into(),
                attempt_count: 2,
                error: Some("schannel: server closed abruptly".into()),
            },
            TaskRow {
                status: "failed".into(),
                attempt_count: 4,
                error: Some("Schannel:  server  closed  abruptly".into()),
            },
            // single-attempt → ignored even though same error.
            TaskRow {
                status: "completed".into(),
                attempt_count: 1,
                error: Some("schannel: server closed abruptly".into()),
            },
            // a different one-off retry error → its own group, below threshold.
            TaskRow {
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
}
