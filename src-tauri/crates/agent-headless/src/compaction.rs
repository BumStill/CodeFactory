// SPDX-License-Identifier: Apache-2.0
//! History compaction: the char-budget digest and tool-history bookkeeping.
//!
//! Extracted verbatim from `main.rs` (keystone slice 4.8a) — a pure module
//! split with ZERO behaviour change, so the later seam adoption (4.8b) shows up
//! as a small readable diff instead of being buried in a 2775-line file.


use serde_json::{json, Value};

pub(crate) const MAX_CONTEXT_CHARS: usize = 40_000;

pub(crate) const MAX_TOOL_STREAM_CHARS: usize = 3_000;

#[derive(Debug, Clone)]
pub(crate) struct ToolHistoryEntry {
    pub(crate) command: String,
    pub(crate) return_code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) error: Option<String>,
}

impl ToolHistoryEntry {
    pub(crate) fn new(
        command: impl Into<String>,
        return_code: Option<i32>,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
        error: Option<String>,
    ) -> Self {
        Self {
            command: command.into(),
            return_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
            error,
        }
    }
}

pub(crate) fn message_content(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(content)) => content.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub(crate) fn tool_result_content(
    return_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    error: Option<&str>,
) -> String {
    json!({
        "return_code": return_code,
        "stdout": truncate_for_model(stdout, MAX_TOOL_STREAM_CHARS),
        "stderr": truncate_for_model(stderr, MAX_TOOL_STREAM_CHARS),
        "error": error,
    })
    .to_string()
}

pub(crate) fn compact_messages(messages: &mut Vec<Value>, history: &[ToolHistoryEntry], max_chars: usize) {
    if messages.len() <= 3
        || serde_json::to_string(messages)
            .map(|value| value.len() <= max_chars)
            .unwrap_or(true)
    {
        return;
    }

    let recent_start = messages
        .iter()
        .enumerate()
        .skip(2)
        .rev()
        .find(|(_, message)| {
            message.get("role").and_then(Value::as_str) == Some("assistant")
                && message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_some()
        })
        .map(|(index, _)| index)
        .unwrap_or_else(|| messages.len().saturating_sub(2));

    let summary = history_digest(history);
    let summary = truncate_for_model(&summary, max_chars.saturating_div(2).max(800));

    let system = messages[0].clone();
    let task = messages[1].clone();
    let recent = messages[recent_start..].to_vec();
    *messages = vec![system, task, json!({"role": "user", "content": summary})];
    messages.extend(recent);
}

pub(crate) fn truncate_for_model(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let head_limit = limit / 4;
    let tail_limit = limit.saturating_sub(head_limit);
    let head = value.chars().take(head_limit).collect::<String>();
    let tail = value
        .chars()
        .rev()
        .take(tail_limit)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!(
        "{head}\n[truncated middle; kept first {head_limit} and last {tail_limit} characters]\n{tail}"
    )
}

/// The compaction digest: the last 30 tool outcomes, oldest-first, one line
/// each. Extracted from `compact_messages` (keystone slice 4.8) so the shared
/// loop's `ContextCompactor` produces the byte-identical summary the sidecar's
/// own loop did.
pub(crate) fn history_digest(history: &[ToolHistoryEntry]) -> String {
    let mut summary = String::from("Compacted execution history (oldest details omitted):\n");
    for (index, entry) in history
        .iter()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .enumerate()
    {
        let command = entry.command.lines().next().unwrap_or("");
        let output = if let Some(error) = &entry.error {
            error.as_str()
        } else if !entry.stderr.trim().is_empty() {
            entry.stderr.trim()
        } else {
            entry.stdout.trim()
        };
        summary.push_str(&format!(
            "{}. rc={:?} command={} output={}\n",
            index + 1,
            entry.return_code,
            truncate_for_model(command, 200),
            truncate_for_model(output, 400),
        ));
    }
    summary
}
