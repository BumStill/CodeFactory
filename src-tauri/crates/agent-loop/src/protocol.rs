// SPDX-License-Identifier: Apache-2.0
//! Provider wire-protocol helpers (keystone slice 4.6b): pure transforms over the
//! crate's `ChatMessage`/`MessageContent` (and the Anthropic-shaped JSON array)
//! that both provider loops and the reactive retries need. No `Settings`, no DB,
//! no `AppHandle` — moved out of the bin so the shared loop can reach them.

use std::collections::HashSet;

use crate::types::{ChatMessage, MessageContent};

/// Placeholder that replaces an image part when the active model rejects vision
/// input. Visible to the model (so it knows an image existed) and stable for the
/// strip functions' idempotence checks.
pub const IMAGE_STRIPPED_PLACEHOLDER: &str = "[图片已省略:当前模型不支持图片输入]";

/// Does this provider error mean "the model can't accept image input"?
/// Deliberately narrow: capability wording only, never generic failures —
/// a false positive would silently drop the user's images on a transient
/// error, so unknown errors must stay unmatched and surface as-is.
pub fn is_vision_rejection(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    ["image", "vision", "multimodal"]
        .iter()
        .any(|needle| lower.contains(needle))
        && !lower.contains("rate limit")
}

/// Replace image parts in OpenAI-shaped messages with a text placeholder.
/// Returns how many were stripped (0 = nothing to do → do not retry again).
pub fn strip_image_parts(messages: &mut [ChatMessage]) -> usize {
    let mut stripped = 0;
    for message in messages.iter_mut() {
        if let MessageContent::Parts(parts) = &mut message.content {
            for part in parts.iter_mut() {
                if part.r#type == "image_url" {
                    part.r#type = "text".into();
                    part.text = Some(IMAGE_STRIPPED_PLACEHOLDER.to_string());
                    part.image_url = None;
                    stripped += 1;
                }
            }
        }
    }
    stripped
}

/// Same as [`strip_image_parts`] for the Anthropic-shaped JSON message array
/// (`type: "image"` blocks and any `image_url` compatibility parts).
pub fn strip_image_values(messages: &mut [serde_json::Value]) -> usize {
    let mut stripped = 0;
    for message in messages.iter_mut() {
        let Some(parts) = message.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        for part in parts.iter_mut() {
            let kind = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if kind == "image" || kind == "image_url" {
                *part = serde_json::json!({
                    "type": "text",
                    "text": IMAGE_STRIPPED_PLACEHOLDER,
                });
                stripped += 1;
            }
        }
    }
    stripped
}

/// Repair a compressed/replayed OpenAI history so it satisfies the strict
/// tool-call protocol: every `assistant` tool_call must be followed by a matching
/// `tool` result, tool_call ids are de-duplicated and non-empty, and any missing
/// result is backfilled with a synthetic placeholder. Ordering of the synthetic
/// insertion and the tool_call_id pairing is protocol-critical.
pub fn repair_openai_tool_protocol(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    fn synthetic_tool_message(tool_call_id: String) -> ChatMessage {
        ChatMessage {
            role: "tool".into(),
            content: MessageContent::Text(
                "Tool result unavailable in persisted history; continue from current workspace state."
                    .into(),
            ),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
            name: None,
            reasoning_content: None,
        }
    }

    fn append_missing_results(repaired: &mut Vec<ChatMessage>, pending: &mut Vec<String>) {
        repaired.extend(pending.drain(..).map(synthetic_tool_message));
    }

    let mut repaired = Vec::with_capacity(messages.len());
    let mut pending_tool_calls: Vec<String> = Vec::new();

    for mut message in messages {
        if message.role != "tool" && !pending_tool_calls.is_empty() {
            append_missing_results(&mut repaired, &mut pending_tool_calls);
        }

        if message.role == "tool" {
            let Some(tool_call_id) = message.tool_call_id.as_deref() else {
                continue;
            };
            let Some(index) = pending_tool_calls
                .iter()
                .position(|pending| pending == tool_call_id)
            else {
                continue;
            };
            pending_tool_calls.remove(index);
            repaired.push(message);
            continue;
        }

        if message.role == "assistant" {
            if let Some(tool_calls) = message.tool_calls.as_mut() {
                let mut seen_ids = HashSet::new();
                tool_calls.retain(|tool_call| {
                    !tool_call.id.trim().is_empty() && seen_ids.insert(tool_call.id.clone())
                });
                if tool_calls.is_empty() {
                    message.tool_calls = None;
                }
            }
            pending_tool_calls = message
                .tool_calls
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|tool_call| tool_call.id.clone())
                .collect();
        }
        repaired.push(message);
    }

    append_missing_results(&mut repaired, &mut pending_tool_calls);
    repaired
}
