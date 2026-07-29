// SPDX-License-Identifier: Apache-2.0
//! Tauri commands for remote Git collaboration (GitHub / GitLab).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use tauri::State;
use uuid::Uuid;

use crate::config::settings::{self, GitProvider, GitRemoteConfig};
use crate::git_remote::client::RemoteGitClient;
use crate::git_remote::{RemoteIssue, RemotePR, RemoteRepo};
use crate::util::no_window::NoWindow;
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
    Ok(RemoteGitClient::new(
        &cfg.base_url,
        &token,
        cfg.provider.clone(),
    ))
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
pub async fn list_git_remotes(state: State<'_, AppState>) -> Result<Vec<GitRemoteView>, String> {
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
pub async fn delete_git_remote(id: String, state: State<'_, AppState>) -> Result<(), String> {
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
pub async fn test_git_remote(id: String, state: State<'_, AppState>) -> Result<String, String> {
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
        GitProvider::Github => {
            crate::git_remote::github::list_issues(&client, &repo, &state_filter).await
        }
        GitProvider::Gitlab => {
            crate::git_remote::gitlab::list_issues(&client, &repo, &state_filter).await
        }
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
        GitProvider::Github => {
            crate::git_remote::github::list_prs(&client, &repo, &state_filter).await
        }
        GitProvider::Gitlab => {
            crate::git_remote::gitlab::list_prs(&client, &repo, &state_filter).await
        }
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
            crate::git_remote::github::create_pr(&client, &repo, &title, &body, &head, &base, draft)
                .await
        }
        GitProvider::Gitlab => {
            crate::git_remote::gitlab::create_pr(&client, &repo, &title, &body, &head, &base, draft)
                .await
        }
    }
}

// ── Workspace delivery status ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceDeliveryPr {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub draft: bool,
    pub head_branch: String,
    pub base_branch: String,
    pub head_sha: String,
    pub merge_commit_sha: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceRelease {
    pub tag: String,
    pub url: String,
    pub published_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceDeliverySnapshot {
    pub remote_available: bool,
    pub pr: Option<WorkspaceDeliveryPr>,
    pub ci_status: String,
    pub release: Option<WorkspaceRelease>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryLookup<'a> {
    Number(u64),
    Branch(&'a str),
}

fn delivery_lookup(pr_number: u64, branch: &str) -> DeliveryLookup<'_> {
    if pr_number > 0 {
        DeliveryLookup::Number(pr_number)
    } else {
        DeliveryLookup::Branch(branch)
    }
}

fn json_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn parse_github_delivery_pr(value: &Value) -> WorkspaceDeliveryPr {
    let merged = value.get("merged_at").and_then(Value::as_str).is_some();
    WorkspaceDeliveryPr {
        number: value.get("number").and_then(Value::as_u64).unwrap_or(0),
        title: json_str(value, "title"),
        state: if merged {
            "merged".into()
        } else {
            json_str(value, "state")
        },
        draft: value.get("draft").and_then(Value::as_bool).unwrap_or(false),
        head_branch: value
            .get("head")
            .and_then(|v| v.get("ref"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        base_branch: value
            .get("base")
            .and_then(|v| v.get("ref"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        head_sha: value
            .get("head")
            .and_then(|v| v.get("sha"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        merge_commit_sha: value
            .get("merge_commit_sha")
            .and_then(Value::as_str)
            .map(String::from),
        url: json_str(value, "html_url"),
    }
}

fn compare_proves_release_contains_merge(status: &str) -> bool {
    matches!(status, "ahead" | "identical")
}

fn default_git_remote_name(cwd: &Path) -> String {
    let output = Command::new("git")
        .no_window()
        .arg("-C")
        .arg(cwd)
        .arg("remote")
        .output();
    let Ok(output) = output else {
        return "origin".into();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let remotes: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if remotes.contains(&"origin") {
        "origin".into()
    } else {
        remotes.first().copied().unwrap_or("origin").into()
    }
}

fn workspace_remote_url(cwd: &Path) -> Result<String, String> {
    let remote = default_git_remote_name(cwd);
    let output = Command::new("git")
        .no_window()
        .arg("-C")
        .arg(cwd)
        .args(["remote", "get-url", &remote])
        .output()
        .map_err(|error| format!("无法读取 Git remote {remote}: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn github_repo_from_remote_url(origin: &str) -> Result<(String, String), String> {
    let host = crate::agent::delivery::remote_host(origin)
        .ok_or_else(|| "无法从 Git remote 解析 host".to_string())?;
    if crate::agent::delivery::classify_forge(origin) != crate::agent::delivery::ForgeFamily::Github
    {
        let family = crate::agent::delivery::classify_forge(origin).label();
        return Err(format!(
            "当前 remote 是 {family}({host})，workspace_delivery_status 尚未内置该平台状态查询；请配置 delivery_provider hook，不能按 GitHub 状态判断上线。"
        ));
    }
    let repo = crate::agent::delivery::remote_repo_path(origin)
        .and_then(|path| {
            let mut parts = path.split('/');
            Some(format!("{}/{}", parts.next()?, parts.next()?))
        })
        .ok_or_else(|| "无法从 remote 解析 GitHub owner/repo".to_string())?;
    Ok((host, repo))
}

fn github_owner_repo(cwd: &Path) -> Result<(String, String), String> {
    let origin = workspace_remote_url(cwd)?;
    github_repo_from_remote_url(&origin)
}

async fn workspace_github_client(
    cwd: &Path,
    state: &AppState,
) -> Result<(RemoteGitClient, String), String> {
    let (host, repo) = github_owner_repo(cwd)?;
    if let Some(token) = crate::util::github_cli::auth_token(&host) {
        let base_url = if host == "github.com" {
            "https://api.github.com".to_string()
        } else {
            format!("https://{host}/api/v3")
        };
        return Ok((
            RemoteGitClient::new(&base_url, &token, GitProvider::Github),
            repo,
        ));
    }
    let settings = state.settings.read().await;
    let remote = settings
        .git_remotes
        .iter()
        .find(|remote| {
            matches!(remote.provider, GitProvider::Github)
                && remote.default_repo.as_deref() == Some(repo.as_str())
                && remote.base_url.contains(&host)
        })
        .or_else(|| {
            settings.git_remotes.iter().find(|remote| {
                matches!(remote.provider, GitProvider::Github)
                    && remote.default_repo.as_deref() == Some(repo.as_str())
            })
        })
        .or_else(|| {
            settings.git_remotes.iter().find(|remote| {
                matches!(remote.provider, GitProvider::Github) && remote.base_url.contains(&host)
            })
        })
        .ok_or_else(|| {
            let login = if host == "github.com" {
                "gh auth login".to_string()
            } else {
                format!("gh auth login --hostname {host}")
            };
            format!(
                "GitHub remote 状态不可用：请运行 `{login}`，或为 {host}/{repo} 配置远程仓库 token。"
            )
        })?;
    let token = settings::resolve_git_remote_token(remote).map_err(|error| error.to_string())?;
    Ok((
        RemoteGitClient::new(&remote.base_url, &token, GitProvider::Github),
        repo,
    ))
}

async fn find_workspace_pr(
    client: &RemoteGitClient,
    repo: &str,
    lookup: DeliveryLookup<'_>,
) -> Result<Option<WorkspaceDeliveryPr>, String> {
    let value = match lookup {
        DeliveryLookup::Number(number) => {
            client.get(&format!("/repos/{repo}/pulls/{number}")).await?
        }
        DeliveryLookup::Branch(branch) => {
            if branch.is_empty() || matches!(branch, "main" | "master") {
                return Ok(None);
            }
            let owner = repo.split('/').next().unwrap_or_default();
            let list = client
                .get(&format!("/repos/{repo}/pulls?state=all&head={owner}:{branch}&sort=updated&direction=desc&per_page=1"))
                .await?;
            let Some(value) = list.as_array().and_then(|items| items.first()).cloned() else {
                return Ok(None);
            };
            value
        }
    };
    Ok(Some(parse_github_delivery_pr(&value)))
}

#[tauri::command]
pub async fn workspace_delivery_status(
    cwd: String,
    session_id: Option<String>,
    branch: Option<String>,
    pr_number: Option<u64>,
    state: State<'_, AppState>,
) -> Result<WorkspaceDeliverySnapshot, String> {
    let stored = if let Some(session_id) = session_id.as_deref() {
        let db = state.db.read().await.clone();
        sqlx::query_as::<_, (String, i64)>(
            "SELECT branch, pr_number FROM session_delivery_refs WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&db)
        .await
        .map_err(|error| error.to_string())?
    } else {
        None
    };
    let effective_branch = stored
        .as_ref()
        .map(|row| row.0.clone())
        .or(branch)
        .unwrap_or_default();
    let effective_number = stored
        .map(|row| row.1.max(0) as u64)
        .or(pr_number)
        .unwrap_or(0);
    let (client, repo) = workspace_github_client(Path::new(&cwd), &state).await?;
    let Some(pr) = find_workspace_pr(
        &client,
        &repo,
        delivery_lookup(effective_number, &effective_branch),
    )
    .await?
    else {
        return Ok(WorkspaceDeliverySnapshot {
            remote_available: true,
            pr: None,
            ci_status: "none".into(),
            release: None,
            error: None,
        });
    };

    // CI belongs to the PR head commit, never to whichever branch is currently checked out.
    let ci_status = crate::git_remote::github::ci_status(&client, &repo, &pr.head_sha).await?;
    let release = if pr.state == "merged" {
        if let (Some(merge_sha), Ok(latest)) = (
            pr.merge_commit_sha.as_deref(),
            client.get(&format!("/repos/{repo}/releases/latest")).await,
        ) {
            let tag = json_str(&latest, "tag_name");
            let compare = client
                .get(&format!("/repos/{repo}/compare/{merge_sha}...{tag}"))
                .await;
            compare
                .ok()
                .filter(|value| compare_proves_release_contains_merge(&json_str(value, "status")))
                .map(|_| WorkspaceRelease {
                    tag,
                    url: json_str(&latest, "html_url"),
                    published_at: json_str(&latest, "published_at"),
                })
        } else {
            None
        }
    } else {
        None
    };

    Ok(WorkspaceDeliverySnapshot {
        remote_available: true,
        pr: Some(pr),
        ci_status,
        release,
        error: None,
    })
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

#[cfg(test)]
mod workspace_delivery_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn github_pr_snapshot_uses_head_sha_for_ci_and_real_merge_fields() {
        let snapshot = parse_github_delivery_pr(&json!({
            "number": 175,
            "title": "Improve workspace",
            "state": "closed",
            "draft": false,
            "html_url": "https://github.com/acme/repo/pull/175",
            "head": { "ref": "feat/workspace-ui", "sha": "head123" },
            "base": { "ref": "main" },
            "merged_at": "2026-07-23T10:00:00Z",
            "merge_commit_sha": "merge456"
        }));
        assert_eq!(snapshot.number, 175);
        assert_eq!(snapshot.state, "merged");
        assert_eq!(snapshot.head_sha, "head123");
        assert_eq!(snapshot.merge_commit_sha.as_deref(), Some("merge456"));
    }

    #[test]
    fn github_repo_from_remote_url_supports_enterprise_and_rejects_other_forges() {
        assert_eq!(
            github_repo_from_remote_url("git@github.corp.example:team/app.git").unwrap(),
            ("github.corp.example".into(), "team/app".into())
        );
        assert_eq!(
            github_repo_from_remote_url("https://github.com/acme/repo.git").unwrap(),
            ("github.com".into(), "acme/repo".into())
        );
        let err = github_repo_from_remote_url("git@gitlab.corp.example:platform/app.git")
            .expect_err("GitLab must not be handled as GitHub status");
        assert!(err.contains("GitLab"));
        assert!(err.contains("delivery_provider hook"));
        assert!(err.contains("不能按 GitHub 状态判断上线"));
    }

    #[test]
    fn release_is_only_live_when_tag_contains_the_pr_merge_commit() {
        assert!(compare_proves_release_contains_merge("ahead"));
        assert!(compare_proves_release_contains_merge("identical"));
        assert!(!compare_proves_release_contains_merge("behind"));
        assert!(!compare_proves_release_contains_merge("diverged"));
    }

    #[test]
    fn exact_pr_number_wins_over_current_branch_fallback() {
        assert_eq!(delivery_lookup(175, "main"), DeliveryLookup::Number(175));
        assert_eq!(
            delivery_lookup(0, "feat/workspace-ui"),
            DeliveryLookup::Branch("feat/workspace-ui")
        );
    }
}
