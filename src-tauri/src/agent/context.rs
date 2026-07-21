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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedContextWindow {
    /// Normal effective input budget for ordinary turns.
    pub default_limit: u32,
    /// Largest effective input budget explicitly allowed by this route.
    pub max_limit: u32,
}

impl ResolvedContextWindow {
    /// Expand only when the estimated prompt would otherwise trigger
    /// compression. This keeps normal turns on the provider's default budget
    /// and pays the long-context cost only when the conversation needs it.
    pub fn select_limit(self, estimated_prompt_tokens: u32) -> u32 {
        let expansion_threshold = (self.default_limit as f32 * COMPRESSION_TRIGGER) as u32;
        if self.max_limit > self.default_limit && estimated_prompt_tokens > expansion_threshold {
            self.max_limit
        } else {
            self.default_limit
        }
    }
}

fn apply_effective_percent(limit: u32, percent: Option<u8>) -> u32 {
    let percent = percent.unwrap_or(100).clamp(1, 100) as u64;
    (((limit as u64) * percent) / 100).max(1) as u32
}

/// Resolve both the default and maximum context budgets for
/// `(endpoint, model_id)`.
///
/// Order: user-provided custom model metadata > cached `/models` entry >
/// model-family fallback. Route metadata wins over public model-card capacity.
pub fn resolve_context_window(
    settings: &Settings,
    endpoint_name: &str,
    model_id: &str,
    remote_models: Option<&[crate::openrouter::types::ModelInfo]>,
) -> ResolvedContextWindow {
    // 1. User-defined or catalog-backed model metadata wins.
    if let Some(ep) = settings.endpoints.get(endpoint_name) {
        for cm in &ep.custom_models {
            if cm.id == model_id {
                if let Some(n) = cm.context_length {
                    if n > 0 {
                        let max = cm
                            .max_context_length
                            .filter(|value| *value > 0)
                            .unwrap_or(n);
                        return ResolvedContextWindow {
                            default_limit: apply_effective_percent(
                                n,
                                cm.effective_context_window_percent,
                            ),
                            max_limit: apply_effective_percent(
                                max.max(n),
                                cm.effective_context_window_percent,
                            ),
                        };
                    }
                }
                if let Some(max) = cm.max_context_length.filter(|value| *value > 0) {
                    let effective =
                        apply_effective_percent(max, cm.effective_context_window_percent);
                    return ResolvedContextWindow {
                        default_limit: effective,
                        max_limit: effective,
                    };
                }
                break;
            }
        }
    }

    // 2. Remote /models entry if available
    if let Some(list) = remote_models {
        for m in list {
            if m.id == model_id && m.context_length > 0 {
                return ResolvedContextWindow {
                    default_limit: m.context_length,
                    max_limit: m.context_length,
                };
            }
        }
    }

    // 3. Pattern-match against known model families. Direct providers don't
    //    expose /models or return context_length, so without this the UI bar
    //    shows nonsense like "60K / 16K = 375 %" for a real 128K-context call.
    if let Some(n) = guess_context_from_name(model_id) {
        return ResolvedContextWindow {
            default_limit: n,
            max_limit: n,
        };
    }

    ResolvedContextWindow {
        default_limit: FALLBACK_CONTEXT_LENGTH,
        max_limit: FALLBACK_CONTEXT_LENGTH,
    }
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
        if id.contains("pro") {
            return Some(2_000_000);
        }
        return Some(1_000_000);
    }

    // DeepSeek family
    if id.starts_with("deepseek") {
        if id.contains("v4") {
            return Some(131_072);
        }
        if id.contains("v3") || id.contains("chat") || id.contains("reasoner") {
            return Some(65_536);
        }
        return Some(65_536);
    }

    // Current OpenAI API models expose a 1.05M context window. A provider or
    // ChatGPT subscription catalog can still advertise a smaller route-specific
    // cap; explicit endpoint metadata wins before this name-based fallback.
    if id == "gpt-5.4"
        || id.starts_with("gpt-5.4-")
        || id == "gpt-5.5"
        || id.starts_with("gpt-5.5-")
        || id == "gpt-5.6"
        || id.starts_with("gpt-5.6-")
    {
        return Some(1_050_000);
    }

    // Older OpenAI GPT-5 / Codex subscription family — 272K input window, per
    // Codex model metadata. Match the gpt-5 prefix only: a bare "codex"
    // substring would wrongly catch legacy small-window completion models.
    if id.starts_with("gpt-5") {
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
/// for English and up to 2.5× for code/CJK, so we use 3.0 chars/token
/// for ASCII-heavy text and 2.0 for CJK-heavy — err on the safe side.
pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    if chars == 0 {
        return 0;
    }
    let cjk_fraction = text
        .chars()
        .filter(|ch| is_cjk(*ch))
        .count() as f32
        / chars as f32;
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

/// Elide oversized messages (tool results AND assistant prose) from the
/// older half of the conversation when the prompt estimate exceeds
/// `limit * COMPRESSION_TRIGGER`. After pass 1, if the estimate still
/// exceeds the limit, drop the oldest messages (user + assistant pairs
/// with their tool results) until we're within budget.
///
/// Why assistant messages too: in long conversations the model's own
/// markdown output can consume more tokens than tool results — leaving
/// those untouched is how we shipped "context window exceeded" errors.
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

    // Pass 1: elide large messages in the older half — tool results AND
    // assistant prose both get compressed.
    let messages: Vec<ChatMessage> = messages
        .into_iter()
        .enumerate()
        .map(|(i, mut m)| {
            let elidible = i < half && (m.role == "tool" || m.role == "assistant");
            if elidible {
                let original = content_text(&m.content);
                let est = estimate_tokens(&original);
                if est >= MIN_ELIDE_TOKENS {
                    let bytes = original.len();
                    let preview: String = original.chars().take(120).collect();
                    let role_label = if m.role == "tool" { "tool result" }
                                     else { "assistant response" };
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
        let est = estimate_prompt_tokens(&trimmed, system_prompt);
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
            tokens_freed = tokens_freed.saturating_add(
                estimate_tokens(&content_text(&msg.content)));
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

#[cfg(test)]
mod tests {
    use super::{
        compress_if_needed, estimate_prompt_tokens, estimate_tokens, guess_context_from_name as guess,
        resolve_context_window,
    };
    use crate::config::settings::{ApiStyle, CustomModel, Endpoint, Settings};
    use crate::openrouter::types::{ChatMessage, MessageContent};

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

    #[test]
    fn compresses_oversized_assistant_content_in_the_older_half() {
        let messages = vec![
            msg("user", "hi"),
            large_assistant(),       // index 1, older half, ~13.3K tokens
            msg("user", "continue"),
            msg("assistant", "ok"),  // index 3, recent half — untouched
        ];
        // Use a small 10K window so 13.3K triggers compression at 75% = 7.5K.
        let result = compress_if_needed(messages, "", 10_000);
        assert!(result.compressed, "large assistant content should be elided");
        let body = content_of(&result.messages[1]);
        assert!(body.starts_with("[elided assistant response"), "assistant body should be replaced with elision marker, got: {body:.100}");
        assert_eq!(content_of(&result.messages[3]), "ok", "recent half assistant must stay verbatim");
    }

    #[test]
    fn compresses_tool_results_from_older_half_only() {
        let messages = vec![
            msg("user", "read the file"),
            msg("tool", &"T".repeat(80_000)), // index 1, old half, ~26.6K tokens
            msg("user", "now edit it"),
            msg("tool", &"E".repeat(80_000)), // index 3, new half — untouched
        ];
        // Small window so the old tool triggers compression.
        let result = compress_if_needed(messages, "", 10_000);
        assert!(result.compressed);
        assert!(content_of(&result.messages[1]).contains("elided"));
        assert_eq!(content_of(&result.messages[3]), "E".repeat(80_000));
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

        assert!(result.compressed, "should have elided at least some tool results");
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
        assert_eq!(english.chars().count(), chinese.chars().count(), "char counts equal");
        let eng_est = estimate_tokens(&english);
        let cn_est = estimate_tokens(&chinese);
        assert!(cn_est >= eng_est, "CJK estimate {cn_est} ≥ English {eng_est} for same char count");
    }

    fn content_of(msg: &ChatMessage) -> String {
        super::content_text(&msg.content)
    }

    fn settings_with_model(model: CustomModel) -> Settings {
        let mut settings = Settings::default();
        settings.endpoints.insert(
            "chatgpt".into(),
            Endpoint {
                base_url: "https://chatgpt.com/backend-api/codex".into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                custom_models: vec![model],
                active_model: Some("gpt-5.6-sol".into()),
            },
        );
        settings
    }

    #[test]
    fn gpt5_and_codex_families_resolve_to_real_window() {
        // Regression: the ChatGPT-subscription models (gpt-5.x / *-codex) were
        // falling through to the 128K fallback. Codex publishes 272K for the
        // gpt-5 / codex family — that's what the context meter should show.
        assert_eq!(guess("gpt-5.5"), Some(1_050_000));
        assert_eq!(guess("gpt-5.3-codex"), Some(272_000));
        assert_eq!(guess("gpt-5.1-codex-mini"), Some(272_000));
        assert_eq!(guess("gpt-5"), Some(272_000));
        assert_eq!(guess("gpt-5-codex"), Some(272_000));
        // Narrowed to the gpt-5 prefix: a bare legacy "codex-*" id is NOT
        // assumed to be 272K (those were small-window completion models).
        assert_eq!(guess("codex-mini-latest"), None);
    }

    #[test]
    fn current_gpt55_and_gpt56_api_models_use_the_official_one_million_window() {
        assert_eq!(guess("gpt-5.5"), Some(1_050_000));
        assert_eq!(guess("gpt-5.6-sol"), Some(1_050_000));
        assert_eq!(guess("gpt-5.6-terra"), Some(1_050_000));
        assert_eq!(guess("gpt-5.6-luna"), Some(1_050_000));
    }

    #[test]
    fn catalog_context_adapts_from_default_to_advertised_maximum() {
        let settings = settings_with_model(CustomModel {
            id: "gpt-5.6-sol".into(),
            name: None,
            context_length: Some(272_000),
            max_context_length: Some(1_050_000),
            effective_context_window_percent: Some(95),
            default_reasoning_effort: None,
            supported_reasoning_efforts: None,
        });

        let window = resolve_context_window(&settings, "chatgpt", "gpt-5.6-sol", None);
        assert_eq!(window.default_limit, 258_400);
        assert_eq!(window.max_limit, 997_500);
        assert_eq!(window.select_limit(190_000), 258_400);
        assert_eq!(window.select_limit(200_000), 997_500);
    }

    #[test]
    fn route_catalog_cap_wins_over_the_public_api_model_capacity() {
        let settings = settings_with_model(CustomModel {
            id: "gpt-5.6-sol".into(),
            name: None,
            context_length: Some(272_000),
            max_context_length: Some(272_000),
            effective_context_window_percent: Some(95),
            default_reasoning_effort: None,
            supported_reasoning_efforts: None,
        });

        let window = resolve_context_window(&settings, "chatgpt", "gpt-5.6-sol", None);
        assert_eq!(window.default_limit, 258_400);
        assert_eq!(window.max_limit, 258_400);
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
