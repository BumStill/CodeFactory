// SPDX-License-Identifier: Apache-2.0
//! Subagent runner.
//!
//! Each subagent runs the existing [`AgentLoop`] in **isolation**: a brand-new
//! session row is created so the subagent's messages do NOT pollute the parent
//! chat. After the loop returns we walk the sub-session's tool-call records to
//! produce a [`SubagentResult`] summary that the dashboard can show.
//!
//! Phase 2 keeps this deliberately minimal — Phase 3 will layer retries and
//! auto-verification on top. The brief is wired through as the first (and
//! only) user message; the agent then runs to completion just like a normal
//! chat would.

use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::agent::{AgentExecutionContext, AgentLoop};
use crate::config::settings::{ApiStyle, Settings};
use crate::errors::{AppError, Result};
use crate::mcp::McpManager;
use crate::storage::tasks::TaskConnectorContext;
use crate::storage::Message;
use crate::PendingPermissionMap;

/// Brief handed to a subagent. The brief MUST be self-contained — the
/// subagent has no view of the parent's message history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentBrief {
    pub task_id: String,
    pub title: String,
    pub description: String,
    pub cwd: String,
    /// Optional context summary from the parent (e.g. "Repo uses Tauri + React").
    /// Kept short on purpose — full history would defeat the isolation goal.
    pub parent_summary: Option<String>,
    /// Tool allow-list hint surfaced in the system prompt. Tool *enforcement*
    /// still happens through the global permission policy; this is just a nudge
    /// so the model knows what it's expected to use.
    pub allowed_tools: Vec<String>,
    /// Optional acceptance criteria the subagent should self-verify against.
    pub acceptance_criteria: Option<String>,
    /// Connector scope selected by the parent task. Persisted before execution
    /// and rendered into the brief so tool access is explicit.
    pub connector_context: Option<TaskConnectorContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AcceptanceCheck {
    pub passed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    pub summary: String,
    pub files_changed: Vec<String>,
    pub tool_calls_count: u32,
    pub completed: bool,
    /// Sub-session id for deep-linking from the dashboard.
    pub sub_session_id: String,
    /// Result of the self-verification step (if acceptance_criteria was provided).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acceptance_check: Option<AcceptanceCheck>,
}

impl Default for SubagentResult {
    fn default() -> Self {
        Self {
            summary: String::new(),
            files_changed: Vec::new(),
            tool_calls_count: 0,
            completed: false,
            sub_session_id: String::new(),
            acceptance_check: None,
        }
    }
}

/// Run a single subagent end-to-end.
///
/// 1. Creates a child session row (`parent_session_id` set so it stays out of
///    the main session list).
/// 2. Inserts a synthetic "user" message containing the brief.
/// 3. Runs [`AgentLoop::run`] against that fresh history.
/// 4. Walks the sub-session's tool-call records to derive the result summary.
pub async fn run_subagent(
    brief: SubagentBrief,
    pool: &SqlitePool,
    parent_session_id: &str,
    settings: &Settings,
    app_handle: &AppHandle,
    pending_perms: &PendingPermissionMap,
) -> std::result::Result<SubagentResult, AppError> {
    // 1. Create the sub-session.
    let sub_session_id = Uuid::new_v4().to_string();
    let sub_title = format!("Subtask: {}", truncate(&brief.title, 60));
    let now = Utc::now().timestamp_millis();

    sqlx::query(
        "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at, \
         total_input_tokens, total_output_tokens, parent_session_id) \
         VALUES (?,?,?,?,?,?,0,0,?)",
    )
    .bind(&sub_session_id)
    .bind(&sub_title)
    .bind(&brief.cwd)
    .bind(&settings.default_model)
    .bind(now)
    .bind(now)
    .bind(parent_session_id)
    .execute(pool)
    .await?;

    // 2. Persist the brief as the first user message of the sub-session.
    let brief_text = render_brief(&brief);
    let msg_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,?,?,?,?)",
    )
    .bind(&msg_id)
    .bind(&sub_session_id)
    .bind("user")
    .bind(&brief_text)
    .bind(now)
    .execute(pool)
    .await?;

    // 3. Resolve endpoint + key the same way commands::chat does.
    let endpoint = settings
        .endpoints
        .get(&settings.default_endpoint)
        .ok_or_else(|| AppError::Other("No default endpoint configured".into()))?
        .clone();
    let key_ref = endpoint
        .key_ref
        .clone()
        .unwrap_or_else(|| format!("codefactory.endpoint.{}", settings.default_endpoint));
    let api_key = crate::secrets::get_key(&key_ref)?.unwrap_or_default();
    if api_key.is_empty() {
        return Err(AppError::Other(format!(
            "API key not found for key_ref '{}'",
            key_ref
        )));
    }

    // 4. Build the AgentLoop with the sub-session id and a one-message history.
    let history = vec![Message {
        id: msg_id.clone(),
        session_id: sub_session_id.clone(),
        role: "user".into(),
        content: brief_text.clone(),
        model_id: None,
        input_tokens: None,
        output_tokens: None,
        tool_calls: None,
        reasoning_content: None,
        created_at: now,
    }];

    // We wrap settings in an RwLock to satisfy AgentLoop's signature without
    // sharing it with the caller's live config.
    let settings_lock = Arc::new(RwLock::new(settings.clone()));

    // Subagent uses a fresh, empty McpManager (MCP tools are available via the
    // parent process's shared manager but the subagent runs in isolation).
    let mcp_manager = Arc::new(McpManager::new());

    // Autonomous mode is the whole reason subagents exist — the user
    // approved the plan at the parent level and is no longer in this
    // turn. Without this, the subagent inherits the interactive
    // SYSTEM_PROMPT which tells the model to stop and ask for
    // confirmation (the v1.0 bug: tasks ended after ~30 seconds with
    // "Ready to proceed?").
    let mut agent = AgentLoop::new_with_mode(
        app_handle.clone(),
        pool.clone(),
        sub_session_id.clone(),
        settings.default_model.clone(),
        endpoint.base_url.clone(),
        api_key.clone(),
        endpoint.api_style.clone(),
        PathBuf::from(&brief.cwd),
        settings_lock,
        pending_perms.clone(),
        mcp_manager,
        Some(AgentExecutionContext {
            parent_session_id: Some(parent_session_id.to_string()),
            task_id: Some(brief.task_id.clone()),
            knowledge_library_ids: brief
                .connector_context
                .as_ref()
                .map(|ctx| ctx.knowledge_library_ids())
                .unwrap_or_default(),
        }),
        crate::agent::AgentMode::Autonomous,
    );

    // Hard wall-clock cap per subagent. Without this an unbounded
    // tool-call loop (model keeps asking to read more files) can burn
    // tokens and clock indefinitely. 10 minutes is generous for a real
    // task but bounded enough to catch runaway behaviour.
    const PER_TASK_TIMEOUT_SECS: u64 = 600;
    match tokio::time::timeout(
        std::time::Duration::from_secs(PER_TASK_TIMEOUT_SECS),
        agent.run(history),
    )
    .await
    {
        Ok(r) => r?,
        Err(_) => {
            tracing::warn!(
                "subagent task '{}' hit {}s wall-clock cap; aborting",
                brief.title,
                PER_TASK_TIMEOUT_SECS
            );
            return Err(AppError::Other(format!(
                "Task exceeded {}s execution cap and was aborted to prevent drift",
                PER_TASK_TIMEOUT_SECS
            )));
        }
    }

    // 5. Walk messages to produce a result summary.
    let result = summarize_run(pool, &sub_session_id).await?;

    // 6. Acceptance check (Phase 3): if criteria were provided, ask the model
    //    to self-verify. We do a lightweight, single-turn call using the same
    //    endpoint so we don't disturb the agent history.
    let acceptance_check = if let Some(criteria) = &brief.acceptance_criteria {
        Some(
            run_acceptance_check(
                criteria,
                &result.summary,
                &endpoint.base_url,
                &api_key,
                &settings.default_model,
                &endpoint.api_style,
            )
            .await,
        )
    } else {
        None
    };

    Ok(SubagentResult {
        summary: result.summary,
        files_changed: result.files_changed,
        tool_calls_count: result.tool_calls_count,
        completed: true,
        sub_session_id,
        acceptance_check,
    })
}

struct RunSummary {
    summary: String,
    files_changed: Vec<String>,
    tool_calls_count: u32,
}

/// Produce a result summary by reading back the sub-session's persisted messages.
/// Uses the final assistant message as the summary text (capped) and pulls
/// `path` arguments out of write/edit tool calls to compute `files_changed`.
async fn summarize_run(pool: &SqlitePool, sub_session_id: &str) -> Result<RunSummary> {
    let messages = sqlx::query_as::<_, Message>(
        "SELECT * FROM messages WHERE session_id = ? ORDER BY created_at ASC, rowid ASC",
    )
    .bind(sub_session_id)
    .fetch_all(pool)
    .await?;

    let mut files_changed: HashSet<String> = HashSet::new();
    let mut tool_calls_count: u32 = 0;
    let mut last_assistant_text: Option<String> = None;

    for m in &messages {
        if m.role == "assistant" {
            if !m.content.trim().is_empty() {
                last_assistant_text = Some(m.content.clone());
            }
            if let Some(raw) = &m.tool_calls {
                if let Ok(tcs) = serde_json::from_str::<serde_json::Value>(raw) {
                    if let Some(arr) = tcs.as_array() {
                        for tc in arr {
                            tool_calls_count += 1;
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if matches!(name, "write_file" | "edit_file") {
                                if let Some(args_str) = tc
                                    .get("function")
                                    .and_then(|f| f.get("arguments"))
                                    .and_then(|v| v.as_str())
                                {
                                    if let Ok(args) =
                                        serde_json::from_str::<serde_json::Value>(args_str)
                                    {
                                        if let Some(p) = args
                                            .get("path")
                                            .or_else(|| args.get("file_path"))
                                            .and_then(|v| v.as_str())
                                        {
                                            files_changed.insert(p.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let summary = last_assistant_text
        .map(|s| truncate(&s, 1500).to_string())
        .unwrap_or_else(|| "(no summary produced)".to_string());

    Ok(RunSummary {
        summary,
        files_changed: files_changed.into_iter().collect(),
        tool_calls_count,
    })
}

fn render_brief(brief: &SubagentBrief) -> String {
    let mut out = String::new();
    out.push_str("You are a focused subagent inside CodeFactory. ");
    out.push_str(
        "Complete the task below independently and report back with a concise summary.\n\n",
    );

    out.push_str(&format!("# Task: {}\n", brief.title));
    out.push_str(&format!("\n{}\n", brief.description));

    out.push_str(&format!("\n## Working directory\n`{}`\n", brief.cwd));

    if !brief.allowed_tools.is_empty() {
        out.push_str("\n## Suggested tools\n");
        for t in &brief.allowed_tools {
            out.push_str(&format!("- `{}`\n", t));
        }
    }

    if let Some(criteria) = &brief.acceptance_criteria {
        out.push_str("\n## Acceptance criteria\n");
        out.push_str(criteria);
        out.push('\n');
    }

    if let Some(context) = &brief.connector_context {
        let rendered = context.render_markdown();
        if !rendered.is_empty() {
            out.push('\n');
            out.push_str(&rendered);
        }
    }

    if let Some(ctx) = &brief.parent_summary {
        out.push_str("\n## Parent context\n");
        out.push_str(ctx);
        out.push('\n');
    }

    // Include shared context from the brief file if it exists
    let brief_file = format!("{}/_codefactory_brief.md", brief.cwd);
    if let Ok(content) = std::fs::read_to_string(&brief_file) {
        if content.len() > 100 {
            // Cap at 3000 chars to avoid token bloat
            let capped = if content.len() > 3000 {
                &content[..3000]
            } else {
                &content
            };
            out.push_str("\n## Shared Project Brief\n");
            out.push_str(capped);
            out.push_str(
                "\n\n_Other parallel tasks are listed above \u{2014} coordinate to avoid conflicts._\n",
            );
        }
    }

    out.push_str(
        "\n## Reporting\n\
         When done, end with a short final message that summarizes what you did, \
         which files you touched (if any), and any follow-ups for the parent.\n",
    );
    out
}

fn truncate(s: &str, max: usize) -> &str {
    // Char-boundary safe truncation.
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Ask the model to self-check the acceptance criteria.
///
/// Sends a single non-streaming chat completion. Returns `AcceptanceCheck`
/// parsed from the model's JSON response. On any error, returns
/// `passed: false` with the error as the reason so retries are triggered.
pub(crate) async fn run_acceptance_check(
    criteria: &str,
    work_summary: &str,
    base_url: &str,
    api_key: &str,
    model_id: &str,
    api_style: &ApiStyle,
) -> AcceptanceCheck {
    let prompt = format!(
        "Review the work done. Do the following acceptance criteria pass?\n\n\
         {criteria}\n\n\
         Work summary:\n{work_summary}\n\n\
         Reply with JSON only (no markdown): {{ \"passed\": bool, \"reason\": string }}"
    );

    let mut body = match api_style {
        ApiStyle::Anthropic => serde_json::json!({
            "model": model_id,
            "max_tokens": 256,
            "messages": [{"role": "user", "content": prompt}]
        }),
        // NOTE: ChatGPT endpoints would need the Responses API + OAuth token
        // here. This one-shot acceptance check (autonomous-task mode only) does
        // not yet support that — it falls through to the chat/completions shape,
        // which the ChatGPT backend will reject. Interactive chat is unaffected
        // (that path uses AgentLoop::call_chatgpt_model). TODO: route this
        // through the Responses path for ChatGPT endpoints.
        ApiStyle::Openai | ApiStyle::Chatgpt => serde_json::json!({
            "model": model_id,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 256,
            "stream": false
        }),
    };

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let client = Client::new();
    // Send as-is; post_chat_completions reactively switches to
    // max_completion_tokens only if the server rejects max_tokens (no-op for the
    // Anthropic shape above, and for endpoints happy with the legacy fields).
    let response =
        match crate::http_util::post_chat_completions(&client, &url, api_key, &mut body).await {
            Ok(r) => r,
            Err(e) => {
                return AcceptanceCheck {
                    passed: false,
                    reason: format!("HTTP error during acceptance check: {e}"),
                };
            }
        };

    let raw_json: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            return AcceptanceCheck {
                passed: false,
                reason: format!("Failed to parse acceptance check response: {e}"),
            };
        }
    };

    // Extract the text content from the response (handles both Anthropic and OpenAI shapes).
    let text = raw_json
        .pointer("/choices/0/message/content")
        .or_else(|| raw_json.pointer("/content/0/text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Strip potential markdown fences.
    let clean = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    match serde_json::from_str::<serde_json::Value>(clean) {
        Ok(v) => {
            let passed = v.get("passed").and_then(|p| p.as_bool()).unwrap_or(false);
            let reason = v
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("(no reason)")
                .to_string();
            AcceptanceCheck { passed, reason }
        }
        Err(_) => AcceptanceCheck {
            passed: false,
            reason: format!(
                "Could not parse acceptance-check JSON: {}",
                truncate(clean, 200)
            ),
        },
    }
}
