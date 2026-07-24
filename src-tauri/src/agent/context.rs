// SPDX-License-Identifier: Apache-2.0
//! Context window management (Settings-coupled half).
//!
//! Resolves the model's context window from `Settings`/catalog metadata and the
//! name-based vision-capability guess. The `Settings`-FREE estimation + adaptive
//! compression logic moved to `agent-loop::context` (keystone slice 4.6b) and is
//! re-exported here, so every `context::…` call site across the bin (both provider
//! loops, `DesktopContextPolicy`, the UI context meter) keeps resolving unchanged.

use crate::config::Settings;

// Settings-free estimation/compression, relocated to the shared crate.
pub use codefactory_agent_loop::context::{
    compress_if_needed, estimate_prompt_tokens, is_context_overflow, COMPRESSION_TRIGGER,
};

/// Conservative window when nothing is known about the model.
/// 128K is the modern baseline (GPT-4o, Claude, Gemini, DeepSeek-v4-pro all
/// at or above this). Going lower triggers spurious compression and shows
/// "100%" too early — see the user report where a 60K prompt against the
/// old 16K fallback showed 375 % usage and triggered phantom compression.
pub const FALLBACK_CONTEXT_LENGTH: u32 = 128_000;

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

    // 3. Route-capability override BEFORE the public name-based capacity:
    //    the ChatGPT subscription backend caps gpt-5.x at the Codex route
    //    window (272K) regardless of the API model's 1.05M capacity. Guessing
    //    1.05M put the compression trigger at ~787K so compression never
    //    fired, and the route rejected real prompts with "input exceeds the
    //    context window" (2026-07-21, three killed turns). Explicit catalog
    //    metadata above still wins.
    if let Some(ep) = settings.endpoints.get(endpoint_name) {
        if matches!(ep.api_style, crate::config::settings::ApiStyle::Chatgpt)
            && model_id.to_lowercase().starts_with("gpt-5")
        {
            return ResolvedContextWindow {
                default_limit: 272_000,
                max_limit: 272_000,
            };
        }
    }

    // 4. Pattern-match against known model families. Direct providers don't
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

/// Whether `(endpoint, model)` accepts image input. Explicit CustomModel
/// metadata wins; otherwise a conservative name-based guess: only families
/// KNOWN to be text-only return false (deepseek chat/reasoner rejected
/// images in production, 2026-07-21). Unknown models default to true — the
/// reactive strip-and-retry remains the net for wrong guesses; proactively
/// dropping images on a false negative would be worse than one failed
/// round-trip.
pub fn model_supports_vision(
    settings: &Settings,
    endpoint_name: &str,
    model_id: &str,
) -> bool {
    if let Some(ep) = settings.endpoints.get(endpoint_name) {
        for cm in &ep.custom_models {
            if cm.id == model_id {
                if let Some(explicit) = cm.supports_vision {
                    return explicit;
                }
                break;
            }
        }
    }
    let id = model_id.to_lowercase();
    let id = id.split('/').last().unwrap_or(&id);
    if id.starts_with("deepseek") && !id.contains("vl") {
        return false;
    }
    true
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

#[cfg(test)]
mod tests {
    use super::{guess_context_from_name as guess, model_supports_vision, resolve_context_window};
    use crate::config::settings::{ApiStyle, CustomModel, Endpoint, Settings};

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
            supports_vision: None,
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
            supports_vision: None,
        });

        let window = resolve_context_window(&settings, "chatgpt", "gpt-5.6-sol", None);
        assert_eq!(window.default_limit, 258_400);
        assert_eq!(window.max_limit, 258_400);
    }

    #[test]
    fn chatgpt_subscription_route_caps_gpt56_at_the_route_window() {
        // 2026-07-21 field failure: gpt-5.6-sol via the ChatGPT subscription
        // backend rejected prompts with "input exceeds the context window"
        // three times. Name-guess said 1.05M (API capacity) so compression
        // never triggered; the subscription route caps at 272K.
        let mut settings = Settings::default();
        settings.endpoints.insert(
            "chatgpt".into(),
            Endpoint {
                base_url: "https://chatgpt.com/backend-api/codex".into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                custom_models: vec![],
                active_model: Some("gpt-5.6-sol".into()),
            },
        );
        let window = resolve_context_window(&settings, "chatgpt", "gpt-5.6-sol", None);
        assert_eq!(window.default_limit, 272_000);
        assert_eq!(window.max_limit, 272_000);

        let mut api = Settings::default();
        api.endpoints.insert(
            "openai".into(),
            Endpoint {
                base_url: "https://api.openai.com/v1".into(),
                key_ref: None,
                api_style: ApiStyle::Openai,
                custom_models: vec![],
                active_model: None,
            },
        );
        let window = resolve_context_window(&api, "openai", "gpt-5.6-sol", None);
        assert_eq!(window.default_limit, 1_050_000);
    }

    #[test]
    fn vision_capability_guess_is_conservative() {
        // Only families KNOWN text-only return false (deepseek chat/reasoner
        // rejected images in production on 2026-07-21); vision variants and
        // unknown models default to true — the reactive strip-and-retry stays
        // as the net for wrong guesses, and proactively dropping images on a
        // false negative would be worse than one failed round-trip.
        let settings = Settings::default();
        assert!(!model_supports_vision(&settings, "any", "deepseek-v4-pro"));
        assert!(!model_supports_vision(&settings, "any", "deepseek-reasoner"));
        assert!(model_supports_vision(&settings, "any", "deepseek-vl2"));
        assert!(model_supports_vision(&settings, "any", "gpt-5.6-sol"));
        assert!(model_supports_vision(&settings, "any", "claude-opus-4-8"));
        assert!(model_supports_vision(&settings, "any", "totally-unknown-model"));
    }

    #[test]
    fn custom_model_vision_metadata_wins_over_the_guess() {
        let mut settings = Settings::default();
        settings.endpoints.insert(
            "ep".into(),
            Endpoint {
                base_url: "https://example.com/v1".into(),
                key_ref: None,
                api_style: ApiStyle::Openai,
                custom_models: vec![
                    CustomModel {
                        id: "deepseek-v4-pro".into(),
                        name: None,
                        context_length: None,
                        max_context_length: None,
                        effective_context_window_percent: None,
                        default_reasoning_effort: None,
                        supported_reasoning_efforts: None,
                        supports_vision: Some(true),
                    },
                    CustomModel {
                        id: "gpt-5.6-sol".into(),
                        name: None,
                        context_length: None,
                        max_context_length: None,
                        effective_context_window_percent: None,
                        default_reasoning_effort: None,
                        supported_reasoning_efforts: None,
                        supports_vision: Some(false),
                    },
                ],
                active_model: None,
            },
        );
        assert!(model_supports_vision(&settings, "ep", "deepseek-v4-pro"));
        assert!(!model_supports_vision(&settings, "ep", "gpt-5.6-sol"));
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
