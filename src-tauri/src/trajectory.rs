// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use sqlx::SqlitePool;

use crate::errors::AppError;
use crate::openrouter::types::ToolCall;

const MAX_ARGUMENT_CHARS: usize = 8_000;
const MAX_RESULT_CHARS: usize = 4_000;
const MAX_ERROR_CHARS: usize = 1_000;

static SENSITIVE_TEXT_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?is)-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----")
            .expect("private key regex"),
        Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]{6,}").expect("bearer regex"),
        Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{6,}|gh[pousr]_[A-Za-z0-9_]{6,})\b")
            .expect("token regex"),
        Regex::new(
            r#"(?i)(["']?(?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|password|passwd|secret|authorization|cookie|credential)["']?\s*[:=]\s*["']?)[^\s,;'"`&\\}]+"#,
        )
        .expect("assignment secret regex"),
        Regex::new(r"(?i)(://[^:/\s]+:)[^@/\s]+@").expect("url userinfo regex"),
    ]
});

pub(crate) fn trace_record_id(session_id: &str, provider_tool_call_id: &str) -> String {
    format!("{session_id}:{provider_tool_call_id}")
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
    [
        "api_key",
        "access_token",
        "refresh_token",
        "token",
        "password",
        "passwd",
        "secret",
        "authorization",
        "cookie",
        "credential",
    ]
    .iter()
    .any(|needle| key == *needle || key.ends_with(&format!("_{needle}")))
}

fn redact_json_with_limit(value: &Value, max_string_chars: usize) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = if sensitive_key(key) {
                        Value::String("<redacted>".into())
                    } else {
                        redact_json_with_limit(value, max_string_chars)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| redact_json_with_limit(value, max_string_chars))
                .collect(),
        ),
        Value::String(text) => Value::String(redact_text(text, max_string_chars)),
        other => other.clone(),
    }
}

pub(crate) fn redact_json(value: &Value) -> Value {
    redact_json_with_limit(value, MAX_ARGUMENT_CHARS)
}

pub(crate) fn redact_text(text: &str, max_chars: usize) -> String {
    let mut out = text.to_string();
    for pattern in SENSITIVE_TEXT_PATTERNS.iter() {
        out = pattern
            .replace_all(&out, |captures: &regex::Captures<'_>| {
                if captures.len() > 1 {
                    format!(
                        "{}<redacted>",
                        captures.get(1).map(|m| m.as_str()).unwrap_or("")
                    )
                } else {
                    "<redacted>".to_string()
                }
            })
            .into_owned();
    }
    if max_chars == usize::MAX || out.chars().count() <= max_chars {
        out
    } else {
        format!(
            "{}…<truncated>",
            out.chars().take(max_chars).collect::<String>()
        )
    }
}

pub(crate) fn redact_tool_calls_for_storage(tool_calls: &[ToolCall]) -> serde_json::Result<String> {
    let mut redacted = tool_calls.to_vec();
    for tool_call in &mut redacted {
        tool_call.function.arguments =
            match serde_json::from_str::<Value>(&tool_call.function.arguments) {
                Ok(arguments) => {
                    serde_json::to_string(&redact_json_with_limit(&arguments, usize::MAX))?
                }
                Err(_) => redact_text(&tool_call.function.arguments, usize::MAX),
            };
    }
    serde_json::to_string(&redacted)
}

pub(crate) fn redact_tool_result_for_storage(result: &str) -> String {
    redact_derived_message_for_storage(result)
}

pub(crate) fn redact_derived_message_for_storage(message: &str) -> String {
    match serde_json::from_str::<Value>(message) {
        Ok(value) => serde_json::to_string(&redact_json_with_limit(&value, usize::MAX))
            .unwrap_or_else(|_| redact_text(message, usize::MAX)),
        Err(_) => redact_text(message, usize::MAX),
    }
}

fn redacted_arguments(arguments: &Value) -> String {
    let serialized = serde_json::to_string(&redact_json(arguments)).unwrap_or_else(|_| "{}".into());
    if serialized.chars().count() <= MAX_ARGUMENT_CHARS {
        serialized
    } else {
        serde_json::json!({
            "truncated_preview": redact_text(&serialized, MAX_ARGUMENT_CHARS),
        })
        .to_string()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_tool_call_started(
    pool: &SqlitePool,
    session_id: &str,
    message_id: &str,
    provider_tool_call_id: &str,
    tool_name: &str,
    arguments: &Value,
) -> Result<(), AppError> {
    let id = trace_record_id(session_id, provider_tool_call_id);
    sqlx::query(
        "INSERT OR IGNORE INTO tool_calls \
         (id, message_id, tool_name, arguments, status, created_at) \
         VALUES (?, ?, ?, ?, 'pending', ?)",
    )
    .bind(id)
    .bind(message_id)
    .bind(tool_name)
    .bind(redacted_arguments(arguments))
    .bind(Utc::now().timestamp_millis())
    .execute(pool)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) async fn record_tool_call_finished(
    pool: &SqlitePool,
    session_id: &str,
    provider_tool_call_id: &str,
    status: &str,
    result: Option<&str>,
    error: Option<&str>,
    duration_ms: i64,
) -> Result<(), AppError> {
    if !matches!(status, "done" | "error" | "denied" | "cancelled") {
        return Err(AppError::Other(format!(
            "unsupported normalized tool-call status: {status}"
        )));
    }
    let id = trace_record_id(session_id, provider_tool_call_id);
    let result = result.map(|text| redact_text(text, MAX_RESULT_CHARS));
    let error = error.map(|text| redact_text(text, MAX_ERROR_CHARS));
    let updated = sqlx::query(
        "UPDATE tool_calls SET result = ?, status = ?, error = ?, duration_ms = ? WHERE id = ?",
    )
    .bind(result)
    .bind(status)
    .bind(error)
    .bind(duration_ms.max(0))
    .bind(&id)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Other(format!(
            "normalized tool call missing for completion: {id}"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_terminal_tool_outcome(
    pool: &SqlitePool,
    session_id: &str,
    provider_tool_call_id: &str,
    status: &str,
    result: Option<&str>,
    error: Option<&str>,
    duration_ms: i64,
) -> Result<(), AppError> {
    if !matches!(status, "done" | "error" | "denied" | "cancelled") {
        return Err(AppError::Other(format!(
            "unsupported normalized tool-call status: {status}"
        )));
    }

    let mut transaction = pool.begin().await?;
    let trace_id = trace_record_id(session_id, provider_tool_call_id);
    let redacted_result = result.map(|text| redact_text(text, MAX_RESULT_CHARS));
    let redacted_error = error.map(|text| redact_text(text, MAX_ERROR_CHARS));
    let updated = sqlx::query(
        "UPDATE tool_calls SET result = ?, status = ?, error = ?, duration_ms = ? WHERE id = ?",
    )
    .bind(redacted_result)
    .bind(status)
    .bind(redacted_error)
    .bind(duration_ms.max(0))
    .bind(&trace_id)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Other(format!(
            "normalized tool call missing for completion: {trace_id}"
        )));
    }

    // Provider replay requires one tool-result message for every declared
    // tool call, including permission denials, hook cancellations, and
    // dispatch failures. Use a stable id and update its content together with
    // the normalized row so a terminal-state retry cannot leave stale replay.
    let replay_message_id = format!("{trace_id}:result");
    let replay_text = redact_tool_result_for_storage(result.or(error).unwrap_or(""));
    let content = serde_json::json!({
        "tool_call_id": provider_tool_call_id,
        "content": replay_text,
        "status": status,
    })
    .to_string();
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at) \
         VALUES (?, ?, 'tool', ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             session_id = excluded.session_id, role = 'tool', content = excluded.content",
    )
    .bind(replay_message_id)
    .bind(session_id)
    .bind(content)
    .bind(Utc::now().timestamp_millis())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openrouter::types::{FunctionCall, ToolCall};
    use serde_json::json;
    use sqlx::{sqlite::SqlitePoolOptions, Row};

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE tool_calls (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                arguments TEXT NOT NULL DEFAULT '{}',
                result TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                error TEXT,
                duration_ms INTEGER,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[test]
    fn redaction_masks_secret_keys_and_token_patterns() {
        let value = json!({
            "command": "curl -H 'Authorization: Bearer live-token-123' https://example.test",
            "api_key": "sk-live-secret",
            "nested": {"password": "hunter2", "safe": "visible"}
        });
        let redacted = redact_json(&value).to_string();
        assert!(!redacted.contains("live-token-123"));
        assert!(!redacted.contains("sk-live-secret"));
        assert!(!redacted.contains("hunter2"));
        assert!(redacted.contains("visible"));
        assert!(redacted.contains("<redacted>"));
    }

    #[test]
    fn persisted_tool_call_payloads_are_redacted_without_changing_live_calls() {
        let live = vec![ToolCall {
            id: "call-1".into(),
            r#type: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: json!({
                    "command": "printf token=CF_EVO_TEST_SECRET",
                    "token": "CF_EVO_TEST_SECRET"
                })
                .to_string(),
            },
        }];

        let persisted = redact_tool_calls_for_storage(&live).unwrap();

        assert!(live[0].function.arguments.contains("CF_EVO_TEST_SECRET"));
        assert!(!persisted.contains("CF_EVO_TEST_SECRET"));
        assert!(persisted.contains("<redacted>"));
        assert!(persisted.contains("bash"));
    }

    #[test]
    fn derived_message_redaction_preserves_quotes_and_length() {
        let message = "command failed: printf 'token=CF_EVO_MESSAGE_SECRET\\n'";
        let persisted = redact_derived_message_for_storage(message);

        assert!(!persisted.contains("CF_EVO_MESSAGE_SECRET"));
        assert_eq!(persisted, "command failed: printf 'token=<redacted>\\n'");

        let long_safe_message = "x".repeat(MAX_RESULT_CHARS + 100);
        assert_eq!(
            redact_derived_message_for_storage(&long_safe_message),
            long_safe_message
        );

        assert_eq!(
            redact_derived_message_for_storage("stderr: `token=CF_EVO_MARKDOWN_SECRET`"),
            "stderr: `token=<redacted>`"
        );
    }

    #[test]
    fn derived_json_redaction_masks_quoted_secret_keys() {
        let message = r#"{"token":"CF_EVO_JSON_SECRET","nested":{"password":"CF_EVO_JSON_PASSWORD"},"safe":"visible"}"#;

        let persisted = redact_derived_message_for_storage(message);

        assert!(!persisted.contains("CF_EVO_JSON_SECRET"));
        assert!(!persisted.contains("CF_EVO_JSON_PASSWORD"));
        assert!(persisted.contains("visible"));
        let parsed: serde_json::Value = serde_json::from_str(&persisted).unwrap();
        assert_eq!(parsed["token"], "<redacted>");
        assert_eq!(parsed["nested"]["password"], "<redacted>");
    }

    #[test]
    fn derived_json_redaction_handles_escaped_and_non_string_secret_values() {
        let message = json!({
            "token": "prefix\\\"CF_EVO_ESCAPED_SUFFIX",
            "password": 123456,
            "authorization": true,
            "safe": "visible"
        })
        .to_string();

        let persisted = redact_derived_message_for_storage(&message);
        let parsed: serde_json::Value = serde_json::from_str(&persisted).unwrap();

        assert!(!persisted.contains("CF_EVO_ESCAPED_SUFFIX"));
        assert_eq!(parsed["token"], "<redacted>");
        assert_eq!(parsed["password"], "<redacted>");
        assert_eq!(parsed["authorization"], "<redacted>");
        assert_eq!(parsed["safe"], "visible");
    }

    #[tokio::test]
    async fn normalized_tool_lifecycle_persists_redacted_success() {
        let pool = test_pool().await;
        record_tool_call_started(
            &pool,
            "session-1",
            "message-1",
            "call-1",
            "bash",
            &json!({"command":"echo ok", "token":"ghp_super_secret"}),
        )
        .await
        .unwrap();

        record_tool_call_finished(
            &pool,
            "session-1",
            "call-1",
            "done",
            Some("ok; Authorization: Bearer result-secret"),
            None,
            42,
        )
        .await
        .unwrap();

        let row = sqlx::query(
            "SELECT id, message_id, tool_name, arguments, result, status, error, duration_ms \
             FROM tool_calls",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("id"), "session-1:call-1");
        assert_eq!(row.get::<String, _>("message_id"), "message-1");
        assert_eq!(row.get::<String, _>("tool_name"), "bash");
        assert_eq!(row.get::<String, _>("status"), "done");
        assert_eq!(row.get::<i64, _>("duration_ms"), 42);
        assert!(row.try_get::<Option<String>, _>("error").unwrap().is_none());
        let args = row.get::<String, _>("arguments");
        let result = row.get::<String, _>("result");
        assert!(!args.contains("ghp_super_secret"));
        assert!(!result.contains("result-secret"));
    }

    #[tokio::test]
    async fn cancelled_tool_is_terminal_and_replayable() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        record_tool_call_started(
            &pool,
            "session-1",
            "message-1",
            "call-cancelled",
            "bash",
            &json!({"command":"sleep 10"}),
        )
        .await
        .unwrap();

        record_terminal_tool_outcome(
            &pool,
            "session-1",
            "call-cancelled",
            "cancelled",
            None,
            Some("Tool call cancelled by user."),
            0,
        )
        .await
        .unwrap();

        let status: String = sqlx::query_scalar(
            "SELECT status FROM tool_calls WHERE id = 'session-1:call-cancelled'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let replay: String = sqlx::query_scalar(
            "SELECT content FROM messages WHERE id = 'session-1:call-cancelled:result'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "cancelled");
        assert_eq!(
            serde_json::from_str::<Value>(&replay).unwrap()["status"],
            "cancelled"
        );
    }

    #[tokio::test]
    async fn normalized_tool_lifecycle_persists_error_and_is_idempotent() {
        let pool = test_pool().await;
        for _ in 0..2 {
            record_tool_call_started(
                &pool,
                "session-1",
                "message-1",
                "call-err",
                "bash",
                &json!({"command":"false"}),
            )
            .await
            .unwrap();
        }
        record_tool_call_finished(
            &pool,
            "session-1",
            "call-err",
            "error",
            None,
            Some("failed with password=super-secret"),
            7,
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_calls")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let row = sqlx::query("SELECT status, error, duration_ms FROM tool_calls")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("status"), "error");
        assert_eq!(row.get::<i64, _>("duration_ms"), 7);
        assert!(!row.get::<String, _>("error").contains("super-secret"));
    }

    #[tokio::test]
    async fn terminal_outcomes_always_persist_one_redacted_replay_message() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (call_id, status, result, error, duration_ms) in [
            (
                "call-done",
                "done",
                Some(r#"{"safe":"visible","token":"CF_EVO_DONE_SECRET"}"#),
                None,
                19,
            ),
            (
                "call-error",
                "error",
                None,
                Some(r#"dispatch failed: {"password":"CF_EVO_ERROR_SECRET"}"#),
                7,
            ),
            (
                "call-denied",
                "denied",
                None,
                Some("Tool call cancelled by hook."),
                0,
            ),
        ] {
            record_tool_call_started(
                &pool,
                "session-1",
                "assistant-message-1",
                call_id,
                "bash",
                &json!({"command":"printf ok"}),
            )
            .await
            .unwrap();

            record_terminal_tool_outcome(
                &pool,
                "session-1",
                call_id,
                status,
                result,
                error,
                duration_ms,
            )
            .await
            .unwrap();
        }

        record_terminal_tool_outcome(
            &pool,
            "session-1",
            "call-denied",
            "denied",
            None,
            Some("Tool call cancelled by hook."),
            0,
        )
        .await
        .unwrap();

        record_terminal_tool_outcome(
            &pool,
            "session-1",
            "call-error",
            "done",
            Some(r#"{"safe":"recovered"}"#),
            None,
            11,
        )
        .await
        .unwrap();

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT role, content FROM messages ORDER BY created_at ASC")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|(role, _)| role == "tool"));
        let joined = rows
            .iter()
            .map(|(_, content)| content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("call-done"));
        assert!(joined.contains("call-error"));
        assert!(joined.contains("call-denied"));
        assert!(joined.contains("visible"));
        assert!(joined.contains("recovered"));
        assert!(!joined.contains("dispatch failed"));
        assert!(!joined.contains("CF_EVO_DONE_SECRET"));
        assert!(!joined.contains("CF_EVO_ERROR_SECRET"));

        let terminal_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tool_calls WHERE status IN ('done','error','denied')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(terminal_count, 3);
    }

    #[tokio::test]
    async fn terminal_outcome_rolls_back_when_replay_cannot_be_persisted() {
        let pool = test_pool().await;
        record_tool_call_started(
            &pool,
            "session-atomic",
            "assistant-message-atomic",
            "call-atomic",
            "bash",
            &json!({"command":"printf ok"}),
        )
        .await
        .unwrap();

        let error = record_terminal_tool_outcome(
            &pool,
            "session-atomic",
            "call-atomic",
            "done",
            Some("ok"),
            None,
            3,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("messages"));
        let row: (String, Option<String>) =
            sqlx::query_as("SELECT status, result FROM tool_calls WHERE id = ?")
                .bind("session-atomic:call-atomic")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "pending");
        assert!(row.1.is_none());
    }
}
