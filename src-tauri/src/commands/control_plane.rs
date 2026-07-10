// SPDX-License-Identifier: Apache-2.0
//! AI Coding OS control-plane snapshot.
//!
//! This is intentionally read-only in v1. It aggregates the authority surfaces,
//! memory proposal state, capability inventory, and delivery gates that already
//! exist across CodeFactory into one auditable view.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use tauri::{AppHandle, State};
use tokio::io::AsyncReadExt;

use crate::util::no_window::NoWindow;
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlPlaneStatus {
    Ok,
    Missing,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneItem {
    pub id: String,
    pub label: String,
    pub status: ControlPlaneStatus,
    pub path: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProposalSummary {
    pub pending: i64,
    pub accepted: i64,
    pub rejected: i64,
    pub preference_pending: i64,
    pub latest_pending: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySummary {
    pub id: String,
    pub label: String,
    pub total: usize,
    pub enabled: usize,
    pub status: ControlPlaneStatus,
    pub detail: String,
}

const GIT_PROBE_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GitProbeStatus {
    Ok,
    Partial,
    NotRepository,
    Unavailable,
    #[default]
    NotChecked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitProbeSummary {
    pub status: GitProbeStatus,
    pub timeout_ms: u64,
    pub timed_out: Vec<String>,
    pub failed: Vec<String>,
}

impl Default for GitProbeSummary {
    fn default() -> Self {
        Self {
            status: GitProbeStatus::NotChecked,
            timeout_ms: GIT_PROBE_TIMEOUT_MS,
            timed_out: Vec::new(),
            failed: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverySummary {
    pub git_branch: Option<String>,
    pub is_dirty: Option<bool>,
    pub dirty_count: Option<usize>,
    pub sync_gate_present: bool,
    pub sync_gate_configured: Option<bool>,
    pub release_workflow_present: bool,
    pub auto_release_present: bool,
    pub latest_release_tag: Option<String>,
    #[serde(default)]
    pub git_probe: GitProbeSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneRisk {
    pub id: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneSnapshot {
    pub generated_at: String,
    pub cwd: Option<String>,
    pub authority: Vec<ControlPlaneItem>,
    pub memory: MemoryProposalSummary,
    pub capabilities: Vec<CapabilitySummary>,
    pub delivery: DeliverySummary,
    pub risks: Vec<ControlPlaneRisk>,
}

fn status_for_path(path: &Path, present_detail: &str, missing_detail: &str) -> ControlPlaneItem {
    let exists = path.exists();
    ControlPlaneItem {
        id: path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("surface")
            .to_string(),
        label: path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("surface")
            .to_string(),
        status: if exists {
            ControlPlaneStatus::Ok
        } else {
            ControlPlaneStatus::Missing
        },
        path: Some(path.to_string_lossy().to_string()),
        detail: if exists {
            present_detail.to_string()
        } else {
            missing_detail.to_string()
        },
    }
}

fn authority_for_project(cwd: Option<&Path>) -> Vec<ControlPlaneItem> {
    let Some(cwd) = cwd else {
        return vec![ControlPlaneItem {
            id: "project-context".into(),
            label: "Project context".into(),
            status: ControlPlaneStatus::Warning,
            path: None,
            detail: "No active project; open a workspace to scan project authority surfaces."
                .into(),
        }];
    };

    let mut items = Vec::new();

    let mut agents = status_for_path(
        &cwd.join("AGENTS.md"),
        "Project agent rules are present.",
        "Project agent rules are missing.",
    );
    agents.id = "agents-md".into();
    agents.label = "AGENTS.md".into();
    items.push(agents);

    let mut repo_specs = status_for_path(
        &cwd.join("docs").join("specs"),
        "Long-lived repo specs are present.",
        "No docs/specs directory found.",
    );
    repo_specs.id = "repo-specs".into();
    repo_specs.label = "docs/specs".into();
    items.push(repo_specs);

    let project_specs_path = cwd.join(".codefactory").join("specs");
    let mut project_specs = status_for_path(
        &project_specs_path,
        "Project delivery contracts are present.",
        "No project delivery contracts yet.",
    );
    project_specs.id = "project-specs".into();
    project_specs.label = ".codefactory/specs".into();
    items.push(project_specs);

    let hook_path = cwd.join(".githooks").join("pre-commit");
    let mut sync_gate = status_for_path(
        &hook_path,
        "Versioned sync-before-commit hook is present.",
        "Sync-before-commit hook is missing.",
    );
    sync_gate.id = "sync-gate".into();
    sync_gate.label = ".githooks/pre-commit".into();
    items.push(sync_gate);

    let cadence_path = cwd
        .join("docs")
        .join("principles")
        .join("release-cadence.md");
    let mut cadence = status_for_path(
        &cadence_path,
        "Release cadence principle is present.",
        "Release cadence principle is missing.",
    );
    cadence.id = "release-cadence".into();
    cadence.label = "release cadence".into();
    items.push(cadence);

    items
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitCommandFailureKind {
    Timeout,
    NotRepository,
    Unavailable,
    Failed,
}

#[cfg(unix)]
fn isolate_process_tree(command: &mut tokio::process::Command) {
    use std::os::unix::process::CommandExt;

    command.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
fn isolate_process_tree(_command: &mut tokio::process::Command) {}

#[cfg(unix)]
async fn terminate_process_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // The child is the process-group leader, so a negative PID terminates
        // descendants that may still own the stdout/stderr pipes.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

#[cfg(windows)]
async fn terminate_process_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        let mut taskkill = tokio::process::Command::new("taskkill").no_window();
        taskkill.args(["/PID", &pid.to_string(), "/T", "/F"]);
        let _ = tokio::time::timeout(Duration::from_secs(1), taskkill.status()).await;
    }
    let _ = child.start_kill();
}

#[cfg(not(any(unix, windows)))]
async fn terminate_process_tree(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
}

async fn settle_reader(task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>) {
    if tokio::time::timeout(Duration::from_millis(500), &mut *task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

async fn terminate_and_reap(
    child: &mut tokio::process::Child,
    stdout_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) {
    terminate_process_tree(child).await;
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    settle_reader(stdout_task).await;
    settle_reader(stderr_task).await;
}

async fn process_output_with_timeout(
    mut command: tokio::process::Command,
    timeout_duration: Duration,
) -> Result<Output, GitCommandFailureKind> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_tree(&mut command);
    let mut child = match command.spawn() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(GitCommandFailureKind::Unavailable)
        }
        Err(_) => Err(GitCommandFailureKind::Failed),
        Ok(child) => Ok(child),
    }?;
    let mut stdout = child.stdout.take().ok_or(GitCommandFailureKind::Failed)?;
    let mut stderr = child.stderr.take().ok_or(GitCommandFailureKind::Failed)?;
    let mut stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let mut stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });

    let status = match tokio::time::timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(_)) => {
            terminate_and_reap(&mut child, &mut stdout_task, &mut stderr_task).await;
            return Err(GitCommandFailureKind::Failed);
        }
        Err(_) => {
            terminate_and_reap(&mut child, &mut stdout_task, &mut stderr_task).await;
            return Err(GitCommandFailureKind::Timeout);
        }
    };

    let stdout = stdout_task
        .await
        .map_err(|_| GitCommandFailureKind::Failed)?
        .map_err(|_| GitCommandFailureKind::Failed)?;
    let stderr = stderr_task
        .await
        .map_err(|_| GitCommandFailureKind::Failed)?
        .map_err(|_| GitCommandFailureKind::Failed)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn git_command_output(
    cwd: &Path,
    args: &[&str],
    allow_missing: bool,
) -> Result<Option<String>, GitCommandFailureKind> {
    let mut command = tokio::process::Command::new("git").no_window();
    command
        .arg("--no-pager")
        .args(args)
        .current_dir(cwd)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("GIT_TERMINAL_PROMPT", "0");
    let output =
        process_output_with_timeout(command, Duration::from_millis(GIT_PROBE_TIMEOUT_MS)).await?;

    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ));
    }

    let stderr = String::from_utf8_lossy(&output.stderr)
        .trim()
        .to_lowercase();
    if stderr.contains("not a git repository") || stderr.contains("not a git repo") {
        return Err(GitCommandFailureKind::NotRepository);
    }
    if allow_missing && output.status.code() == Some(1) && stderr.is_empty() {
        return Ok(None);
    }
    Err(GitCommandFailureKind::Failed)
}

fn hooks_path_is_configured(cwd: &Path, hooks_path: &str) -> bool {
    if hooks_path.trim().is_empty() {
        return false;
    }

    let normalized = hooks_path
        .trim()
        .trim_matches('"')
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    if normalized == ".githooks" {
        return true;
    }

    let configured = PathBuf::from(hooks_path.trim());
    if !configured.is_absolute() {
        return false;
    }

    let expected = cwd.join(".githooks");
    match (configured.canonicalize(), expected.canonicalize()) {
        (Ok(configured), Ok(expected)) => configured == expected,
        _ => false,
    }
}

#[derive(Debug)]
struct GitProbeData {
    branch: Option<String>,
    dirty_count: Option<usize>,
    sync_gate_configured: Option<bool>,
    latest_release_tag: Option<String>,
    summary: GitProbeSummary,
}

impl GitProbeData {
    fn with_status(status: GitProbeStatus) -> Self {
        Self {
            branch: None,
            dirty_count: None,
            sync_gate_configured: None,
            latest_release_tag: None,
            summary: GitProbeSummary {
                status,
                ..GitProbeSummary::default()
            },
        }
    }
}

fn record_git_failure(summary: &mut GitProbeSummary, probe: &str, failure: GitCommandFailureKind) {
    match failure {
        GitCommandFailureKind::Timeout => summary.timed_out.push(probe.to_string()),
        GitCommandFailureKind::NotRepository
        | GitCommandFailureKind::Unavailable
        | GitCommandFailureKind::Failed => summary.failed.push(probe.to_string()),
    }
}

async fn collect_git_probe(cwd: &Path) -> GitProbeData {
    match git_command_output(cwd, &["rev-parse", "--is-inside-work-tree"], false).await {
        Ok(Some(value)) if value == "true" => {}
        Err(GitCommandFailureKind::NotRepository) | Ok(_) => {
            return GitProbeData::with_status(GitProbeStatus::NotRepository)
        }
        Err(GitCommandFailureKind::Unavailable) => {
            return GitProbeData::with_status(GitProbeStatus::Unavailable)
        }
        Err(GitCommandFailureKind::Timeout) => {
            let mut data = GitProbeData::with_status(GitProbeStatus::Partial);
            data.summary.timed_out.push("repository".into());
            return data;
        }
        Err(GitCommandFailureKind::Failed) => {
            let mut data = GitProbeData::with_status(GitProbeStatus::Partial);
            data.summary.failed.push("repository".into());
            return data;
        }
    }

    let (branch_result, dirty_result, hooks_result, tags_result) = tokio::join!(
        git_command_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"], false),
        git_command_output(cwd, &["status", "--porcelain=v1", "-z"], false),
        git_command_output(cwd, &["config", "--get", "core.hooksPath"], true),
        git_command_output(cwd, &["tag", "--sort=-version:refname"], false),
    );

    let mut summary = GitProbeSummary {
        status: GitProbeStatus::Ok,
        ..GitProbeSummary::default()
    };
    let branch = match branch_result {
        Ok(value) => value.filter(|value| !value.is_empty()),
        Err(failure) => {
            record_git_failure(&mut summary, "branch", failure);
            None
        }
    };
    let dirty_count = match dirty_result {
        Ok(value) => Some(
            value
                .unwrap_or_default()
                .split('\0')
                .filter(|entry| !entry.is_empty())
                .count(),
        ),
        Err(failure) => {
            record_git_failure(&mut summary, "status", failure);
            None
        }
    };
    let sync_gate_configured = match hooks_result {
        Ok(value) => Some(
            value
                .as_deref()
                .map(|hooks_path| hooks_path_is_configured(cwd, hooks_path))
                .unwrap_or(false),
        ),
        Err(failure) => {
            record_git_failure(&mut summary, "hook_config", failure);
            None
        }
    };
    let latest_release_tag = match tags_result {
        Ok(value) => value
            .and_then(|tags| tags.lines().next().map(str::to_string))
            .filter(|value| !value.is_empty()),
        Err(failure) => {
            record_git_failure(&mut summary, "tag", failure);
            None
        }
    };

    if !summary.timed_out.is_empty() || !summary.failed.is_empty() {
        summary.status = GitProbeStatus::Partial;
    }

    GitProbeData {
        branch,
        dirty_count,
        sync_gate_configured,
        latest_release_tag,
        summary,
    }
}

async fn delivery_for_project(cwd: Option<&Path>) -> DeliverySummary {
    let Some(cwd) = cwd else {
        return DeliverySummary {
            git_branch: None,
            is_dirty: None,
            dirty_count: None,
            sync_gate_present: false,
            sync_gate_configured: None,
            release_workflow_present: false,
            auto_release_present: false,
            latest_release_tag: None,
            git_probe: GitProbeSummary::default(),
        };
    };

    let git = collect_git_probe(cwd).await;

    DeliverySummary {
        git_branch: git.branch,
        is_dirty: git.dirty_count.map(|count| count > 0),
        dirty_count: git.dirty_count,
        sync_gate_present: cwd.join(".githooks").join("pre-commit").exists(),
        sync_gate_configured: git.sync_gate_configured,
        release_workflow_present: cwd
            .join(".github")
            .join("workflows")
            .join("release.yml")
            .exists(),
        auto_release_present: cwd
            .join(".github")
            .join("workflows")
            .join("auto-release.yml")
            .exists(),
        latest_release_tag: git.latest_release_tag,
        git_probe: git.summary,
    }
}

fn risks_for(
    cwd: Option<&Path>,
    memory: &MemoryProposalSummary,
    delivery: &DeliverySummary,
) -> Vec<ControlPlaneRisk> {
    let mut risks = Vec::new();
    if cwd.is_none() {
        risks.push(ControlPlaneRisk {
            id: "no-project-context".into(),
            severity: "warning".into(),
            message: "No active project; authority and delivery gates are partial.".into(),
        });
    }
    if cwd.is_some() && !delivery.sync_gate_present {
        risks.push(ControlPlaneRisk {
            id: "missing-sync-gate".into(),
            severity: "warning".into(),
            message: "Sync-before-commit gate is not installed in this project.".into(),
        });
    }
    if cwd.is_some() && delivery.sync_gate_present && delivery.sync_gate_configured == Some(false) {
        risks.push(ControlPlaneRisk {
            id: "sync-gate-not-configured".into(),
            severity: "warning".into(),
            message: "Versioned pre-commit hook exists but this checkout is not using it.".into(),
        });
    }
    if delivery.is_dirty == Some(true) {
        risks.push(ControlPlaneRisk {
            id: "dirty-worktree".into(),
            severity: "warning".into(),
            message: format!(
                "Working tree has {} changed/untracked item(s).",
                delivery.dirty_count.unwrap_or_default()
            ),
        });
    }
    match delivery.git_probe.status {
        GitProbeStatus::Partial => {
            let mut reasons = Vec::new();
            if !delivery.git_probe.timed_out.is_empty() {
                reasons.push(format!(
                    "timed out: {}",
                    delivery.git_probe.timed_out.join(", ")
                ));
            }
            if !delivery.git_probe.failed.is_empty() {
                reasons.push(format!("failed: {}", delivery.git_probe.failed.join(", ")));
            }
            risks.push(ControlPlaneRisk {
                id: "git-probe-partial".into(),
                severity: "warning".into(),
                message: format!(
                    "Git observation is partial after {}ms; {}.",
                    delivery.git_probe.timeout_ms,
                    reasons.join("; ")
                ),
            });
        }
        GitProbeStatus::NotRepository => risks.push(ControlPlaneRisk {
            id: "not-git-repository".into(),
            severity: "info".into(),
            message: "The active project is not a Git repository; Git delivery fields are not applicable."
                .into(),
        }),
        GitProbeStatus::Unavailable => risks.push(ControlPlaneRisk {
            id: "git-unavailable".into(),
            severity: "warning".into(),
            message: "Git executable is unavailable; Git delivery fields could not be observed."
                .into(),
        }),
        GitProbeStatus::Ok | GitProbeStatus::NotChecked => {}
    }
    if cwd.is_some() && !delivery.release_workflow_present {
        risks.push(ControlPlaneRisk {
            id: "no-release-workflow".into(),
            severity: "warning".into(),
            message:
                "Release workflow is missing; publishing cannot be proven from GitHub Actions."
                    .into(),
        });
    }
    if memory.pending > 0 {
        risks.push(ControlPlaneRisk {
            id: "memory-review-pending".into(),
            severity: "info".into(),
            message: format!(
                "{} memory proposal(s) are waiting for review.",
                memory.pending
            ),
        });
    }
    risks
}

async fn memory_summary(state: &AppState, cwd: Option<&str>) -> MemoryProposalSummary {
    let pool = state.db.read().await;
    let mut base = "FROM learning_events".to_string();
    let mut bind_cwd = false;
    if cwd.is_some() {
        base.push_str(" WHERE cwd = ?");
        bind_cwd = true;
    }

    async fn count(
        pool: &sqlx::SqlitePool,
        base: &str,
        cwd: Option<&str>,
        status: &str,
        kind: Option<&str>,
    ) -> i64 {
        let filter = if base.contains("WHERE") {
            " AND "
        } else {
            " WHERE "
        };
        let mut sql = format!("SELECT COUNT(*) {base}{filter}status = ?");
        if kind.is_some() {
            sql.push_str(" AND kind = ?");
        }
        let mut q = sqlx::query_scalar::<_, i64>(&sql);
        if let Some(cwd) = cwd {
            q = q.bind(cwd);
        }
        q = q.bind(status);
        if let Some(kind) = kind {
            q = q.bind(kind);
        }
        q.fetch_one(pool).await.unwrap_or(0)
    }

    let cwd_for_bind = cwd.filter(|_| bind_cwd);
    let pending = count(&pool, &base, cwd_for_bind, "pending", None).await;
    let accepted = count(&pool, &base, cwd_for_bind, "accepted", None).await;
    let rejected = count(&pool, &base, cwd_for_bind, "rejected", None).await;
    let preference_pending = count(&pool, &base, cwd_for_bind, "pending", Some("preference")).await;

    let mut latest_sql = "SELECT suggestion FROM learning_events".to_string();
    if cwd.is_some() {
        latest_sql.push_str(" WHERE cwd = ? AND status = 'pending'");
    } else {
        latest_sql.push_str(" WHERE status = 'pending'");
    }
    latest_sql.push_str(" ORDER BY created_at DESC LIMIT 3");
    let mut q = sqlx::query_scalar::<_, String>(&latest_sql);
    if let Some(cwd) = cwd {
        q = q.bind(cwd);
    }
    let latest_pending = q.fetch_all(&*pool).await.unwrap_or_default();

    MemoryProposalSummary {
        pending,
        accepted,
        rejected,
        preference_pending,
        latest_pending,
    }
}

#[tauri::command]
pub async fn get_control_plane_snapshot(
    cwd: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ControlPlaneSnapshot, String> {
    let cwd_path = cwd.as_deref().map(PathBuf::from);
    let cwd_ref = cwd_path.as_deref().filter(|p| p.exists());

    let authority = authority_for_project(cwd_ref);
    let delivery = delivery_for_project(cwd_ref).await;
    let memory = memory_summary(&state, cwd_ref.map(|p| p.to_string_lossy()).as_deref()).await;

    let skills = crate::commands::skills::list_skills(app)
        .await
        .unwrap_or_default();
    let (mcp_total, mcp_enabled, hooks_total, hooks_enabled, git_total) = {
        let settings = state.settings.read().await;
        (
            settings.mcp_servers.len(),
            settings.mcp_servers.iter().filter(|s| s.enabled).count(),
            settings.hooks.len(),
            settings.hooks.iter().filter(|h| h.enabled).count(),
            settings.git_remotes.len(),
        )
    };
    let knowledge = {
        let pool = state.db.read().await;
        crate::knowledge::list_libraries(&pool)
            .await
            .unwrap_or_default()
    };
    let capabilities = vec![
        CapabilitySummary {
            id: "skills".into(),
            label: "Skills".into(),
            total: skills.len(),
            enabled: skills.iter().filter(|s| s.enabled).count(),
            status: ControlPlaneStatus::Ok,
            detail: "Prompt and slash-command capability packs.".into(),
        },
        CapabilitySummary {
            id: "mcp".into(),
            label: "MCP servers".into(),
            total: mcp_total,
            enabled: mcp_enabled,
            status: ControlPlaneStatus::Ok,
            detail: "External tool servers available to the agent runtime.".into(),
        },
        CapabilitySummary {
            id: "knowledge".into(),
            label: "Knowledge libraries".into(),
            total: knowledge.len(),
            enabled: knowledge.iter().filter(|k| k.enabled).count(),
            status: ControlPlaneStatus::Ok,
            detail: "Local document libraries exposed through knowledge tools.".into(),
        },
        CapabilitySummary {
            id: "hooks".into(),
            label: "Hooks".into(),
            total: hooks_total,
            enabled: hooks_enabled,
            status: ControlPlaneStatus::Ok,
            detail: "Event hooks for task/tool lifecycle automation.".into(),
        },
        CapabilitySummary {
            id: "git-remotes".into(),
            label: "Git remotes".into(),
            total: git_total,
            enabled: git_total,
            status: ControlPlaneStatus::Ok,
            detail: "Token-redacted GitHub/GitLab remote integrations.".into(),
        },
    ];
    let risks = risks_for(cwd_ref, &memory, &delivery);

    Ok(ControlPlaneSnapshot {
        generated_at: Utc::now().to_rfc3339(),
        cwd: cwd_ref.map(|p| p.to_string_lossy().to_string()),
        authority,
        memory,
        capabilities,
        delivery,
        risks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn tmp_project() -> PathBuf {
        let p = std::env::temp_dir().join(format!("cf-control-plane-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn authority_reports_present_and_missing_surfaces() {
        let cwd = tmp_project();
        std::fs::write(cwd.join("AGENTS.md"), "# rules\n").unwrap();
        std::fs::create_dir_all(cwd.join("docs/specs")).unwrap();

        let items = authority_for_project(Some(&cwd));

        assert_eq!(
            items.iter().find(|i| i.id == "agents-md").unwrap().status,
            ControlPlaneStatus::Ok
        );
        assert_eq!(
            items.iter().find(|i| i.id == "sync-gate").unwrap().status,
            ControlPlaneStatus::Missing
        );
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[tokio::test]
    async fn delivery_handles_non_git_projects_without_error() {
        let cwd = tmp_project();

        let delivery = delivery_for_project(Some(&cwd)).await;

        assert_eq!(delivery.git_branch, None);
        assert_eq!(delivery.git_probe.status, GitProbeStatus::NotRepository);
        assert_eq!(delivery.is_dirty, None);
        assert!(!delivery.sync_gate_present);
        let memory = MemoryProposalSummary {
            pending: 0,
            accepted: 0,
            rejected: 0,
            preference_pending: 0,
            latest_pending: Vec::new(),
        };
        let risks = risks_for(Some(&cwd), &memory, &delivery);
        assert!(risks.iter().any(|risk| risk.id == "not-git-repository"));
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[tokio::test]
    async fn normal_git_delivery_is_complete_and_confirmed_clean() {
        let cwd = tmp_project();
        std::process::Command::new("git")
            .no_window()
            .args(["init"])
            .current_dir(&cwd)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .no_window()
            .args([
                "-c",
                "user.name=CodeFactory Test",
                "-c",
                "user.email=codefactory@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ])
            .current_dir(&cwd)
            .output()
            .unwrap();

        let delivery = delivery_for_project(Some(&cwd)).await;

        assert_eq!(delivery.git_probe.status, GitProbeStatus::Ok);
        assert!(delivery.git_branch.is_some());
        assert_eq!(delivery.is_dirty, Some(false));
        assert_eq!(delivery.dirty_count, Some(0));
        assert_eq!(delivery.sync_gate_configured, Some(false));
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[tokio::test]
    async fn missing_process_is_classified_as_unavailable() {
        let command =
            tokio::process::Command::new("codefactory-control-plane-command-that-does-not-exist");
        let result = process_output_with_timeout(command, Duration::from_millis(50)).await;
        assert!(matches!(result, Err(GitCommandFailureKind::Unavailable)));
    }

    #[test]
    fn partial_git_risk_keeps_unknown_fields_unknown() {
        let cwd = tmp_project();
        let delivery = DeliverySummary {
            git_branch: None,
            is_dirty: None,
            dirty_count: None,
            sync_gate_present: true,
            sync_gate_configured: None,
            release_workflow_present: true,
            auto_release_present: true,
            latest_release_tag: None,
            git_probe: GitProbeSummary {
                status: GitProbeStatus::Partial,
                timeout_ms: GIT_PROBE_TIMEOUT_MS,
                timed_out: vec!["status".into()],
                failed: vec!["tag".into()],
            },
        };
        let memory = MemoryProposalSummary {
            pending: 0,
            accepted: 0,
            rejected: 0,
            preference_pending: 0,
            latest_pending: Vec::new(),
        };

        let risks = risks_for(Some(&cwd), &memory, &delivery);

        let risk = risks
            .iter()
            .find(|risk| risk.id == "git-probe-partial")
            .unwrap();
        assert!(risk.message.contains("timed out: status"));
        assert!(risk.message.contains("failed: tag"));
        assert_eq!(delivery.is_dirty, None);
        assert_eq!(delivery.sync_gate_configured, None);
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[tokio::test]
    async fn delivery_distinguishes_present_hook_from_configured_hook() {
        let cwd = tmp_project();
        std::fs::create_dir_all(cwd.join(".githooks")).unwrap();
        std::fs::write(cwd.join(".githooks").join("pre-commit"), "#!/bin/sh\n").unwrap();
        std::process::Command::new("git")
            .no_window()
            .args(["init"])
            .current_dir(&cwd)
            .output()
            .unwrap();

        let unconfigured = delivery_for_project(Some(&cwd)).await;
        assert!(unconfigured.sync_gate_present);
        assert_eq!(unconfigured.sync_gate_configured, Some(false));

        std::process::Command::new("git")
            .no_window()
            .args(["config", "core.hooksPath", ".githooks"])
            .current_dir(&cwd)
            .output()
            .unwrap();

        let configured = delivery_for_project(Some(&cwd)).await;
        assert_eq!(configured.sync_gate_configured, Some(true));

        let _ = std::fs::remove_dir_all(cwd);
    }

    #[test]
    #[ignore]
    fn control_plane_timeout_child_fixture() {
        let Some(state_dir) = std::env::var_os("CODEFACTORY_CONTROL_PLANE_TIMEOUT_STATE_DIR")
        else {
            return;
        };
        std::fs::write(
            std::path::Path::new(&state_dir).join("pid"),
            std::process::id().to_string(),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[cfg(unix)]
    #[test]
    #[ignore]
    fn control_plane_timeout_parent_fixture() {
        let Some(state_dir) = std::env::var_os("CODEFACTORY_CONTROL_PLANE_TIMEOUT_STATE_DIR")
        else {
            return;
        };
        let _ = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "control_plane_timeout_child_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("CODEFACTORY_CONTROL_PLANE_TIMEOUT_STATE_DIR", state_dir)
            .status();
    }

    fn timeout_fixture_pid(state_dir: &Path) -> u32 {
        std::fs::read_to_string(state_dir.join("pid"))
            .expect("timeout fixture should report its pid before the probe deadline")
            .trim()
            .parse()
            .expect("timeout fixture pid should be numeric")
    }

    #[cfg(unix)]
    fn assert_process_stopped(pid: u32, label: &str) {
        for _ in 0..20 {
            let result = unsafe { libc::kill(pid as i32, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        panic!("timed-out {label} process {pid} kept running");
    }

    #[cfg(windows)]
    fn assert_process_stopped(pid: u32, label: &str) {
        // `taskkill` returning success means the process was still alive. It
        // also cleans up that process before the assertion fails.
        let output = std::process::Command::new("taskkill")
            .no_window()
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
            .expect("taskkill should be available on Windows");
        assert!(
            !output.status.success(),
            "timed-out {label} process {pid} kept running"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_timeout_stops_and_reaps_child() {
        let state_dir = tmp_project();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "control_plane_timeout_child_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("CODEFACTORY_CONTROL_PLANE_TIMEOUT_STATE_DIR", &state_dir);

        let result = process_output_with_timeout(
            command,
            std::time::Duration::from_millis(GIT_PROBE_TIMEOUT_MS),
        )
        .await;

        assert!(matches!(result, Err(GitCommandFailureKind::Timeout)));
        assert_process_stopped(timeout_fixture_pid(&state_dir), "child");
        let _ = std::fs::remove_dir_all(state_dir);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_timeout_stops_descendants_holding_output_pipes() {
        let state_dir = tmp_project();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "control_plane_timeout_parent_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("CODEFACTORY_CONTROL_PLANE_TIMEOUT_STATE_DIR", &state_dir);

        let result = process_output_with_timeout(
            command,
            std::time::Duration::from_millis(GIT_PROBE_TIMEOUT_MS),
        )
        .await;

        assert!(matches!(result, Err(GitCommandFailureKind::Timeout)));
        assert_process_stopped(timeout_fixture_pid(&state_dir), "descendant");
        let _ = std::fs::remove_dir_all(state_dir);
    }
}
