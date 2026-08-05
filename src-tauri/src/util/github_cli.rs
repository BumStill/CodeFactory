// SPDX-License-Identifier: Apache-2.0
//! GitHub CLI authentication adapter.
//!
//! `gh` owns and persists its credential. CodeFactory only asks it for the
//! active token at the moment a GitHub operation starts; the token is never
//! logged or copied into application settings.

use serde::Serialize;
use std::process::Command;

use crate::util::command_env;
use crate::util::no_window::NoWindow;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GithubCliAuthStatus {
    pub installed: bool,
    pub authenticated: bool,
}

fn gh_command() -> Command {
    let mut command = Command::new(command_env::resolve_developer_command("gh")).no_window();
    command_env::apply_developer_path_std(&mut command);
    command
}

/// Remaining core API quota for the authenticated user, or `None` when it
/// cannot be read.
///
/// `/rate_limit` is explicitly exempt from GitHub's own rate limit, so asking
/// costs nothing. Best-effort: not knowing must never fail or slow the command
/// the caller actually wanted to run.
pub fn read_core_quota() -> Option<(u32, u32)> {
    let out = gh_command()
        .args([
            "api",
            "rate_limit",
            "--jq",
            ".resources.core.remaining,.resources.core.limit",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let remaining = lines.next()?.trim().parse().ok()?;
    let limit = lines.next()?.trim().parse().ok()?;
    Some((remaining, limit))
}

fn parse_token(success: bool, stdout: &[u8]) -> Option<String> {
    if !success {
        return None;
    }
    let token = String::from_utf8_lossy(stdout).trim().to_owned();
    (!token.is_empty()).then_some(token)
}

/// Read the active token from GitHub CLI's own credential store.
///
/// Deliberately returns only `Option`: callers must never include command
/// output in an error because stdout contains the secret.
pub fn auth_token(hostname: &str) -> Option<String> {
    let output = gh_command()
        .args(["auth", "token", "--hostname", hostname])
        .output()
        .ok()?;
    parse_token(output.status.success(), &output.stdout)
}

pub fn auth_status(hostname: &str) -> GithubCliAuthStatus {
    let installed = gh_command()
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    let authenticated = installed && auth_token(hostname).is_some();
    GithubCliAuthStatus {
        installed,
        authenticated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_parser_requires_success_and_non_empty_output() {
        assert_eq!(
            parse_token(true, b"gho_secret\n"),
            Some("gho_secret".into())
        );
        assert_eq!(parse_token(true, b"  \n"), None);
        assert_eq!(parse_token(false, b"gho_must_not_be_used"), None);
    }

    #[test]
    fn live_cli_adapter_reads_authenticated_token_when_required() {
        if std::env::var_os("CODEFACTORY_EXPECT_GH_AUTH").is_none() {
            return;
        }
        let status = auth_status("github.com");
        assert!(status.installed, "GitHub CLI must be installed");
        assert!(status.authenticated, "GitHub CLI must be authenticated");
        assert!(auth_token("github.com").is_some());
    }
}
