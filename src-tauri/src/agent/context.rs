// SPDX-License-Identifier: Apache-2.0
//! Context window management.
//!
//! Three responsibilities:
//!
//! 1. **Resolve the model's context window** — different models have wildly
//!    different limits (DeepSeek-Chat 64K, GPT-4o 128K, Claude 200K, Gemini
//!    up to 2M). We pull from `Endpoint.custom_models[].context_length`
//!    first (user override), then a cached `OpenRouter /models` lookup,
//!    then a conservative 16K fallback.
//!
//! 2. **Estimate the prompt token count** from the message list. We do a
//!    fast char-count approximation (≈ 1 token per 4 chars) without
//!    pulling in a tokenizer dep. Good enough (±20%) to trigger
//!    compression with a safety margin.
//!
//! 3. **Adaptive compression** — when the estimate exceeds 75% of the
//!    window, walk the message list and replace large tool results from
//!    the older half with `[elided: N bytes]` markers. Tool outputs are
//!    where the bulk of context typically lives (file reads, grep dumps),
//!    so this single pass usually reclaims enough room without losing
//!    the conversation thread itself.
//!
//! Returns metadata so the caller can emit a `context_compressed` event
//! and the UI can show the user what happened.

use crate::config::Settings;
use crate::openrouter::types::{ChatMessage, MessageContent};

/// Conservative window when nothing is known about the model.
/// 128K is the modern baseline (GPT-4o, Claude, Gemini, DeepSeek-v4-pro all
/// at or above this). Going lower triggers spurious compression and shows
/// "100%" too early — see the user report where a 60K prompt against the
/// old 16K fallback showed 375 % usage and triggered phantom compression.
pub const FALLBACK_CONTEXT_LENGTH: u32 = 128_000;

/// Compression kicks in above this fraction of the window. 0.75 leaves
/// headroom for the system prompt, tool definitions, and the new user
/// turn we're about to add.
pub const COMPRESSION_TRIGGER: f32 = 0.75;

/// Tool results below this token estimate are left untouched. Compressing
/// short results saves nothing and hurts the model's ability to use them.
pub const MIN_ELIDE_TOKENS: u32 = 200;

/// Resolve the active context window for `(endpoint, model_id)`.
///
/// Order: user-provided custom_models[].context_length > cached /models
/// entry > pattern-match against well-known model families > fallback.
pub fn resolve_context_length(
    settings: &Settings,
    endpoint_name: &str,
    model_id: &str,
    remote_models: Option<&[crate::openrouter::types::ModelInfo]>,
) -> u32 {
    // 1. User-defined custom model with explicit context_length wins
    if let Some(ep) = settings.endpoints.get(endpoint_name) {
        for cm in &ep.custom_models {
            if cm.id == model_id {
                if let Some(n) = cm.context_length {
                    if n > 0 {
                        return n;
                    }
                }
                break;
            }
        }
    }

    // 2. Remote /models entry if available
    if let Some(list) = remote_models {
        for m in list {
            if m.id == model_id && m.context_length > 0 {
                return m.context_length;
            }
        }
    }

    // 3. Pattern-match against known model families. Direct providers don't
    //    expose /models or return context_length, so without this the UI bar
    //    shows nonsense like "60K / 16K = 375 %" for a real 128K-context call.
    if let Some(n) = guess_context_from_name(model_id) {
        return n;
    }

    FALLBACK_CONTEXT_LENGTH
}

/// Best-effort context-length lookup from a model id string. Tuned for the
/// providers our users actually hit. Update as new families ship.
fn guess_context_from_name(model_id: &str) -> Option<u32> {
    let id = model_id.to_lowercase();
    let id = id.split('/').last().unwrap_or(&id); // strip vendor prefix if any

    // Anthropic Claude family — 200K across the board for current models
    if id.contains("claude") {
        return Some(200_000);
    }

    // Google Gemini — 1M+ for Pro, 1M for Flash
    if id.contains("gemini") {
        if id.contains("pro") { return Some(2_000_000); }
        return Some(1_000_000);
    }

    // DeepSeek family
    if id.starts_with("deepseek") {
        if id.contains("v4") { return Some(131_072); }
        if id.contains("v3") || id.contains("chat") || id.contains("reasoner") {
            return Some(65_536);
        }
        return Some(65_536);
    }

    // OpenAI GPT-5 / Codex family (incl. ChatGPT-subscription models like
    // gpt-5.5, gpt-5.3-codex, gpt-5.1-codex-mini) — 272K input window, per
    // codex's published model metadata. Must come before the gpt-4 branch.
    if id.starts_with("gpt-5") || id.contains("codex") {
        return Some(272_000);
    }

    // OpenAI family
    if id.starts_with("gpt-4") || id.contains("gpt-4o") || id.contains("o1") || id.contains("o3") {
        return Some(128_000);
    }
    if id.starts_with("gpt-3.5") {
        return Some(16_385);
    }

    // Qwen / Yi / Llama large-context — common Chinese-vendor families
    if id.contains("qwen") || id.contains("yi-") {
        return Some(128_000);
    }
    if id.contains("llama-3") || id.contains("llama3") {
        return Some(128_000);
    }

    None
}

/// Quick char→token estimate. Real BPE tokenization varies 1.0-1.5×
/// for English and up to 2.5× for code/CJK, so we use 3.5 chars/token
/// to err on the safe side (overestimate slightly → compress sooner).
pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count() as f32;
    (chars / 3.5).ceil() as u32
}

/// Estimate the total prompt tokens for a message list, including a system
/// prompt and the tool schema overhead (rough constant).
pub fn estimate_prompt_tokens(messages: &[ChatMessage], system_prompt: &str) -> u32 {
    // Rough overhead per message for role/separators/wrappers
    const PER_MESSAGE_OVERHEAD: u32 = 4;

    let mut total = estimate_tokens(system_prompt);
    for m in messages {
        total += PER_MESSAGE_OVERHEAD;
        total += estimate_tokens(&content_text(&m.content));
        if let Some(tcs) = &m.tool_calls {
            for tc in tcs {
                total += estimate_tokens(&tc.function.name);
                total += estimate_tokens(&tc.function.arguments);
            }
        }
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
    /// Number of tool-result messages that got elided.
    pub elided_count: usize,
    /// Approximate tokens reclaimed.
    pub tokens_freed: u32,
}

/// Elide oversized tool results from the older half of the conversation
/// when the prompt estimate exceeds `limit * COMPRESSION_TRIGGER`.
///
/// Why only the older half: recent tool results are usually still relevant
/// to what the model is doing right now. Older results — especially file
/// reads and grep dumps from earlier exploration — rarely need to stick
/// around verbatim.
///
/// Why not summarise via LLM yet: simpler, deterministic, no extra latency
/// or cost. A second-stage LLM summary pass can be added later when
/// elision alone isn't enough.
pub fn compress_if_needed(
    messages: Vec<ChatMessage>,
    system_prompt: &str,
    limit: u32,
) -> CompressionResult {
    let trigger = (limit as f32 * COMPRESSION_TRIGGER) as u32;
    let estimate = estimate_prompt_tokens(&messages, system_prompt);

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

    let messages: Vec<ChatMessage> = messages
        .into_iter()
        .enumerate()
        .map(|(i, mut m)| {
            // Tool result messages have role == "tool" — those are the biggest
            // wins (file reads, grep output). Only touch the older half so the
            // model still has recent results verbatim.
            if i < half && m.role == "tool" {
                let original = content_text(&m.content);
                let est = estimate_tokens(&original);
                if est >= MIN_ELIDE_TOKENS {
                    let bytes = original.len();
                    let preview: String = original.chars().take(120).collect();
                    let replacement = format!(
                        "[elided to fit context window — {} bytes / ~{} tokens]\n\nPreview:\n{}{}",
                        bytes,
                        est,
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

    CompressionResult {
        messages,
        compressed: elided_count > 0,
        elided_count,
        tokens_freed,
    }
}

#[cfg(test)]
mod tests {
    use super::guess_context_from_name as guess;

    #[test]
    fn gpt5_and_codex_families_resolve_to_real_window() {
        // Regression: the ChatGPT-subscription models (gpt-5.x / *-codex) were
        // falling through to the 128K fallback. Codex publishes 272K for the
        // gpt-5 / codex family — that's what the context meter should show.
        assert_eq!(guess("gpt-5.5"), Some(272_000));
        assert_eq!(guess("gpt-5.3-codex"), Some(272_000));
        assert_eq!(guess("gpt-5.1-codex-mini"), Some(272_000));
        assert_eq!(guess("gpt-5"), Some(272_000));
        assert_eq!(guess("gpt-5-codex"), Some(272_000));
        assert_eq!(guess("codex-mini-latest"), Some(272_000));
    }

    #[test]
    fn known_families_unchanged() {
        assert_eq!(guess("claude-3-5-sonnet"), Some(200_000));
        assert_eq!(guess("gemini-2.5-pro"), Some(2_000_000));
        assert_eq!(guess("deepseek-v4-pro"), Some(131_072));
        assert_eq!(guess("gpt-4o"), Some(128_000));
        assert_eq!(guess("gpt-3.5-turbo"), Some(16_385));
        assert_eq!(guess("totally-unknown-model"), None);
    }
}
