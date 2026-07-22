// SPDX-License-Identifier: Apache-2.0
//! Tauri commands for remote Git collaboration (GitHub / GitLab).

use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::config::settings::{self, GitProvider, GitRemoteConfig};
use crate::git_remote::client::RemoteGitClient;
use crate::git_remote::{RemoteIssue, RemotePR, RemoteRepo};
use crate::AppState;

// ── Helpers ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GithubCliCredentialStatus {
    pub installed: bool,
    pub authenticated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitRemoteView {
    pub id: String,
    pub name: String,
    pub provider: GitProvider,
    pub base_url: String,
    pub token_ref: Option<String>,
    pub default_repo: Option<String>,
    pub has_token: bool,
}

#[derive(Clone, Deserialize)]
pub struct AddGitRemoteRequest {
    pub name: String,
    pub provider: GitProvider,
    pub base_url: String,
    pub token: String,
    pub default_repo: Option<String>,
}

fn remote_view(cfg: &GitRemoteConfig) -> GitRemoteView {
    GitRemoteView {
        id: cfg.id.clone(),
        name: cfg.name.clone(),
        provider: cfg.provider.clone(),
        base_url: cfg.base_url.clone(),
        token_ref: cfg.token_ref.clone(),
        default_repo: cfg.default_repo.clone(),
        has_token: cfg.token_ref.is_some() || !cfg.token.trim().is_empty(),
    }
}

fn make_client(cfg: &GitRemoteConfig) -> Result<RemoteGitClient, String> {
    let token = settings::resolve_git_remote_token(cfg).map_err(|e| e.to_string())?;
    Ok(RemoteGitClient::new(&cfg.base_url, &token, cfg.provider.clone()))
}

async fn find_remote(state: &AppState, remote_id: &str) -> Result<GitRemoteConfig, String> {
    let settings = state.settings.read().await;
    settings
        .git_remotes
        .iter()
        .find(|r| r.id == remote_id)
        .cloned()
        .ok_or_else(|| format!("Remote '{}' not found", remote_id))
}

// ── Remote config commands ────────────────────────────────────────────────────

#[tauri::command]
pub async fn github_cli_credential_status() -> GithubCliCredentialStatus {
    let status = crate::util::github_cli::auth_status("github.com");
    GithubCliCredentialStatus {
        installed: status.installed,
        authenticated: status.authenticated,
    }
}

#[tauri::command]
pub async fn list_git_remotes(
    state: State<'_, AppState>,
) -> Result<Vec<GitRemoteView>, String> {
    let settings = state.settings.read().await;
    Ok(settings.git_remotes.iter().map(remote_view).collect())
}

#[tauri::command]
pub async fn add_git_remote(
    config: AddGitRemoteRequest,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id = Uuid::new_v4().to_string();
    let token = config.token.trim().to_string();
    if token.is_empty() {
        return Err("Git remote token is required.".into());
    }
    let token_ref = settings::default_git_remote_token_ref(&id);
    crate::secrets::set_key(&token_ref, &token).map_err(|e| e.to_string())?;
    let config = GitRemoteConfig {
        id,
        name: config.name,
        provider: config.provider,
        base_url: config.base_url,
        token_ref: Some(token_ref),
        token: String::new(),
        default_repo: config.default_repo,
    };
    {
        let mut settings = state.settings.write().await;
        settings.git_remotes.push(config);
        crate::config::settings::save(&settings).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_git_remote(
    id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    {
        let mut settings = state.settings.write().await;
        if let Some(remote) = settings.git_remotes.iter().find(|r| r.id == id) {
            if let Some(token_ref) = &remote.token_ref {
                crate::secrets::delete_key(token_ref).map_err(|e| e.to_string())?;
            }
        }
        settings.git_remotes.retain(|r| r.id != id);
        crate::config::settings::save(&settings).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Test connectivity — returns authenticated username.
#[tauri::command]
pub async fn test_git_remote(
    id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let cfg = find_remote(&state, &id).await?;
    let client = make_client(&cfg)?;
    let v = client.get("/user").await?;
    let username = match cfg.provider {
        GitProvider::Github => v
            .get("login")
            .and_then(|x| x.as_str())
            .unwrap_or("(unknown)")
            .to_string(),
        GitProvider::Gitlab => v
            .get("username")
            .and_then(|x| x.as_str())
            .unwrap_or("(unknown)")
            .to_string(),
    };
    Ok(username)
}

// ── Issue commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_issues(
    remote_id: String,
    repo: String,
    state_filter: String,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteIssue>, String> {
    let cfg = find_remote(&state, &remote_id).await?;
    let client = make_client(&cfg)?;
    match cfg.provider {
        GitProvider::Github => crate::git_remote::github::list_issues(&client, &repo, &state_filter).await,
        GitProvider::Gitlab => crate::git_remote::gitlab::list_issues(&client, &repo, &state_filter).await,
    }
}

#[tauri::command]
pub async fn get_issue(
    remote_id: String,
    repo: String,
    number: u64,
    state: State<'_, AppState>,
) -> Result<RemoteIssue, String> {
    let cfg = find_remote(&state, &remote_id).await?;
    let client = make_client(&cfg)?;
    match cfg.provider {
        GitProvider::Github => crate::git_remote::github::get_issue(&client, &repo, number).await,
        GitProvider::Gitlab => crate::git_remote::gitlab::get_issue(&client, &repo, number).await,
    }
}

#[tauri::command]
pub async fn create_issue(
    remote_id: String,
    repo: String,
    title: String,
    body: String,
    labels: Vec<String>,
    state: State<'_, AppState>,
) -> Result<RemoteIssue, String> {
    let cfg = find_remote(&state, &remote_id).await?;
    let client = make_client(&cfg)?;
    match cfg.provider {
        GitProvider::Github => {
            crate::git_remote::github::create_issue(&client, &repo, &title, &body, &labels).await
        }
        GitProvider::Gitlab => {
            crate::git_remote::gitlab::create_issue(&client, &repo, &title, &body, &labels).await
        }
    }
}

// ── PR commands ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_prs(
    remote_id: String,
    repo: String,
    state_filter: String,
    state: State<'_, AppState>,
) -> Result<Vec<RemotePR>, String> {
    let cfg = find_remote(&state, &remote_id).await?;
    let client = make_client(&cfg)?;
    match cfg.provider {
        GitProvider::Github => crate::git_remote::github::list_prs(&client, &repo, &state_filter).await,
        GitProvider::Gitlab => crate::git_remote::gitlab::list_prs(&client, &repo, &state_filter).await,
    }
}

#[tauri::command]
pub async fn create_pr(
    remote_id: String,
    repo: String,
    title: String,
    body: String,
    head: String,
    base: String,
    draft: bool,
    state: State<'_, AppState>,
) -> Result<RemotePR, String> {
    let cfg = find_remote(&state, &remote_id).await?;
    let client = make_client(&cfg)?;
    match cfg.provider {
        GitProvider::Github => {
            crate::git_remote::github::create_pr(&client, &repo, &title, &body, &head, &base, draft).await
        }
        GitProvider::Gitlab => {
            crate::git_remote::gitlab::create_pr(&client, &repo, &title, &body, &head, &base, draft).await
        }
    }
}

// ── Repo listing ──────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_repos(
    remote_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<RemoteRepo>, String> {
    let cfg = find_remote(&state, &remote_id).await?;
    let client = make_client(&cfg)?;
    match cfg.provider {
        GitProvider::Github => crate::git_remote::github::list_repos(&client).await,
        GitProvider::Gitlab => crate::git_remote::gitlab::list_repos(&client).await,
    }
}
