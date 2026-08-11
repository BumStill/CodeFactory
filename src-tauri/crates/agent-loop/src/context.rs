// SPDX-License-Identifier: Apache-2.0
//! Context estimation + adaptive compression (keystone slice 4.6b).
//!
//! The `Settings`-free half of the desktop `agent::context` module: char→token
//! estimation, prompt-token totals, the context-overflow error detector, and the
//! two-pass adaptive compression pass. Pure over the crate's `ChatMessage`, so
//! the shared loop reaches them directly. The `Settings`-coupled window
//! resolution (`resolve_context_window`, `model_supports_vision`) stays in the
//! bin behind `ContextPolicy` and re-exports these under the `context::` path.

use crate::types::{ChatMessage, MessageContent, ToolDefinition};

/// Compression kicks in above this fraction of the window. 0.75 leaves
/// headroom for the system prompt, tool definitions, and the new user
/// turn we're about to add.
pub const COMPRESSION_TRIGGER: f32 = 0.75;

/// Tool results below this token estimate are left untouched. Compressing
/// short results saves nothing and hurts the model's ability to use them.
pub const MIN_ELIDE_TOKENS: u32 = 200;

/// A single tool result or assistant response must never monopolize the model
/// window. The character cap is deliberately conservative for CJK/code and is
/// further reduced for models with smaller context windows.
const MAX_SINGLE_MESSAGE_CHARS: usize = 64 * 1024;

/// Quick char→token estimate. Real BPE tokenization varies 1.0-1.5×
/// for English and up to 2.5× for code/CJK, so we use 3.0 chars/token
/// for ASCII-heavy text and 2.0 for CJK-heavy — err on the safe side.
pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    if chars == 0 {
        return 0;
    }
    let cjk_fraction = text.chars().filter(|ch| is_cjk(*ch)).count() as f32 / chars as f32;
    // Blend divisor: 3.0 for pure ASCII, 2.0 for pure CJK.
    let divisor = 3.0 - cjk_fraction; // 2.0 .. 3.0
    (chars as f32 / divisor).ceil() as u32
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}'       // CJK Unified
        | '\u{3400}'..='\u{4DBF}'     // CJK Extension A
        | '\u{20000}'..='\u{2A6DF}'   // CJK Extension B
        | '\u{F900}'..='\u{FAFF}'     // CJK Compatibility
        | '\u{2F800}'..='\u{2FA1F}'   // CJK Compatibility Supplement
        | '\u{3000}'..='\u{303F}'     // CJK Symbols
        | '\u{FF00}'..='\u{FFEF}'     // Halfwidth/Fullwidth
        | '\u{3040}'..='\u{309F}'     // Hiragana
        | '\u{30A0}'..='\u{30FF}'     // Katakana
        | '\u{AC00}'..='\u{D7AF}'     // Hangul
    )
}

/// Estimate the total prompt tokens for a message list, including a system
/// prompt and message/tool-call overhead. Use
/// [`estimate_prompt_tokens_with_tools`] when the active tool definitions are
/// available.
pub fn estimate_prompt_tokens(messages: &[ChatMessage], system_prompt: &str) -> u32 {
    estimate_prompt_tokens_with_tools(messages, system_prompt, &[])
}

/// Estimate the complete prompt envelope that is sent to a provider. Besides
/// message text and tool calls, this includes replayed reasoning and the exact
/// tool definitions active for this round. The estimate is intentionally
/// conservative: an underestimated budget causes a failed request followed by
/// an avoidable compression/retry cycle.
pub fn estimate_prompt_tokens_with_tools(
    messages: &[ChatMessage],
    system_prompt: &str,
    tools: &[ToolDefinition],
) -> u32 {
    // Rough overhead per message for role/separators/wrappers.
    const PER_MESSAGE_OVERHEAD: u32 = 4;
    // Tool envelopes carry type/function keys and provider-side separators in
    // addition to the serialized definition fields.
    const TOOL_ENVELOPE_OVERHEAD: u32 = 8;

    let mut total = estimate_tokens(system_prompt);
    for m in messages {
        total += PER_MESSAGE_OVERHEAD;
        total += estimate_tokens(&content_text(&m.content));
        if let Some(reasoning) = &m.reasoning_content {
            total += estimate_tokens(reasoning);
        }
        if let Some(tcs) = &m.tool_calls {
            for tc in tcs {
                total += estimate_tokens(&tc.function.name);
                total += estimate_tokens(&tc.function.arguments);
            }
        }
    }
    for tool in tools {
        total += TOOL_ENVELOPE_OVERHEAD;
        total += estimate_tokens(&tool.r#type);
        total += estimate_tokens(&tool.function.name);
        total += estimate_tokens(&tool.function.description);
        total += estimate_tokens(&tool.function.parameters.to_string());
    }
    total
}

fn content_text(c: &MessageContent) -> String {
    match c {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| p.text.clone())
            .collect::<Vec<_>>()
            .join(""),
    }
}

/// Result of one compression pass.
#[derive(Debug, Clone)]
pub struct CompressionResult {
    pub messages: Vec<ChatMessage>,
    /// True if anything was elided.
    pub compressed: bool,
    /// Number of messages that got elided or removed.
    pub elided_count: usize,
    /// Approximate tokens reclaimed.
    pub tokens_freed: u32,
}

/// Does this provider error mean "the prompt is over the context window"?
/// Narrow on purpose (capacity wording only) — a false positive would
/// silently degrade history on unrelated failures.
pub fn is_context_overflow(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "context window",
        "context length",
        "context_length",
        "maximum context",
        "prompt is too long",
        "input exceeds",
        "too many tokens",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Does this provider error mean "transiently overloaded, retry with backoff"?
/// (Anthropic 529 "Overloaded", rate limits, 503.) Drives the loop's overload
/// backoff arm (keystone slice 4.7).
pub fn is_provider_overloaded(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "overloaded",
        "try again later",
        "rate limit",
        "429",
        "503",
        "529",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// Bound oversized messages (tool results AND assistant prose) anywhere in
/// the conversation, then elide moderately large messages from the older
/// half when the prompt estimate exceeds `limit * COMPRESSION_TRIGGER`.
/// If the estimate still exceeds the limit, drop the oldest messages (user +
/// assistant pairs with their tool results) until we're within budget.
///
/// Why assistant messages too: in long conversations the model's own
/// markdown output can consume more tokens than tool results — leaving
/// those untouched is how we shipped "context window exceeded" errors.
///
/// Why recent messages too: one minified asset produced a 2.30 MB grep result
/// in production. With only two user turns left, the old "preserve the recent
/// half" rule could not reclaim any more space, so the overflow retry failed
/// again. Head/tail compaction retains the newest evidence and tool protocol
/// envelope while keeping the provider payload bounded. Raw SQLite history is
/// not changed.
pub fn compress_if_needed(
    messages: Vec<ChatMessage>,
    system_prompt: &str,
    limit: u32,
) -> CompressionResult {
    compress_if_needed_with_tools(messages, system_prompt, limit, &[])
}

/// Compress a prompt using the actual tool definitions active for this round.
/// The legacy [`compress_if_needed`] wrapper remains for callers that do not
/// send tools (and for tests that exercise message-only compression).
pub fn compress_if_needed_with_tools(
    messages: Vec<ChatMessage>,
    system_prompt: &str,
    limit: u32,
    tools: &[ToolDefinition],
) -> CompressionResult {
    let trigger = (limit as f32 * COMPRESSION_TRIGGER) as u32;
    let estimate = estimate_prompt_tokens_with_tools(&messages, system_prompt, tools);

    if estimate <= trigger {
        return CompressionResult {
            messages,
            compressed: false,
            elided_count: 0,
            tokens_freed: 0,
        };
    }

    let half = messages.len() / 2;
    let mut elided_count = 0;
    let mut tokens_freed: u32 = 0;
    let dynamic_message_cap = ((limit as usize / 4) * 2).clamp(400, MAX_SINGLE_MESSAGE_CHARS);

    // Pass 1: hard-bound individually oversized messages anywhere, then
    // elide moderately large messages in the older half. Tool results and
    // assistant prose are both eligible; user input remains verbatim.
    let messages: Vec<ChatMessage> = messages
        .into_iter()
        .enumerate()
        .map(|(i, mut m)| {
            let model_generated = m.role == "tool" || m.role == "assistant";
            if model_generated {
                let original = content_text(&m.content);
                let est = estimate_tokens(&original);
                let original_chars = original.chars().count();
                if original_chars > dynamic_message_cap {
                    let replacement = compact_head_tail(
                        &original,
                        dynamic_message_cap,
                        if m.role == "tool" {
                            "tool result"
                        } else {
                            "assistant response"
                        },
                        est,
                    );
                    let new_est = estimate_tokens(&replacement);
                    tokens_freed =
                        tokens_freed.saturating_add(est.saturating_sub(new_est));
                    elided_count += 1;
                    m.content = MessageContent::Text(replacement);
                } else if i < half && est >= MIN_ELIDE_TOKENS {
                    let bytes = original.len();
                    let preview: String = original.chars().take(120).collect();
                    let role_label = if m.role == "tool" {
                        "tool result"
                    } else {
                        "assistant response"
                    };
                    let replacement = format!(
                        "[elided {role_label} to fit context window — {bytes} bytes / ~{est} tokens]\n\nPreview:\n{}{}",
                        preview,
                        if original.chars().count() > 120 { "…" } else { "" },
                    );
                    let new_est = estimate_tokens(&replacement);
                    tokens_freed = tokens_freed.saturating_add(est.saturating_sub(new_est));
                    elided_count += 1;
                    m.content = MessageContent::Text(replacement);
                }
            }
            m
        })
        .collect();

    // Pass 2: if post-elision estimate still exceeds the hard limit, drop
    // the oldest messages from the front until we're under budget. Preserve
    // at least MIN_KEEP_PAIRS user messages as conversation skeleton.
    const MIN_KEEP_PAIRS: usize = 2;
    let mut trimmed = messages;
    loop {
        let est = estimate_prompt_tokens_with_tools(&trimmed, system_prompt, tools);
        if est <= limit {
            break;
        }
        let user_count = trimmed.iter().filter(|m| m.role == "user").count();
        if user_count <= MIN_KEEP_PAIRS {
            // At minimum skeleton — stop here. Dropping more would lose
            // the thread of the conversation.
            break;
        }
        // Drop the oldest user message and everything that belongs to that
        // turn (following assistant/tool messages until the next user).
        let first_user = trimmed.iter().position(|m| m.role == "user").unwrap();
        let next_user = trimmed
            .iter()
            .skip(first_user + 1)
            .position(|m| m.role == "user")
            .map(|p| first_user + 1 + p)
            .unwrap_or(trimmed.len());
        for msg in trimmed.drain(first_user..next_user) {
            tokens_freed =
                tokens_freed.saturating_add(estimate_tokens(&content_text(&msg.content)));
            elided_count += 1;
        }
    }

    CompressionResult {
        messages: trimmed,
        compressed: elided_count > 0,
        elided_count,
        tokens_freed,
    }
}

fn compact_head_tail(
    original: &str,
    retained_chars: usize,
    role_label: &str,
    estimated_tokens: u32,
) -> String {
    let total_chars = original.chars().count();
    let head_chars = retained_chars / 2;
    let tail_chars = retained_chars.saturating_sub(head_chars);
    let head: String = original.chars().take(head_chars).collect();
    let mut tail: Vec<char> = original.chars().rev().take(tail_chars).collect();
    tail.reverse();
    let tail: String = tail.into_iter().collect();
    let omitted_chars = total_chars.saturating_sub(retained_chars);

    format!(
        "[elided middle of {role_label} to fit context window — kept {retained_chars} of {total_chars} chars, omitted {omitted_chars}, original ~{estimated_tokens} tokens]\n\nHead:\n{head}\n\n[… elided middle …]\n\nTail:\n{tail}"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        compress_if_needed, estimate_prompt_tokens, estimate_prompt_tokens_with_tools,
        estimate_tokens,
    };
    use crate::types::{ChatMessage, FunctionDefinition, MessageContent, ToolDefinition};

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Text(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    fn large_assistant() -> ChatMessage {
        msg("assistant", &"A".repeat(40000))
    }

    fn content_of(msg: &ChatMessage) -> String {
        super::content_text(&msg.content)
    }

    #[test]
    fn prompt_estimate_includes_tool_schema_and_reasoning_replay() {
        let mut assistant = msg("assistant", "short reply");
        assistant.reasoning_content = Some("R".repeat(900));
        let messages = vec![msg("user", "inspect the project"), assistant];
        let tools = vec![ToolDefinition {
            r#type: "function".into(),
            function: FunctionDefinition {
                name: "read_file".into(),
                description: "D".repeat(900),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }),
            },
        }];

        let messages_only = estimate_prompt_tokens(&messages, "");
        let complete = estimate_prompt_tokens_with_tools(&messages, "", &tools);
        let reasoning = estimate_tokens(&"R".repeat(900));

        assert!(
            complete >= messages_only + reasoning,
            "reasoning replay must count toward the prompt estimate"
        );
        assert!(
            complete > messages_only + reasoning,
            "tool schema must add its own prompt cost"
        );
    }

    #[test]
    fn compression_accounts_for_active_tool_schema() {
        let messages = vec![
            msg("user", "old task"),
            msg("assistant", &"A".repeat(1_200)),
            msg("user", "inspect"),
            msg("assistant", "ready"),
        ];
        let tools = vec![ToolDefinition {
            r#type: "function".into(),
            function: FunctionDefinition {
                name: "large_tool".into(),
                description: "D".repeat(2_000),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"payload": {"type": "string", "description": "P".repeat(2_000)}}
                }),
            },
        }];
        let result = super::compress_if_needed_with_tools(messages, "", 1_000, &tools);
        assert!(
            result.compressed,
            "tool schema must participate in the compression trigger"
        );
    }

    #[test]
    fn compresses_oversized_assistant_content_in_the_older_half() {
        let messages = vec![
            msg("user", "hi"),
            large_assistant(), // index 1, older half, ~13.3K tokens
            msg("user", "continue"),
            msg("assistant", "ok"), // index 3, recent half — untouched
        ];
        // Use a small 10K window so 13.3K triggers compression at 75% = 7.5K.
        let result = compress_if_needed(messages, "", 10_000);
        assert!(
            result.compressed,
            "large assistant content should be elided"
        );
        let body = content_of(&result.messages[1]);
        assert!(
            body.contains("assistant response") && body.contains("elided"),
            "assistant body should be replaced with elision marker, got: {body:.100}"
        );
        assert_eq!(
            content_of(&result.messages[3]),
            "ok",
            "recent half assistant must stay verbatim"
        );
    }

    #[test]
    fn compresses_old_tool_results_and_hard_bounds_oversized_recent_results() {
        let messages = vec![
            msg("user", "read the file"),
            msg("tool", &"T".repeat(80_000)), // index 1, old half, ~26.6K tokens
            msg("user", "now edit it"),
            msg("tool", &"E".repeat(80_000)), // index 3, new half — hard bounded
        ];
        // A small window reduces the per-message cap to preserve total budget.
        let result = compress_if_needed(messages, "", 10_000);
        assert!(result.compressed);
        assert!(content_of(&result.messages[1]).contains("elided"));
        let recent = content_of(&result.messages[3]);
        assert!(recent.contains("Head:\nEEE"));
        assert!(recent.contains("Tail:\nEEE"));
        assert!(recent.len() < 10_000);
    }

    #[test]
    fn production_shape_recent_tool_result_is_bounded_without_dropping_two_user_turns() {
        let huge_tool_result = format!(
            "PRODUCTION_HEAD\n{}\nPRODUCTION_TAIL",
            "minified-dist-asset;".repeat(121_000),
        );
        assert!(
            huge_tool_result.len() >= 2_298_000,
            "fixture must remain comparable to the 2.30 MB production result",
        );

        let mut recent_tool_result = msg("tool", &huge_tool_result);
        recent_tool_result.tool_call_id = Some("call_production_grep".into());
        let messages = vec![
            msg("user", "inspect the repository"),
            msg("assistant", "I will search for the implementation."),
            msg("user", "continue"),
            recent_tool_result, // recent half: the production failure mode
        ];
        let limit = 272_000_u32;
        let result = compress_if_needed(messages, "", limit);

        assert!(
            result.compressed,
            "recent oversized tool output must be compacted"
        );
        assert_eq!(
            result.messages.iter().filter(|m| m.role == "user").count(),
            2,
            "the two recent user turns are the conversation skeleton",
        );
        let body = content_of(result.messages.last().expect("recent tool result"));
        assert!(
            body.contains("PRODUCTION_HEAD"),
            "head context must be retained"
        );
        assert!(
            body.contains("PRODUCTION_TAIL"),
            "tail context must be retained"
        );
        assert!(
            body.contains("elided") || body.contains("truncated"),
            "compaction must be explicit to the model",
        );
        assert!(
            estimate_prompt_tokens(&result.messages, "") <= limit,
            "the replay sent to the provider must fit the configured model window",
        );
        assert_eq!(
            result
                .messages
                .last()
                .and_then(|m| m.tool_call_id.as_deref()),
            Some("call_production_grep"),
            "content compaction must preserve the provider tool protocol envelope",
        );
    }

    #[test]
    fn after_compression_estimate_is_strictly_below_limit() {
        // A pathological case: the assistant wrote huge prose every turn, tool
        // results are moderate. Single-pass elision of tool results barely
        // dents total — we need a phase-2 hard trim of oldest assistant/user
        // messages to stay under the limit.
        let messages: Vec<ChatMessage> = (0..20)
            .flat_map(|i| {
                let user = msg("user", &format!("q{}", i));
                let asst = msg("assistant", &"AB".repeat(20000));
                let tool = msg("tool", &"T".repeat(2000));
                vec![user, asst, tool]
            })
            .collect();
        let limit = 128_000_u32;
        let result = compress_if_needed(messages, "", limit);

        assert!(
            result.compressed,
            "should have elided at least some tool results"
        );
        let post_estimate = estimate_prompt_tokens(&result.messages, "");
        assert!(
            post_estimate <= limit,
            "after compression, estimated tokens {} must be ≤ limit {}",
            post_estimate,
            limit,
        );
    }

    #[test]
    fn cjk_text_yields_higher_token_estimate_than_english_for_same_char_count() {
        let english = "hello world this is a test message".repeat(100);
        let chinese = "这是中文".repeat(850); // 4 chars × 850 = 3400 = same as English
        assert_eq!(
            english.chars().count(),
            chinese.chars().count(),
            "char counts equal"
        );
        let eng_est = estimate_tokens(&english);
        let cn_est = estimate_tokens(&chinese);
        assert!(
            cn_est >= eng_est,
            "CJK estimate {cn_est} ≥ English {eng_est} for same char count"
        );
    }

    #[test]
    fn context_overflow_detector_matches_capacity_errors_only() {
        for err in [
            "ChatGPT 后端返回错误:Your input exceeds the context window of this model.",
            "400 This model's maximum context length is 65536 tokens",
            "context_length_exceeded",
            "prompt is too long: 210000 tokens > 200000 maximum",
        ] {
            assert!(super::is_context_overflow(err), "{err}");
        }
        for err in [
            "429 rate limit exceeded",
            "This model does not support image input",
            "500 Internal Server Error",
        ] {
            assert!(!super::is_context_overflow(err), "{err}");
        }
    }
}
