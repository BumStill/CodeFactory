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
        .any(|needle| starts_a_word(&lower, needle))
        && !lower.contains("rate limit")
}

/// `contains`, but the needle must begin a word.
///
/// A bare `contains` made "a new Objective revision" match "vision", so a
/// provider-ownership failure was reported to the user as "this model rejected
/// your images, switch to one that supports them" — pointing at something that
/// was never wrong, on a session with no images, while the Fatal wrapper burned
/// a recovery attempt each time. `revision` is a core word in this codebase
/// (objective_revision, admission_revision), so that collision is routine.
///
/// Only the LEADING boundary is enforced. Trailing inflections carry the same
/// capability meaning ("images are not supported", "image_url"), so requiring a
/// trailing boundary too would trade one silent misread for another.
fn starts_a_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let at = from + offset;
        let preceded_by_word_char = haystack[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric());
        if !preceded_by_word_char {
            return true;
        }
        // `needle` is ASCII, so `at` lands on a char boundary and `at + 1` is
        // a valid resume point.
        from = at + 1;
    }
    false
}

/// Count image parts without mutating history. Capability gating must preserve
/// the user's original prompt so switching to a vision model can retry it.
pub fn image_part_count(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::Parts(parts) => Some(parts),
            MessageContent::Text(_) => None,
        })
        .flatten()
        .filter(|part| part.r#type == "image_url" && part.image_url.is_some())
        .count()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-14, production: a provider-ownership failure reached the user as
    /// "当前模型拒绝了图片输入……请切换到支持图片的模型后重试", because the
    /// underlying text ended in "before a new Objective revision" and
    /// `revision` ends with `vision`. The session had no images at all, so the
    /// advice pointed at something that was never wrong — and the wrapper is
    /// Fatal, so each occurrence also burned a recovery attempt. `revision` is
    /// a core word here (objective_revision, admission_revision), so this
    /// collision is routine, not exotic.
    #[test]
    fn a_revision_error_is_not_a_vision_rejection() {
        assert!(!is_vision_rejection(
            "open provider episode: prior provider episode is not proven replay-safe; \
             observe/reconcile before a new Objective revision"
        ));
        assert!(!is_vision_rejection(
            "objective revision conflict: expected 3, actual 4"
        ));
        assert!(!is_vision_rejection("admission_revision mismatch"));
    }

    /// The guard above must not cost us the real capability signal: a genuine
    /// vision rejection still has to strip images and prompt a model switch.
    #[test]
    fn real_capability_wording_still_matches() {
        assert!(is_vision_rejection("This model does not support image input"));
        assert!(is_vision_rejection("images are not supported by this model"));
        assert!(is_vision_rejection("Invalid content type: image_url"));
        assert!(is_vision_rejection("vision input is not supported"));
        assert!(is_vision_rejection("multimodal input rejected"));
    }

    #[test]
    fn a_rate_limited_image_error_is_not_a_capability_rejection() {
        assert!(!is_vision_rejection("image rate limit exceeded"));
    }
}
