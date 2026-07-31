// SPDX-License-Identifier: Apache-2.0
//! GitHub REST API adapter.

use serde_json::{json, Value};

use super::client::RemoteGitClient;
use super::{RemoteIssue, RemotePR, RemoteRepo};

fn str_val(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn u64_val(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn bool_val(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn parse_issue(v: &Value) -> RemoteIssue {
    let labels: Vec<String> = v
        .get("labels")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let author = v
        .get("user")
        .and_then(|u| u.get("login"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    RemoteIssue {
        id: u64_val(v, "id"),
        number: u64_val(v, "number"),
        title: str_val(v, "title"),
        body: str_val(v, "body"),
        state: str_val(v, "state"),
        labels,
        created_at: str_val(v, "created_at"),
        updated_at: str_val(v, "updated_at"),
        url: str_val(v, "html_url"),
        author,
    }
}

fn parse_pr(v: &Value) -> RemotePR {
    let base_branch = v
        .get("base")
        .and_then(|b| b.get("ref"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let head_branch = v
        .get("head")
        .and_then(|h| h.get("ref"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // GitHub: state is "open"/"closed"; merged PRs have merged_at set
    let mut state = str_val(v, "state");
    if state == "closed" && v.get("merged_at").and_then(Value::as_str).is_some() {
        state = "merged".to_string();
    }

    RemotePR {
        id: u64_val(v, "id"),
        number: u64_val(v, "number"),
        title: str_val(v, "title"),
        body: str_val(v, "body"),
        state,
        base_branch,
        head_branch,
        created_at: str_val(v, "created_at"),
        url: str_val(v, "html_url"),
        draft: bool_val(v, "draft"),
    }
}

pub async fn list_issues(
    client: &RemoteGitClient,
    repo: &str,
    state: &str,
) -> Result<Vec<RemoteIssue>, String> {
    let path = format!("/repos/{}/issues?state={}&per_page=100", repo, state);
    let v = client.get(&path).await?;
    let arr = v.as_array().ok_or("Expected array")?;
    // GitHub issues endpoint includes PRs — filter them out
    Ok(arr
        .iter()
        .filter(|i| i.get("pull_request").is_none())
        .map(parse_issue)
        .collect())
}

pub async fn create_issue(
    client: &RemoteGitClient,
    repo: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<RemoteIssue, String> {
    let path = format!("/repos/{}/issues", repo);
    let payload = json!({
        "title": title,
        "body": body,
        "labels": labels,
    });
    let v = client.post(&path, payload).await?;
    Ok(parse_issue(&v))
}

pub async fn list_prs(
    client: &RemoteGitClient,
    repo: &str,
    state: &str,
) -> Result<Vec<RemotePR>, String> {
    let path = format!("/repos/{}/pulls?state={}&per_page=100", repo, state);
    let v = client.get(&path).await?;
    let arr = v.as_array().ok_or("Expected array")?;
    Ok(arr.iter().map(parse_pr).collect())
}

pub async fn create_pr(
    client: &RemoteGitClient,
    repo: &str,
    title: &str,
    body: &str,
    head: &str,
    base: &str,
    draft: bool,
) -> Result<RemotePR, String> {
    let path = format!("/repos/{}/pulls", repo);
    let payload = json!({
        "title": title,
        "body": body,
        "head": head,
        "base": base,
        "draft": draft,
    });
    let v = client.post(&path, payload).await?;
    Ok(parse_pr(&v))
}

/// Merge a PR. `method` is one of "squash" | "merge" | "rebase". A 405 from a
/// protected branch / required review surfaces as the REST error verbatim.
pub async fn merge_pr(
    client: &RemoteGitClient,
    repo: &str,
    number: u64,
    method: &str,
    commit_title: Option<&str>,
    commit_body: Option<&str>,
) -> Result<(), String> {
    let path = format!("/repos/{}/pulls/{}/merge", repo, number);
    let payload = merge_payload(method, commit_title, commit_body);
    let merged = client.put(&path, payload).await?;

    if method != "squash" {
        return Ok(());
    }
    let Some(expected_message) = commit_body else {
        return Ok(());
    };
    let sha = str_val(&merged, "sha");
    if sha.is_empty() {
        return Err("squash merge succeeded but GitHub returned no merge commit SHA".into());
    }
    let commit = client.get(&format!("/repos/{repo}/commits/{sha}")).await?;
    let message = commit
        .get("commit")
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let missing = missing_release_metadata(expected_message, message);
    if !missing.is_empty() {
        return Err(format!(
            "squash merge commit {sha} lost release metadata: {}",
            missing.join(", ")
        ));
    }
    Ok(())
}

fn merge_payload(method: &str, commit_title: Option<&str>, commit_body: Option<&str>) -> Value {
    let mut payload = json!({ "merge_method": method });
    if method == "squash" {
        if let Some(title) = commit_title {
            payload["commit_title"] = Value::String(title.to_string());
        }
        if let Some(body) = commit_body {
            payload["commit_message"] = Value::String(body.to_string());
        }
    }
    payload
}

fn release_urgency_trailers(message: &str) -> Vec<String> {
    let lines: Vec<&str> = message.trim_end().lines().collect();
    let start = lines
        .iter()
        .rposition(|line| line.trim().is_empty())
        .map(|index| index + 1)
        .unwrap_or(0);
    lines[start..]
        .iter()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("Release-Urgency")
                .then(|| value.trim().to_ascii_lowercase())
        })
        .collect()
}

fn breaking_change_trailers(message: &str) -> Vec<String> {
    let lines: Vec<&str> = message.trim_end().lines().collect();
    let start = lines
        .iter()
        .rposition(|line| line.trim().is_empty())
        .map(|index| index + 1)
        .unwrap_or(0);
    lines[start..]
        .iter()
        .filter_map(|line| {
            let line = line.trim();
            (line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:"))
                .then(|| line.to_string())
        })
        .collect()
}

fn missing_release_metadata(expected_message: &str, actual_message: &str) -> Vec<String> {
    let expected_urgencies = release_urgency_trailers(expected_message);
    let actual_urgencies = release_urgency_trailers(actual_message);
    let expected_breaking_changes = breaking_change_trailers(expected_message);
    let actual_breaking_changes = breaking_change_trailers(actual_message);

    let mut missing: Vec<String> = expected_urgencies
        .iter()
        .filter(|value| !actual_urgencies.contains(value))
        .map(|value| format!("Release-Urgency: {value}"))
        .collect();
    missing.extend(
        expected_breaking_changes
            .iter()
            .filter(|value| !actual_breaking_changes.contains(value))
            .cloned(),
    );
    missing
}

#[cfg(test)]
mod release_policy_tests {
    use super::*;

    #[test]
    fn squash_payload_carries_the_final_commit_message() {
        let payload = merge_payload(
            "squash",
            Some("fix: guarded"),
            Some("Details\n\nBREAKING CHANGE: migration required\nRelease-Urgency: hold"),
        );
        assert_eq!(payload["merge_method"], "squash");
        assert_eq!(payload["commit_title"], "fix: guarded");
        assert_eq!(
            payload["commit_message"],
            "Details\n\nBREAKING CHANGE: migration required\nRelease-Urgency: hold"
        );
        assert_eq!(
            release_urgency_trailers(payload["commit_message"].as_str().unwrap()),
            vec!["hold"]
        );
        assert_eq!(
            breaking_change_trailers(payload["commit_message"].as_str().unwrap()),
            vec!["BREAKING CHANGE: migration required"]
        );
        assert!(missing_release_metadata(
            payload["commit_message"].as_str().unwrap(),
            payload["commit_message"].as_str().unwrap(),
        )
        .is_empty());
        assert_eq!(
            missing_release_metadata(
                payload["commit_message"].as_str().unwrap(),
                "Details\n\nRelease-Urgency: hold",
            ),
            vec!["BREAKING CHANGE: migration required"]
        );
    }

    #[test]
    fn non_squash_payload_does_not_rewrite_commit_messages() {
        let payload = merge_payload("merge", Some("fix: guarded"), Some("Release-Urgency: hold"));
        assert!(payload.get("commit_title").is_none());
        assert!(payload.get("commit_message").is_none());
    }
}

/// CI conclusion for a commit, from the GitHub Actions check-runs API.
///   - any queued/in_progress            → "pending"
///   - any failure/cancelled/timed_out   → "failure"
///   - all success/neutral/skipped       → "success"
///   - zero check runs                    → "none" (no CI configured)
pub async fn ci_status(client: &RemoteGitClient, repo: &str, sha: &str) -> Result<String, String> {
    let path = format!("/repos/{}/commits/{}/check-runs", repo, sha);
    let v = client.get(&path).await?;
    let runs = v
        .get("check_runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if runs.is_empty() {
        return Ok("none".into());
    }
    let mut any_pending = false;
    for run in &runs {
        let status = str_val(run, "status"); // queued | in_progress | completed
        if status != "completed" {
            any_pending = true;
            continue;
        }
        match str_val(run, "conclusion").as_str() {
            "success" | "neutral" | "skipped" => {}
            other => return Ok(format!("failure:{other}")),
        }
    }
    Ok(if any_pending {
        "pending".into()
    } else {
        "success".into()
    })
}

pub async fn list_repos(client: &RemoteGitClient) -> Result<Vec<RemoteRepo>, String> {
    let v = client.get("/user/repos?per_page=100&sort=updated").await?;
    let arr = v.as_array().ok_or("Expected array")?;
    Ok(arr
        .iter()
        .map(|r| RemoteRepo {
            full_name: str_val(r, "full_name"),
            description: str_val(r, "description"),
            default_branch: str_val(r, "default_branch"),
            url: str_val(r, "html_url"),
            private: bool_val(r, "private"),
            stars: u64_val(r, "stargazers_count"),
        })
        .collect())
}
