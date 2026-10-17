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

pub async fn update_pr_body(
    client: &RemoteGitClient,
    repo: &str,
    number: u64,
    title: Option<&str>,
    body: &str,
) -> Result<(), String> {
    let mut payload = serde_json::json!({ "body": body });
    if let Some(title) = title {
        payload["title"] = serde_json::Value::String(title.to_string());
    }
    client
        .patch(&format!("/repos/{repo}/pulls/{number}"), payload)
        .await
        .map(|_| ())
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
    expected_head: &str,
) -> Result<String, String> {
    let path = format!("/repos/{}/pulls/{}/merge", repo, number);
    let mut payload = merge_payload(method, commit_title, commit_body);
    payload["sha"] = Value::String(expected_head.to_string());
    let merged = client.put(&path, payload).await?;

    let sha = str_val(&merged, "sha");
    if sha.is_empty() {
        return Err("merge succeeded but GitHub returned no merge commit SHA".into());
    }
    if method != "squash" {
        return Ok(sha);
    }
    let Some(expected_message) = commit_body else {
        return Ok(sha);
    };
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
    Ok(sha)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiObservation {
    pub status: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredStatusCheck {
    pub context: String,
    pub integration_id: Option<u64>,
}

pub(crate) fn parse_required_status_checks(value: &Value) -> Vec<RequiredStatusCheck> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|rule| rule.get("type").and_then(Value::as_str) == Some("required_status_checks"))
        .flat_map(|rule| {
            rule.get("parameters")
                .and_then(|parameters| parameters.get("required_status_checks"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|check| {
            let context = check.get("context").and_then(Value::as_str)?.to_string();
            Some(RequiredStatusCheck {
                context,
                integration_id: check.get("integration_id").and_then(Value::as_u64),
            })
        })
        .collect()
}

fn run_identity(run: &Value) -> (String, Option<u64>) {
    (
        str_val(run, "name"),
        run.get("app")
            .and_then(|app| app.get("id"))
            .and_then(Value::as_u64),
    )
}

/// M108: keep only the newest check-run per (context, GitHub App) and drop runs
/// that are not a required context. Shared by the one-line CI status and the
/// per-check decision so both read exactly the same snapshot.
fn latest_relevant_runs(
    value: &Value,
    required_checks: &[RequiredStatusCheck],
) -> (Vec<Value>, Vec<String>) {
    let raw_runs = value
        .get("check_runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut latest_runs: Vec<Value> = Vec::new();
    for run in raw_runs {
        let identity = run_identity(&run);
        if let Some(index) = latest_runs
            .iter()
            .position(|existing| run_identity(existing) == identity)
        {
            let current_id = latest_runs[index]
                .get("id")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let candidate_id = run.get("id").and_then(Value::as_u64).unwrap_or(0);
            if candidate_id >= current_id {
                latest_runs[index] = run;
            }
        } else {
            latest_runs.push(run);
        }
    }
    let mut runs = if required_checks.is_empty() {
        latest_runs
    } else {
        latest_runs
            .into_iter()
            .filter(|run| {
                let (name, integration_id) = run_identity(run);
                required_checks.iter().any(|required| {
                    name == required.context
                        && required
                            .integration_id
                            .map_or(true, |expected| integration_id == Some(expected))
                })
            })
            .collect()
    };
    runs.sort_by_key(|run| str_val(run, "name"));
    let missing_required = required_checks
        .iter()
        .filter(|required| {
            !runs.iter().any(|run| {
                let (name, integration_id) = run_identity(run);
                name == required.context
                    && required
                        .integration_id
                        .map_or(true, |expected| integration_id == Some(expected))
            })
        })
        .map(|required| required.context.clone())
        .collect::<Vec<_>>();
    (runs, missing_required)
}

fn is_passing_conclusion(conclusion: &str) -> bool {
    matches!(conclusion, "success" | "neutral" | "skipped")
}

/// M108/R2: one failed check as the decision model sees it — the job name, the
/// trust-root marker when the gate refused a self-modification, and the failing
/// test name when the log named one.
fn failed_check_detail(run: &Value, name: &str, conclusion: &str) -> String {
    let url = str_val(run, "details_url");
    let mut detail = if url.is_empty() {
        format!("failure:{name}:{conclusion}")
    } else {
        format!("failure:{name}:{conclusion} [{url}]")
    };
    let output = run.get("output");
    let output_text = |key: &str| {
        output
            .and_then(|output| output.get(key))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    if ["title", "summary", "text"]
        .iter()
        .any(|key| output_text(key).contains(crate::agent::objective::TRUST_ROOT_SELF_MODIFY_MARKER))
    {
        detail.push_str(&format!(
            " ({})",
            crate::agent::objective::TRUST_ROOT_SELF_MODIFY_MARKER
        ));
    }
    if let Some(test) = ["text", "summary", "title"]
        .iter()
        .find_map(|key| crate::agent::objective::failing_test_name(output_text(key)))
    {
        detail.push_str(&format!(" — test {test} ... FAILED"));
    }
    detail
}

/// M108: the full per-check snapshot of one PR head. The delivery outcome is
/// decided from this list, so a trust-root refusal on one required check can
/// never erase a red test running beside it.
pub(crate) fn ci_check_states(
    value: &Value,
    required_checks: &[RequiredStatusCheck],
) -> Vec<crate::agent::objective::CiCheckState> {
    let (runs, missing_required) = latest_relevant_runs(value, required_checks);
    let mut states = runs
        .iter()
        .map(|run| {
            let job = str_val(run, "name");
            let status = str_val(run, "status");
            let conclusion = run
                .get("conclusion")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let detail = match conclusion.as_deref() {
                Some(passing) if is_passing_conclusion(passing) => None,
                Some(failed) if status == "completed" => {
                    Some(failed_check_detail(run, &job, failed))
                }
                _ => None,
            };
            crate::agent::objective::CiCheckState {
                job,
                status,
                conclusion,
                detail,
            }
        })
        .collect::<Vec<_>>();
    for context in missing_required {
        states.push(crate::agent::objective::CiCheckState {
            job: context,
            status: "queued".into(),
            conclusion: None,
            detail: None,
        });
    }
    states.sort_by(|left, right| left.job.cmp(&right.job));
    states
}

pub(crate) fn classify_ci_observation(
    value: &Value,
    required_checks: &[RequiredStatusCheck],
) -> CiObservation {
    let (runs, missing_required) = latest_relevant_runs(value, required_checks);
    let missing_required = missing_required
        .into_iter()
        .map(|context| format!("{context}:absent"))
        .collect::<Vec<_>>();
    let fingerprint = if runs.is_empty() && missing_required.is_empty() {
        "none".to_string()
    } else {
        let mut parts = runs
            .iter()
            .map(|run| {
                format!(
                    "{}:{}:{}:{}",
                    str_val(run, "name"),
                    str_val(run, "status"),
                    str_val(run, "conclusion"),
                    run.get("id").and_then(Value::as_u64).unwrap_or(0)
                )
            })
            .collect::<Vec<_>>();
        parts.extend(missing_required.iter().cloned());
        parts.sort();
        parts.join("|")
    };
    if runs.is_empty() && required_checks.is_empty() {
        return CiObservation {
            status: "none".into(),
            fingerprint,
        };
    }
    if !missing_required.is_empty() {
        return CiObservation {
            status: "pending".into(),
            fingerprint,
        };
    }
    let mut any_pending = false;
    // M108/R2: report *every* failed check on this head. Returning the first one
    // let a trust-root refusal on one required check hide a red test running
    // beside it (#713: `check-rust` panicked while `scenario-gate-pr` refused a
    // registry edit), so the whole run was read as "wait for the orchestrator".
    let mut failures = Vec::new();
    for run in &runs {
        let status = str_val(run, "status");
        if status != "completed" {
            any_pending = true;
            continue;
        }
        let conclusion = str_val(run, "conclusion");
        if is_passing_conclusion(&conclusion) {
            continue;
        }
        failures.push(failed_check_detail(run, &str_val(run, "name"), &conclusion));
    }
    if !failures.is_empty() {
        return CiObservation {
            status: failures.join("；"),
            fingerprint,
        };
    }
    CiObservation {
        status: if any_pending { "pending" } else { "success" }.into(),
        fingerprint,
    }
}

/// CI conclusion for a commit, from the GitHub Actions check-runs API.
///   - any queued/in_progress            → "pending"
///   - any failure/cancelled/timed_out   → "failure"
///   - all success/neutral/skipped       → "success"
///   - zero check runs                    → "none" (no CI configured)
pub async fn ci_status(client: &RemoteGitClient, repo: &str, sha: &str) -> Result<String, String> {
    Ok(ci_observation(client, repo, sha, "main").await?.status)
}

pub async fn ci_observation(
    client: &RemoteGitClient,
    repo: &str,
    sha: &str,
    base_branch: &str,
) -> Result<CiObservation, String> {
    let path = format!("/repos/{}/commits/{}/check-runs", repo, sha);
    let value = client.get(&path).await?;
    let rules = client
        .get(&format!("/repos/{repo}/rules/branches/{base_branch}"))
        .await
        .unwrap_or(Value::Null);
    let required = parse_required_status_checks(&rules);
    Ok(classify_ci_observation(&value, &required))
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

/// M108: the shape of a real PR head, taken from objective #713 (session
/// cd8d910e). Both required checks concluded red at the same moment: an
/// ordinary Windows test panic in `check-rust`, and the expected trust-root
/// refusal in `scenario-gate-pr`. Reporting only one of them loses the fact
/// that the session still has work to do, so this fixture is the regression
/// shape for the decision the waiting session receives.
#[cfg(test)]
mod m108_ci_shape_tests {
    use super::*;
    use crate::agent::objective::{classify_ci_checks, ci_outcome_wake_prompt, CiOutcome};

    fn required() -> Vec<RequiredStatusCheck> {
        vec![
            RequiredStatusCheck {
                context: "check-rust".into(),
                integration_id: None,
            },
            RequiredStatusCheck {
                context: "scenario-gate-pr".into(),
                integration_id: None,
            },
        ]
    }

    fn pr_713_head() -> Value {
        json!({
            "check_runs": [
                {
                    "id": 9001,
                    "name": "check-rust",
                    "status": "completed",
                    "conclusion": "failure",
                    "details_url": "https://github.com/BumStill/CodeFactory/runs/9001",
                    "output": {
                        "title": "lifecycle_smoke panicked at lifecycle_smoke.rs:141:46",
                        "summary": "test windows_worktree_target_is_shared ... FAILED",
                        "text": "test windows_worktree_target_is_shared ... FAILED\n\n---- windows_worktree_target_is_shared stdout ----\nthread 'windows_worktree_target_is_shared' panicked at lifecycle_smoke.rs:141:46"
                    }
                },
                {
                    "id": 9002,
                    "name": "scenario-gate-pr",
                    "status": "completed",
                    "conclusion": "failure",
                    "details_url": "https://github.com/BumStill/CodeFactory/runs/9002",
                    "output": {
                        "title": "trust roots cannot self-modify",
                        "summary": "cannot self-modify: docs/testing/scenario-registry.json",
                        "text": ""
                    }
                }
            ]
        })
    }

    #[test]
    fn a_trust_root_refusal_must_not_hide_the_red_check_beside_it() {
        let value = pr_713_head();
        let observation = classify_ci_observation(&value, &required());
        assert!(
            observation.status.contains("check-rust"),
            "the ordinary failure must survive the report: {}",
            observation.status
        );
        assert!(
            observation.status.contains("scenario-gate-pr"),
            "the trust-root refusal must be reported too: {}",
            observation.status
        );
        assert!(
            observation.status.contains("windows_worktree_target_is_shared"),
            "the failing test name must reach the session: {}",
            observation.status
        );

        let outcome = classify_ci_checks(&ci_check_states(&value, &required()), 0);
        match &outcome {
            CiOutcome::Failed(failures) => assert_eq!(
                failures.iter().map(|check| check.job.as_str()).collect::<Vec<_>>(),
                vec!["check-rust", "scenario-gate-pr"],
                "both red checks belong to the failure evidence"
            ),
            other => panic!("mixed failures must stay repairable, got {other:?}"),
        }
        assert!(outcome.is_session_repairable());
        let prompt = ci_outcome_wake_prompt("PR #713", &outcome);
        assert!(prompt.contains("check-rust"), "{prompt}");
        assert!(
            prompt.contains("windows_worktree_target_is_shared"),
            "the failing test name must be in the wake input: {prompt}"
        );
        assert!(
            !prompt.contains("等编排方放行"),
            "an ordinary failure is not the orchestrator's to release: {prompt}"
        );
    }

    #[test]
    fn a_trust_root_refusal_alone_still_waits_for_the_orchestrator() {
        let value = json!({
            "check_runs": [
                {"id": 9101, "name": "check-rust", "status": "completed", "conclusion": "success"},
                {
                    "id": 9102,
                    "name": "scenario-gate-pr",
                    "status": "completed",
                    "conclusion": "failure",
                    "output": {"title": "trust roots cannot self-modify"}
                }
            ]
        });
        let observation = classify_ci_observation(&value, &required());
        assert!(observation.status.contains("cannot self-modify"), "{}", observation.status);
        let outcome = classify_ci_checks(&ci_check_states(&value, &required()), 0);
        assert!(matches!(outcome, CiOutcome::TrustRootOnly(_)), "{outcome:?}");
        assert!(!outcome.is_session_repairable());
        let prompt = ci_outcome_wake_prompt("PR #713", &outcome);
        assert!(prompt.contains("等编排方放行"), "{prompt}");
    }

    #[test]
    fn a_green_head_wakes_the_session_to_the_arm_step() {
        let value = json!({
            "check_runs": [
                {"id": 9201, "name": "check-rust", "status": "completed", "conclusion": "success"},
                {"id": 9202, "name": "scenario-gate-pr", "status": "completed", "conclusion": "success"}
            ]
        });
        assert_eq!(
            classify_ci_observation(&value, &required()).status,
            "success",
            "green stays green"
        );
        let outcome = classify_ci_checks(&ci_check_states(&value, &required()), 0);
        assert_eq!(outcome, CiOutcome::AllGreen);
        let prompt = ci_outcome_wake_prompt("PR #713", &outcome);
        assert!(prompt.contains("merge-queue: armed"), "{prompt}");
    }
}
