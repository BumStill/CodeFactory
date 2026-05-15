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

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodeFactory")
        .join("settings.json")
}

pub fn load() -> Settings {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) -> crate::errors::Result<()> {
    let path = config_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}
