// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
use std::fmt;
use std::collections::HashMap;
use std::path::PathBuf;

// ── Git remote types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GitProvider {
    Github,
    Gitlab,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct GitRemoteConfig {
    pub id: String,
    pub name: String,
    pub provider: GitProvider,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_ref: Option<String>,
    #[serde(default, skip_serializing)]
    pub token: String,
    pub default_repo: Option<String>,
}

impl fmt::Debug for GitRemoteConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitRemoteConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("token_ref", &self.token_ref)
            .field("token", &"<redacted>")
            .field("default_repo", &self.default_repo)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub endpoints: HashMap<String, Endpoint>,
    pub default_endpoint: String,
    pub default_model: String,
    pub permissions: PermissionPolicy,
    pub shell: ShellConfig,
    #[serde(default)]
    pub hooks: Vec<crate::commands::hooks::HookConfig>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub git_remotes: Vec<GitRemoteConfig>,
    #[serde(default)]
    pub auto_create_pr: bool,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u8,
    /// True once the user has either finished or explicitly skipped the
    /// first-run onboarding flow. Default-false on a clean install so the
    /// overlay shows on first launch and never again.
    #[serde(default)]
    pub onboarded: bool,
    /// Default reasoning effort for reasoning-capable models. Editable in
    /// Settings and via the chat header quick-control.
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    System,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::Dark
    }
}

/// Reasoning effort for reasoning-capable models (currently the ChatGPT/Codex
/// Responses path). Maps directly to `reasoning.effort` in the request body.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

impl Default for ReasoningEffort {
    fn default() -> Self {
        ReasoningEffort::Medium
    }
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }

    /// Parse a stored string (e.g. a per-session override) into an effort,
    /// or None if it's not a recognised value.
    pub fn parse(s: &str) -> Option<ReasoningEffort> {
        match s {
            "minimal" => Some(ReasoningEffort::Minimal),
            "low" => Some(ReasoningEffort::Low),
            "medium" => Some(ReasoningEffort::Medium),
            "high" => Some(ReasoningEffort::High),
            _ => None,
        }
    }
}

#[cfg(test)]
mod git_remote_secret_tests {
    use super::*;

    #[test]
    fn git_remote_legacy_token_gets_default_token_ref() {
        let mut remote: GitRemoteConfig = serde_json::from_value(serde_json::json!({
            "id": "remote-1",
            "name": "GitHub",
            "provider": "github",
            "base_url": "https://api.github.com",
            "token": "ghp_legacy",
            "default_repo": "owner/repo"
        }))
        .expect("legacy remote config should deserialize");

        let changed = normalize_git_remote_secret_refs(&mut remote);

        assert!(changed, "legacy token should trigger a migration marker");
        assert_eq!(
            remote.token_ref.as_deref(),
            Some("codefactory.git_remote.remote-1")
        );
        assert_eq!(remote.token, "ghp_legacy");
    }

    #[test]
    fn git_remote_serialization_omits_plaintext_token() {
        let remote = GitRemoteConfig {
            id: "remote-1".into(),
            name: "GitHub".into(),
            provider: GitProvider::Github,
            base_url: "https://api.github.com".into(),
            token_ref: Some("codefactory.git_remote.remote-1".into()),
            token: "ghp_secret".into(),
            default_repo: Some("owner/repo".into()),
        };

        let json = serde_json::to_string(&remote).expect("serialize remote");

        assert!(json.contains("token_ref"));
        assert!(!json.contains("ghp_secret"));
        assert!(!json.contains("\"token\""));
    }

    #[test]
    fn git_remote_debug_omits_plaintext_token() {
        let remote = GitRemoteConfig {
            id: "remote-1".into(),
            name: "GitHub".into(),
            provider: GitProvider::Github,
            base_url: "https://api.github.com".into(),
            token_ref: None,
            token: "ghp_secret".into(),
            default_repo: Some("owner/repo".into()),
        };

        let debug = format!("{remote:?}");

        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("ghp_secret"));
    }

    #[test]
    fn git_remote_inline_token_without_ref_is_not_used() {
        let remote = GitRemoteConfig {
            id: "remote-1".into(),
            name: "GitHub".into(),
            provider: GitProvider::Github,
            base_url: "https://api.github.com".into(),
            token_ref: None,
            token: "ghp_secret".into(),
            default_repo: Some("owner/repo".into()),
        };

        let err = resolve_git_remote_token(&remote).expect_err("inline token should not be used");
        let message = err.to_string();

        assert!(message.contains("migration is required"));
        assert!(!message.contains("ghp_secret"));
    }
}

fn default_font_family() -> String {
    "inter".into()
}

fn default_font_size() -> u8 {
    14
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ApiStyle {
    Openai,
    Anthropic,
    /// ChatGPT (Codex) subscription — requests go to the ChatGPT backend
    /// Responses API using the OAuth access token from `codex_auth`, not an
    /// API key. See AgentLoop::call_chatgpt_model.
    Chatgpt,
}

impl Default for ApiStyle {
    fn default() -> Self {
        ApiStyle::Openai
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_ref: Option<String>,
    #[serde(default)]
    pub api_style: ApiStyle,
    /// User-defined models bound to this endpoint. Merged with the remote
    /// /models list when the user opens the model picker.
    #[serde(default)]
    pub custom_models: Vec<CustomModel>,
    /// The model the user currently has selected when this endpoint is
    /// active. Each endpoint remembers its own choice so switching
    /// endpoints doesn't carry an incompatible model id along.
    ///
    /// Optional for backward compatibility — the migration in
    /// [`load`] back-fills this from the legacy top-level `default_model`
    /// for whichever endpoint was the default at the time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomModel {
    /// The exact string passed as `model` in chat completion requests.
    pub id: String,
    /// Optional display name (falls back to `id` when empty/missing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
    #[serde(default)]
    pub full_access: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    pub shell: String,
}

fn default_shell_name() -> &'static str {
    #[cfg(windows)]
    {
        "powershell"
    }
    #[cfg(target_os = "macos")]
    {
        "zsh"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "bash"
    }
}

impl Default for Settings {
    fn default() -> Self {
        let mut endpoints = HashMap::new();
        endpoints.insert(
            "openrouter".into(),
            Endpoint {
                base_url: "https://openrouter.ai/api/v1".into(),
                key_ref: Some("codefactory.endpoint.openrouter".into()),
                api_style: ApiStyle::Openai,
                custom_models: vec![],
                active_model: Some("anthropic/claude-opus-4-7".into()),
            },
        );
        Self {
            endpoints,
            default_endpoint: "openrouter".into(),
            default_model: "anthropic/claude-opus-4-7".into(),
            // Action-biased default: non-destructive reads + writes (incl.
            // document generation) auto-approve, so the agent produces
            // deliverables without a confirmation prompt on every file write.
            // Only `bash` (external commands) is gated — and its own shell
            // policy still hard-denies the genuinely dangerous ones.
            permissions: PermissionPolicy {
                allow: vec![
                    "read_file".into(),
                    "glob".into(),
                    "grep".into(),
                    "read_pptx".into(),
                    "write_file".into(),
                    "edit_file".into(),
                    "write_pptx".into(),
                    "edit_pptx".into(),
                    "format_pptx".into(),
                    "write_docx".into(),
                    "read_xlsx".into(),
                    "edit_xlsx".into(),
                ],
                ask: vec!["bash".into()],
                deny: vec![],
                full_access: false,
            },
            shell: ShellConfig {
                shell: default_shell_name().into(),
            },
            hooks: vec![],
            mcp_servers: vec![],
            git_remotes: vec![],
            auto_create_pr: false,
            theme: Theme::Dark,
            font_family: default_font_family(),
            font_size: default_font_size(),
            onboarded: false,
            reasoning_effort: ReasoningEffort::Medium,
        }
    }
}

#[cfg(test)]
mod reasoning_effort_tests {
    use super::*;

    #[test]
    fn default_is_medium() {
        assert_eq!(ReasoningEffort::default(), ReasoningEffort::Medium);
        assert_eq!(Settings::default().reasoning_effort, ReasoningEffort::Medium);
    }

    #[test]
    fn serde_is_lowercase_roundtrip() {
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::Minimal).unwrap(),
            "\"minimal\""
        );
        let parsed: ReasoningEffort = serde_json::from_str("\"high\"").unwrap();
        assert_eq!(parsed, ReasoningEffort::High);
        assert_eq!(ReasoningEffort::Low.as_str(), "low");
        assert_eq!(ReasoningEffort::parse("high"), Some(ReasoningEffort::High));
        assert_eq!(ReasoningEffort::parse("minimal"), Some(ReasoningEffort::Minimal));
        assert_eq!(ReasoningEffort::parse("bogus"), None);
    }

    #[test]
    fn old_settings_without_field_default_to_medium() {
        // Configs that predate this field must still load.
        let s: Settings = serde_json::from_value(serde_json::json!({
            "endpoints": {},
            "default_endpoint": "x",
            "default_model": "y",
            "permissions": {"allow": [], "ask": [], "deny": [], "full_access": false},
            "shell": {"shell": "bash"}
        }))
        .expect("settings without reasoning_effort should deserialize");
        assert_eq!(s.reasoning_effort, ReasoningEffort::Medium);
    }

    #[test]
    fn resolves_chatgpt_request_to_supported_endpoint_model() {
        let mut settings = Settings::default();
        settings.default_endpoint = "chatgpt".into();
        settings.default_model = "anthropic/claude-opus-4-7".into();
        settings.endpoints.insert(
            "chatgpt".into(),
            Endpoint {
                base_url: "https://chatgpt.com/backend-api/codex".into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                custom_models: vec![CustomModel {
                    id: "gpt-5.5".into(),
                    name: Some("GPT-5.5".into()),
                    context_length: Some(272000),
                }],
                active_model: Some("gpt-5.5".into()),
            },
        );

        let resolved = settings.resolve_model_for_endpoint("chatgpt", "anthropic/claude-opus-4-7");

        assert_eq!(resolved.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn resolves_direct_endpoint_to_active_model_even_without_custom_list() {
        let mut settings = Settings::default();
        settings.default_endpoint = "deepseek".into();
        settings.endpoints.insert(
            "deepseek".into(),
            Endpoint {
                base_url: "https://api.deepseek.com".into(),
                key_ref: Some("codefactory.endpoint.deepseek".into()),
                api_style: ApiStyle::Openai,
                custom_models: vec![],
                active_model: Some("deepseek-v4-pro".into()),
            },
        );

        let resolved = settings.resolve_model_for_endpoint(
            "deepseek",
            "anthropic/claude-opus-4-7",
        );

        assert_eq!(resolved.as_deref(), Some("deepseek-v4-pro"));
    }
}

/// Active settings location. Lives alongside the SQLite DB under the Tauri
/// identifier-based app data directory so a single folder covers all user
/// state — survives upgrades and uninstalls cleanly.
///
/// Windows: `%APPDATA%\com.codefactory.app\settings.json`
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.codefactory.app")
        .join("settings.json")
}

/// Legacy path used by versions ≤ 0.3.3 (productName-based folder, separate
/// from the DB). Kept only so [`load`] can migrate old installs forward on
/// the first launch after upgrading.
fn legacy_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodeFactory")
        .join("settings.json")
}

pub fn load() -> Settings {
    let new_path = config_path();

    // One-shot migration: if there's a settings.json in the legacy location
    // but none at the new one, copy it across and rename the original to
    // .migrated-backup so the user can tell what happened (and we never
    // accidentally re-migrate over fresher data).
    let legacy = legacy_config_path();
    if legacy.exists() && !new_path.exists() {
        if let Some(parent) = new_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::copy(&legacy, &new_path) {
            Ok(_) => {
                let backup = legacy.with_extension("json.migrated-backup");
                let _ = std::fs::rename(&legacy, &backup);
                tracing::info!(
                    "settings: migrated {} -> {}",
                    legacy.display(),
                    new_path.display()
                );
            }
            Err(e) => {
                tracing::warn!(
                    "settings: migration {} -> {} failed: {e}",
                    legacy.display(),
                    new_path.display()
                );
            }
        }
    }

    let mut settings: Settings = std::fs::read_to_string(&new_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    // Schema migration v0.3.5 -> v0.3.6:
    //   Promote the legacy top-level `default_model` into the matching
    //   endpoint's `active_model` field (per-endpoint model selection).
    //   Only fills endpoints that don't already have one — never
    //   overwrites a user choice.
    let legacy_default = settings.default_model.clone();
    let default_ep = settings.default_endpoint.clone();
    if let Some(ep) = settings.endpoints.get_mut(&default_ep) {
        if ep.active_model.is_none() && !legacy_default.is_empty() {
            ep.active_model = Some(legacy_default);
        }
    }
    // For every other endpoint that has no active_model yet, leave it
    // None — the resolver will fall back to "first custom_model or
    // first remote model" when the user switches there.

    // Permissions migration: the early default gated every write behind a
    // confirmation prompt, which made the agent feel like it constantly asks.
    // If the user never customized it (the policy still equals that exact old
    // default), upgrade it to the action-biased default. A customized policy is
    // left untouched.
    {
        let p = &settings.permissions;
        let old_allow = ["read_file", "glob", "grep"];
        let old_ask = ["write_file", "edit_file", "bash"];
        let is_old_default = !p.full_access
            && p.deny.is_empty()
            && p.allow.len() == old_allow.len()
            && p.allow.iter().zip(old_allow).all(|(a, b)| a == b)
            && p.ask.len() == old_ask.len()
            && p.ask.iter().zip(old_ask).all(|(a, b)| a == b);
        if is_old_default {
            settings.permissions = Settings::default().permissions;
        }
    }

    // Forward-add the xlsx tools to any policy that already auto-allows document
    // writes (the action-biased default is marked by `write_docx` in allow).
    // Idempotent via the read_xlsx guard; leaves a hand-narrowed policy that
    // dropped write_docx untouched.
    {
        let p = &mut settings.permissions;
        if p.allow.iter().any(|t| t == "write_docx") && !p.allow.iter().any(|t| t == "read_xlsx") {
            p.allow.push("read_xlsx".into());
            p.allow.push("edit_xlsx".into());
        }
    }

    #[cfg(unix)]
    {
        if settings.shell.shell == "powershell" || settings.shell.shell == "cmd" {
            settings.shell.shell = default_shell_name().into();
        }
    }

    match persist_git_remote_inline_tokens(&mut settings) {
        Ok(true) => {
            if let Err(e) = save(&settings) {
                tracing::warn!("settings: failed to persist redacted git remote settings: {e}");
            }
        }
        Ok(false) => {}
        Err(e) => {
            tracing::warn!("settings: failed to migrate git remote token: {e}");
        }
    }

    settings
}

pub fn default_git_remote_token_ref(remote_id: &str) -> String {
    format!("codefactory.git_remote.{remote_id}")
}

#[cfg(test)]
fn normalize_git_remote_secret_refs(remote: &mut GitRemoteConfig) -> bool {
    if remote.token_ref.is_none() && !remote.token.trim().is_empty() && !remote.id.is_empty() {
        remote.token_ref = Some(default_git_remote_token_ref(&remote.id));
        return true;
    }
    false
}

pub fn persist_git_remote_inline_tokens(settings: &mut Settings) -> crate::errors::Result<bool> {
    let mut migrated = false;
    for remote in &mut settings.git_remotes {
        let token = remote.token.trim().to_string();
        if token.is_empty() {
            continue;
        }

        let token_ref = remote
            .token_ref
            .clone()
            .unwrap_or_else(|| default_git_remote_token_ref(&remote.id));
        crate::secrets::set_key(&token_ref, &token)?;
        remote.token_ref = Some(token_ref);
        remote.token.clear();
        migrated = true;
    }
    Ok(migrated)
}

pub fn resolve_git_remote_token(remote: &GitRemoteConfig) -> crate::errors::Result<String> {
    if let Some(token_ref) = &remote.token_ref {
        return crate::secrets::get_key(token_ref)?.ok_or_else(|| {
            crate::errors::AppError::Other(format!(
                "Git remote token is missing for remote '{}' (key_ref '{}')",
                remote.id, token_ref
            ))
        });
    }

    if !remote.token.trim().is_empty() {
        return Err(crate::errors::AppError::Other(format!(
            "Git remote '{}' still has a legacy inline token; migration is required before use",
            remote.id
        )));
    }

    Err(crate::errors::AppError::Other(format!(
        "Git remote token is missing for remote '{}'",
        remote.id
    )))
}

impl Settings {
    /// Resolve the active model id for a given endpoint.
    ///
    /// Order of precedence:
    /// 1. The endpoint's own `active_model` field (preferred — per-endpoint memory)
    /// 2. The endpoint's first `custom_models` entry (user knows it exists)
    /// 3. The legacy top-level `default_model` (back-compat only)
    /// 4. Empty string (caller must handle "no model" UI)
    ///
    /// Never returns an OpenRouter-prefixed id for a non-OpenRouter endpoint
    /// when one isn't appropriate — but normalisation of arbitrary ids is
    /// the caller's job (see [`normalize_model_id`]).
    pub fn active_model_for(&self, endpoint_name: &str) -> String {
        if let Some(ep) = self.endpoints.get(endpoint_name) {
            if let Some(m) = &ep.active_model {
                if !m.is_empty() {
                    return m.clone();
                }
            }
            if let Some(first) = ep.custom_models.first() {
                return first.id.clone();
            }
        }
        if !self.default_model.is_empty() {
            return self.default_model.clone();
        }
        String::new()
    }

    /// Resolve the model that is safe to send to a concrete endpoint.
    ///
    /// Sessions store only `model_id` for backward compatibility. If the user
    /// switches the default endpoint after a session was created, that stored
    /// model can belong to a different provider. ChatGPT/Codex is strict about
    /// accepted model slugs, and direct custom endpoints usually expect the
    /// endpoint's own active model. This resolver repairs that mismatch before
    /// the request leaves the app.
    pub fn resolve_model_for_endpoint(
        &self,
        endpoint_name: &str,
        requested_model: &str,
    ) -> Option<String> {
        let ep = self.endpoints.get(endpoint_name)?;
        let requested = requested_model.trim();
        let active = ep.active_model.as_deref().unwrap_or("").trim();

        if !requested.is_empty() && requested == active {
            return Some(requested.to_string());
        }

        if !requested.is_empty() && ep.custom_models.iter().any(|m| m.id == requested) {
            return Some(requested.to_string());
        }

        let has_endpoint_model = !active.is_empty() || !ep.custom_models.is_empty();
        let should_repair_to_endpoint_model = matches!(ep.api_style, ApiStyle::Chatgpt)
            || (has_endpoint_model && !ep.base_url.contains("openrouter.ai"));

        if should_repair_to_endpoint_model {
            if !active.is_empty() {
                return Some(active.to_string());
            }
            if let Some(first) = ep.custom_models.first() {
                return Some(first.id.clone());
            }
        }

        if !requested.is_empty() {
            return Some(requested.to_string());
        }

        let fallback = self.active_model_for(endpoint_name);
        if fallback.is_empty() {
            None
        } else {
            Some(fallback)
        }
    }

    /// Set the active model for an endpoint. Returns true if anything changed.
    pub fn set_active_model(&mut self, endpoint_name: &str, model_id: &str) -> bool {
        if let Some(ep) = self.endpoints.get_mut(endpoint_name) {
            if ep.active_model.as_deref() != Some(model_id) {
                ep.active_model = Some(model_id.to_string());
                // Mirror to the legacy field when this is the active endpoint
                // so older code reading `default_model` still sees a sane value.
                if endpoint_name == self.default_endpoint {
                    self.default_model = model_id.to_string();
                }
                return true;
            }
        }
        false
    }
}

/// Strip an OpenRouter-style `vendor/` prefix from a model id when the
/// target endpoint isn't OpenRouter. Direct provider APIs (DeepSeek,
/// Anthropic, OpenAI, etc.) reject ids like `deepseek/deepseek-v4-pro`
/// because the leading vendor segment is OpenRouter's own routing
/// convention, not part of the canonical model name.
///
/// Safety: only strips when there's a clear vendor prefix AND the URL
/// isn't openrouter.ai. Pass-through for everything else, including
/// ids that legitimately contain slashes downstream of the vendor part.
pub fn normalize_model_id(model_id: &str, base_url: &str) -> String {
    if base_url.contains("openrouter.ai") {
        return model_id.to_string();
    }
    // Only strip the very first segment; preserve everything after.
    if let Some((_, rest)) = model_id.split_once('/') {
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    model_id.to_string()
}

/// Rewrite a chat body to the GPT-5 / o-series shape
/// (`max_tokens` → floored `max_completion_tokens`, drop `temperature`),
/// independent of the model name.
///
/// The reactive fallback: when a provider answers a request with HTTP 400
/// "use 'max_completion_tokens' instead" for any model — including names we
/// would never flag (custom aliases, proxies, Azure deployment ids, `gpt5`…) —
/// the caller applies this and resends, so the fix never depends on guessing
/// model names.
pub fn force_max_completion_tokens(body: &mut serde_json::Value) {
    if let Some(obj) = body.as_object_mut() {
        if let Some(cap) = obj.remove("max_tokens") {
            let floored = cap.as_u64().unwrap_or(8192).max(8192);
            obj.entry("max_completion_tokens")
                .or_insert_with(|| serde_json::json!(floored));
        }
        obj.remove("temperature");
    }
}

pub fn save(settings: &Settings) -> crate::errors::Result<()> {
    let path = config_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

#[cfg(test)]
mod reasoning_model_adaptation_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn force_converts_regardless_of_model_name() {
        // The reactive fallback fires on the provider's 400 with no model in
        // hand — it must convert unconditionally (here a name we would NOT flag).
        let mut body = json!({ "model": "weird-alias-x", "temperature": 0.2, "max_tokens": 1024 });
        force_max_completion_tokens(&mut body);
        let obj = body.as_object().unwrap();
        assert!(!obj.contains_key("max_tokens"));
        assert!(!obj.contains_key("temperature"));
        assert_eq!(obj["max_completion_tokens"], json!(8192)); // floored
    }
}
