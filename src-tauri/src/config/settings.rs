// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ── Git remote types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GitProvider {
    Github,
    Gitlab,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRemoteConfig {
    pub id: String,
    pub name: String,
    pub provider: GitProvider,
    pub base_url: String,
    pub token: String,
    pub default_repo: Option<String>,
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
            permissions: PermissionPolicy {
                allow: vec!["read_file".into(), "glob".into(), "grep".into()],
                ask: vec!["write_file".into(), "edit_file".into(), "bash".into()],
                deny: vec![],
                full_access: false,
            },
            shell: ShellConfig {
                shell: "powershell".into(),
            },
            hooks: vec![],
            mcp_servers: vec![],
            git_remotes: vec![],
            auto_create_pr: false,
        }
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

    settings
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

pub fn save(settings: &Settings) -> crate::errors::Result<()> {
    let path = config_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}
