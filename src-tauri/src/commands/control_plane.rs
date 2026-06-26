// SPDX-License-Identifier: Apache-2.0
//! AI Coding OS control-plane snapshot.
//!
//! This is intentionally read-only in v1. It aggregates the authority surfaces,
//! memory proposal state, capability inventory, and delivery gates that already
//! exist across CodeFactory into one auditable view.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, State};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverySummary {
    pub git_branch: Option<String>,
    pub is_dirty: bool,
    pub dirty_count: usize,
    pub sync_gate_present: bool,
    pub sync_gate_configured: bool,
    pub release_workflow_present: bool,
    pub auto_release_present: bool,
    pub latest_release_tag: Option<String>,
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

fn command_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .no_window()
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn sync_gate_configured(cwd: &Path) -> bool {
    let Some(hooks_path) = command_output(cwd, &["config", "--get", "core.hooksPath"]) else {
        return false;
    };

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

fn delivery_for_project(cwd: Option<&Path>) -> DeliverySummary {
    let Some(cwd) = cwd else {
        return DeliverySummary {
            git_branch: None,
            is_dirty: false,
            dirty_count: 0,
            sync_gate_present: false,
            sync_gate_configured: false,
            release_workflow_present: false,
            auto_release_present: false,
            latest_release_tag: None,
        };
    };

    let git_branch = command_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let dirty = command_output(cwd, &["status", "--porcelain=v1"]).unwrap_or_default();
    let dirty_count = dirty.lines().filter(|l| !l.trim().is_empty()).count();
    let latest_release_tag = command_output(cwd, &["tag", "--sort=-version:refname"])
        .and_then(|tags| tags.lines().next().map(str::to_string))
        .filter(|s| !s.is_empty());

    DeliverySummary {
        git_branch,
        is_dirty: dirty_count > 0,
        dirty_count,
        sync_gate_present: cwd.join(".githooks").join("pre-commit").exists(),
        sync_gate_configured: sync_gate_configured(cwd),
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
        latest_release_tag,
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
    if cwd.is_some() && delivery.sync_gate_present && !delivery.sync_gate_configured {
        risks.push(ControlPlaneRisk {
            id: "sync-gate-not-configured".into(),
            severity: "warning".into(),
            message: "Versioned pre-commit hook exists but this checkout is not using it.".into(),
        });
    }
    if delivery.is_dirty {
        risks.push(ControlPlaneRisk {
            id: "dirty-worktree".into(),
            severity: "warning".into(),
            message: format!(
                "Working tree has {} changed/untracked item(s).",
                delivery.dirty_count
            ),
        });
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
    let delivery = delivery_for_project(cwd_ref);
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

    #[test]
    fn delivery_handles_non_git_projects_without_error() {
        let cwd = tmp_project();

        let delivery = delivery_for_project(Some(&cwd));

        assert_eq!(delivery.git_branch, None);
        assert!(!delivery.is_dirty);
        assert!(!delivery.sync_gate_present);
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[test]
    fn delivery_distinguishes_present_hook_from_configured_hook() {
        let cwd = tmp_project();
        std::fs::create_dir_all(cwd.join(".githooks")).unwrap();
        std::fs::write(cwd.join(".githooks").join("pre-commit"), "#!/bin/sh\n").unwrap();
        Command::new("git")
            .no_window()
            .args(["init"])
            .current_dir(&cwd)
            .output()
            .unwrap();

        let unconfigured = delivery_for_project(Some(&cwd));
        assert!(unconfigured.sync_gate_present);
        assert!(!unconfigured.sync_gate_configured);

        Command::new("git")
            .no_window()
            .args(["config", "core.hooksPath", ".githooks"])
            .current_dir(&cwd)
            .output()
            .unwrap();

        let configured = delivery_for_project(Some(&cwd));
        assert!(configured.sync_gate_configured);

        let _ = std::fs::remove_dir_all(cwd);
    }
}
