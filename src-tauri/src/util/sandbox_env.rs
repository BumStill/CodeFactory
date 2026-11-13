// SPDX-License-Identifier: Apache-2.0
//! 沙箱子进程环境：去掉凭据，并把网络指到宿主出口代理。

use std::path::Path;
use std::process::Command as StdCommand;
use tokio::process::Command as TokioCommand;

pub struct SandboxChildEnv<'a> {
    pub proxy_port: Option<u16>,
    pub private_tmpdir: Option<&'a Path>,
}

pub fn should_strip(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    if upper.starts_with("CODEFACTORY_DISPATCH_") {
        return true;
    }
    const EXACT: &[&str] = &[
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITLAB_TOKEN",
        "GL_TOKEN",
        "NPM_TOKEN",
        "NODE_AUTH_TOKEN",
        "CARGO_REGISTRY_TOKEN",
        "TWINE_PASSWORD",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENROUTER_API_KEY",
        "DEEPSEEK_API_KEY",
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "SSH_AUTH_SOCK",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_SECURITY_TOKEN",
    ];
    if EXACT.contains(&upper.as_str()) {
        return true;
    }
    if upper.ends_with("_TOKEN")
        || upper.ends_with("_API_KEY")
        || upper.ends_with("_SECRET")
        || upper.ends_with("_PASSWORD")
    {
        return true;
    }
    upper.starts_with("AWS_")
        && (upper.contains("KEY") || upper.contains("SECRET") || upper.contains("TOKEN") || upper.contains("PASSWORD"))
}

pub fn apply_tokio(command: &mut TokioCommand, env: &SandboxChildEnv<'_>) {
    strip_inherited(command);
    apply_common(
        |key, value| {
            command.env(key, value);
        },
        env,
    );
}

pub fn apply_std(command: &mut StdCommand, env: &SandboxChildEnv<'_>) {
    for (key, _) in std::env::vars() {
        if should_strip(&key) {
            command.env_remove(&key);
        }
    }
    apply_common(
        |key, value| {
            command.env(key, value);
        },
        env,
    );
}

fn strip_inherited(command: &mut TokioCommand) {
    for (key, _) in std::env::vars() {
        if should_strip(&key) {
            command.env_remove(&key);
        }
    }
}

fn apply_common(mut set: impl FnMut(&str, &str), env: &SandboxChildEnv<'_>) {
    if let Some(dir) = env.private_tmpdir {
        let text = dir.to_string_lossy();
        set("TMPDIR", text.as_ref());
        set("TMP", text.as_ref());
        set("TEMP", text.as_ref());
    }
    if let Some(port) = env.proxy_port {
        let proxy = format!("http://127.0.0.1:{port}");
        for key in [
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "ALL_PROXY",
            "https_proxy",
            "http_proxy",
            "all_proxy",
            "npm_config_proxy",
            "npm_config_https_proxy",
            "CARGO_HTTP_PROXY",
            "PIP_PROXY",
        ] {
            set(key, &proxy);
        }
        set("NO_PROXY", "localhost,127.0.0.1,::1");
        set("no_proxy", "localhost,127.0.0.1,::1");
    }
    set("GIT_CONFIG_COUNT", "1");
    set("GIT_CONFIG_KEY_0", "credential.helper");
    set("GIT_CONFIG_VALUE_0", "");
    set("GIT_TERMINAL_PROMPT", "0");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h4_strip_list_covers_forge_registry_and_provider_keys() {
        for key in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "NPM_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "GIT_ASKPASS",
            "SSH_AUTH_SOCK",
            "CODEFACTORY_DISPATCH_DIR",
            "CODEFACTORY_DISPATCH_TOKEN",
        ] {
            assert!(should_strip(key), "{key}");
        }
        for key in ["PATH", "HOME", "CARGO_HOME", "NO_PROXY", "TMPDIR"] {
            assert!(!should_strip(key), "{key}");
        }
    }
}
