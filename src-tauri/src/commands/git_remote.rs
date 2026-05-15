// SPDX-License-Identifier: Apache-2.0
//! Tauri commands for remote Git collaboration (GitHub / GitLab).

use chrono::Utc;
use std::path::PathBuf;
use tauri::State;
use uuid::Uuid;

use crate::config::settings::{GitProvider, GitRemoteConfig};
use crate::git_remote::client::RemoteGitClient;
use crate::git_remote::{RemoteIssue, RemotePR, RemoteRepo};
use crate::AppState;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_client(cfg: &GitRemoteConfig) -> RemoteGitClient {
    RemoteGitClient::new(&cfg.base_url, &cfg.token, cfg.provider.clone())
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
pub async fn list_git_remotes(
    state: State<'_, AppState>,
) -> Result<Vec<GitRemoteConfig>, String> {
    let settings = state.settings.read().await;
    Ok(settings.git_remotes.clone())
}

#[tauri::command]
pub async fn add_git_remote(
    mut config: GitRemoteConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Assign a new id if empty
    if config.id.is_empty() {
        config.id = Uuid::new_v4().to_string();
    }
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
    let client = make_client(&cfg);
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
    let client = make_client(&cfg);
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
    let client = make_client(&cfg);
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
    let client = make_client(&cfg);
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
    let client = make_client(&cfg);
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
    let client = make_client(&cfg);
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
    let client = make_client(&cfg);
    match cfg.provider {
        GitProvider::Github => crate::git_remote::github::list_repos(&client).await,
        GitProvider::Gitlab => crate::git_remote::gitlab::list_repos(&client).await,
    }
}

// ── issue_to_spec ─────────────────────────────────────────────────────────────

/// Fetch a remote issue and convert it to a spec file saved in `.codefactory/specs/`.
/// Returns the absolute path to the created file.
#[tauri::command]
pub async fn issue_to_spec(
    remote_id: String,
    repo: String,
    issue_number: u64,
    cwd: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let cfg = find_remote(&state, &remote_id).await?;
    let client = make_client(&cfg);
    let issue = match cfg.provider {
        GitProvider::Github => crate::git_remote::github::get_issue(&client, &repo, issue_number).await?,
        GitProvider::Gitlab => crate::git_remote::gitlab::get_issue(&client, &repo, issue_number).await?,
    };

    // Build a slug from the title
    let slug: String = issue
        .title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.len() > 40 { slug[..40].to_string() } else { slug };

    let specs_dir = PathBuf::from(&cwd)
        .join(".codefactory")
        .join("specs");
    std::fs::create_dir_all(&specs_dir)
        .map_err(|e| format!("Could not create specs dir: {}", e))?;

    let filename = format!("issue-{}-{}.md", issue.number, slug);
    let file_path = specs_dir.join(&filename);

    let now = Utc::now().to_rfc3339();
    let req_id = format!("GH-{}", issue.number);
    let labels_str = issue.labels.join(", ");

    let content = format!(
        r#"---
req_id: {req_id}
title: {title}
status: draft
created_at: {now}
updated_at: {now}
tags:
  - github-issue
  - {provider}
acceptance_criteria: []
---

## Overview

Imported from {provider} issue #{number}: [{title}]({url})

**Author:** {author}
**Labels:** {labels}
**State:** {state}
**Created:** {created_at}

---

## Issue Description

{body}

---

## Requirements

<!-- Derived from issue body above. Refine as needed. -->

| # | Requirement | Priority |
|---|-------------|----------|
| 1 | (to be defined) | High |

---

## Decision Points

<!-- Add <!-- DECISION: ... --> comments here for ambiguous areas -->

---

## Testing Matrix

| Scenario | Expected | Status |
|----------|----------|--------|
| (to be defined) | | |
"#,
        req_id = req_id,
        title = issue.title,
        now = now,
        provider = match cfg.provider { GitProvider::Github => "github", GitProvider::Gitlab => "gitlab" },
        number = issue.number,
        url = issue.url,
        author = issue.author,
        labels = labels_str,
        state = issue.state,
        created_at = issue.created_at,
        body = if issue.body.is_empty() { "(no description provided)".to_string() } else { issue.body.clone() },
    );

    std::fs::write(&file_path, &content)
        .map_err(|e| format!("Could not write spec file: {}", e))?;

    Ok(file_path.to_string_lossy().into_owned())
}
