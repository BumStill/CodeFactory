// SPDX-License-Identifier: Apache-2.0
//! Safe semantic titles for newly materialized sessions.

use once_cell::sync::Lazy;
use regex::Regex;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use unicode_segmentation::UnicodeSegmentation;

use crate::agent::failover::RouteCandidate;
use crate::agent::{generate_bounded_text, InternalTextOutput};
use crate::commands::costs::{record_usage_event, UsageEventInput};
use crate::openrouter::types::{ChatMessage, MessageContent};
use crate::storage::Session;

pub(crate) const PLACEHOLDER_TITLE: &str = "新会话";
pub(crate) const TITLE_SOURCE_PLACEHOLDER: &str = "placeholder";
pub(crate) const TITLE_SOURCE_GENERATED: &str = "generated";
pub(crate) const TITLE_SOURCE_FALLBACK: &str = "fallback";
pub(crate) const TITLE_SOURCE_MANUAL: &str = "manual";
pub(crate) const SESSION_TITLE_UPDATED_EVENT: &str = "session-title-updated";

const MAX_INPUT_CHARS: usize = 2_000;
const MAX_ASSISTANT_CHARS: usize = 800;
const MAX_TITLE_CHARS: usize = 40;
const MAX_TITLE_OUTPUT_TOKENS: u32 = 256;
const TITLE_DEADLINE: Duration = Duration::from_secs(12);
const TITLE_JOB_LEASE_MS: i64 = 60_000;

static CODE_FENCE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)```.*?```").expect("code fence regex"));
static INLINE_CODE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"`[^`\n]+`").expect("inline code regex"));
static FILE_ATTACHMENT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"!\[[^\]]*\]\((?:<)?file://[^)\n>]+(?:>)?\)").expect("file attachment regex")
});
static DOCUMENT_ATTACHMENT_BLOCK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)(?:\n\n)?已上传以下文件（.*\z").expect("document attachment regex")
});
static LOG_LINE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?im)^[ \t]*(?:\d{4}-\d{2}-\d{2}[T ][^\n]*|\[(?:ERROR|WARN|INFO|DEBUG|TRACE)\][^\n]*|(?:ERROR|WARN|INFO|DEBUG|TRACE)\b[^\n]*|thread '[^\n]*' panicked[^\n]*)$",
    )
    .expect("log line regex")
});
static URL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?-u:\b)(?:https?|file|ssh|sftp|ftp)://\S+").expect("url regex"));
static EMAIL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?-u:\b)[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}(?-u:\b)").expect("email regex")
});
static UUID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?-u:\b)[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}(?-u:\b)")
        .expect("uuid regex")
});
static LONG_HASH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?-u:\b)[0-9a-f]{20,}(?-u:\b)").expect("hash regex"));
static JWT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?-u:\b)eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?-u:\b)")
        .expect("jwt regex")
});
static AWS_ACCESS_KEY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?-u:\b)(?:AKIA|ASIA)[A-Z0-9]{16}(?-u:\b)").expect("aws key regex"));
static GITLAB_TOKEN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?-u:\b)glpat-[A-Za-z0-9_-]+(?-u:\b)").expect("gitlab token regex")
});
static HIGH_ENTROPY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[A-Za-z0-9_+/=-]{32,}").expect("high entropy credential regex"));
static PHONE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\+?\d[\d ()-]{7,}\d").expect("phone regex"));
static ABSOLUTE_PATH_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(?:[A-Z]:[\\/]|~[/\\]|/(?:[^/\s]+/)+|\\\\)[^\n\r,;，；!?！？<>\"'`]*"#)
        .expect("absolute path regex")
});
static ROOT_POSIX_PATH_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)(?P<prefix>^|[\s(\[{\"'“‘（【])/[^\n\r\s/,:;，；!?！？<>\"'`]+"#)
        .expect("root POSIX path regex")
});
static WHITESPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").expect("whitespace regex"));
static ACTIVE_TITLE_JOBS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

struct ActiveTitleJobGuard {
    session_id: String,
}

impl Drop for ActiveTitleJobGuard {
    fn drop(&mut self) {
        ACTIVE_TITLE_JOBS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.session_id);
    }
}

fn claim_title_job(session_id: &str) -> Option<ActiveTitleJobGuard> {
    let mut active = ACTIVE_TITLE_JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !active.insert(session_id.to_string()) {
        return None;
    }
    Some(ActiveTitleJobGuard {
        session_id: session_id.to_string(),
    })
}

fn redact_metadata_text(value: &str, max_chars: usize) -> String {
    // The shared helper truncates by Unicode scalar values. Keep it focused on
    // redaction here, then enforce this feature's grapheme limit below.
    let value = crate::trajectory::redact_text(value, usize::MAX);
    let value = DOCUMENT_ATTACHMENT_BLOCK_RE.replace_all(&value, " [附件] ");
    let value = CODE_FENCE_RE.replace_all(&value, " [代码] ");
    let value = INLINE_CODE_RE.replace_all(&value, " [代码] ");
    let value = FILE_ATTACHMENT_RE.replace_all(&value, " [附件] ");
    let value = LOG_LINE_RE.replace_all(&value, " [日志] ");
    let value = URL_RE.replace_all(&value, " <redacted> ");
    let value = EMAIL_RE.replace_all(&value, " <redacted> ");
    let value = UUID_RE.replace_all(&value, " <redacted> ");
    let value = LONG_HASH_RE.replace_all(&value, " <redacted> ");
    let value = JWT_RE.replace_all(&value, " <redacted> ");
    let value = AWS_ACCESS_KEY_RE.replace_all(&value, " <redacted> ");
    let value = GITLAB_TOKEN_RE.replace_all(&value, " <redacted> ");
    let value = HIGH_ENTROPY_RE.replace_all(&value, " <redacted> ");
    let value = PHONE_RE.replace_all(&value, " <redacted> ");
    let value = ABSOLUTE_PATH_RE.replace_all(&value, " <redacted> ");
    let value = ROOT_POSIX_PATH_RE.replace_all(&value, "${prefix}<redacted>");
    let collapsed = WHITESPACE_RE.replace_all(value.trim(), " ");
    collapsed.graphemes(true).take(max_chars).collect()
}

pub(crate) fn is_low_information(value: &str) -> bool {
    let normalized = value
        .trim()
        .trim_matches(|c: char| c.is_ascii_punctuation() || "，。！？；：、…".contains(c))
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "" | "好"
            | "好的"
            | "继续"
            | "继续吧"
            | "看看这个"
            | "按上面做"
            | "开始吧"
            | "做吧"
            | "ok"
            | "okay"
            | "continue"
            | "go on"
            | "do it"
    )
}

fn prepared_title_input(user_message: &str, assistant_summary: Option<&str>) -> Option<String> {
    let user = redact_metadata_text(user_message, MAX_INPUT_CHARS);
    if is_low_information(&user) && assistant_summary.is_none() {
        return None;
    }
    let mut input = format!("首条用户消息：{user}");
    if let Some(summary) = assistant_summary {
        let summary = redact_metadata_text(summary, MAX_ASSISTANT_CHARS);
        if !summary.is_empty() {
            input.push_str("\n首轮用户可见结论：");
            input.push_str(&summary);
        }
    }
    Some(input)
}

fn title_messages(input: String) -> Vec<ChatMessage> {
    let system = concat!(
        "你负责给软件开发会话命名。只输出一个标题，不要解释。",
        "标题必须概括核心主题和预期结果，使用用户主要语言；中文建议 8-20 字，",
        "英文建议 3-8 个词，任何语言不超过 40 个字符。",
        "不要复述完整用户原句，不要包含请/帮我/Can you 等请求措辞，",
        "不要输出 Markdown、引号、标题前缀、路径、URL、账号、token、代码或日志。",
        "示例：‘新建 session 的名字，要自动总结合理’ -> ‘会话自动命名优化’。"
    );
    vec![
        ChatMessage {
            role: "system".into(),
            content: MessageContent::Text(system.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        },
        ChatMessage {
            role: "user".into(),
            content: MessageContent::Text(input),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        },
    ]
}

fn strip_title_prefix(mut title: &str) -> &str {
    const PREFIXES: &[&str] = &[
        "会话标题：",
        "会话标题:",
        "标题：",
        "标题:",
        "Title:",
        "title:",
        "请帮我",
        "帮我",
        "能否",
        "请",
        "Please ",
        "please ",
        "Can you ",
        "can you ",
    ];
    loop {
        let trimmed = title.trim_start();
        let Some(prefix) = PREFIXES.iter().find(|prefix| trimmed.starts_with(**prefix)) else {
            return trimmed;
        };
        title = &trimmed[prefix.len()..];
    }
}

fn contains_sensitive_shape(value: &str) -> bool {
    value.contains("<redacted>")
        || value.contains('`')
        || value.contains("```")
        || value.contains("=>")
        || value.contains('{')
        || value.contains('}')
        || URL_RE.is_match(value)
        || EMAIL_RE.is_match(value)
        || UUID_RE.is_match(value)
        || LONG_HASH_RE.is_match(value)
        || JWT_RE.is_match(value)
        || AWS_ACCESS_KEY_RE.is_match(value)
        || GITLAB_TOKEN_RE.is_match(value)
        || HIGH_ENTROPY_RE.is_match(value)
        || PHONE_RE.is_match(value)
        || ABSOLUTE_PATH_RE.is_match(value)
        || ROOT_POSIX_PATH_RE.is_match(value)
}

fn copies_prompt_prefix(title: &str, prompt: &str) -> bool {
    let compact_title: String = title
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let compact_prompt: String = prompt
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let title_graphemes: Vec<&str> = compact_title.graphemes(true).collect();
    if title_graphemes
        .windows(12)
        .any(|window| compact_prompt.contains(&window.concat()))
    {
        return true;
    }

    let normalize_words = |value: &str| {
        value
            .split_whitespace()
            .map(|word| {
                word.trim_matches(|c: char| {
                    c.is_ascii_punctuation() || "，。！？；：、…“”「」《》".contains(c)
                })
                .to_lowercase()
            })
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
    };
    let title_words = normalize_words(title);
    let prompt_words = normalize_words(prompt);
    title_words.windows(6).any(|title_window| {
        prompt_words
            .windows(6)
            .any(|prompt_window| title_window == prompt_window)
    })
}

pub(crate) fn normalize_generated_title(raw: &str, prompt: &str) -> Option<String> {
    let first_line = raw.lines().find(|line| !line.trim().is_empty())?.trim();
    let first_line = first_line.trim_matches(|c| {
        matches!(
            c,
            '`' | '#' | '*' | '_' | '"' | '\'' | '“' | '”' | '「' | '」'
        )
    });
    let first_line = strip_title_prefix(first_line);
    let first_line = crate::trajectory::redact_text(first_line, usize::MAX);
    let collapsed = WHITESPACE_RE.replace_all(first_line.trim(), " ");
    let normalized = collapsed.trim_matches(|c: char| {
        c.is_whitespace() || c.is_ascii_punctuation() || "，。！？；：、…“”「」《》".contains(c)
    });
    let title: String = normalized.graphemes(true).take(MAX_TITLE_CHARS).collect();
    if title.is_empty()
        || is_low_information(&title)
        || contains_sensitive_shape(&title)
        || copies_prompt_prefix(&title, prompt)
    {
        None
    } else {
        Some(title)
    }
}

fn safe_local_fallback(prompt: &str) -> String {
    let lower = redact_metadata_text(prompt, MAX_INPUT_CHARS).to_ascii_lowercase();
    let title = if (lower.contains("session") || lower.contains("会话"))
        && (lower.contains("标题") || lower.contains("名字") || lower.contains("命名"))
    {
        "会话命名优化"
    } else if lower.contains("登录") || lower.contains("认证") || lower.contains("auth") {
        "登录问题排查"
    } else if lower.contains("ci")
        || lower.contains("构建")
        || lower.contains("编译")
        || lower.contains("测试")
    {
        "CI 与构建问题排查"
    } else if lower.contains("界面")
        || lower.contains("布局")
        || lower.contains("sidebar")
        || lower.contains("ui")
        || lower.contains("ux")
    {
        "界面体验优化"
    } else if lower.contains("文档") || lower.contains("readme") {
        "文档内容整理"
    } else if lower.contains("图片")
        || lower.contains("截图")
        || lower.contains("附件")
        || lower.contains("上传")
    {
        "图片与附件分析"
    } else if lower.contains("性能") || lower.contains("卡顿") || lower.contains("slow") {
        "性能问题排查"
    } else if lower.contains("错误")
        || lower.contains("失败")
        || lower.contains("报错")
        || lower.contains("bug")
    {
        "问题原因排查"
    } else if lower.contains("代码")
        || lower.contains("实现")
        || lower.contains("重构")
        || lower.contains("refactor")
    {
        "代码实现与优化"
    } else {
        PLACEHOLDER_TITLE
    };
    title.into()
}

async fn compare_and_set_title(
    db: &SqlitePool,
    session_id: &str,
    title: &str,
    source: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE sessions SET title = ?, title_source = ?
         WHERE id = ? AND title_source = 'placeholder'",
    )
    .bind(title)
    .bind(source)
    .bind(session_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

async fn session_is_placeholder(db: &SqlitePool, session_id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM sessions WHERE id = ? AND title_source = 'placeholder'
         )",
    )
    .bind(session_id)
    .fetch_one(db)
    .await
}

async fn claim_title_job_lease(
    db: &SqlitePool,
    session_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let lease_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let stale_before = now - TITLE_JOB_LEASE_MS;
    let result = sqlx::query(
        "INSERT INTO session_title_jobs (session_id, lease_id, started_at)
         VALUES (?, ?, ?)
         ON CONFLICT(session_id) DO UPDATE SET
           lease_id = excluded.lease_id,
           started_at = excluded.started_at
         WHERE session_title_jobs.started_at <= ?",
    )
    .bind(session_id)
    .bind(&lease_id)
    .bind(now)
    .bind(stale_before)
    .execute(db)
    .await?;
    Ok((result.rows_affected() == 1).then_some(lease_id))
}

async fn release_title_job_lease(db: &SqlitePool, session_id: &str, lease_id: &str) {
    if let Err(error) =
        sqlx::query("DELETE FROM session_title_jobs WHERE session_id = ? AND lease_id = ?")
            .bind(session_id)
            .bind(lease_id)
            .execute(db)
            .await
    {
        tracing::warn!("session title lease release failed: {error}");
    }
}

async fn record_title_attempt(
    db: &SqlitePool,
    attempt_id: &str,
    session_id: &str,
    endpoint: &str,
    model: &str,
    status: &str,
    failure_code: Option<&str>,
    duration: Duration,
) {
    let duration_ms = duration.as_millis().min(i64::MAX as u128) as i64;
    if let Err(error) = sqlx::query(
        "INSERT OR IGNORE INTO session_title_attempts
         (id, session_id, endpoint, model, status, failure_code, duration_ms, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(attempt_id)
    .bind(session_id)
    .bind(endpoint)
    .bind(model)
    .bind(status)
    .bind(failure_code)
    .bind(duration_ms)
    .bind(chrono::Utc::now().timestamp_millis())
    .execute(db)
    .await
    {
        tracing::warn!("session title attempt persistence failed: {error}");
    }
}

fn classify_title_usage(
    endpoint_name: &str,
    base_url: &str,
    is_chatgpt: bool,
    provider_cost: Option<f64>,
) -> (String, Option<f64>, &'static str) {
    let base = base_url.to_ascii_lowercase();
    let local_endpoint = base.contains("127.0.0.1")
        || base.contains("localhost")
        || base.contains("0.0.0.0")
        || base.starts_with("http://[::1]");
    if is_chatgpt {
        ("chatgpt".into(), None, "subscription")
    } else if local_endpoint {
        (endpoint_name.into(), None, "local")
    } else if let Some(cost) = provider_cost.filter(|cost| cost.is_finite() && *cost >= 0.0) {
        let provider = if base.contains("openrouter.ai") {
            "openrouter".into()
        } else {
            endpoint_name.into()
        };
        (provider, Some(cost), "provider_actual")
    } else {
        (endpoint_name.into(), None, "unknown")
    }
}

async fn record_title_usage(
    app: &AppHandle,
    db: &SqlitePool,
    session_id: &str,
    attempt_id: &str,
    output: &InternalTextOutput,
) {
    let Some(usage) = output.usage.as_ref() else {
        return;
    };
    let (provider, actual_cost, cost_source) = classify_title_usage(
        &output.endpoint_name,
        &output.base_url,
        output.is_chatgpt,
        usage.cost,
    );
    let event = UsageEventInput {
        request_id: format!("session-title:{attempt_id}"),
        session_id: session_id.to_string(),
        task_id: None,
        surface: crate::agent::UsageSurface::SessionTitle.as_str().into(),
        provider,
        endpoint: output.endpoint_name.clone(),
        model: output.model_id.clone(),
        input_tokens: usage.prompt_tokens as i64,
        output_tokens: usage.completion_tokens as i64,
        reasoning_tokens: usage
            .completion_tokens_details
            .as_ref()
            .map_or(0, |details| details.reasoning_tokens as i64),
        cached_tokens: usage
            .prompt_tokens_details
            .as_ref()
            .map_or(0, |details| details.cached_tokens as i64),
        actual_cost_usd: actual_cost,
        estimated_cost_usd: None,
        cost_source: cost_source.into(),
        created_at: None,
    };
    match record_usage_event(db, event).await {
        Ok(true) => {
            app.emit("model-usage-recorded", session_id).ok();
            app.emit("token-usage-recorded", session_id).ok();
        }
        Ok(false) => {}
        Err(error) => tracing::warn!("session title usage persistence failed: {error}"),
    }
}

async fn emit_updated_title(app: &AppHandle, db: &SqlitePool, session_id: &str) {
    if let Ok(session) = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ?")
        .bind(session_id)
        .fetch_one(db)
        .await
    {
        let event_name = format!("session_updated:{session_id}");
        app.emit(&event_name, &session).ok();
        app.emit(SESSION_TITLE_UPDATED_EVENT, &session).ok();
    }
}

/// Finish a failed primary turn without making another Provider request. This
/// keeps the placeholder lifecycle deterministic when credentials/network fail.
pub(crate) async fn apply_local_title_fallback(
    app: &AppHandle,
    db: &SqlitePool,
    session_id: &str,
    route: &RouteCandidate,
    user_message: &str,
    failure_code: &'static str,
) {
    let attempt_id = uuid::Uuid::new_v4().to_string();
    let started_at = Instant::now();
    let title = safe_local_fallback(user_message);
    let status = match compare_and_set_title(db, session_id, &title, TITLE_SOURCE_FALLBACK).await {
        Ok(true) => {
            emit_updated_title(app, db, session_id).await;
            TITLE_SOURCE_FALLBACK
        }
        Ok(false) => "cas_lost",
        Err(error) => {
            tracing::warn!("session title local fallback persistence failed: {error}");
            "persistence_error"
        }
    };
    record_title_attempt(
        db,
        &attempt_id,
        session_id,
        &route.endpoint_name,
        &route.model_id,
        status,
        Some(if status == "persistence_error" {
            "persistence_error"
        } else {
            failure_code
        }),
        started_at.elapsed(),
    )
    .await;
}

/// Start one best-effort title request. Returns `false` when the input is not
/// eligible or another task already owns this session's title job.
pub(crate) fn spawn_title_generation(
    app: AppHandle,
    db: SqlitePool,
    session_id: String,
    route: RouteCandidate,
    user_message: String,
    assistant_summary: Option<String>,
) -> bool {
    let Some(input) = prepared_title_input(&user_message, assistant_summary.as_deref()) else {
        return false;
    };
    let Some(active_job) = claim_title_job(&session_id) else {
        tracing::debug!("session title generation discarded: already_running");
        return false;
    };
    tokio::spawn(async move {
        let _active_job = active_job;
        let lease_id = match claim_title_job_lease(&db, &session_id).await {
            Ok(Some(lease_id)) => lease_id,
            Ok(None) => {
                tracing::debug!("session title generation discarded: lease_held");
                return;
            }
            Err(error) => {
                tracing::warn!("session title lease claim failed: {error}");
                return;
            }
        };
        match session_is_placeholder(&db, &session_id).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!("session title generation discarded: no_longer_placeholder");
                release_title_job_lease(&db, &session_id, &lease_id).await;
                return;
            }
            Err(error) => {
                tracing::warn!("session title placeholder check failed: {error}");
                release_title_job_lease(&db, &session_id, &lease_id).await;
                return;
            }
        }
        // The caller gates this behind the completed primary turn. Yield once
        // more so any immediately-ready user-visible cleanup stays ahead.
        tokio::task::yield_now().await;
        let started_at = Instant::now();
        let attempt_endpoint = route.endpoint_name.clone();
        let attempt_model = route.model_id.clone();
        let generated = generate_bounded_text(
            route,
            &session_id,
            title_messages(input.clone()),
            MAX_TITLE_OUTPUT_TOKENS,
            TITLE_DEADLINE,
        )
        .await;
        let (title, source, failure_code) = match generated {
            Ok(output) => {
                record_title_usage(&app, &db, &session_id, &lease_id, &output).await;
                match normalize_generated_title(&output.text, &input) {
                    Some(title) => (title, TITLE_SOURCE_GENERATED, None),
                    None => {
                        tracing::warn!("session title generation rejected: invalid_output");
                        (
                            safe_local_fallback(&user_message),
                            TITLE_SOURCE_FALLBACK,
                            Some("invalid_output"),
                        )
                    }
                }
            }
            Err(error) => {
                let code = if error.contains("SESSION_TITLE_TIMEOUT") {
                    "timeout"
                } else if error.contains("AUTH_") || error.contains("CREDENTIAL_") {
                    "credential_unavailable"
                } else {
                    "provider_error"
                };
                tracing::warn!("session title generation failed: {code}");
                (
                    safe_local_fallback(&user_message),
                    TITLE_SOURCE_FALLBACK,
                    Some(code),
                )
            }
        };
        let duration = started_at.elapsed();
        tracing::info!(
            "session title generation completed: source={source} duration_ms={}",
            duration.as_millis()
        );
        let (status, recorded_failure_code) =
            match compare_and_set_title(&db, &session_id, &title, source).await {
                Ok(true) => {
                    emit_updated_title(&app, &db, &session_id).await;
                    (source, failure_code)
                }
                Ok(false) => {
                    tracing::debug!("session title generation discarded: cas_lost");
                    ("cas_lost", failure_code)
                }
                Err(error) => {
                    tracing::warn!("session title persistence failed: {error}");
                    ("persistence_error", Some("persistence_error"))
                }
            };
        record_title_attempt(
            &db,
            &lease_id,
            &session_id,
            &attempt_endpoint,
            &attempt_model,
            status,
            recorded_failure_code,
            duration,
        )
        .await;
        release_title_job_lease(&db, &session_id, &lease_id).await;
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_input_removes_code_attachments_paths_and_secrets() {
        let input = prepared_title_input(
            "请排查 token=super-secret /Users/leo/private/app.log /Volumes/客户项目/财务.xlsx ~/hidden.txt ssh://private.example/a eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ0ZXN0LXVzZXIifQ.signature123456 AKIAIOSFODNN7EXAMPLE abcdefghijklmnopqrstuvwxyzABCDEF https://example.com/a `let hidden = 1`\n```rust\nfn secret() {}\n```\nERROR database password leaked\n![screen.png](file:///Users/leo/private/screen.png)\n\n已上传以下文件（.pdf 用 read_file）：\n- private-plan.pdf — 本地路径: /Users/leo/private/private-plan.pdf",
            None,
        )
        .unwrap();
        assert!(!input.contains("super-secret"), "{input}");
        assert!(!input.contains("/Users/leo"), "{input}");
        assert!(!input.contains("/Volumes"), "{input}");
        assert!(!input.contains("~/hidden"), "{input}");
        assert!(!input.contains("private.example"), "{input}");
        assert!(!input.contains("eyJhbGci"), "{input}");
        assert!(!input.contains("AKIAIOSFODNN7EXAMPLE"), "{input}");
        assert!(
            !input.contains("abcdefghijklmnopqrstuvwxyzABCDEF"),
            "{input}"
        );
        assert!(!input.contains("example.com"), "{input}");
        assert!(!input.contains("fn secret"), "{input}");
        assert!(!input.contains("let hidden"), "{input}");
        assert!(!input.contains("database password"), "{input}");
        assert!(!input.contains("screen.png"), "{input}");
        assert!(!input.contains("private-plan.pdf"), "{input}");
        assert!(input.contains("[代码]"), "{input}");
        assert!(input.contains("[附件]"), "{input}");
    }

    #[test]
    fn title_input_and_output_reject_short_gitlab_tokens_and_root_posix_paths() {
        let input = redact_metadata_text(
            "请检查 /secrets.env 中的 glpat-a1b2c3d4 配置",
            MAX_INPUT_CHARS,
        );
        assert!(!input.contains("/secrets.env"), "{input}");
        assert!(!input.contains("glpat-a1b2c3d4"), "{input}");
        assert!(normalize_generated_title("检查 /secrets.env 配置", "排查配置问题").is_none());
        assert!(normalize_generated_title("撤销 glpat-a1b2c3d4", "排查认证问题").is_none());
    }

    #[test]
    fn chinese_adjacent_sensitive_shapes_are_redacted_and_rejected() {
        let fixtures = [
            ("https://private.example", "链接https://private.example继续"),
            ("leo@example.com", "邮箱leo@example.com继续"),
            (
                "550e8400-e29b-41d4-a716-446655440000",
                "标识550e8400-e29b-41d4-a716-446655440000继续",
            ),
            ("AKIAIOSFODNN7EXAMPLE", "密钥AKIAIOSFODNN7EXAMPLE继续"),
        ];
        for (secret, input) in fixtures {
            let redacted = redact_metadata_text(input, MAX_INPUT_CHARS);
            assert!(!redacted.contains(secret), "{redacted}");
        }

        for title in [
            "修复https://private.example故障",
            "联系leo@example.com处理",
            "标识550e8400-e29b-41d4-a716-446655440000泄露",
            "撤销AKIAIOSFODNN7EXAMPLE密钥",
        ] {
            assert!(
                normalize_generated_title(title, "完全不同的用户需求").is_none(),
                "{title}"
            );
        }
    }

    #[test]
    fn low_information_first_message_waits_for_visible_context() {
        assert!(prepared_title_input("继续", None).is_none());
        assert!(prepared_title_input("继续", Some("已定位到会话标题只是原文截断")).is_some());
    }

    #[test]
    fn generated_title_is_single_line_clean_and_bounded() {
        let title = normalize_generated_title(
            "**标题：\"会话自动命名优化。\"**\n这是解释",
            "新建 session 的名字，要自动进行总结个合理的",
        )
        .unwrap();
        assert_eq!(title, "会话自动命名优化");
        let long = "界".repeat(MAX_TITLE_CHARS + 5);
        assert_eq!(
            normalize_generated_title(&long, "完全不同的需求描述")
                .unwrap()
                .graphemes(true)
                .count(),
            MAX_TITLE_CHARS
        );
        let emoji = "👨‍👩‍👧‍👦".repeat(MAX_TITLE_CHARS + 2);
        let emoji_title = normalize_generated_title(&emoji, "完全不同的需求描述").unwrap();
        assert_eq!(emoji_title.graphemes(true).count(), MAX_TITLE_CHARS);
        assert!(emoji_title.ends_with("👨‍👩‍👧‍👦"));
    }

    #[test]
    fn generated_title_rejects_secret_and_long_prompt_copy() {
        assert!(normalize_generated_title("登录 token=secret-value 排查", "排查登录").is_none());
        assert!(normalize_generated_title(
            "请你先仔细阅读下面的背景信息不要马上改代码",
            "请你先仔细阅读下面的背景信息不要马上改代码。我发现标题有问题"
        )
        .is_none());
    }

    #[test]
    fn generated_title_strips_can_you_chinese_prefix() {
        assert_eq!(
            normalize_generated_title("能否优化会话自动命名", "完全不同的用户需求").as_deref(),
            Some("优化会话自动命名")
        );
    }

    #[test]
    fn generated_title_rejects_any_twelve_grapheme_prompt_window() {
        assert!(normalize_generated_title(
            "前缀请你先仔细阅读下面的背景信息后缀",
            "请你先仔细阅读下面的背景信息。我发现标题有问题"
        )
        .is_none());
    }

    #[test]
    fn generated_title_rejects_any_six_word_prompt_window() {
        assert!(normalize_generated_title(
            "X one two three four five six Y",
            "Please one two three four five six now"
        )
        .is_none());
    }

    #[test]
    fn fallback_is_safe_and_never_returns_the_prompt_prefix() {
        assert_eq!(
            safe_local_fallback("新建 session 的名字，需要自动总结"),
            "会话命名优化"
        );
        assert_eq!(
            safe_local_fallback("登录失败 token=super-secret"),
            "登录问题排查"
        );
        assert_eq!(safe_local_fallback("谈谈今天"), PLACEHOLDER_TITLE);
    }

    #[test]
    fn title_usage_keeps_custom_local_and_openrouter_provider_identity() {
        assert_eq!(
            classify_title_usage("custom", "https://llm.example/v1", false, Some(0.01)),
            ("custom".into(), Some(0.01), "provider_actual")
        );
        assert_eq!(
            classify_title_usage("ollama", "http://localhost:11434/v1", false, Some(0.01)),
            ("ollama".into(), None, "local")
        );
        assert_eq!(
            classify_title_usage("router", "https://openrouter.ai/api/v1", false, Some(0.01)),
            ("openrouter".into(), Some(0.01), "provider_actual")
        );
        assert_eq!(
            classify_title_usage("chatgpt", "https://chatgpt.com", true, Some(0.01)),
            ("chatgpt".into(), None, "subscription")
        );
    }

    #[tokio::test]
    async fn title_attempt_telemetry_records_failure_without_prompt_or_title_columns() {
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE session_title_attempts (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                endpoint TEXT NOT NULL, model TEXT NOT NULL,
                status TEXT NOT NULL, failure_code TEXT,
                duration_ms INTEGER NOT NULL, created_at INTEGER NOT NULL
             )",
        )
        .execute(&db)
        .await
        .unwrap();

        record_title_attempt(
            &db,
            "attempt-1",
            "session-1",
            "fixture",
            "model-1",
            "fallback",
            Some("timeout"),
            Duration::from_millis(42),
        )
        .await;

        let row: (String, String, String, String, Option<String>, i64) = sqlx::query_as(
            "SELECT session_id, endpoint, model, status, failure_code, duration_ms
             FROM session_title_attempts WHERE id='attempt-1'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                "session-1".into(),
                "fixture".into(),
                "model-1".into(),
                "fallback".into(),
                Some("timeout".into()),
                42,
            )
        );
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('session_title_attempts')")
                .fetch_all(&db)
                .await
                .unwrap();
        assert!(!columns.iter().any(|column| column == "prompt"));
        assert!(!columns.iter().any(|column| column == "title"));
    }

    #[tokio::test]
    async fn late_generated_title_never_overwrites_manual_title_or_recency() {
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, title_source TEXT NOT NULL,
                updated_at INTEGER NOT NULL
             )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions VALUES ('auto', '新会话', 'placeholder', 7),
                                        ('manual', '我的标题', 'manual', 9)",
        )
        .execute(&db)
        .await
        .unwrap();

        assert!(session_is_placeholder(&db, "auto").await.unwrap());
        assert!(!session_is_placeholder(&db, "manual").await.unwrap());
        assert!(!session_is_placeholder(&db, "missing").await.unwrap());

        assert!(
            compare_and_set_title(&db, "auto", "会话命名优化", "generated")
                .await
                .unwrap()
        );
        assert!(!session_is_placeholder(&db, "auto").await.unwrap());
        assert!(
            !compare_and_set_title(&db, "manual", "迟到标题", "generated")
                .await
                .unwrap()
        );
        let rows: Vec<(String, String, String, i64)> =
            sqlx::query_as("SELECT id, title, title_source, updated_at FROM sessions ORDER BY id")
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("auto".into(), "会话命名优化".into(), "generated".into(), 7),
                ("manual".into(), "我的标题".into(), "manual".into(), 9),
            ]
        );
    }

    #[tokio::test]
    async fn title_job_lease_is_single_flight_and_stale_safe() {
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE session_title_jobs (
                session_id TEXT PRIMARY KEY,
                lease_id TEXT NOT NULL,
                started_at INTEGER NOT NULL
             )",
        )
        .execute(&db)
        .await
        .unwrap();

        let first = claim_title_job_lease(&db, "session")
            .await
            .unwrap()
            .expect("first lease");
        assert!(claim_title_job_lease(&db, "session")
            .await
            .unwrap()
            .is_none());
        sqlx::query("UPDATE session_title_jobs SET started_at = ? WHERE session_id = 'session'")
            .bind(chrono::Utc::now().timestamp_millis() - TITLE_JOB_LEASE_MS - 1)
            .execute(&db)
            .await
            .unwrap();
        let recovered = claim_title_job_lease(&db, "session")
            .await
            .unwrap()
            .expect("stale lease recovered");
        release_title_job_lease(&db, "session", &first).await;
        let still_owned: String = sqlx::query_scalar(
            "SELECT lease_id FROM session_title_jobs WHERE session_id='session'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(still_owned, recovered);
        release_title_job_lease(&db, "session", &recovered).await;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_title_jobs")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn title_jobs_are_single_flight_per_session() {
        let session_id = format!("single-flight-{}", uuid::Uuid::new_v4());
        let first = claim_title_job(&session_id).expect("first job claims the session");
        assert!(claim_title_job(&session_id).is_none());
        drop(first);
        assert!(claim_title_job(&session_id).is_some());
    }
}
