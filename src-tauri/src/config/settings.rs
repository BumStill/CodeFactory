// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    /// Explicit privacy opt-in for sending a bounded, redacted post-mortem
    /// summary to the currently configured model. Local deterministic
    /// cross-session mining does not require this flag.
    #[serde(default)]
    pub remote_postmortem_enabled: bool,
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
    /// Max concurrent subagents for parallel task execution. The scheduler
    /// clamps this to 1..=8 at run time, so out-of-range persisted values
    /// never stall or overload a session.
    #[serde(default = "default_max_parallel_tasks")]
    pub max_parallel_tasks: u8,
    /// Disk isolation mode for parallel subagents.
    #[serde(default)]
    pub subagent_isolation: SubagentIsolation,
    /// How far the agent auto-delivers code changes (commit → push → PR → CI →
    /// merge → release). The USER owns this ceiling; the app never hardcodes a
    /// policy. Default `PrOnly` opens a PR and stops — reviewable, PR-first, and
    /// enough to break the "green build but no PR" stall.
    #[serde(default)]
    pub delivery_ceiling: DeliveryCeiling,
    /// Merge strategy used when the ceiling reaches `ThroughMerge`+.
    #[serde(default)]
    pub delivery_merge_method: MergeMethod,
    /// Extra path prefixes/globs excluded from delivery commits, on top of the
    /// built-in noise denylist. Repo-relative, `/`-separated.
    #[serde(default)]
    pub delivery_exclude_globs: Vec<String>,
    /// Max seconds to poll CI for a conclusion before reporting it still
    /// pending (bounded so a delivery never hangs a turn forever).
    #[serde(default = "default_delivery_ci_timeout_secs")]
    pub delivery_ci_timeout_secs: u32,
}

/// How far the agent carries a code change toward production, unattended. The
/// user selects the ceiling; a per-call request may only LOWER it, never raise
/// it above what the user configured.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryCeiling {
    /// Never auto-deliver. The `deliver_changes` tool still exists for explicit
    /// use, but delivery is not part of the agent's definition of "done".
    Off,
    /// Commit the real changed source files, push, and open a PR, then stop.
    #[default]
    PrOnly,
    /// …then poll CI to a conclusion; stop before merging.
    ThroughCiGreen,
    /// …then merge the PR (per `delivery_merge_method`); stop before release.
    ThroughMerge,
    /// …then trigger a release. Deliberate by design — only reached when the
    /// user explicitly raises the ceiling this far.
    ThroughRelease,
}

impl DeliveryCeiling {
    /// 0..=4, so a per-call override can be clamped to `min(request, configured)`.
    pub fn rank(self) -> u8 {
        match self {
            DeliveryCeiling::Off => 0,
            DeliveryCeiling::PrOnly => 1,
            DeliveryCeiling::ThroughCiGreen => 2,
            DeliveryCeiling::ThroughMerge => 3,
            DeliveryCeiling::ThroughRelease => 4,
        }
    }

    /// The effective ceiling for a call that requested `requested`: never above
    /// what the user configured (`self`).
    pub fn clamp_request(self, requested: DeliveryCeiling) -> DeliveryCeiling {
        if requested.rank() <= self.rank() {
            requested
        } else {
            self
        }
    }
}

/// Merge strategy for `ThroughMerge`+.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    #[default]
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Squash => "squash",
            MergeMethod::Merge => "merge",
            MergeMethod::Rebase => "rebase",
        }
    }
}

fn default_delivery_ci_timeout_secs() -> u32 {
    1800
}

/// How parallel subagents are isolated from each other on disk.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SubagentIsolation {
    /// All subagents share the task cwd, guarded by per-file locks only.
    #[default]
    Shared,
    /// Each subagent works in its own git worktree; its diff is applied back
    /// to the shared cwd only after verification passes. Falls back to
    /// `Shared` when the task cwd is not inside a git repository.
    Worktree,
}

fn default_max_parallel_tasks() -> u8 {
    3
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

/// Reasoning selection for the ChatGPT/Codex path. Values through `Max` map to
/// `reasoning.effort`; `Ultra` is a client orchestration mode and must be
/// translated to `Max` at the Responses transport boundary.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
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
            ReasoningEffort::XHigh => "xhigh",
            ReasoningEffort::Max => "max",
            ReasoningEffort::Ultra => "ultra",
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
            "xhigh" => Some(ReasoningEffort::XHigh),
            "max" => Some(ReasoningEffort::Max),
            "ultra" => Some(ReasoningEffort::Ultra),
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomModel {
    /// The exact string passed as `model` in chat completion requests.
    pub id: String,
    /// Optional display name (falls back to `id` when empty/missing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    /// Default context window advertised for the active provider route.
    /// `max_context_length` may be larger when the provider permits clients
    /// to expand long-running sessions beyond this normal operating budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_length: Option<u32>,
    /// Percentage of the advertised window considered usable for input after
    /// reserving provider/client headroom. Missing means 100% for compatibility
    /// with existing custom endpoints that only supplied `context_length`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_context_window_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_reasoning_efforts: Option<Vec<ReasoningEffort>>,
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
            remote_postmortem_enabled: false,
            theme: Theme::Dark,
            font_family: default_font_family(),
            font_size: default_font_size(),
            onboarded: false,
            reasoning_effort: ReasoningEffort::Medium,
            max_parallel_tasks: default_max_parallel_tasks(),
            subagent_isolation: SubagentIsolation::Shared,
            delivery_ceiling: DeliveryCeiling::PrOnly,
            delivery_merge_method: MergeMethod::Squash,
            delivery_exclude_globs: Vec::new(),
            delivery_ci_timeout_secs: default_delivery_ci_timeout_secs(),
        }
    }
}

#[cfg(test)]
mod subagent_isolation_tests {
    use super::*;

    #[test]
    fn defaults_are_shared_and_three_parallel() {
        let s = Settings::default();
        assert_eq!(s.subagent_isolation, SubagentIsolation::Shared);
        assert_eq!(s.max_parallel_tasks, 3);
    }

    #[test]
    fn legacy_settings_json_without_new_fields_deserializes_to_defaults() {
        // A settings.json written before these fields existed must load with
        // today's defaults instead of failing deserialization.
        let legacy = serde_json::json!({
            "endpoints": {},
            "default_endpoint": "openrouter",
            "default_model": "m",
            "permissions": { "allow": [], "ask": [], "deny": [], "full_access": false },
            "shell": { "shell": "bash" }
        });
        let s: Settings = serde_json::from_value(legacy).expect("legacy settings must parse");
        assert_eq!(s.subagent_isolation, SubagentIsolation::Shared);
        assert_eq!(s.max_parallel_tasks, 3);
    }

    #[test]
    fn isolation_serde_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&SubagentIsolation::Worktree).unwrap(),
            "\"worktree\""
        );
        let parsed: SubagentIsolation = serde_json::from_str("\"shared\"").unwrap();
        assert_eq!(parsed, SubagentIsolation::Shared);
    }
}

#[cfg(test)]
mod delivery_ceiling_tests {
    use super::*;

    #[test]
    fn default_ceiling_is_pr_only() {
        let s = Settings::default();
        assert_eq!(s.delivery_ceiling, DeliveryCeiling::PrOnly);
        assert_eq!(s.delivery_merge_method, MergeMethod::Squash);
        assert_eq!(s.delivery_ci_timeout_secs, 1800);
        assert!(s.delivery_exclude_globs.is_empty());
    }

    #[test]
    fn legacy_settings_without_delivery_fields_default_to_pr_only() {
        // A settings.json written before delivery existed must load with the
        // PrOnly default, not fail to deserialize.
        let legacy = serde_json::json!({
            "endpoints": {},
            "default_endpoint": "openrouter",
            "default_model": "m",
            "permissions": { "allow": [], "ask": [], "deny": [], "full_access": false },
            "shell": { "shell": "bash" }
        });
        let s: Settings = serde_json::from_value(legacy).expect("legacy settings must parse");
        assert_eq!(s.delivery_ceiling, DeliveryCeiling::PrOnly);
        assert_eq!(s.delivery_ci_timeout_secs, 1800);
    }

    #[test]
    fn ceiling_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&DeliveryCeiling::ThroughCiGreen).unwrap(),
            "\"through_ci_green\""
        );
        let parsed: DeliveryCeiling = serde_json::from_str("\"through_release\"").unwrap();
        assert_eq!(parsed, DeliveryCeiling::ThroughRelease);
    }

    #[test]
    fn clamp_request_never_raises_above_configured() {
        // A per-call request may lower the ceiling but never exceed the user's.
        let configured = DeliveryCeiling::ThroughCiGreen;
        assert_eq!(
            configured.clamp_request(DeliveryCeiling::ThroughRelease),
            DeliveryCeiling::ThroughCiGreen,
            "request above configured is clamped down"
        );
        assert_eq!(
            configured.clamp_request(DeliveryCeiling::PrOnly),
            DeliveryCeiling::PrOnly,
            "request below configured is honored"
        );
        assert_eq!(
            DeliveryCeiling::Off.clamp_request(DeliveryCeiling::ThroughMerge),
            DeliveryCeiling::Off,
            "Off disables delivery regardless of request"
        );
    }
}

#[cfg(test)]
mod reasoning_effort_tests {
    use super::*;

    #[test]
    fn remote_postmortem_is_opt_in_by_default() {
        assert!(!Settings::default().remote_postmortem_enabled);
    }

    #[test]
    fn default_is_medium() {
        assert_eq!(ReasoningEffort::default(), ReasoningEffort::Medium);
        assert_eq!(
            Settings::default().reasoning_effort,
            ReasoningEffort::Medium
        );
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
        assert_eq!(
            ReasoningEffort::parse("minimal"),
            Some(ReasoningEffort::Minimal)
        );
        assert_eq!(ReasoningEffort::parse("bogus"), None);
    }

    #[test]
    fn extended_reasoning_efforts_are_lowercase_and_parseable() {
        for (effort, serialized) in [
            (ReasoningEffort::XHigh, "\"xhigh\""),
            (ReasoningEffort::Max, "\"max\""),
            (ReasoningEffort::Ultra, "\"ultra\""),
        ] {
            assert_eq!(serde_json::to_string(&effort).unwrap(), serialized);
            assert_eq!(ReasoningEffort::parse(effort.as_str()), Some(effort));
        }
    }

    #[test]
    fn custom_model_capabilities_are_optional_for_old_configs() {
        let legacy: CustomModel = serde_json::from_value(serde_json::json!({
            "id": "legacy-codex",
            "name": "Legacy Codex",
            "context_length": 272000
        }))
        .expect("legacy custom model should deserialize");

        assert_eq!(legacy.default_reasoning_effort, None);
        assert_eq!(legacy.supported_reasoning_efforts, None);

        let current: CustomModel = serde_json::from_value(serde_json::json!({
            "id": "gpt-5.6-sol",
            "default_reasoning_effort": "low",
            "supported_reasoning_efforts": ["low", "medium", "xhigh", "max", "ultra"]
        }))
        .expect("current custom model should deserialize");

        assert_eq!(current.default_reasoning_effort, Some(ReasoningEffort::Low));
        assert_eq!(
            current.supported_reasoning_efforts,
            Some(vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
                ReasoningEffort::Ultra,
            ])
        );
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
                    max_context_length: Some(272000),
                    effective_context_window_percent: Some(95),
                    default_reasoning_effort: None,
                    supported_reasoning_efforts: None,
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

        let resolved = settings.resolve_model_for_endpoint("deepseek", "anthropic/claude-opus-4-7");

        assert_eq!(resolved.as_deref(), Some("deepseek-v4-pro"));
    }

    #[test]
    fn resolves_default_ai_helpers_to_the_active_endpoint_model() {
        let mut settings = Settings::default();
        settings.default_endpoint = "deepseek".into();
        settings.default_model = "gpt-5.5".into();
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

        assert_eq!(
            settings.resolved_default_model().as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[test]
    fn rejects_stale_provider_model_when_direct_endpoint_has_no_model_metadata() {
        let mut settings = Settings::default();
        settings.default_endpoint = "deepseek".into();
        settings.default_model = "gpt-5.5".into();
        settings.endpoints.insert(
            "deepseek".into(),
            Endpoint {
                base_url: "https://api.deepseek.com".into(),
                key_ref: Some("codefactory.endpoint.deepseek".into()),
                api_style: ApiStyle::Openai,
                custom_models: vec![],
                active_model: None,
            },
        );

        assert_eq!(settings.resolved_default_model(), None);
    }

    #[test]
    fn resolves_stale_direct_provider_model_to_openrouter_active_model() {
        let mut settings = Settings::default();
        settings.default_endpoint = "openrouter".into();
        let endpoint = settings.endpoints.get_mut("openrouter").unwrap();
        endpoint.active_model = Some("anthropic/claude-sonnet-4".into());

        let resolved = settings.resolve_model_for_endpoint("openrouter", "deepseek-v4-pro");

        assert_eq!(resolved.as_deref(), Some("anthropic/claude-sonnet-4"));
    }

    #[test]
    fn preserves_explicit_openrouter_session_model_when_endpoint_active_model_changes() {
        let mut settings = Settings::default();
        settings.default_endpoint = "openrouter".into();
        let endpoint = settings.endpoints.get_mut("openrouter").unwrap();
        endpoint.active_model = Some("anthropic/claude-sonnet-4".into());

        let resolved = settings.resolve_model_for_endpoint("openrouter", "google/gemini-2.5-pro");

        assert_eq!(resolved.as_deref(), Some("google/gemini-2.5-pro"));
    }
}

/// Active settings location. Lives alongside the SQLite DB under the Tauri
/// identifier-based app data directory so a single folder covers all user
/// state — survives upgrades and uninstalls cleanly.
///
/// Windows release: `%APPDATA%\com.codefactory.app\settings.json`
/// Windows dev: `%APPDATA%\com.codefactory.dev\settings.json`
fn config_path_for(config_root: &Path, is_debug: bool) -> PathBuf {
    config_root
        .join(if is_debug {
            "com.codefactory.dev"
        } else {
            "com.codefactory.app"
        })
        .join("settings.json")
}

pub fn config_path() -> PathBuf {
    config_path_for(
        &dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")),
        cfg!(debug_assertions),
    )
}

fn release_config_path() -> PathBuf {
    config_path_for(
        &dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")),
        false,
    )
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

fn migrate_settings_file(
    source: &Path,
    target: &Path,
    archive_source: bool,
) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, target)?;
    if archive_source {
        std::fs::rename(source, source.with_extension("json.migrated-backup"))?;
    }
    Ok(())
}

pub fn load() -> Settings {
    let new_path = config_path();

    // Dev builds copy the release settings once into their own identifier-based
    // directory. The release file stays in place, so the two apps never share a
    // writable catalog/settings file after migration.
    let migration_source = if cfg!(debug_assertions) && release_config_path().exists() {
        Some((release_config_path(), false))
    } else if legacy_config_path().exists() {
        Some((legacy_config_path(), true))
    } else {
        None
    };
    if let Some((legacy, archive_source)) = migration_source.filter(|_| !new_path.exists()) {
        match migrate_settings_file(&legacy, &new_path, archive_source) {
            Ok(()) => {
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
        let active_is_compatible = !active.is_empty()
            && !model_obviously_targets_another_provider(endpoint_name, ep, active);
        let requested_is_compatible = !requested.is_empty()
            && !model_obviously_targets_another_provider(endpoint_name, ep, requested);

        if !requested.is_empty() && requested == active && active_is_compatible {
            return Some(requested.to_string());
        }

        if !requested.is_empty() && ep.custom_models.iter().any(|m| m.id == requested) {
            return Some(requested.to_string());
        }

        // OpenRouter's remote catalog uses provider-qualified `owner/model`
        // ids. They are not copied into custom_models, and a session may keep
        // one even after another session changes the endpoint-wide active
        // model. Preserve that explicit per-session choice. Unqualified slugs
        // from direct providers or ChatGPT still fall through to repair.
        if ep.base_url.contains("openrouter.ai") && requested.contains('/') {
            return Some(requested.to_string());
        }

        let has_endpoint_model = active_is_compatible || !ep.custom_models.is_empty();
        // A remaining model that is neither active nor explicitly compatible
        // is stale. ModelPicker persists a real selection as active before
        // session creation, so this repairs provider-switch races.
        let should_repair_to_endpoint_model =
            matches!(ep.api_style, ApiStyle::Chatgpt) || has_endpoint_model;

        if should_repair_to_endpoint_model {
            if active_is_compatible {
                return Some(active.to_string());
            }
            if let Some(first) = ep.custom_models.first() {
                return Some(first.id.clone());
            }
        }

        if requested_is_compatible {
            return Some(requested.to_string());
        }

        let fallback = self.active_model_for(endpoint_name);
        if fallback.is_empty()
            || model_obviously_targets_another_provider(endpoint_name, ep, &fallback)
        {
            None
        } else {
            Some(fallback)
        }
    }

    /// Resolve the model for AI helpers that use the configured default
    /// endpoint but are not attached to a session, such as task decomposition,
    /// spec assistance, and post-session learning.
    pub fn resolved_default_model(&self) -> Option<String> {
        self.resolve_model_for_endpoint(&self.default_endpoint, &self.default_model)
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

fn model_obviously_targets_another_provider(
    endpoint_name: &str,
    endpoint: &Endpoint,
    model_id: &str,
) -> bool {
    let Some(endpoint_provider) = identifiable_endpoint_provider(endpoint_name, endpoint) else {
        return false;
    };
    let Some(model_provider) = identifiable_model_provider(model_id) else {
        return false;
    };
    endpoint_provider != model_provider
}

fn identifiable_endpoint_provider(
    endpoint_name: &str,
    endpoint: &Endpoint,
) -> Option<&'static str> {
    let identity = format!("{} {}", endpoint_name, endpoint.base_url).to_ascii_lowercase();
    if identity.contains("openrouter.ai") {
        return None;
    }
    match endpoint.api_style {
        ApiStyle::Anthropic => return Some("anthropic"),
        ApiStyle::Chatgpt => return Some("openai"),
        ApiStyle::Openai => {}
    }
    if identity.contains("deepseek") {
        Some("deepseek")
    } else if identity.contains("anthropic") {
        Some("anthropic")
    } else if identity.contains("openai") || identity.contains("chatgpt") {
        Some("openai")
    } else if identity.contains("google") || identity.contains("gemini") {
        Some("google")
    } else {
        None
    }
}

fn identifiable_model_provider(model_id: &str) -> Option<&'static str> {
    let model = model_id.to_ascii_lowercase();
    if model.contains("deepseek") {
        Some("deepseek")
    } else if model.contains("claude") || model.contains("anthropic") {
        Some("anthropic")
    } else if model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("openai/")
    {
        Some("openai")
    } else if model.contains("gemini") || model.starts_with("google/") {
        Some("google")
    } else {
        None
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
    let identity = base_url.to_ascii_lowercase();
    if identity.contains("openrouter.ai") {
        return model_id.to_string();
    }
    let Some((provider, rest)) = model_id.split_once('/') else {
        return model_id.to_string();
    };
    if rest.is_empty() {
        return model_id.to_string();
    }
    let aliases: &[&str] = match provider.to_ascii_lowercase().as_str() {
        "deepseek" => &["deepseek"],
        "anthropic" => &["anthropic", "claude"],
        "openai" => &["openai", "chatgpt"],
        "google" => &["google", "gemini"],
        _ => return model_id.to_string(),
    };
    if aliases.iter().any(|alias| identity.contains(alias)) {
        rest.to_string()
    } else {
        model_id.to_string()
    }
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

fn save_to_path(path: &Path, settings: &Settings) -> crate::errors::Result<()> {
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".settings.json.tmp.")
        .tempfile_in(parent)?;
    temporary.write_all(&serde_json::to_vec_pretty(settings)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn save(settings: &Settings) -> crate::errors::Result<()> {
    save_to_path(&config_path(), settings)
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

#[cfg(test)]
mod settings_persistence_tests {
    use super::*;

    #[test]
    fn debug_and_release_settings_use_distinct_identifier_directories() {
        let root = PathBuf::from("/config-root");

        assert_eq!(
            config_path_for(&root, true),
            root.join("com.codefactory.dev").join("settings.json")
        );
        assert_eq!(
            config_path_for(&root, false),
            root.join("com.codefactory.app").join("settings.json")
        );
    }

    #[test]
    fn settings_save_replaces_existing_json_without_leaving_partial_file() {
        let root = std::env::temp_dir().join(format!(
            "codefactory-settings-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = root.join("settings.json");
        let mut first = Settings::default();
        first.default_model = "first".into();
        let mut second = first.clone();
        second.default_model = "second".into();

        save_to_path(&path, &first).unwrap();
        save_to_path(&path, &second).unwrap();

        let stored: Settings =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored.default_model, "second");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dev_settings_migration_copies_release_file_without_removing_it() {
        let root = std::env::temp_dir().join(format!(
            "codefactory-settings-migration-test-{}",
            uuid::Uuid::new_v4()
        ));
        let release = config_path_for(&root, false);
        let dev = config_path_for(&root, true);
        std::fs::create_dir_all(release.parent().unwrap()).unwrap();
        std::fs::write(&release, br#"{"default_model":"gpt-5.6-sol"}"#).unwrap();

        migrate_settings_file(&release, &dev, false).unwrap();

        assert!(release.exists());
        assert_eq!(
            std::fs::read(&release).unwrap(),
            std::fs::read(&dev).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
