// SPDX-License-Identifier: Apache-2.0
//! GitLab REST API adapter.

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

/// URL-encode `owner/repo` → `owner%2Frepo`
fn encode_repo(repo: &str) -> String {
    repo.replace('/', "%2F")
}

fn parse_issue(v: &Value) -> RemoteIssue {
    let labels: Vec<String> = v
        .get("labels")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let author = v
        .get("author")
        .and_then(|u| u.get("username"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // GitLab uses "iid" for issue number within project
    let number = v.get("iid").and_then(Value::as_u64).unwrap_or(0);

    // GitLab state: "opened" | "closed"
    let state = match str_val(v, "state").as_str() {
        "opened" => "open".to_string(),
        s => s.to_string(),
    };

    RemoteIssue {
        id: u64_val(v, "id"),
        number,
        title: str_val(v, "title"),
        body: str_val(v, "description"),
        state,
        labels,
        created_at: str_val(v, "created_at"),
        updated_at: str_val(v, "updated_at"),
        url: str_val(v, "web_url"),
        author,
    }
}

fn parse_mr(v: &Value) -> RemotePR {
    let base_branch = str_val(v, "target_branch");
    let head_branch = str_val(v, "source_branch");
    let number = v.get("iid").and_then(Value::as_u64).unwrap_or(0);

    let state = match str_val(v, "state").as_str() {
        "opened" => "open".to_string(),
        "merged" => "merged".to_string(),
        s => s.to_string(),
    };

    RemotePR {
        id: u64_val(v, "id"),
        number,
        title: str_val(v, "title"),
        body: str_val(v, "description"),
        state,
        base_branch,
        head_branch,
        created_at: str_val(v, "created_at"),
        url: str_val(v, "web_url"),
        draft: bool_val(v, "draft"),
    }
}

pub async fn list_issues(
    client: &RemoteGitClient,
    repo: &str,
    state: &str,
) -> Result<Vec<RemoteIssue>, String> {
    let gl_state = if state == "open" { "opened" } else { state };
    let path = format!(
        "/projects/{}/issues?state={}&per_page=100",
        encode_repo(repo),
        gl_state
    );
    let v = client.get(&path).await?;
    let arr = v.as_array().ok_or("Expected array")?;
    Ok(arr.iter().map(parse_issue).collect())
}

pub async fn create_issue(
    client: &RemoteGitClient,
    repo: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<RemoteIssue, String> {
    let path = format!("/projects/{}/issues", encode_repo(repo));
    let payload = json!({
        "title": title,
        "description": body,
        "labels": labels.join(","),
    });
    let v = client.post(&path, payload).await?;
    Ok(parse_issue(&v))
}

pub async fn list_prs(
    client: &RemoteGitClient,
    repo: &str,
    state: &str,
) -> Result<Vec<RemotePR>, String> {
    let gl_state = if state == "open" { "opened" } else { state };
    let path = format!(
        "/projects/{}/merge_requests?state={}&per_page=100",
        encode_repo(repo),
        gl_state
    );
    let v = client.get(&path).await?;
    let arr = v.as_array().ok_or("Expected array")?;
    Ok(arr.iter().map(parse_mr).collect())
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
    let path = format!("/projects/{}/merge_requests", encode_repo(repo));
    let payload = json!({
        "title": title,
        "description": body,
        "source_branch": head,
        "target_branch": base,
        "draft": draft,
    });
    let v = client.post(&path, payload).await?;
    Ok(parse_mr(&v))
}

pub async fn list_repos(client: &RemoteGitClient) -> Result<Vec<RemoteRepo>, String> {
    let v = client
        .get("/projects?membership=true&per_page=100&order_by=last_activity_at")
        .await?;
    let arr = v.as_array().ok_or("Expected array")?;
    Ok(arr
        .iter()
        .map(|r| {
            let full_name = r
                .get("path_with_namespace")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            RemoteRepo {
                full_name,
                description: str_val(r, "description"),
                default_branch: str_val(r, "default_branch"),
                url: str_val(r, "web_url"),
                private: !bool_val(r, "public"),
                stars: u64_val(r, "star_count"),
            }
        })
        .collect())
}
