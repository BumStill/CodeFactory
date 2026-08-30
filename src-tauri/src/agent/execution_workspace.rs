// SPDX-License-Identifier: Apache-2.0
//! Objective-owned primary execution workspaces.

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Row, Sqlite, SqlitePool, Transaction};
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::util::no_window::NoWindow;

static ALLOCATION_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

const ATTACHABLE_STATES: &[&str] = &["allocating", "active", "delivering"];
const CLEANUP_LEASE_TTL_MS: i64 = 300_000;
const CLEANUP_RETRY_BACKOFF_MS: i64 = 300_000;
const CLEANUP_BATCH_LIMIT: i64 = 16;
const CLEANUP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ExecutionWorkspaceRequest {
    pub objective_id: String,
    pub session_id: Option<String>,
    pub source_cwd: PathBuf,
    pub workspace_container: PathBuf,
    pub process_instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionWorkspace {
    pub id: String,
    pub objective_id: String,
    pub session_id: Option<String>,
    pub repo_identity: String,
    pub repo_root: PathBuf,
    pub git_common_dir: PathBuf,
    pub worktree_path: PathBuf,
    pub worktree_identity: String,
    pub branch_name: String,
    pub base_ref: String,
    pub base_sha: String,
    pub head_sha: String,
    pub state: String,
    pub lease_owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, FromRow)]
pub struct ExecutionWorkspaceView {
    pub objective_id: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub base_ref: String,
    pub base_sha: String,
    pub state: String,
    pub failure_code: Option<String>,
    pub failure_detail: Option<String>,
}

impl ExecutionWorkspace {
    pub fn view(&self) -> ExecutionWorkspaceView {
        ExecutionWorkspaceView {
            objective_id: self.objective_id.clone(),
            worktree_path: self.worktree_path.to_string_lossy().into_owned(),
            branch_name: self.branch_name.clone(),
            base_ref: self.base_ref.clone(),
            base_sha: self.base_sha.clone(),
            state: self.state.clone(),
            failure_code: None,
            failure_detail: None,
        }
    }
}

#[derive(Debug, FromRow)]
struct WorkspaceRow {
    id: String,
    objective_id: String,
    session_id: Option<String>,
    repo_identity: String,
    repo_root: String,
    git_common_dir: String,
    worktree_path: String,
    worktree_identity: Option<String>,
    branch_name: String,
    base_ref: String,
    base_sha: String,
    head_sha: Option<String>,
    state: String,
    lease_owner: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct CleanupCandidate {
    id: String,
    objective_id: String,
    repo_identity: String,
    repo_root: String,
    git_common_dir: String,
    worktree_path: String,
    worktree_identity: String,
    branch_name: String,
    head_sha: String,
}

#[derive(Debug, FromRow)]
struct CleanupAuthority {
    workspace_path: String,
    worktree_identity: String,
    repo_identity: String,
    head_branch: String,
    expected_head_sha: String,
    canonical_pr_number: i64,
    canonical_pr_url: String,
    canonical_head_sha: String,
    reached_ceiling: String,
    evidence_json: String,
}

#[derive(Debug, Deserialize)]
struct MergeReceiptEvidence {
    pr_number: i64,
    merge_sha: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CleanupPassOutcome {
    pub scanned: usize,
    pub closed: usize,
    pub preserved: usize,
    pub incidents: usize,
}

enum CandidateDisposition {
    Closed { pr_number: i64, pr_url: String },
    Preserved { code: &'static str, detail: String },
    Incident(String),
}

impl WorkspaceRow {
    fn ready(self) -> Result<ExecutionWorkspace> {
        let worktree_identity = self
            .worktree_identity
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("managed workspace has no durable worktree identity"))?;
        let head_sha = self
            .head_sha
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("managed workspace has no durable HEAD"))?;
        Ok(ExecutionWorkspace {
            id: self.id,
            objective_id: self.objective_id,
            session_id: self.session_id,
            repo_identity: self.repo_identity,
            repo_root: PathBuf::from(self.repo_root),
            git_common_dir: PathBuf::from(self.git_common_dir),
            worktree_path: PathBuf::from(self.worktree_path),
            worktree_identity,
            branch_name: self.branch_name,
            base_ref: self.base_ref,
            base_sha: self.base_sha,
            head_sha,
            state: self.state,
            lease_owner: self.lease_owner,
        })
    }
}

#[derive(Debug)]
struct RepoSeed {
    root: PathBuf,
    git_common_dir: PathBuf,
    repo_identity: String,
    remote: Option<String>,
    default_branch: Option<String>,
}

#[derive(Debug)]
struct RepoObservation {
    root: PathBuf,
    git_common_dir: PathBuf,
    repo_identity: String,
    base_ref: String,
    base_sha: String,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    let mut command = command.no_window();
    let output = command
        .output()
        .with_context(|| format!("run git {:?} in {}", args, cwd.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git {:?} failed: {stderr}", args);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn digest(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update([0]);
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn canonical_git_path(root: &Path, raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    path.canonicalize()
        .with_context(|| format!("canonicalize git path {}", path.display()))
}

fn remote_names(root: &Path) -> Vec<String> {
    git(root, &["remote"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn choose_remote(root: &Path) -> Option<String> {
    let names = remote_names(root);
    if names.iter().any(|name| name == "origin") {
        Some("origin".into())
    } else {
        names.into_iter().next()
    }
}

fn remote_default_branch(root: &Path, remote: &str) -> String {
    git(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            &format!("refs/remotes/{remote}/HEAD"),
        ],
    )
    .ok()
    .and_then(|value| value.rsplit('/').next().map(str::to_string))
    .or_else(|| {
        ["main", "master"].into_iter().find_map(|candidate| {
            git(
                root,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/{remote}/{candidate}"),
                ],
            )
            .ok()
            .map(|_| candidate.to_string())
        })
    })
    .unwrap_or_else(|| "main".into())
}

fn inspect_source_repo(source_cwd: &Path) -> Result<RepoSeed> {
    let root = PathBuf::from(
        git(source_cwd, &["rev-parse", "--show-toplevel"]).context("not a git repository")?,
    )
    .canonicalize()
    .context("canonicalize source repository")?;
    let git_common_dir = canonical_git_path(
        &root,
        &git(&root, &["rev-parse", "--git-common-dir"]).context("resolve common git directory")?,
    )?;
    let remote = choose_remote(&root);
    let default_branch = remote
        .as_deref()
        .map(|remote| remote_default_branch(&root, remote));
    let repo_source = if let Some(remote) = remote.as_deref() {
        git(&root, &["remote", "get-url", remote])?
    } else {
        git_common_dir.to_string_lossy().into_owned()
    };
    let repo_identity = format!("sha256:{:x}", Sha256::digest(repo_source.as_bytes()));
    Ok(RepoSeed {
        root,
        git_common_dir,
        repo_identity,
        remote,
        default_branch,
    })
}

fn refresh_source_base(seed: RepoSeed) -> Result<RepoObservation> {
    let (base_ref, base_sha) = if let (Some(remote), Some(default_branch)) =
        (seed.remote.as_deref(), seed.default_branch.as_deref())
    {
        git(&seed.root, &["fetch", "--prune", remote, default_branch]).with_context(|| {
            format!("fetch latest {remote}/{default_branch} before workspace allocation")
        })?;
        let base_ref = format!("{remote}/{default_branch}");
        let base_sha = git(&seed.root, &["rev-parse", &base_ref])?;
        (base_ref, base_sha)
    } else {
        let branch = git(&seed.root, &["branch", "--show-current"])?;
        if branch.is_empty() {
            bail!("detached HEAD cannot seed a managed execution workspace");
        }
        let head = git(&seed.root, &["rev-parse", "HEAD"])?;
        ("HEAD".into(), head)
    };
    Ok(RepoObservation {
        root: seed.root,
        git_common_dir: seed.git_common_dir,
        repo_identity: seed.repo_identity,
        base_ref,
        base_sha,
    })
}

fn workspace_identity(path: &Path) -> Result<(String, String, String)> {
    let repo = crate::agent::delivery::resolve_repo(path, None).map_err(|error| anyhow!(error))?;
    let identity =
        crate::agent::delivery::capture_delivery_identity(&repo).map_err(|error| anyhow!(error))?;
    Ok((
        identity.repo_identity,
        identity.worktree_identity,
        identity.head_sha,
    ))
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    git(
        root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .with_context(|| format!("canonicalize Git worktree path {}", path.display()));
    }

    let mut ancestor = path.parent();
    while ancestor.is_some_and(|candidate| !candidate.exists()) {
        ancestor = ancestor.and_then(Path::parent);
    }
    let ancestor = ancestor.ok_or_else(|| {
        anyhow!(
            "Git worktree path has no existing ancestor: {}",
            path.display()
        )
    })?;
    let relative = path
        .strip_prefix(ancestor)
        .with_context(|| format!("resolve missing Git worktree path {}", path.display()))?;
    Ok(ancestor
        .canonicalize()
        .with_context(|| format!("canonicalize Git worktree ancestor {}", ancestor.display()))?
        .join(relative))
}

fn worktree_is_registered(root: &Path, path: &Path) -> Result<bool> {
    let listed = git(root, &["worktree", "list", "--porcelain"])?;
    let expected = canonicalize_allow_missing(path)?;
    for registered in listed
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
    {
        let registered = Path::new(registered);
        if registered == path {
            return Ok(true);
        }
        // Git for Windows reports `C:/...`, while std::fs::canonicalize stores
        // the same path as `\\?\C:\...`. Canonicalizing both sides (or their
        // nearest live parent after a prior removal) keeps the equality check
        // exact without weakening the app-owned-container boundary.
        if canonicalize_allow_missing(registered).is_ok_and(|value| value == expected) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_managed_path(workspace_path: &Path, workspace_container: &Path) -> Result<()> {
    if !workspace_path.is_absolute() {
        bail!("managed workspace path is not absolute");
    }
    let container = workspace_container
        .canonicalize()
        .context("canonicalize managed workspace container")?;
    let relative = workspace_path
        .strip_prefix(&container)
        .context("managed workspace path is outside the app-owned container")?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("managed workspace path is not a strict normalized child of the app-owned container");
    }
    if workspace_path.exists() {
        let canonical = workspace_path
            .canonicalize()
            .context("canonicalize managed workspace path")?;
        if canonical != workspace_path || !canonical.starts_with(&container) {
            bail!("managed workspace path resolves outside its durable app-owned identity");
        }
        return Ok(());
    }

    // A prior process may have removed the worktree and crashed before deleting
    // its branch or closing the SQLite row. Validate the nearest existing
    // ancestor so that this idempotent recovery path still cannot cross a
    // symlink or `..` boundary.
    let mut ancestor = workspace_path.parent();
    while ancestor.is_some_and(|path| !path.exists()) {
        ancestor = ancestor.and_then(Path::parent);
    }
    let ancestor =
        ancestor.ok_or_else(|| anyhow!("managed workspace path has no live ancestor"))?;
    let canonical_ancestor = ancestor
        .canonicalize()
        .context("canonicalize managed workspace ancestor")?;
    if !canonical_ancestor.starts_with(&container) {
        bail!("managed workspace ancestor resolves outside the app-owned container");
    }
    Ok(())
}

async fn load_cleanup_authority(
    pool: &SqlitePool,
    candidate: &CleanupCandidate,
) -> Result<Option<(i64, String)>> {
    let rows = sqlx::query_as::<_, CleanupAuthority>(
        "SELECT delivery_runs.workspace_path,
                delivery_runs.worktree_identity,
                delivery_runs.repo_identity,
                delivery_runs.head_branch,
                delivery_runs.expected_head_sha,
                delivery_runs.canonical_pr_number,
                delivery_runs.canonical_pr_url,
                delivery_runs.canonical_head_sha,
                delivery_runs.reached_ceiling,
                delivery_mutation_intents.evidence_json
         FROM delivery_runs
         JOIN delivery_mutation_intents
           ON delivery_mutation_intents.run_id=delivery_runs.id
         WHERE delivery_runs.objective_id=?
           AND delivery_mutation_intents.rung='provider_pr_merge'
           AND delivery_mutation_intents.status IN ('committed', 'reconciled_committed')
           AND delivery_runs.canonical_pr_number IS NOT NULL
           AND delivery_runs.canonical_pr_url IS NOT NULL
           AND delivery_runs.canonical_head_sha IS NOT NULL
           AND delivery_mutation_intents.evidence_json IS NOT NULL
         ORDER BY delivery_runs.updated_at DESC,
                  delivery_mutation_intents.updated_at DESC",
    )
    .bind(&candidate.objective_id)
    .fetch_all(pool)
    .await?;

    for row in rows {
        if !matches!(
            row.reached_ceiling.as_str(),
            "merged" | "release_triggered" | "deployment_succeeded" | "live_verified"
        ) || row.workspace_path != candidate.worktree_path
            || row.worktree_identity != candidate.worktree_identity
            || row.repo_identity != candidate.repo_identity
            || row.head_branch != candidate.branch_name
            || row.expected_head_sha != candidate.head_sha
            || row.canonical_head_sha != candidate.head_sha
            || row.canonical_pr_number <= 0
            || row.canonical_pr_url.trim().is_empty()
        {
            continue;
        }
        let Ok(evidence) = serde_json::from_str::<MergeReceiptEvidence>(&row.evidence_json) else {
            continue;
        };
        if evidence.pr_number == row.canonical_pr_number && !evidence.merge_sha.trim().is_empty() {
            return Ok(Some((row.canonical_pr_number, row.canonical_pr_url)));
        }
    }
    Ok(None)
}

async fn claim_cleanup_candidate(
    pool: &SqlitePool,
    candidate: &CleanupCandidate,
    owner: &str,
) -> Result<bool> {
    let now = Utc::now().timestamp_millis();
    Ok(sqlx::query(
        "UPDATE execution_workspaces
         SET lease_owner=?, lease_expires_at=?, updated_at=?
         WHERE id=? AND state='cleanup_pending'
           AND ((lease_owner IS NULL AND lease_expires_at IS NULL)
                OR lease_expires_at<=?)",
    )
    .bind(owner)
    .bind(now + CLEANUP_LEASE_TTL_MS)
    .bind(now)
    .bind(&candidate.id)
    .bind(now)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

async fn preserve_cleanup_candidate(
    pool: &SqlitePool,
    candidate: &CleanupCandidate,
    owner: &str,
    code: &str,
    detail: &str,
) -> Result<()> {
    let updated = sqlx::query(
        "UPDATE execution_workspaces
         SET failure_code=?, failure_detail=?, lease_owner=NULL,
             lease_expires_at=NULL, updated_at=?
         WHERE id=? AND state='cleanup_pending' AND lease_owner=?",
    )
    .bind(code)
    .bind(detail.chars().take(1000).collect::<String>())
    .bind(Utc::now().timestamp_millis())
    .bind(&candidate.id)
    .bind(owner)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        bail!("managed workspace cleanup claim changed before preservation receipt");
    }
    Ok(())
}

async fn incident_cleanup_candidate(
    pool: &SqlitePool,
    candidate: &CleanupCandidate,
    owner: &str,
    detail: &str,
) -> Result<()> {
    let updated = sqlx::query(
        "UPDATE execution_workspaces
         SET state='incident', failure_code='workspace_cleanup_identity_conflict',
             failure_detail=?, lease_owner=NULL, lease_expires_at=NULL, updated_at=?
         WHERE id=? AND state='cleanup_pending' AND lease_owner=?",
    )
    .bind(detail.chars().take(1000).collect::<String>())
    .bind(Utc::now().timestamp_millis())
    .bind(&candidate.id)
    .bind(owner)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        bail!("managed workspace cleanup claim changed before incident receipt");
    }
    Ok(())
}

async fn close_cleanup_candidate(
    pool: &SqlitePool,
    candidate: &CleanupCandidate,
    owner: &str,
    pr_number: i64,
    pr_url: &str,
) -> Result<()> {
    let now = Utc::now().timestamp_millis();
    let updated = sqlx::query(
        "UPDATE execution_workspaces
         SET state='closed', canonical_pr_number=?, canonical_pr_url=?,
             failure_code=NULL, failure_detail=NULL, lease_owner=NULL,
             lease_expires_at=NULL, closed_at=?, updated_at=?
         WHERE id=? AND state='cleanup_pending' AND lease_owner=?",
    )
    .bind(pr_number)
    .bind(pr_url)
    .bind(now)
    .bind(now)
    .bind(&candidate.id)
    .bind(owner)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        bail!("managed workspace cleanup claim changed before closeout receipt");
    }
    Ok(())
}

fn inspect_and_remove_candidate(
    candidate: &CleanupCandidate,
    workspace_container: &Path,
    authority: (i64, String),
) -> Result<CandidateDisposition> {
    let workspace_path = PathBuf::from(&candidate.worktree_path);
    if let Err(error) = validate_managed_path(&workspace_path, workspace_container) {
        return Ok(CandidateDisposition::Incident(error.to_string()));
    }
    if !candidate.branch_name.starts_with("codefactory/objective-") {
        return Ok(CandidateDisposition::Incident(
            "managed workspace branch is outside the reserved CodeFactory namespace".into(),
        ));
    }
    let repo_root = PathBuf::from(&candidate.repo_root);
    let canonical_repo_root = match repo_root.canonicalize() {
        Ok(path) if path == repo_root => path,
        Ok(_) => {
            return Ok(CandidateDisposition::Incident(
                "managed workspace repository root changed canonical identity".into(),
            ))
        }
        Err(error) => {
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_inspection_failed",
                detail: format!("managed repository root unavailable: {error}"),
            })
        }
    };
    if canonical_repo_root == workspace_path {
        return Ok(CandidateDisposition::Incident(
            "managed workspace path points at the source repository root".into(),
        ));
    }
    let common_dir_raw = match git(&canonical_repo_root, &["rev-parse", "--git-common-dir"]) {
        Ok(path) => path,
        Err(error) => {
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_inspection_failed",
                detail: error.to_string(),
            })
        }
    };
    let common_dir = match canonical_git_path(&canonical_repo_root, &common_dir_raw) {
        Ok(path) => path,
        Err(error) => {
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_inspection_failed",
                detail: error.to_string(),
            })
        }
    };
    if common_dir != PathBuf::from(&candidate.git_common_dir) {
        return Ok(CandidateDisposition::Incident(
            "managed workspace common Git directory changed identity".into(),
        ));
    }

    let registered = match worktree_is_registered(&canonical_repo_root, &workspace_path) {
        Ok(registered) => registered,
        Err(error) => {
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_inspection_failed",
                detail: error.to_string(),
            })
        }
    };
    if workspace_path.exists() {
        if !registered {
            return Ok(CandidateDisposition::Incident(
                "managed workspace path exists without an exact Git worktree registration".into(),
            ));
        }
        let branch = match git(&workspace_path, &["branch", "--show-current"]) {
            Ok(branch) => branch,
            Err(error) => {
                return Ok(CandidateDisposition::Preserved {
                    code: "workspace_cleanup_inspection_failed",
                    detail: error.to_string(),
                })
            }
        };
        let identity = match workspace_identity(&workspace_path) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(CandidateDisposition::Preserved {
                    code: "workspace_cleanup_inspection_failed",
                    detail: error.to_string(),
                })
            }
        };
        if branch != candidate.branch_name
            || identity.0 != candidate.repo_identity
            || identity.1 != candidate.worktree_identity
            || identity.2 != candidate.head_sha
        {
            return Ok(CandidateDisposition::Incident(
                "managed workspace repo, gitdir, branch, or HEAD changed before cleanup".into(),
            ));
        }
        let status = match git(
            &workspace_path,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        ) {
            Ok(status) => status,
            Err(error) => {
                return Ok(CandidateDisposition::Preserved {
                    code: "workspace_cleanup_inspection_failed",
                    detail: error.to_string(),
                })
            }
        };
        if !status.is_empty() {
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_dirty",
                detail: "canonical merged workspace has uncommitted or untracked changes".into(),
            });
        }
        if let Err(error) = git(
            &canonical_repo_root,
            &[
                "worktree",
                "remove",
                workspace_path
                    .to_str()
                    .ok_or_else(|| anyhow!("managed workspace path is not valid UTF-8"))?,
            ],
        ) {
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_remove_failed",
                detail: error.to_string(),
            });
        }
        if workspace_path.exists()
            || worktree_is_registered(&canonical_repo_root, &workspace_path).unwrap_or(true)
        {
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_remove_failed",
                detail:
                    "Git reported success but the managed worktree still exists or is registered"
                        .into(),
            });
        }
    } else if registered {
        return Ok(CandidateDisposition::Incident(
            "managed workspace path is missing while Git still registers it".into(),
        ));
    }

    if branch_exists(&canonical_repo_root, &candidate.branch_name) {
        let branch_ref = format!("refs/heads/{}", candidate.branch_name);
        let branch_head = match git(&canonical_repo_root, &["rev-parse", &branch_ref]) {
            Ok(head) => head,
            Err(error) => {
                return Ok(CandidateDisposition::Preserved {
                    code: "workspace_cleanup_branch_failed",
                    detail: error.to_string(),
                })
            }
        };
        if branch_head != candidate.head_sha {
            return Ok(CandidateDisposition::Incident(
                "managed workspace branch HEAD changed after worktree removal".into(),
            ));
        }
        if let Err(error) = git(
            &canonical_repo_root,
            &["branch", "-D", "--", &candidate.branch_name],
        ) {
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_branch_failed",
                detail: error.to_string(),
            });
        }
    }
    if branch_exists(&canonical_repo_root, &candidate.branch_name) {
        return Ok(CandidateDisposition::Preserved {
            code: "workspace_cleanup_branch_failed",
            detail: "managed workspace branch still exists after exact deletion".into(),
        });
    }
    Ok(CandidateDisposition::Closed {
        pr_number: authority.0,
        pr_url: authority.1,
    })
}

pub(crate) async fn run_cleanup_pass(
    pool: &SqlitePool,
    workspace_container: &Path,
    process_instance: &str,
) -> Result<CleanupPassOutcome> {
    ensure_schema(pool).await?;
    let _guard = ALLOCATION_LOCK.lock().await;
    let now = Utc::now().timestamp_millis();
    let candidates = sqlx::query_as::<_, CleanupCandidate>(
        "SELECT execution_workspaces.id, execution_workspaces.objective_id,
                execution_workspaces.repo_identity, execution_workspaces.repo_root,
                execution_workspaces.git_common_dir, execution_workspaces.worktree_path,
                execution_workspaces.worktree_identity, execution_workspaces.branch_name,
                execution_workspaces.head_sha
         FROM execution_workspaces
         JOIN objectives ON objectives.id=execution_workspaces.objective_id
         WHERE execution_workspaces.state='cleanup_pending'
           AND objectives.status IN ('completed', 'cancelled')
           AND execution_workspaces.worktree_identity IS NOT NULL
           AND execution_workspaces.head_sha IS NOT NULL
           AND ((execution_workspaces.lease_owner IS NULL
                 AND execution_workspaces.lease_expires_at IS NULL)
                OR execution_workspaces.lease_expires_at<=?)
           AND (execution_workspaces.failure_code IS NULL
                OR execution_workspaces.updated_at<=?)
         ORDER BY execution_workspaces.updated_at ASC
         LIMIT ?",
    )
    .bind(now)
    .bind(now - CLEANUP_RETRY_BACKOFF_MS)
    .bind(CLEANUP_BATCH_LIMIT)
    .fetch_all(pool)
    .await?;
    let mut outcome = CleanupPassOutcome {
        scanned: candidates.len(),
        ..CleanupPassOutcome::default()
    };
    let owner = format!("workspace-cleanup:{process_instance}");

    for candidate in candidates {
        if !claim_cleanup_candidate(pool, &candidate, &owner).await? {
            continue;
        }
        if let Err(error) =
            acquire_repo_allocation_lock(pool, &candidate.repo_identity, &owner).await
        {
            preserve_cleanup_candidate(
                pool,
                &candidate,
                &owner,
                "workspace_cleanup_repo_busy",
                &error.to_string(),
            )
            .await?;
            outcome.preserved += 1;
            continue;
        }

        let disposition = if let Err(error) =
            validate_managed_path(Path::new(&candidate.worktree_path), workspace_container)
        {
            CandidateDisposition::Incident(error.to_string())
        } else {
            match load_cleanup_authority(pool, &candidate).await {
                Ok(Some(authority)) => {
                    inspect_and_remove_candidate(&candidate, workspace_container, authority)?
                }
                Ok(None) => CandidateDisposition::Preserved {
                    code: "workspace_cleanup_merge_unproven",
                    detail: "no exact committed canonical PR merge receipt matches this workspace"
                        .into(),
                },
                Err(error) => CandidateDisposition::Preserved {
                    code: "workspace_cleanup_inspection_failed",
                    detail: error.to_string(),
                },
            }
        };

        let apply_result = match disposition {
            CandidateDisposition::Closed { pr_number, pr_url } => {
                outcome.closed += 1;
                close_cleanup_candidate(pool, &candidate, &owner, pr_number, &pr_url).await
            }
            CandidateDisposition::Preserved { code, detail } => {
                outcome.preserved += 1;
                preserve_cleanup_candidate(pool, &candidate, &owner, code, &detail).await
            }
            CandidateDisposition::Incident(detail) => {
                outcome.incidents += 1;
                incident_cleanup_candidate(pool, &candidate, &owner, &detail).await
            }
        };
        release_repo_allocation_lock(pool, &candidate.repo_identity, &owner).await;
        apply_result?;
    }
    Ok(outcome)
}

pub(crate) fn spawn_cleanup_supervisor(
    pool: SqlitePool,
    workspace_container: PathBuf,
    process_instance: String,
) {
    tauri::async_runtime::spawn(async move {
        tracing::info!(
            poll_ms = CLEANUP_POLL_INTERVAL.as_millis(),
            "managed workspace cleanup supervisor started"
        );
        loop {
            match run_cleanup_pass(&pool, &workspace_container, &process_instance).await {
                Ok(outcome) if outcome.scanned > 0 => tracing::info!(
                    scanned = outcome.scanned,
                    closed = outcome.closed,
                    preserved = outcome.preserved,
                    incidents = outcome.incidents,
                    "managed workspace cleanup pass completed"
                ),
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    %error,
                    "managed workspace cleanup pass failed closed; retrying later"
                ),
            }
            tokio::time::sleep(CLEANUP_POLL_INTERVAL).await;
        }
    });
}

#[cfg(not(test))]
#[derive(Debug, Serialize)]
pub(crate) struct CleanupSmokeReceipt {
    pub scenario_id: &'static str,
    pub status: &'static str,
    pub evidence_level: &'static str,
    pub clean_workspace_closed: bool,
    pub clean_branch_deleted: bool,
    pub dirty_workspace_preserved: bool,
    pub dirty_branch_preserved: bool,
    pub root_checkout_unchanged: bool,
    pub closeout_count: usize,
    pub preservation_count: usize,
}

#[cfg(not(test))]
async fn record_smoke_merge_receipt(
    pool: &SqlitePool,
    workspace: &ExecutionWorkspace,
    pr_number: i64,
) -> Result<()> {
    let head_sha = git(&workspace.worktree_path, &["rev-parse", "HEAD"])?;
    let now = Utc::now().timestamp_millis();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE objectives
         SET status='cancelled', decision_type='cancelled',
             cancellation_provenance='explicit_cancel', completed_at=?,
             lease_owner=NULL, lease_expires_at=NULL, updated_at=?
         WHERE id=?",
    )
    .bind(now)
    .bind(now)
    .bind(&workspace.objective_id)
    .execute(&mut *tx)
    .await?;
    mark_objective_terminal_in_tx(&mut tx, &workspace.objective_id, now).await?;
    sqlx::query("UPDATE execution_workspaces SET head_sha=? WHERE objective_id=?")
        .bind(&head_sha)
        .bind(&workspace.objective_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let run_id = format!("cleanup-smoke-delivery-{pr_number}");
    sqlx::query(
        "INSERT INTO delivery_runs
         (id, objective_id, run_kind, workspace_path, worktree_identity,
          repo_identity, head_branch, expected_head_sha, canonical_pr_number,
          canonical_pr_url, canonical_head_sha, requested_ceiling,
          reached_ceiling, stage, status, last_observed_at, last_progress_at,
          app_version, app_build, process_instance, created_at, updated_at,
          claim_epoch, reconciled_claim_epoch)
         VALUES (?, ?, 'objective', ?, ?, ?, ?, ?, ?, ?, ?, 'through_merge',
                 'merged', 'completed', 'completed', ?, ?, 'smoke', 'smoke',
                 'cleanup-smoke', ?, ?, 1, 1)",
    )
    .bind(&run_id)
    .bind(&workspace.objective_id)
    .bind(workspace.worktree_path.to_string_lossy().into_owned())
    .bind(&workspace.worktree_identity)
    .bind(&workspace.repo_identity)
    .bind(&workspace.branch_name)
    .bind(&head_sha)
    .bind(pr_number)
    .bind(format!("https://example.invalid/pull/{pr_number}"))
    .bind(&head_sha)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO delivery_mutation_intents
         (intent_id, run_id, claim_epoch, rung, operation_key, status,
          process_instance, evidence_json, started_at, updated_at)
         VALUES (?, ?, 1, 'provider_pr_merge', ?, 'committed',
                 'cleanup-smoke', ?, ?, ?)",
    )
    .bind(format!("cleanup-smoke-intent-{pr_number}"))
    .bind(&run_id)
    .bind(format!("cleanup-smoke-merge-{pr_number}"))
    .bind(
        serde_json::json!({
            "pr_number": pr_number,
            "merge_sha": format!("synthetic-merge-{pr_number}"),
        })
        .to_string(),
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Exact-binary real Git + SQLite smoke for the managed workspace closeout
/// contract. It is fully synthetic and never opens the installed user DB.
#[cfg(not(test))]
pub(crate) async fn run_cleanup_smoke() -> Result<CleanupSmokeReceipt> {
    use crate::agent::objective::{CreateObjective, ObjectiveKind, ObjectiveStore, RecoveryDomain};

    let temp = tempfile::tempdir().context("create cleanup smoke directory")?;
    let root = temp.path().join("root");
    let remote = temp.path().join("remote.git");
    let container = temp.path().join("managed");
    std::fs::create_dir_all(&root)?;
    git(
        temp.path(),
        &[
            "init",
            "--bare",
            "--initial-branch=main",
            remote
                .to_str()
                .ok_or_else(|| anyhow!("smoke remote path is not valid UTF-8"))?,
        ],
    )?;
    git(&root, &["init", "--initial-branch=main"])?;
    git(&root, &["config", "user.name", "CodeFactory Smoke"])?;
    git(
        &root,
        &["config", "user.email", "smoke@codefactory.invalid"],
    )?;
    std::fs::write(root.join("base.txt"), "base\n")?;
    git(&root, &["add", "base.txt"])?;
    git(&root, &["commit", "-m", "synthetic base"])?;
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            remote
                .to_str()
                .ok_or_else(|| anyhow!("smoke remote path is not valid UTF-8"))?,
        ],
    )?;
    git(&root, &["push", "-u", "origin", "main"])?;
    git(&root, &["checkout", "-b", "old-session"])?;
    std::fs::write(root.join("root-user-change.txt"), "preserve root\n")?;

    let db_path = temp.path().join("cleanup-smoke.db");
    let pool = crate::storage::db::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .map_err(|error| anyhow!(error.to_string()))?;
    let store = ObjectiveStore::new(pool.clone());
    for objective_id in ["cleanup-smoke-clean", "cleanup-smoke-dirty"] {
        store
            .create(CreateObjective {
                id: objective_id.into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: None,
                root_turn_id: None,
                domain: RecoveryDomain::Delivery,
                requested_acceptance: "merged".into(),
                created_surface: "release_artifact_smoke".into(),
            })
            .await?;
    }

    let clean = allocate_or_attach(
        &pool,
        ExecutionWorkspaceRequest {
            objective_id: "cleanup-smoke-clean".into(),
            session_id: None,
            source_cwd: root.clone(),
            workspace_container: container.clone(),
            process_instance: "cleanup-smoke-allocator".into(),
        },
    )
    .await?;
    let dirty = allocate_or_attach(
        &pool,
        ExecutionWorkspaceRequest {
            objective_id: "cleanup-smoke-dirty".into(),
            session_id: None,
            source_cwd: root.clone(),
            workspace_container: container.clone(),
            process_instance: "cleanup-smoke-allocator".into(),
        },
    )
    .await?;
    for (workspace, marker) in [(&clean, "clean"), (&dirty, "dirty")] {
        let filename = format!("{marker}-delivery.txt");
        std::fs::write(
            workspace.worktree_path.join(&filename),
            format!("{marker}\n"),
        )?;
        git(&workspace.worktree_path, &["add", &filename])?;
        git(
            &workspace.worktree_path,
            &["commit", "-m", &format!("synthetic {marker} delivery")],
        )?;
    }
    record_smoke_merge_receipt(&pool, &clean, 41).await?;
    record_smoke_merge_receipt(&pool, &dirty, 42).await?;
    std::fs::write(
        dirty.worktree_path.join("uncommitted-preservation.txt"),
        "must remain\n",
    )?;

    let root_before = (
        git(&root, &["branch", "--show-current"])?,
        git(&root, &["rev-parse", "HEAD"])?,
        git(&root, &["status", "--porcelain=v1"])?,
        git(&root, &["reflog", "show", "--format=%H %gs", "HEAD"])?,
    );
    let outcome = run_cleanup_pass(&pool, &container, "cleanup-smoke-supervisor").await?;
    let root_after = (
        git(&root, &["branch", "--show-current"])?,
        git(&root, &["rev-parse", "HEAD"])?,
        git(&root, &["status", "--porcelain=v1"])?,
        git(&root, &["reflog", "show", "--format=%H %gs", "HEAD"])?,
    );
    let dirty_state: (String, Option<String>) =
        sqlx::query_as("SELECT state, failure_code FROM execution_workspaces WHERE objective_id=?")
            .bind(&dirty.objective_id)
            .fetch_one(&pool)
            .await?;
    let clean_workspace_closed = !clean.worktree_path.exists();
    let clean_branch_deleted = !branch_exists(&root, &clean.branch_name);
    let dirty_workspace_preserved = dirty
        .worktree_path
        .join("uncommitted-preservation.txt")
        .is_file()
        && dirty_state.0 == "cleanup_pending"
        && dirty_state.1.as_deref() == Some("workspace_cleanup_dirty");
    let dirty_branch_preserved = branch_exists(&root, &dirty.branch_name);
    let root_checkout_unchanged = root_before == root_after;
    if outcome.closed != 1
        || outcome.preserved != 1
        || !clean_workspace_closed
        || !clean_branch_deleted
        || !dirty_workspace_preserved
        || !dirty_branch_preserved
        || !root_checkout_unchanged
    {
        bail!("managed workspace cleanup smoke did not satisfy every closeout oracle");
    }
    Ok(CleanupSmokeReceipt {
        scenario_id: "E2E-009",
        status: "passed",
        evidence_level: "exact_binary_real_git_sqlite",
        clean_workspace_closed,
        clean_branch_deleted,
        dirty_workspace_preserved,
        dirty_branch_preserved,
        root_checkout_unchanged,
        closeout_count: outcome.closed,
        preservation_count: outcome.preserved,
    })
}

pub fn is_git_repository(cwd: &Path) -> bool {
    git(cwd, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|value| value == "true")
}

pub async fn ensure_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(include_str!(
        "../../migrations/0019_managed_execution_workspaces.sql"
    ))
    .execute(pool)
    .await
    .context("ensure managed execution workspace schema")?;
    let objective_columns = sqlx::query("PRAGMA table_info(objectives)")
        .fetch_all(pool)
        .await?;
    let has_objective_status = objective_columns.iter().any(|column| {
        column
            .try_get::<String, _>("name")
            .is_ok_and(|name| name == "status")
    });
    if has_objective_status {
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE execution_workspaces
             SET state='cleanup_pending', lease_owner=NULL, lease_expires_at=NULL,
                 updated_at=?
             WHERE (state IN ('allocating', 'active', 'delivering')
                    OR (state='cleanup_pending'
                        AND (lease_owner IS NOT NULL OR lease_expires_at IS NOT NULL)
                        AND (lease_expires_at IS NULL OR lease_expires_at<=?)))
               AND EXISTS (
                 SELECT 1 FROM objectives
                 WHERE objectives.id=execution_workspaces.objective_id
                   AND objectives.status IN ('completed', 'cancelled')
               )",
        )
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Atomically hand a terminal Objective's workspace from execution ownership
/// to the asynchronous closeout lifecycle. Files and refs remain untouched;
/// the cleanup owner later decides whether a clean merged workspace is safe to
/// remove or whether dirty/unmerged evidence must be preserved.
pub(crate) async fn mark_objective_terminal_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    objective_id: &str,
    now: i64,
) -> Result<u64> {
    let has_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type='table' AND name='execution_workspaces'",
    )
    .fetch_one(&mut **tx)
    .await?;
    if has_table == 0 {
        return Ok(0);
    }
    Ok(sqlx::query(
        "UPDATE execution_workspaces
         SET state='cleanup_pending', lease_owner=NULL, lease_expires_at=NULL,
             updated_at=?
         WHERE objective_id=?
           AND state IN ('allocating', 'active', 'delivering')",
    )
    .bind(now)
    .bind(objective_id)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}

async fn acquire_repo_allocation_lock(
    pool: &SqlitePool,
    repo_identity: &str,
    process_instance: &str,
) -> Result<()> {
    let now = Utc::now().timestamp_millis();
    let acquired = sqlx::query(
        "INSERT INTO execution_workspace_repo_locks
         (repo_identity, lease_owner, lease_expires_at, acquired_at, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(repo_identity) DO UPDATE SET
             lease_owner=excluded.lease_owner,
             lease_expires_at=excluded.lease_expires_at,
             acquired_at=excluded.acquired_at,
             updated_at=excluded.updated_at
         WHERE execution_workspace_repo_locks.lease_owner=excluded.lease_owner
            OR execution_workspace_repo_locks.lease_expires_at<=excluded.acquired_at",
    )
    .bind(repo_identity)
    .bind(process_instance)
    .bind(now + 300_000)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    if acquired.rows_affected() != 1 {
        bail!("repository workspace allocation is owned by another live process");
    }
    Ok(())
}

async fn release_repo_allocation_lock(
    pool: &SqlitePool,
    repo_identity: &str,
    process_instance: &str,
) {
    let _ = sqlx::query(
        "DELETE FROM execution_workspace_repo_locks
         WHERE repo_identity=? AND lease_owner=?",
    )
    .bind(repo_identity)
    .bind(process_instance)
    .execute(pool)
    .await;
}

async fn load_workspace(pool: &SqlitePool, objective_id: &str) -> Result<Option<WorkspaceRow>> {
    Ok(sqlx::query_as::<_, WorkspaceRow>(
        "SELECT id, objective_id, session_id, repo_identity, repo_root,
                git_common_dir, worktree_path, worktree_identity, branch_name,
                base_ref, base_sha, head_sha, state, lease_owner
         FROM execution_workspaces WHERE objective_id=?",
    )
    .bind(objective_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn latest_for_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<ExecutionWorkspaceView>> {
    ensure_schema(pool).await?;
    Ok(sqlx::query_as::<_, ExecutionWorkspaceView>(
        "SELECT objective_id, worktree_path, branch_name, base_ref, base_sha,
                state, failure_code, failure_detail
         FROM execution_workspaces
         WHERE session_id=?
         ORDER BY updated_at DESC
         LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn has_workspace_record(pool: &SqlitePool, objective_id: &str) -> Result<bool> {
    ensure_schema(pool).await?;
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM execution_workspaces WHERE objective_id=?)",
    )
    .bind(objective_id)
    .fetch_one(pool)
    .await?
        != 0)
}

async fn objective_has_unmanaged_side_effects(
    pool: &SqlitePool,
    objective_id: &str,
) -> Result<bool> {
    let columns = sqlx::query("PRAGMA table_info(objectives)")
        .fetch_all(pool)
        .await?;
    let has_side_effect_column = columns.iter().any(|column| {
        column
            .try_get::<String, _>("name")
            .is_ok_and(|name| name == "side_effect_started")
    });
    if !has_side_effect_column {
        return Ok(false);
    }
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT side_effect_started FROM objectives WHERE id=?")
            .bind(objective_id)
            .fetch_optional(pool)
            .await?
            .is_some_and(|started| started != 0),
    )
}

async fn mark_incident(pool: &SqlitePool, objective_id: &str, error: &str) {
    let now = Utc::now().timestamp_millis();
    let _ = sqlx::query(
        "UPDATE execution_workspaces
         SET state='incident', failure_code='workspace_allocation_failed',
             failure_detail=?, lease_owner=NULL, lease_expires_at=NULL, updated_at=?
         WHERE objective_id=? AND state='allocating'",
    )
    .bind(error.chars().take(1000).collect::<String>())
    .bind(now)
    .bind(objective_id)
    .execute(pool)
    .await;
}

async fn mark_identity_incident(pool: &SqlitePool, objective_id: &str, error: &str) {
    let now = Utc::now().timestamp_millis();
    let _ = sqlx::query(
        "UPDATE execution_workspaces
         SET state='incident', failure_code='workspace_identity_conflict',
             failure_detail=?, lease_owner=NULL, lease_expires_at=NULL, updated_at=?
         WHERE objective_id=? AND state IN ('allocating', 'active', 'delivering', 'cleanup_pending')",
    )
    .bind(error.chars().take(1000).collect::<String>())
    .bind(now)
    .bind(objective_id)
    .execute(pool)
    .await;
}

async fn project_objective_identity(
    pool: &SqlitePool,
    workspace: &ExecutionWorkspace,
) -> Result<()> {
    let columns = sqlx::query("PRAGMA table_info(objectives)")
        .fetch_all(pool)
        .await?;
    let names = columns
        .iter()
        .filter_map(|column| column.try_get::<String, _>("name").ok())
        .collect::<std::collections::HashSet<_>>();
    if ["repo_identity", "worktree_identity", "base_sha", "head_sha"]
        .iter()
        .all(|column| names.contains(*column))
    {
        let projected = sqlx::query(
            "UPDATE objectives
             SET repo_identity=?, worktree_identity=?, base_sha=?, head_sha=?, updated_at=?
             WHERE id=?
               AND (repo_identity IS NULL OR repo_identity=? )
               AND (worktree_identity IS NULL OR worktree_identity=? )",
        )
        .bind(&workspace.repo_identity)
        .bind(&workspace.worktree_identity)
        .bind(&workspace.base_sha)
        .bind(&workspace.head_sha)
        .bind(Utc::now().timestamp_millis())
        .bind(&workspace.objective_id)
        .bind(&workspace.repo_identity)
        .bind(&workspace.worktree_identity)
        .execute(pool)
        .await?;
        if projected.rows_affected() != 1 {
            bail!("Objective identity changed before workspace projection");
        }
    }
    Ok(())
}

async fn reattach(
    pool: &SqlitePool,
    row: WorkspaceRow,
    process_instance: &str,
) -> Result<ExecutionWorkspace> {
    if row.state != "allocating" {
        return reattach_inner(pool, row, process_instance).await;
    }
    acquire_repo_allocation_lock(pool, &row.repo_identity, process_instance).await?;
    let repo_identity = row.repo_identity.clone();
    let result = reattach_inner(pool, row, process_instance).await;
    release_repo_allocation_lock(pool, &repo_identity, process_instance).await;
    result
}

async fn reattach_inner(
    pool: &SqlitePool,
    row: WorkspaceRow,
    process_instance: &str,
) -> Result<ExecutionWorkspace> {
    if !ATTACHABLE_STATES.contains(&row.state.as_str()) {
        bail!("managed workspace is not attachable in state {}", row.state);
    }
    let path = PathBuf::from(&row.worktree_path);
    if row.state == "allocating" && !path.exists() {
        let repo_root = PathBuf::from(&row.repo_root);
        if branch_exists(&repo_root, &row.branch_name) {
            let error = anyhow!(
                "reserved managed workspace branch exists without its recorded worktree path"
            );
            mark_incident(pool, &row.objective_id, &error.to_string()).await;
            return Err(error);
        }
        if let Err(error) = git(
            &repo_root,
            &[
                "worktree",
                "add",
                "-b",
                &row.branch_name,
                path.to_str()
                    .ok_or_else(|| anyhow!("workspace path is not valid UTF-8"))?,
                &row.base_sha,
            ],
        ) {
            mark_incident(pool, &row.objective_id, &error.to_string()).await;
            return Err(error);
        }
    }
    let canonical_path = match path.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            let error = anyhow!(
                "managed workspace path missing: {}: {error}",
                path.display()
            );
            mark_identity_incident(pool, &row.objective_id, &error.to_string()).await;
            return Err(error);
        }
    };
    let branch = match git(&canonical_path, &["branch", "--show-current"]) {
        Ok(branch) => branch,
        Err(error) => {
            mark_identity_incident(pool, &row.objective_id, &error.to_string()).await;
            return Err(error);
        }
    };
    if branch != row.branch_name {
        let error = anyhow!("managed workspace identity mismatch: branch changed");
        mark_identity_incident(pool, &row.objective_id, &error.to_string()).await;
        return Err(error);
    }
    let (repo_identity, worktree_identity, head_sha) = match workspace_identity(&canonical_path) {
        Ok(identity) => identity,
        Err(error) => {
            mark_identity_incident(pool, &row.objective_id, &error.to_string()).await;
            return Err(error);
        }
    };
    let recorded_identity_matches = row
        .worktree_identity
        .as_deref()
        .is_none_or(|recorded| recorded == worktree_identity);
    if repo_identity != row.repo_identity || !recorded_identity_matches {
        let error = anyhow!("managed workspace identity mismatch: repo or gitdir changed");
        mark_identity_incident(pool, &row.objective_id, &error.to_string()).await;
        return Err(error);
    }
    if row.state == "allocating" && head_sha != row.base_sha {
        let error = anyhow!("allocating managed workspace HEAD differs from its reserved base");
        mark_incident(pool, &row.objective_id, &error.to_string()).await;
        return Err(error);
    }
    let now = Utc::now().timestamp_millis();
    let updated = sqlx::query(
        "UPDATE execution_workspaces
         SET worktree_path=?, worktree_identity=?, head_sha=?,
             state=CASE WHEN state='allocating' THEN 'active' ELSE state END,
             failure_code=NULL, failure_detail=NULL,
             lease_owner=?, lease_expires_at=?, updated_at=?
         WHERE objective_id=?
           AND state IN ('allocating', 'active', 'delivering', 'cleanup_pending')
           AND (worktree_identity IS NULL OR worktree_identity=?)",
    )
    .bind(canonical_path.to_string_lossy().into_owned())
    .bind(&worktree_identity)
    .bind(&head_sha)
    .bind(process_instance)
    .bind(now + 120_000)
    .bind(now)
    .bind(&row.objective_id)
    .bind(&worktree_identity)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        bail!("managed workspace changed while it was being reattached");
    }
    let workspace = load_workspace(pool, &row.objective_id)
        .await?
        .ok_or_else(|| anyhow!("managed workspace disappeared during reattach"))?
        .ready()?;
    project_objective_identity(pool, &workspace).await?;
    Ok(workspace)
}

async fn allocate_new_locked(
    pool: &SqlitePool,
    request: ExecutionWorkspaceRequest,
    observed: RepoObservation,
) -> Result<ExecutionWorkspace> {
    let objective_digest = digest(
        "codefactory-objective-workspace-v1",
        &[&request.objective_id, &observed.repo_identity],
    );
    let repo_digest = digest("codefactory-workspace-repo-v1", &[&observed.repo_identity]);
    let branch_name = format!("codefactory/objective-{}", &objective_digest[..32]);
    let worktree_path = request
        .workspace_container
        .join(&repo_digest[..16])
        .join(&objective_digest[..24]);
    let workspace_id = format!("workspace-{}", &objective_digest[..32]);
    let now = Utc::now().timestamp_millis();
    std::fs::create_dir_all(
        worktree_path
            .parent()
            .ok_or_else(|| anyhow!("workspace path has no parent"))?,
    )?;
    sqlx::query(
        "INSERT INTO execution_workspaces
         (id, objective_id, session_id, repo_identity, repo_root, git_common_dir,
          worktree_path, branch_name, base_ref, base_sha, state, lease_owner,
          lease_expires_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'allocating', ?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&request.objective_id)
    .bind(&request.session_id)
    .bind(&observed.repo_identity)
    .bind(observed.root.to_string_lossy().into_owned())
    .bind(observed.git_common_dir.to_string_lossy().into_owned())
    .bind(worktree_path.to_string_lossy().into_owned())
    .bind(&branch_name)
    .bind(&observed.base_ref)
    .bind(&observed.base_sha)
    .bind(&request.process_instance)
    .bind(now + 120_000)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .context("reserve managed execution workspace")?;

    let allocation = (|| -> Result<ExecutionWorkspace> {
        if worktree_path.exists() {
            bail!("reserved managed workspace path already exists");
        }
        if branch_exists(&observed.root, &branch_name) {
            bail!("reserved managed workspace branch already exists");
        }
        git(
            &observed.root,
            &[
                "worktree",
                "add",
                "-b",
                &branch_name,
                worktree_path
                    .to_str()
                    .ok_or_else(|| anyhow!("workspace path is not valid UTF-8"))?,
                &observed.base_ref,
            ],
        )?;
        let canonical_path = worktree_path.canonicalize()?;
        let branch = git(&canonical_path, &["branch", "--show-current"])?;
        if branch != branch_name {
            bail!("new managed workspace checked out an unexpected branch");
        }
        let (repo_identity, worktree_identity, head_sha) = workspace_identity(&canonical_path)?;
        if repo_identity != observed.repo_identity || head_sha != observed.base_sha {
            bail!("new managed workspace identity does not match reserved base");
        }
        Ok(ExecutionWorkspace {
            id: workspace_id.clone(),
            objective_id: request.objective_id.clone(),
            session_id: request.session_id.clone(),
            repo_identity,
            repo_root: observed.root.clone(),
            git_common_dir: observed.git_common_dir.clone(),
            worktree_path: canonical_path,
            worktree_identity,
            branch_name: branch,
            base_ref: observed.base_ref.clone(),
            base_sha: observed.base_sha.clone(),
            head_sha,
            state: "active".into(),
            lease_owner: Some(request.process_instance.clone()),
        })
    })();

    let workspace = match allocation {
        Ok(workspace) => workspace,
        Err(error) => {
            mark_incident(pool, &request.objective_id, &error.to_string()).await;
            return Err(error);
        }
    };
    let activated = sqlx::query(
        "UPDATE execution_workspaces
         SET worktree_path=?, worktree_identity=?, head_sha=?, state='active',
             failure_code=NULL, failure_detail=NULL, updated_at=?
         WHERE objective_id=? AND state='allocating' AND base_sha=?",
    )
    .bind(workspace.worktree_path.to_string_lossy().into_owned())
    .bind(&workspace.worktree_identity)
    .bind(&workspace.head_sha)
    .bind(Utc::now().timestamp_millis())
    .bind(&workspace.objective_id)
    .bind(&workspace.base_sha)
    .execute(pool)
    .await?;
    if activated.rows_affected() != 1 {
        bail!("managed workspace allocation changed before activation receipt");
    }
    project_objective_identity(pool, &workspace).await?;
    Ok(workspace)
}

pub async fn allocate_or_attach(
    pool: &SqlitePool,
    request: ExecutionWorkspaceRequest,
) -> Result<ExecutionWorkspace> {
    ensure_schema(pool).await?;
    let _guard = ALLOCATION_LOCK.lock().await;
    if let Some(row) = load_workspace(pool, &request.objective_id).await? {
        return reattach(pool, row, &request.process_instance).await;
    }
    let seed = inspect_source_repo(&request.source_cwd)?;
    acquire_repo_allocation_lock(pool, &seed.repo_identity, &request.process_instance).await?;
    let repo_identity = seed.repo_identity.clone();
    let process_instance = request.process_instance.clone();
    let result = async {
        if let Some(row) = load_workspace(pool, &request.objective_id).await? {
            return reattach_inner(pool, row, &process_instance).await;
        }
        if objective_has_unmanaged_side_effects(pool, &request.objective_id).await? {
            bail!(
                "legacy Objective already recorded side effects without a managed workspace; refusing to bind the user checkout"
            );
        }
        let observed = refresh_source_base(seed)?;
        allocate_new_locked(pool, request, observed).await
    }
    .await;
    release_repo_allocation_lock(pool, &repo_identity, &process_instance).await;
    result
}

pub async fn attach_existing(
    pool: &SqlitePool,
    objective_id: &str,
    process_instance: &str,
) -> Result<Option<ExecutionWorkspace>> {
    ensure_schema(pool).await?;
    let _guard = ALLOCATION_LOCK.lock().await;
    let Some(row) = load_workspace(pool, objective_id).await? else {
        return Ok(None);
    };
    reattach(pool, row, process_instance).await.map(Some)
}

pub async fn verify_objective_workspace(
    pool: &SqlitePool,
    objective_id: &str,
    cwd: &Path,
) -> Result<ExecutionWorkspace> {
    ensure_schema(pool).await?;
    let result = async {
        let row = load_workspace(pool, objective_id)
            .await?
            .ok_or_else(|| anyhow!("managed workspace identity missing for mutation objective"))?;
        if !matches!(row.state.as_str(), "active" | "delivering") {
            bail!("managed workspace is not mutable in state {}", row.state);
        }
        let expected = PathBuf::from(&row.worktree_path).canonicalize()?;
        let actual_root =
            PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
        if actual_root != expected {
            bail!("managed workspace identity mismatch: mutation cwd differs");
        }
        let branch = git(&actual_root, &["branch", "--show-current"])?;
        let (repo_identity, worktree_identity, _head_sha) = workspace_identity(&actual_root)?;
        if branch != row.branch_name
            || repo_identity != row.repo_identity
            || row.worktree_identity.as_deref() != Some(worktree_identity.as_str())
        {
            bail!("managed workspace identity mismatch: repo, gitdir, or branch differs");
        }
        row.ready()
    }
    .await;
    if let Err(error) = &result {
        mark_identity_incident(pool, objective_id, &error.to_string()).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_repo_allocation_lock, allocate_or_attach, attach_existing, ensure_schema,
        latest_for_session, mark_objective_terminal_in_tx, release_repo_allocation_lock,
        run_cleanup_pass, verify_objective_workspace, ExecutionWorkspaceRequest,
    };
    use crate::util::no_window::NoWindow;
    use sqlx::SqlitePool;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .no_window()
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("git must run");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let remote = temp.path().join("remote.git");
        let seed = temp.path().join("seed");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&seed).unwrap();
        assert!(Command::new("git")
            .no_window()
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .status()
            .unwrap()
            .success());
        git(&root, &["init", "--initial-branch=main"]);
        git(&root, &["config", "user.name", "CodeFactory Test"]);
        git(&root, &["config", "user.email", "test@codefactory.invalid"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(&root, &["add", "base.txt"]);
        git(&root, &["commit", "-m", "base"]);
        git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&root, &["push", "-u", "origin", "main"]);

        git(&seed, &["init", "--initial-branch=main"]);
        git(&seed, &["config", "user.name", "CodeFactory Test"]);
        git(&seed, &["config", "user.email", "test@codefactory.invalid"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&seed, &["fetch", "origin", "main"]);
        git(&seed, &["checkout", "-b", "main", "origin/main"]);
        std::fs::write(seed.join("latest.txt"), "latest\n").unwrap();
        git(&seed, &["add", "latest.txt"]);
        git(&seed, &["commit", "-m", "latest remote main"]);
        git(&seed, &["push", "origin", "main"]);

        git(&root, &["checkout", "-b", "old-session"]);
        std::fs::write(root.join("old.txt"), "old branch only\n").unwrap();
        git(&root, &["add", "old.txt"]);
        git(&root, &["commit", "-m", "old branch commit"]);
        std::fs::write(root.join("user-dirty.txt"), "preserve me\n").unwrap();
        let container = temp.path().join("managed");
        (temp, root, remote, container)
    }

    async fn pool_with_objective(objective_id: &str) -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE objectives (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL DEFAULT 'active'
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO objectives(id) VALUES (?)")
            .bind(objective_id)
            .execute(&pool)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        pool
    }

    async fn record_terminal_merge_receipt(
        pool: &SqlitePool,
        workspace: &super::ExecutionWorkspace,
    ) -> String {
        crate::agent::delivery_run::ensure_schema(pool)
            .await
            .unwrap();
        let head_sha = git(&workspace.worktree_path, &["rev-parse", "HEAD"]);
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("UPDATE objectives SET status='completed' WHERE id=?")
            .bind(&workspace.objective_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE execution_workspaces
             SET head_sha=?, state='cleanup_pending', lease_owner=NULL,
                 lease_expires_at=NULL, updated_at=?
             WHERE objective_id=?",
        )
        .bind(&head_sha)
        .bind(now)
        .bind(&workspace.objective_id)
        .execute(pool)
        .await
        .unwrap();
        let run_id = format!("delivery-{}", workspace.id);
        sqlx::query(
            "INSERT INTO delivery_runs
             (id, objective_id, run_kind, workspace_path, worktree_identity,
              repo_identity, head_branch, expected_head_sha, canonical_pr_number,
              canonical_pr_url, canonical_head_sha, requested_ceiling,
              reached_ceiling, stage, status, last_observed_at, last_progress_at,
              app_version, app_build, process_instance, created_at, updated_at,
              claim_epoch, reconciled_claim_epoch)
             VALUES (?, ?, 'objective', ?, ?, ?, ?, ?, 42,
                     'https://example.invalid/pull/42', ?, 'through_merge',
                     'merged', 'completed', 'completed', ?, ?, 'test', 'test',
                     'test-process', ?, ?, 1, 1)",
        )
        .bind(&run_id)
        .bind(&workspace.objective_id)
        .bind(workspace.worktree_path.to_string_lossy().into_owned())
        .bind(&workspace.worktree_identity)
        .bind(&workspace.repo_identity)
        .bind(&workspace.branch_name)
        .bind(&head_sha)
        .bind(&head_sha)
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO delivery_mutation_intents
             (intent_id, run_id, claim_epoch, rung, operation_key, status,
              process_instance, evidence_json, started_at, updated_at)
             VALUES (?, ?, 1, 'provider_pr_merge', 'merge-pr-42', 'committed',
                     'test-process', ?, ?, ?)",
        )
        .bind(format!("intent-{run_id}"))
        .bind(&run_id)
        .bind(r#"{"pr_number":42,"merge_sha":"merge-result-sha"}"#)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        head_sha
    }

    fn request(root: &Path, container: &Path, process_instance: &str) -> ExecutionWorkspaceRequest {
        request_for(root, container, "objective-dirty-root", process_instance)
    }

    fn request_for(
        root: &Path,
        container: &Path,
        objective_id: &str,
        process_instance: &str,
    ) -> ExecutionWorkspaceRequest {
        ExecutionWorkspaceRequest {
            objective_id: objective_id.into(),
            session_id: Some("session-managed-workspace".into()),
            source_cwd: root.to_path_buf(),
            workspace_container: container.to_path_buf(),
            process_instance: process_instance.into(),
        }
    }

    #[tokio::test]
    async fn dirty_root_gets_isolated_before_agent_execution() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let root_branch_before = git(&root, &["branch", "--show-current"]);
        let root_head_before = git(&root, &["rev-parse", "HEAD"]);
        let root_status_before = git(&root, &["status", "--porcelain=v1"]);

        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();

        assert_ne!(workspace.worktree_path, root);
        assert_eq!(workspace.base_ref, "origin/main");
        assert!(workspace.worktree_path.join("latest.txt").is_file());
        assert!(!workspace.worktree_path.join("old.txt").exists());
        let visible = latest_for_session(&pool, "session-managed-workspace")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            visible.worktree_path,
            workspace.worktree_path.to_string_lossy()
        );
        assert_eq!(visible.branch_name, workspace.branch_name);
        assert_eq!(visible.state, "active");
        assert_eq!(
            git(&root, &["branch", "--show-current"]),
            root_branch_before
        );
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), root_head_before);
        assert_eq!(
            git(&root, &["status", "--porcelain=v1"]),
            root_status_before
        );
    }

    #[tokio::test]
    async fn repository_allocation_lease_serializes_processes() {
        let pool = pool_with_objective("objective-dirty-root").await;
        acquire_repo_allocation_lock(&pool, "repo-one", "process-a")
            .await
            .unwrap();

        let error = acquire_repo_allocation_lock(&pool, "repo-one", "process-b")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("another live process"));
        release_repo_allocation_lock(&pool, "repo-one", "process-a").await;
        acquire_repo_allocation_lock(&pool, "repo-one", "process-b")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn restart_reattaches_the_exact_objective_workspace() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let first = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        let second = allocate_or_attach(&pool, request(&root, &container, "process-b"))
            .await
            .unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.worktree_path, first.worktree_path);
        assert_eq!(second.worktree_identity, first.worktree_identity);
        assert_eq!(second.branch_name, first.branch_name);
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution_workspaces WHERE objective_id=?")
                .bind("objective-dirty-root")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn cleanup_pending_workspace_cannot_be_reattached_as_active_execution() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        sqlx::query(
            "UPDATE execution_workspaces
             SET state='cleanup_pending', lease_owner=NULL, lease_expires_at=NULL
             WHERE objective_id=?",
        )
        .bind("objective-dirty-root")
        .execute(&pool)
        .await
        .unwrap();

        let error = attach_existing(&pool, "objective-dirty-root", "process-b")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("not attachable"));
        let terminal: (String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT state, lease_owner, lease_expires_at
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind("objective-dirty-root")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(terminal, ("cleanup_pending".into(), None, None));
    }

    #[tokio::test]
    async fn schema_upgrade_reconciles_terminal_objective_workspace_lease() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE objectives (id TEXT PRIMARY KEY, status TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO objectives(id, status) VALUES ('objective-terminal', 'active')")
            .execute(&pool)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO execution_workspaces
             (id, objective_id, repo_identity, repo_root, git_common_dir,
              worktree_path, worktree_identity, branch_name, base_ref, base_sha,
              head_sha, state, lease_owner, lease_expires_at, created_at, updated_at)
             VALUES ('workspace-terminal', 'objective-terminal', 'repo-terminal',
                     '/tmp/source-terminal', '/tmp/common-terminal',
                     '/tmp/worktree-terminal', 'gitdir-terminal',
                     'codefactory/objective-terminal', 'origin/main', 'base-terminal',
                     'head-terminal', 'active', 'old-process', ?, ?, ?)",
        )
        .bind(now + 120_000)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE objectives SET status='cancelled' WHERE id='objective-terminal'")
            .execute(&pool)
            .await
            .unwrap();

        ensure_schema(&pool).await.unwrap();

        let terminal: (String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT state, lease_owner, lease_expires_at
             FROM execution_workspaces WHERE objective_id='objective-terminal'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(terminal, ("cleanup_pending".into(), None, None));
    }

    #[tokio::test]
    async fn schema_and_terminal_reconciliation_preserve_a_live_cleanup_claim() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("UPDATE objectives SET status='completed' WHERE id=?")
            .bind("objective-dirty-root")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE execution_workspaces
             SET state='cleanup_pending', lease_owner='workspace-cleanup:live',
                 lease_expires_at=? WHERE objective_id=?",
        )
        .bind(now + 120_000)
        .bind("objective-dirty-root")
        .execute(&pool)
        .await
        .unwrap();

        ensure_schema(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let transitioned = mark_objective_terminal_in_tx(
            &mut tx,
            "objective-dirty-root",
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(transitioned, 0);
        let live: (Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT lease_owner, lease_expires_at
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind("objective-dirty-root")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(live.0.as_deref(), Some("workspace-cleanup:live"));
        assert!(live.1.is_some_and(|expires| expires > now));

        sqlx::query("UPDATE execution_workspaces SET lease_expires_at=? WHERE objective_id=?")
            .bind(now - 1)
            .bind("objective-dirty-root")
            .execute(&pool)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        let reclaimed: (Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT lease_owner, lease_expires_at
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind("objective-dirty-root")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(reclaimed, (None, None));
    }

    #[tokio::test]
    async fn restart_finalizes_a_worktree_created_before_allocation_receipt() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let first = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        sqlx::query(
            "UPDATE execution_workspaces
             SET state='allocating', worktree_identity=NULL, head_sha=NULL
             WHERE objective_id=?",
        )
        .bind("objective-dirty-root")
        .execute(&pool)
        .await
        .unwrap();

        let recovered = attach_existing(&pool, "objective-dirty-root", "process-b")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(recovered.state, "active");
        assert_eq!(recovered.worktree_path, first.worktree_path);
        assert_eq!(recovered.worktree_identity, first.worktree_identity);
        assert_eq!(recovered.head_sha, first.base_sha);
    }

    #[tokio::test]
    async fn concurrent_objectives_receive_distinct_branches_and_worktrees() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        sqlx::query("INSERT INTO objectives(id) VALUES (?)")
            .bind("objective-second")
            .execute(&pool)
            .await
            .unwrap();
        let first_request = request_for(&root, &container, "objective-dirty-root", "process-a");
        let second_request = request_for(&root, &container, "objective-second", "process-b");

        let (first, second) = tokio::join!(
            allocate_or_attach(&pool, first_request),
            allocate_or_attach(&pool, second_request),
        );
        let first = first.unwrap();
        let second = second.unwrap();

        assert_ne!(first.branch_name, second.branch_name);
        assert_ne!(first.worktree_path, second.worktree_path);
        assert_ne!(first.worktree_identity, second.worktree_identity);
        assert_eq!(first.base_sha, second.base_sha);
    }

    #[tokio::test]
    async fn reattach_identity_mismatch_becomes_a_durable_incident() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        git(
            &workspace.worktree_path,
            &["checkout", "-q", "-b", "unexpected-branch"],
        );

        let error = attach_existing(&pool, "objective-dirty-root", "process-b")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("branch changed"));
        let state: String =
            sqlx::query_scalar("SELECT state FROM execution_workspaces WHERE objective_id=?")
                .bind("objective-dirty-root")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "incident");
    }

    #[tokio::test]
    async fn delivery_rejects_a_different_workspace_before_git_side_effects() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();

        let error = verify_objective_workspace(&pool, "objective-dirty-root", &root)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("managed workspace identity mismatch"));
        let state: String =
            sqlx::query_scalar("SELECT state FROM execution_workspaces WHERE objective_id=?")
                .bind("objective-dirty-root")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "incident");
        assert!(verify_objective_workspace(
            &pool,
            "objective-dirty-root",
            &workspace.worktree_path,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn legacy_objective_with_prior_side_effects_is_not_bound_to_user_checkout() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        sqlx::query(
            "ALTER TABLE objectives ADD COLUMN side_effect_started INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE objectives SET side_effect_started=1 WHERE id=?")
            .bind("objective-dirty-root")
            .execute(&pool)
            .await
            .unwrap();
        let root_status_before = git(&root, &["status", "--porcelain=v1"]);

        let error = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("legacy Objective"));
        assert_eq!(
            git(&root, &["status", "--porcelain=v1"]),
            root_status_before
        );
        let workspaces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_workspaces")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(workspaces, 0);
    }

    #[tokio::test]
    async fn clean_merged_terminal_workspace_is_closed_without_touching_root_checkout() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        std::fs::write(workspace.worktree_path.join("change.txt"), "delivered\n").unwrap();
        git(&workspace.worktree_path, &["add", "change.txt"]);
        git(
            &workspace.worktree_path,
            &["commit", "-m", "delivered change"],
        );
        record_terminal_merge_receipt(&pool, &workspace).await;
        let root_branch_before = git(&root, &["branch", "--show-current"]);
        let root_head_before = git(&root, &["rev-parse", "HEAD"]);
        let root_status_before = git(&root, &["status", "--porcelain=v1"]);
        let root_reflog_before = git(&root, &["reflog", "show", "--format=%H %gs", "HEAD"]);

        let outcome = run_cleanup_pass(&pool, &container, "cleanup-process")
            .await
            .unwrap();

        assert_eq!(outcome.closed, 1);
        assert_eq!(outcome.preserved, 0);
        assert!(!workspace.worktree_path.exists());
        assert!(!super::branch_exists(&root, &workspace.branch_name));
        let state: (String, Option<i64>, Option<String>) = sqlx::query_as(
            "SELECT state, closed_at, lease_owner
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind(&workspace.objective_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state.0, "closed");
        assert!(state.1.is_some());
        assert_eq!(state.2, None);
        assert_eq!(
            git(&root, &["branch", "--show-current"]),
            root_branch_before
        );
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), root_head_before);
        assert_eq!(
            git(&root, &["status", "--porcelain=v1"]),
            root_status_before
        );
        assert_eq!(
            git(&root, &["reflog", "show", "--format=%H %gs", "HEAD"]),
            root_reflog_before
        );
    }

    #[tokio::test]
    async fn dirty_merged_terminal_workspace_is_preserved() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        record_terminal_merge_receipt(&pool, &workspace).await;
        std::fs::write(
            workspace.worktree_path.join("uncommitted.txt"),
            "preserve\n",
        )
        .unwrap();

        let outcome = run_cleanup_pass(&pool, &container, "cleanup-process")
            .await
            .unwrap();

        assert_eq!(outcome.closed, 0);
        assert_eq!(outcome.preserved, 1);
        assert!(workspace.worktree_path.join("uncommitted.txt").is_file());
        assert!(super::branch_exists(&root, &workspace.branch_name));
        let state: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT state, failure_code, lease_owner
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind(&workspace.objective_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state.0, "cleanup_pending");
        assert_eq!(state.1.as_deref(), Some("workspace_cleanup_dirty"));
        assert_eq!(state.2, None);
        let deferred = run_cleanup_pass(&pool, &container, "cleanup-process")
            .await
            .unwrap();
        assert_eq!(deferred.scanned, 0);
    }

    #[tokio::test]
    async fn terminal_workspace_without_merge_receipt_is_preserved() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET status='cancelled' WHERE id=?")
            .bind(&workspace.objective_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE execution_workspaces
             SET state='cleanup_pending', lease_owner=NULL, lease_expires_at=NULL
             WHERE objective_id=?",
        )
        .bind(&workspace.objective_id)
        .execute(&pool)
        .await
        .unwrap();

        let outcome = run_cleanup_pass(&pool, &container, "cleanup-process")
            .await
            .unwrap();

        assert_eq!(outcome.closed, 0);
        assert_eq!(outcome.preserved, 1);
        assert!(workspace.worktree_path.is_dir());
        assert!(super::branch_exists(&root, &workspace.branch_name));
    }

    #[tokio::test]
    async fn cleanup_restarts_after_worktree_removal_and_deletes_only_exact_branch() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        record_terminal_merge_receipt(&pool, &workspace).await;
        git(
            &root,
            &[
                "worktree",
                "remove",
                workspace.worktree_path.to_str().unwrap(),
            ],
        );
        assert!(super::branch_exists(&root, &workspace.branch_name));
        assert!(super::branch_exists(&root, "old-session"));

        let outcome = run_cleanup_pass(&pool, &container, "cleanup-process")
            .await
            .unwrap();

        assert_eq!(outcome.closed, 1);
        assert!(!super::branch_exists(&root, &workspace.branch_name));
        assert!(super::branch_exists(&root, "old-session"));
    }

    #[tokio::test]
    async fn cleanup_rejects_a_workspace_outside_the_managed_container() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        record_terminal_merge_receipt(&pool, &workspace).await;
        sqlx::query("UPDATE execution_workspaces SET worktree_path=? WHERE objective_id=?")
            .bind(root.to_string_lossy().into_owned())
            .bind(&workspace.objective_id)
            .execute(&pool)
            .await
            .unwrap();

        let outcome = run_cleanup_pass(&pool, &container, "cleanup-process")
            .await
            .unwrap();

        assert_eq!(outcome.closed, 0);
        assert_eq!(outcome.incidents, 1);
        assert!(root.join("user-dirty.txt").is_file());
        assert!(workspace.worktree_path.is_dir());
        let state: String =
            sqlx::query_scalar("SELECT state FROM execution_workspaces WHERE objective_id=?")
                .bind(&workspace.objective_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "incident");
    }

    #[tokio::test]
    async fn concurrent_cleanup_passes_close_the_exact_workspace_once() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        record_terminal_merge_receipt(&pool, &workspace).await;

        let (first, second) = tokio::join!(
            run_cleanup_pass(&pool, &container, "cleanup-process-a"),
            run_cleanup_pass(&pool, &container, "cleanup-process-b"),
        );
        let first = first.unwrap();
        let second = second.unwrap();

        assert_eq!(first.closed + second.closed, 1);
        assert!(!workspace.worktree_path.exists());
        assert!(!super::branch_exists(&root, &workspace.branch_name));
        let closed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution_workspaces
             WHERE objective_id=? AND state='closed'",
        )
        .bind(&workspace.objective_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(closed, 1);
    }
}
