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

pub(crate) mod workspace_recovery_budget {
    use sqlx::SqlitePool;
    pub async fn ensure(pool: &SqlitePool) -> sqlx::Result<()> {
        crate::storage::db::retry_write(|| async {
            sqlx::query("CREATE TABLE IF NOT EXISTS workspace_recovery_budget(session_id TEXT PRIMARY KEY,attempts INTEGER NOT NULL DEFAULT 0,next_at INTEGER NOT NULL DEFAULT 0,last_error TEXT,settled_reason TEXT)").execute(pool).await
        }).await?;
        Ok(())
    }
    pub async fn due(pool: &SqlitePool, session: &str, now: i64) -> sqlx::Result<bool> {
        ensure(pool).await?;
        let row: Option<(i64, i64, Option<String>)> = sqlx::query_as("SELECT attempts,next_at,settled_reason FROM workspace_recovery_budget WHERE session_id=?").bind(session).fetch_optional(pool).await?;
        Ok(row.is_none_or(|(attempts, next, settled)| attempts < 3 && next <= now && settled.is_none()))
    }
    pub async fn fail(pool: &SqlitePool, session: &str, now: i64, error: &str) -> sqlx::Result<i64> {
        ensure(pool).await?;
        sqlx::query_scalar("INSERT INTO workspace_recovery_budget(session_id,attempts,next_at,last_error) VALUES (?,1,?+60,?) ON CONFLICT(session_id) DO UPDATE SET attempts=attempts+1,next_at=?+60*(1 << MIN(attempts,6)),last_error=excluded.last_error RETURNING attempts")
            .bind(session).bind(now).bind(error).bind(now).fetch_one(pool).await
    }
    pub async fn clear(pool: &SqlitePool, session: &str) -> sqlx::Result<()> {
        ensure(pool).await?;
        sqlx::query("DELETE FROM workspace_recovery_budget WHERE session_id=? AND settled_reason IS NULL").bind(session).execute(pool).await?;
        Ok(())
    }
}

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

/// What the canonical PR of a workspace actually means when no committed local
/// `provider_pr_merge` receipt exists: the externally-merged case (a PR merged
/// from the GitHub web UI) and the crash-lost-receipt case both land here.
///
/// CF-CLO-R1 / CF-CLO-R4: only a positive `Merged` answer authorises deletion;
/// `ClosedUnmerged` keeps everything and reports it in plain language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CanonicalPrState {
    Merged { merge_sha: String },
    ClosedUnmerged,
    Open,
    Queued,
    /// M115/R2: the PR still belongs to this delivery but its head moved to a
    /// provider-produced commit. Whether that is the same delivery is a
    /// separate, explicit question ([`UpdatedHeadVerdict`]) — GitHub's
    /// update-branch and the merge queue rewrite the head into a merge commit
    /// whose parents are the session's head plus the default branch, which is
    /// exactly the same work under a new SHA.
    HeadAdvanced { head_sha: String, queued: bool },
    /// The observer could not establish the truth (no remote, network, schema).
    /// This is never a reason to delete or to claim the work is done.
    Unknown(String),
}

/// M115/R2: the read-only answer to "is this new head the same delivery?".
///
/// Only a merge commit that has the already-observed head as one parent and a
/// parent that is on the default branch may continue the delivery. Everything
/// else — a rebase, a force-push, an octopus merge, a lookup failure — stays a
/// stranger and keeps the previous fail-closed `Unknown` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdatedHeadVerdict {
    SameDelivery { synced_from_default_branch: String },
    Foreign(String),
}

/// Read-only authority for "was this session's PR merged?".
///
/// The cleanup pass itself has no network, so the background supervisor supplies
/// a remote-backed implementation and the offline/smoke path supplies
/// [`OfflineCanonicalPrObserver`]. Kept as a trait so the removal decision is
/// testable without a live provider.
#[async_trait::async_trait]
pub(crate) trait CanonicalPrObserver: Send + Sync {
    async fn observe(
        &self,
        repo_root: &Path,
        pr_number: i64,
        pr_url: &str,
        expected_head_sha: &str,
    ) -> Result<CanonicalPrState>;

    /// M108/R4: the CI conclusion of the observed head, when the observer can
    /// read it. `None` keeps the old "still awaiting CI" park; `Some` lets the
    /// reconcile pass hand the waiting session a decision instead of clearing
    /// its wake schedule.
    async fn observe_ci_checks(
        &self,
        _repo_root: &Path,
        _expected_head_sha: &str,
    ) -> Option<Vec<crate::agent::objective::CiCheckState>> {
        None
    }

    /// M115/R2: resolve a moved PR head. The default is `Foreign`, so an
    /// observer that cannot prove the provenance (offline pass, scripted
    /// observer) keeps the old "unknown, back off, never settle" behaviour.
    async fn observe_updated_head(
        &self,
        _repo_root: &Path,
        _pr_number: i64,
        _known_head_sha: &str,
        _actual_head_sha: &str,
    ) -> UpdatedHeadVerdict {
        UpdatedHeadVerdict::Foreign("observer cannot prove the new head's provenance".into())
    }

    /// M114/R4: one batched lookup per repository covers every due candidate
    /// (one `gh pr list --state all` instead of one `gh pr view` per record).
    /// The default keeps per-PR observers — the offline pass and the scripted
    /// tests — behaving exactly as before.
    async fn observe_many(
        &self,
        repo_root: &Path,
        requests: &[PrObservationRequest],
    ) -> std::collections::HashMap<i64, Result<CanonicalPrState>> {
        let mut out = std::collections::HashMap::new();
        for request in requests {
            if out.contains_key(&request.pr_number) {
                continue;
            }
            let state = self
                .observe(repo_root, request.pr_number, "", &request.expected_head_sha)
                .await;
            out.insert(request.pr_number, state);
        }
        out
    }

    /// M114/R6: the shared user-level core budget, read from
    /// `gh api rate_limit` (an endpoint that costs no quota). `None` keeps the
    /// previous policy so offline/scripted observers stay inert.
    async fn observe_rate_limit(
        &self,
    ) -> Option<crate::agent::delivery::github_api_budget::RateLimitSnapshot> {
        None
    }
}

/// M114/R4: one candidate the reconcile pass needs the remote truth for.
#[derive(Debug, Clone)]
pub(crate) struct PrObservationRequest {
    pub pr_number: i64,
    pub expected_head_sha: String,
}

/// No-network observer used by the deterministic offline pass and by tests: it
/// can never claim the work is merged, so the previous "preserve and retry"
/// behaviour is preserved exactly.
pub(crate) struct OfflineCanonicalPrObserver;

#[async_trait::async_trait]
impl CanonicalPrObserver for OfflineCanonicalPrObserver {
    async fn observe(
        &self,
        _repo_root: &Path,
        _pr_number: i64,
        _pr_url: &str,
        _expected_head_sha: &str,
    ) -> Result<CanonicalPrState> {
        Ok(CanonicalPrState::Unknown(
            "closeout pass runs without a remote observer".into(),
        ))
    }
}

/// Remote-backed observer: resolves the same delivery remote the delivery chain
/// uses (logged-in `gh` CLI for GitHub) and asks it for the canonical PR's real
/// state. Any failure is `Unknown`, never a deletion.
pub(crate) struct RemoteCanonicalPrObserver;

#[async_trait::async_trait]
impl CanonicalPrObserver for RemoteCanonicalPrObserver {
    async fn observe(
        &self,
        repo_root: &Path,
        pr_number: i64,
        _pr_url: &str,
        expected_head_sha: &str,
    ) -> Result<CanonicalPrState> {
        use crate::agent::delivery::{gh_remote_for, DeliveryRemote, MergeObservation};
        // A cleaned-up workspace must not make the read-only observation fail:
        // resolve the delivery remote from the recorded repository, and fall
        // back to the process directory, which is enough with `-R owner/repo`.
        let remote = gh_remote_for(repo_root)
            .or_else(|| std::env::current_dir().ok().and_then(|cwd| gh_remote_for(&cwd)));
        let Some(remote) = remote else {
            return Ok(CanonicalPrState::Unknown(
                "no delivery remote is resolvable for this repository".into(),
            ));
        };
        match remote.observe_merge(pr_number as u64, expected_head_sha).await {
            Ok(MergeObservation::Merged { merge_sha }) => Ok(CanonicalPrState::Merged { merge_sha }),
            Ok(MergeObservation::ClosedUnmerged) => Ok(CanonicalPrState::ClosedUnmerged),
            Ok(MergeObservation::OpenSameHead { .. }) => remote
                .observed_queue_arm(pr_number as u64, expected_head_sha)
                .map(|armed| if armed { CanonicalPrState::Queued } else { CanonicalPrState::Open })
                .map_err(|error| anyhow!(error)),
            // M115/R2: a moved head is not a conclusion by itself; the caller
            // asks `observe_updated_head` whether it is the same delivery.
            Ok(MergeObservation::HeadChanged { actual_head }) => {
                Ok(CanonicalPrState::HeadAdvanced { head_sha: actual_head, queued: false })
            }
            Ok(MergeObservation::Unsupported) => Ok(CanonicalPrState::Unknown(
                "provider cannot report merge state".into(),
            )),
            Err(error) => Err(anyhow!("observe canonical PR #{pr_number}: {error}")),
        }
    }

    async fn observe_ci_checks(
        &self,
        repo_root: &Path,
        expected_head_sha: &str,
    ) -> Option<Vec<crate::agent::objective::CiCheckState>> {
        use crate::agent::delivery::gh_remote_for;
        let remote = gh_remote_for(repo_root)
            .or_else(|| std::env::current_dir().ok().and_then(|cwd| gh_remote_for(&cwd)))?;
        remote.observed_ci_checks(expected_head_sha).await.ok()
    }

    /// M115/R2: prove the moved head is the queue/update-branch merge of the
    /// already-observed head with the default branch. Read-only: parents and a
    /// `compare` lookup, no mutation, no label change.
    async fn observe_updated_head(
        &self,
        repo_root: &Path,
        _pr_number: i64,
        known_head_sha: &str,
        actual_head_sha: &str,
    ) -> UpdatedHeadVerdict {
        use crate::agent::delivery::gh_remote_for;
        let Some(remote) = gh_remote_for(repo_root)
            .or_else(|| std::env::current_dir().ok().and_then(|cwd| gh_remote_for(&cwd)))
        else {
            return UpdatedHeadVerdict::Foreign("no delivery remote is resolvable".into());
        };
        use crate::agent::delivery::DeliveryRemote;
        if remote.head_is_explained(known_head_sha, actual_head_sha) {
            UpdatedHeadVerdict::SameDelivery { synced_from_default_branch: "default branch / known delivery history".into() }
        } else {
            UpdatedHeadVerdict::Foreign(format!("cannot explain {actual_head_sha} from delivery head {known_head_sha}"))
        }
    }

    /// M114/R4: one `gh pr list --state all` for the whole repository, shared by
    /// every due candidate, instead of one `gh pr view` (plus a second for the
    /// queue label) per record.
    async fn observe_many(
        &self,
        repo_root: &Path,
        requests: &[PrObservationRequest],
    ) -> std::collections::HashMap<i64, Result<CanonicalPrState>> {
        use crate::agent::delivery::gh_remote_for;
        use std::collections::HashSet;
        let mut out = std::collections::HashMap::new();
        let remote = gh_remote_for(repo_root)
            .or_else(|| std::env::current_dir().ok().and_then(|cwd| gh_remote_for(&cwd)));
        let Some(remote) = remote else {
            let detail = "no delivery remote is resolvable for this repository".to_string();
            for request in requests {
                out.insert(
                    request.pr_number,
                    Ok(CanonicalPrState::Unknown(detail.clone())),
                );
            }
            return out;
        };
        let mut seen = HashSet::new();
        let numbers: Vec<i64> = requests
            .iter()
            .map(|request| request.pr_number)
            .filter(|number| seen.insert(*number))
            .collect();
        match remote.observed_pr_states(&numbers) {
            Ok(states) => {
                for request in requests {
                    if out.contains_key(&request.pr_number) {
                        continue;
                    }
                    let state = match states.get(&request.pr_number) {
                        Some(summary) => canonical_state_from_batch(summary, &request.expected_head_sha),
                        None => CanonicalPrState::Unknown(format!(
                            "PR #{} was not returned by the batched listing",
                            request.pr_number
                        )),
                    };
                    out.insert(request.pr_number, Ok(state));
                }
            }
            Err(error) => {
                for request in requests {
                    out.insert(
                        request.pr_number,
                        Err(anyhow!("observe canonical PRs in one batch: {error}")),
                    );
                }
            }
        }
        out
    }

    /// M114/R6: `gh api rate_limit` costs no core quota, which is exactly why it
    /// is the polling source.
    async fn observe_rate_limit(
        &self,
    ) -> Option<crate::agent::delivery::github_api_budget::RateLimitSnapshot> {
        use crate::agent::delivery::gh_remote_for;
        let cwd = std::env::current_dir().ok()?;
        gh_remote_for(&cwd)?.observed_rate_limit()
    }
}

/// Map one batched `gh pr list` row onto the read-only canonical state.
fn canonical_state_from_batch(
    summary: &crate::agent::delivery::BatchPrState,
    expected_head_sha: &str,
) -> CanonicalPrState {
    match summary.state.to_ascii_uppercase().as_str() {
        // Positive merge evidence is authoritative; the queue may advance the
        // head before merging, so a merged row never needs a head match.
        "MERGED" => match summary.merge_sha.as_deref().map(str::trim) {
            Some(sha) if !sha.is_empty() => CanonicalPrState::Merged {
                merge_sha: sha.to_string(),
            },
            _ => CanonicalPrState::Unknown(
                "GitHub reports MERGED but returned no merge commit SHA".into(),
            ),
        },
        "CLOSED" => CanonicalPrState::ClosedUnmerged,
        "OPEN" if !summary.head_sha.is_empty() && summary.head_sha != expected_head_sha => {
            // M115/R2: the queue's strict-up-to-date sync rewrites the head
            // into a merge commit. Carry the moved head and its arm state so
            // the caller can prove the provenance instead of going silent.
            CanonicalPrState::HeadAdvanced {
                head_sha: summary.head_sha.clone(),
                queued: summary.armed,
            }
        }
        "OPEN" if summary.armed => CanonicalPrState::Queued,
        "OPEN" => CanonicalPrState::Open,
        other => CanonicalPrState::Unknown(format!("unknown GitHub PR state '{other}'")),
    }
}

/// The canonical PR a `delivery_runs` row recorded for this workspace, with the
/// exact identity fields that must agree with the workspace before cleanup may
/// touch anything.
#[derive(Debug, FromRow, Clone)]
struct CanonicalPrReference {
    run_id: String,
    claim_epoch: i64,
    workspace_path: String,
    worktree_identity: String,
    repo_identity: String,
    head_branch: String,
    expected_head_sha: String,
    canonical_pr_number: i64,
    canonical_pr_url: String,
    canonical_head_sha: String,
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

/// Windows `canonicalize` yields verbatim paths (`\\?\C:\...`). `std::fs`
/// accepts them, but child processes do not: `git worktree add
/// "\\?\C:\..."` dies with "could not create leading directories ...
/// Invalid argument". Every path handed to a child process drops it first.
fn external_path(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if !text
        .get(..4)
        .is_some_and(|head| head.eq_ignore_ascii_case(r"\\?\"))
    {
        return path.to_path_buf();
    }
    let rest = &text[4..];
    if rest
        .get(..4)
        .is_some_and(|head| head.eq_ignore_ascii_case("unc\\"))
    {
        return PathBuf::from(format!(r"\\{}", &rest[4..]));
    }
    PathBuf::from(rest)
}

fn external_path_str(path: &Path) -> Result<String> {
    external_path(path)
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow!("path is not valid UTF-8: {}", path.display()))
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(external_path(cwd))
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

/// M161 R1/R2: preserve Git's complete stash object (index + worktree +
/// untracked parent) under a permanent ref before restoring the registered branch.
/// The journal also makes an interrupted repair discoverable on the next attach.
async fn restore_registered_branch(pool: &SqlitePool, row: &WorkspaceRow, worktree: &Path) -> Result<()> {
    let (repo, identity, _) = workspace_identity(worktree)?;
    if repo != row.repo_identity || row.worktree_identity.as_deref() != Some(identity.as_str()) {
        bail!("managed workspace identity mismatch: repo or gitdir changed before repair");
    }
    let pending: Option<(String, String)> = sqlx::query_as(
        "SELECT rescue_ref, phase FROM execution_workspace_rescues WHERE objective_id=? AND phase NOT IN ('applied','conflict') ORDER BY created_at DESC LIMIT 1",
    ).bind(&row.objective_id).fetch_optional(pool).await?;
    let current = git(worktree, &["branch", "--show-current"])?;
    if current == row.branch_name && pending.is_none() { return Ok(()); }
    if !branch_exists(worktree, &row.branch_name) {
        bail!("registered branch is missing; system will retry workspace self-healing");
    }
    let status = git(worktree, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    if status.is_empty() && pending.is_none() {
        git(worktree, &["checkout", &row.branch_name])?;
        return Ok(());
    }
    let (rescue_ref, mut phase) = if let Some(pending) = pending { pending } else {
        // Objective IDs are normally UUIDs, but never let an arbitrary ID form
        // a Git ref or option. Preserve the familiar ID when it is ref-safe.
        let objective = if row.objective_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            row.objective_id.clone()
        } else { digest("workspace-rescue", &[&row.objective_id]) };
        let reference = format!("refs/codefactory/rescue/{objective}/{}-{}", Utc::now().timestamp_millis(), uuid::Uuid::new_v4().simple());
        sqlx::query("INSERT INTO execution_workspace_rescues(rescue_ref,objective_id,phase,created_at,updated_at) VALUES (?,?,'preparing',?,?)")
            .bind(&reference).bind(&row.objective_id).bind(Utc::now().timestamp_millis()).bind(Utc::now().timestamp_millis()).execute(pool).await?;
        (reference, "preparing".into())
    };
    if phase == "preparing" {
        // `stash push` first writes refs/stash and only then cleans files. If
        // interrupted before update-ref, find that exact stash by its marker.
        let marker = format!("codefactory-rescue:{rescue_ref}");
        let find_stash = || -> Result<Option<String>> {
            Ok(git(worktree, &["stash", "list", "--format=%H %gs"])?
                .lines().find(|line| line.ends_with(&marker))
                .and_then(|line| line.split_whitespace().next()).map(str::to_owned))
        };
        let stash = match find_stash()? {
            Some(sha) => sha,
            None => {
                git(worktree, &["-c", "user.name=CodeFactory Rescue", "-c", "user.email=rescue@codefactory.invalid", "stash", "push", "--include-untracked", "-m", &marker])?;
                find_stash()?.ok_or_else(|| anyhow!("rescue snapshot was not created; system will retry self-healing"))?
            }
        };
        git(worktree, &["update-ref", &rescue_ref, &stash])?;
        git(worktree, &["rev-parse", "--verify", &rescue_ref])?;
        rescue_phase(pool, &rescue_ref, "saved").await?;
        phase = "saved".into();
    }
    if phase == "saved" {
        git(worktree, &["checkout", &row.branch_name])?;
        rescue_phase(pool, &rescue_ref, "applying").await?;
        phase = "applying".into();
        match git(worktree, &["stash", "apply", "--index", &rescue_ref]) {
            Ok(_) => phase = "applied".into(),
            Err(error) => {
                tracing::warn!(objective_id=%row.objective_id, %rescue_ref, %error, "workspace rescue reapply conflicted; snapshot retained");
            }
        }
    }
    if phase == "applying" {
        // A failed/interrupted apply never destroys the saved version. Reset
        // only tracked merge debris; remove only files named in the saved
        // untracked tree, never broad `git clean` or ignored user files.
        git(worktree, &["reset", "--hard", "HEAD"])?;
        let mut command = Command::new("git");
        let output = command.no_window().arg("-C").arg(external_path(worktree))
            .args(["ls-tree", "-rz", "--name-only", &format!("{rescue_ref}^3")]).output()?;
        if output.status.success() {
            for name in output.stdout.split(|byte| *byte == 0).filter(|name| !name.is_empty()) {
                let name = std::str::from_utf8(name).context("rescue filename is not UTF-8")?;
                // Git handles literal pathspecs and won't remove tracked or
                // ignored files. Every removed file is already in rescue^3.
                git(worktree, &["--literal-pathspecs", "clean", "-f", "--", name])?;
            }
        } else if git(worktree, &["rev-parse", "--verify", &format!("{rescue_ref}^3")]).is_ok() {
            bail!("cannot enumerate rescued files; system will retry self-healing");
        }
        phase = "conflict".into();
    }
    let detail = if phase == "conflict" {
        format!("改动已完整保存在 {rescue_ref}，自动恢复时有内容冲突，系统已回到原任务分支，会话可以继续。")
    } else {
        format!("改动已完整保存在 {rescue_ref} 并恢复到原任务分支，会话可以继续。")
    };
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE execution_workspaces SET failure_code='workspace_rescued', failure_detail=? WHERE objective_id=?")
        .bind(&detail).bind(&row.objective_id).execute(&mut *tx).await?;
    // Minimal test schemas and non-chat objectives need no messages table.
    let has_messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages'")
        .fetch_one(&mut *tx).await?;
    if has_messages != 0 {
        if let Some(session) = row.session_id.as_deref() {
            sqlx::query("INSERT OR IGNORE INTO messages(id,session_id,role,content,created_at) VALUES (?,?,'assistant',?,?)")
                .bind(format!("workspace-rescue:{rescue_ref}")).bind(session).bind(&detail)
                .bind(Utc::now().timestamp_millis()).execute(&mut *tx).await?;
        }
    }
    sqlx::query("UPDATE execution_workspace_rescues SET phase=?,updated_at=? WHERE rescue_ref=?")
        .bind(&phase).bind(Utc::now().timestamp_millis()).bind(&rescue_ref).execute(&mut *tx).await?;
    tx.commit().await?;
    tracing::info!(objective_id=%row.objective_id, %rescue_ref, %phase, "managed workspace rescue completed");
    Ok(())
}

async fn rescue_phase(pool: &SqlitePool, reference: &str, phase: &str) -> Result<()> {
    sqlx::query("UPDATE execution_workspace_rescues SET phase=?,updated_at=? WHERE rescue_ref=?")
        .bind(phase).bind(Utc::now().timestamp_millis()).bind(reference).execute(pool).await?;
    tracing::info!(rescue_ref=reference, phase, "managed workspace rescue checkpoint");
    Ok(())
}

/// M151 R10: rebuild a managed worktree whose directory disappeared.
fn rebuild_missing_worktree(row: &WorkspaceRow) -> Result<PathBuf> {
    let repo_root = PathBuf::from(&row.repo_root);
    let path = PathBuf::from(&row.worktree_path);
    if !is_git_repository(&repo_root) {
        bail!("repository root {} is no longer a git repository", repo_root.display());
    }
    if !branch_exists(&repo_root, &row.branch_name) {
        git(&repo_root, &["fetch", "origin", &format!("refs/heads/{}:refs/heads/{}", row.branch_name, row.branch_name)])
            .context("cannot restore registered branch from origin")?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A vanished directory leaves stale `worktrees/<name>` metadata behind;
    // pruning it is what lets `worktree add` reuse the same path and branch.
    git(&repo_root, &["worktree", "prune"])?;
    let add_path = external_path_str(&path)?;
    git(
        &repo_root,
        &["worktree", "add", &add_path, &row.branch_name],
    )?;
    path.canonicalize().map_err(|error| {
        anyhow!(
            "rebuilt worktree path is still missing: {}: {error}",
            path.display()
        )
    })
}

fn remote_branch_exists(root: &Path, remote: &str, branch: &str) -> Result<bool> {
    let heads = git(
        root,
        &[
            "ls-remote",
            "--heads",
            remote,
            &format!("refs/heads/{branch}"),
        ],
    )?;
    Ok(!heads.trim().is_empty())
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

/// The canonical PR recorded for this workspace, independent of whether a local
/// merge receipt exists. This is what makes an *external* merge observable: the
/// delivery record already knows the PR number/url/head, the merge receipt is
/// just missing.
async fn load_canonical_pr_reference(
    pool: &SqlitePool,
    candidate: &CleanupCandidate,
) -> Result<Option<CanonicalPrReference>> {
    load_canonical_pr_reference_with_identity(pool, candidate, true).await
}

async fn load_canonical_pr_reference_with_identity(
    pool: &SqlitePool,
    candidate: &CleanupCandidate,
    require_local_identity: bool,
) -> Result<Option<CanonicalPrReference>> {
    let has_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='delivery_runs'",
    )
    .fetch_one(pool)
    .await?;
    if has_table == 0 {
        return Ok(None);
    }
    let rows = sqlx::query_as::<_, CanonicalPrReference>(
        "SELECT delivery_runs.id AS run_id,
                delivery_runs.claim_epoch,
                delivery_runs.workspace_path,
                delivery_runs.worktree_identity,
                delivery_runs.repo_identity,
                delivery_runs.head_branch,
                delivery_runs.expected_head_sha,
                delivery_runs.canonical_pr_number,
                delivery_runs.canonical_pr_url,
                delivery_runs.canonical_head_sha
         FROM delivery_runs
         WHERE delivery_runs.objective_id=?
           AND delivery_runs.canonical_pr_number IS NOT NULL
           AND delivery_runs.canonical_pr_url IS NOT NULL
           AND delivery_runs.canonical_head_sha IS NOT NULL
         ORDER BY delivery_runs.updated_at DESC, delivery_runs.rowid DESC",
    )
    .bind(&candidate.objective_id)
    .fetch_all(pool)
    .await?;

    for row in rows {
        if (require_local_identity && (row.workspace_path != candidate.worktree_path
            || row.worktree_identity != candidate.worktree_identity
            || row.repo_identity != candidate.repo_identity
            || row.head_branch != candidate.branch_name
            || row.expected_head_sha != candidate.head_sha
            || row.canonical_head_sha != candidate.head_sha))
            || row.canonical_pr_number <= 0
            || row.canonical_pr_url.trim().is_empty()
        {
            continue;
        }
        return Ok(Some(row));
    }
    Ok(None)
}

/// M114/R2: durable observation bookkeeping stored on `delivery_runs`.
#[derive(Debug, Clone)]
struct ObservationState {
    concluded_at: Option<i64>,
    next_at: Option<i64>,
    stable_count: i64,
    last_state: Option<String>,
    last_observed_at: i64,
    /// M114b/R2: 30 minutes when the linked objective is waiting for the user,
    /// 0 otherwise.
    min_delay_ms: i64,
}

/// Columns added by M114. `ensure_pr_observation_columns` adds them with the
/// same nullable/idempotent pattern the rest of `delivery_runs` uses, so an
/// existing database keeps every prior row untouched.
const OBSERVATION_COLUMNS: [(&str, &str); 4] = [
    ("pr_observation_concluded_at", "INTEGER"),
    ("pr_observation_next_at", "INTEGER"),
    ("pr_observation_stable_count", "INTEGER NOT NULL DEFAULT 0"),
    ("pr_observation_last_state", "TEXT"),
];

/// Idempotent migration. Returns `false` for a minimal/legacy schema that
/// cannot carry the bookkeeping (some deterministic fixtures only create the
/// handful of columns they need); the pass then keeps the previous behaviour
/// instead of failing.
async fn ensure_pr_observation_columns(pool: &SqlitePool) -> Result<bool> {
    let mut columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('delivery_runs')")
            .fetch_all(pool)
            .await?;
    if columns.is_empty() {
        return Ok(false);
    }
    for (name, definition) in OBSERVATION_COLUMNS {
        if columns.iter().any(|column| column == name) {
            continue;
        }
        sqlx::query(&format!(
            "ALTER TABLE delivery_runs ADD COLUMN {name} {definition}"
        ))
        .execute(pool)
        .await?;
        columns.push(name.to_string());
    }
    Ok(true)
}

async fn load_observation_state(
    pool: &SqlitePool,
    run_id: &str,
    available: bool,
) -> Result<ObservationState> {
    if !available {
        let last_observed_at: i64 =
            sqlx::query_scalar("SELECT COALESCE(last_observed_at,0) FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(pool)
                .await?;
        return Ok(ObservationState {
            concluded_at: None,
            next_at: None,
            stable_count: 0,
            last_state: None,
            last_observed_at,
            min_delay_ms: 0,
        });
    }
    let row: (Option<i64>, Option<i64>, i64, Option<String>, i64) = sqlx::query_as(
        "SELECT pr_observation_concluded_at, pr_observation_next_at,
                COALESCE(pr_observation_stable_count,0), pr_observation_last_state,
                COALESCE(last_observed_at,0)
         FROM delivery_runs WHERE id=?",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    // M114b/R2: a PR that belongs to an objective explicitly waiting for the
    // user is re-observed at most every 30 minutes. Read from durable state so
    // every caller (cleanup pass, recovery supervisor) shares the floor.
    let mut min_delay_ms = 0;
    let has_objectives: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='objectives'",
    )
    .fetch_one(pool)
    .await?;
    if has_objectives != 0 {
        if let Some((status, requires_user_action)) = sqlx::query_as::<_, (String, i64)>(
            "SELECT COALESCE(status,''), COALESCE(requires_user_action,0) FROM objectives
             WHERE id=(SELECT objective_id FROM delivery_runs WHERE id=?)",
        )
        .bind(run_id)
        .fetch_optional(pool)
        .await?
        {
            min_delay_ms = crate::agent::delivery::github_api_budget::min_observation_delay_ms(
                &status,
                requires_user_action != 0,
            );
        }
    }
    Ok(ObservationState {
        concluded_at: row.0,
        next_at: row.1,
        stable_count: row.2,
        last_state: row.3,
        last_observed_at: row.4,
        min_delay_ms,
    })
}

/// R2/R3: persist "what we learned and when to ask again". A changed answer (or
/// an explicit session request, which passes `previous.stable_count = 0`)
/// resets the backoff; an unchanged one doubles it up to one hour; a concluded
/// record keeps its conclusion and never schedules another observation.
async fn conclude_or_schedule(
    pool: &SqlitePool,
    run_id: &str,
    now: i64,
    state_label: &str,
    conclude: bool,
    previous: &ObservationState,
    policy: crate::agent::delivery::github_api_budget::RateLimitPolicy,
) -> Result<()> {
    let unchanged = previous.last_state.as_deref() == Some(state_label);
    let stable = if unchanged { previous.stable_count + 1 } else { 0 };
    let concluded_at = if conclude { Some(now) } else { None };
    let next_at = if conclude {
        None
    } else {
        Some(
            crate::agent::delivery::github_api_budget::next_observation_at(
                now,
                stable.clamp(0, i64::from(u32::MAX)) as u32,
                policy,
            )
            .max(now.saturating_add(previous.min_delay_ms)),
        )
    };
    sqlx::query(
        "UPDATE delivery_runs
         SET last_observed_at=?, pr_observation_last_state=?,
             pr_observation_stable_count=?,
             pr_observation_concluded_at=COALESCE(pr_observation_concluded_at, ?),
             pr_observation_next_at=?
         WHERE id=?",
    )
    .bind(now)
    .bind(state_label)
    .bind(stable)
    .bind(concluded_at)
    .bind(next_at)
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// M114b/R2: true when the run's canonical PR already carries a persisted
/// conclusion (merged, or closed without merge). The recovery supervisor must
/// never re-observe such a run — asking again cannot change the answer.
pub(crate) async fn pr_observation_concluded(pool: &SqlitePool, run_id: &str) -> Result<bool> {
    if !ensure_pr_observation_columns(pool).await? {
        return Ok(false);
    }
    let concluded: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT pr_observation_concluded_at FROM delivery_runs WHERE id=?",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    Ok(concluded.flatten().is_some())
}

/// Record one supervisor observation through the same conclusion/backoff
/// bookkeeping the cleanup pass uses, so both loops share a single answer and
/// neither re-asks about a settled PR.
pub(crate) async fn record_supervisor_pr_observation(
    pool: &SqlitePool,
    run_id: &str,
    state_label: &str,
    conclude: bool,
) -> Result<()> {
    let available = ensure_pr_observation_columns(pool).await?;
    let previous = load_observation_state(pool, run_id, available).await?;
    if previous.concluded_at.is_some() {
        return Ok(());
    }
    conclude_or_schedule(
        pool,
        run_id,
        Utc::now().timestamp_millis(),
        state_label,
        conclude,
        &previous,
        crate::agent::delivery::github_api_budget::current_rate_limit_policy(),
    )
    .await
}

/// Record a remote-observed merge as a `reconciled_committed` local receipt so
/// the ordinary cleanup authority path resumes. Idempotent: a run that already
/// carries a merge receipt is left untouched, so an interrupted closeout simply
/// re-enters the same authority check.
async fn record_reconciled_merge_receipt(
    pool: &SqlitePool,
    reference: &CanonicalPrReference,
    merge_sha: &str,
    owner: &str,
) -> Result<()> {
    let existing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_mutation_intents
         WHERE run_id=? AND rung='provider_pr_merge'
           AND status IN ('committed', 'reconciled_committed')",
    )
    .bind(&reference.run_id)
    .fetch_one(pool)
    .await?;
    if existing > 0 {
        return Ok(());
    }
    let now = Utc::now().timestamp_millis();
    let evidence = serde_json::json!({
        "pr_number": reference.canonical_pr_number,
        "merge_sha": merge_sha,
    })
    .to_string();
    sqlx::query(
        "INSERT INTO delivery_mutation_intents
         (intent_id, run_id, claim_epoch, rung, operation_key, status,
          process_instance, evidence_json, started_at, updated_at)
         VALUES (?, ?, ?, 'provider_pr_merge', ?, 'reconciled_committed',
                 ?, ?, ?, ?)",
    )
    .bind(format!(
        "closeout-reconcile-{}-{}",
        reference.run_id, reference.canonical_pr_number
    ))
    .bind(&reference.run_id)
    .bind(reference.claim_epoch)
    .bind(format!(
        "closeout-provider-pr-merge-{}-{merge_sha}",
        reference.canonical_pr_number
    ))
    .bind(owner)
    .bind(&evidence)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    // CF-CLO-R1: the delivery record must show "merged" (plus the merge commit
    // in the receipt evidence) after a merge, whether CodeFactory or the user
    // merged it. The merge SHA lives in `evidence_json` above.
    sqlx::query("UPDATE delivery_runs SET reached_ceiling='merged', updated_at=? WHERE id=?")
        .bind(now)
        .bind(&reference.run_id)
        .execute(pool)
        .await?;
    Ok(())
}

fn plain_keep_message(pr_number: i64, worktree_path: &str, tail: &str) -> String {
    format!("PR #{pr_number} {tail}，改动保留在 {worktree_path}")
}

/// Decide what a workspace whose local merge receipt is missing really is.
///
/// CF-CLO-R1 (external merge → same closeout), CF-CLO-R3 (nothing is deleted
/// without proof, and what is kept is said in plain words), CF-CLO-R4 (closed
/// without merge → keep everything and say so).
async fn resolve_unrecorded_merge(
    pool: &SqlitePool,
    candidate: &CleanupCandidate,
    observer: &dyn CanonicalPrObserver,
    workspace_container: &Path,
    owner: &str,
) -> Result<CandidateDisposition> {
    let Some(reference) = load_canonical_pr_reference(pool, candidate).await? else {
        return Ok(CandidateDisposition::Preserved {
            code: "workspace_cleanup_merge_unproven",
            detail: format!(
                "no canonical PR is recorded for this managed workspace; 改动保留在 {}",
                candidate.worktree_path
            ),
        });
    };
    let pr_number = reference.canonical_pr_number;
    match observer
        .observe(
            Path::new(&candidate.repo_root),
            pr_number,
            &reference.canonical_pr_url,
            &candidate.head_sha,
        )
        .await
    {
        Ok(CanonicalPrState::Merged { merge_sha }) => {
            record_reconciled_merge_receipt(pool, &reference, &merge_sha, owner).await?;
            // The merge receipt now exists, so the exact same authority check
            // that guards a locally-merged workspace guards this one too: the
            // external merge is settled and cleaned in this very pass.
            match load_cleanup_authority(pool, candidate).await? {
                Some(authority) => {
                    inspect_and_remove_candidate(candidate, workspace_container, authority)
                }
                None => Ok(CandidateDisposition::Preserved {
                    code: "workspace_cleanup_merge_unproven",
                    detail: format!(
                        "远端确认 PR #{pr_number} 已合并为 {merge_sha}，但本地授权尚未对齐；{}",
                        plain_keep_message(pr_number, &candidate.worktree_path, "合并已确认")
                    ),
                }),
            }
        }
        Ok(CanonicalPrState::ClosedUnmerged) => Ok(CandidateDisposition::Preserved {
            code: "workspace_cleanup_pr_closed_unmerged",
            detail: plain_keep_message(pr_number, &candidate.worktree_path, "已关闭但未合并"),
        }),
        Ok(CanonicalPrState::Open | CanonicalPrState::Queued) => Ok(CandidateDisposition::Preserved {
            code: "workspace_cleanup_merge_pending",
            detail: plain_keep_message(pr_number, &candidate.worktree_path, "尚未合并"),
        }),
        // M115/R2: a moved head proves nothing about the merge, so the cleanup
        // pass keeps the workspace exactly as it does for an unknown state.
        Ok(CanonicalPrState::HeadAdvanced { head_sha, .. }) => {
            Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_merge_unproven",
                detail: format!(
                    "PR #{pr_number} 的 head 已移动到 {head_sha}，尚未证明是同一交付；{}",
                    plain_keep_message(pr_number, &candidate.worktree_path, "head 已变化")
                ),
            })
        }
        Ok(CanonicalPrState::Unknown(reason)) => Ok(CandidateDisposition::Preserved {
            code: "workspace_cleanup_merge_unproven",
            detail: format!(
                "暂时无法确认 PR #{pr_number} 的合并状态（{reason}）；{}",
                plain_keep_message(pr_number, &candidate.worktree_path, "状态待确认")
            ),
        }),
        Err(error) => Ok(CandidateDisposition::Preserved {
            code: "workspace_cleanup_merge_unproven",
            detail: format!(
                "核对 PR #{pr_number} 的合并状态失败：{error}；{}",
                plain_keep_message(pr_number, &candidate.worktree_path, "状态待确认")
            ),
        }),
    }
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
            // R3: uncommitted or untracked content means the cleanup stops here,
            // and the reason the user reads names the files and the path.
            let dirty_files = status
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            return Ok(CandidateDisposition::Preserved {
                code: "workspace_cleanup_dirty",
                detail: format!(
                    "这个工作区里还有 {dirty_files} 个文件没提交，清理会丢掉这些改动，所以整个工作区先保留：{}",
                    workspace_path.display()
                ),
            });
        }
        let remove_path = match external_path_str(&workspace_path) {
            Ok(path) => path,
            Err(error) => {
                return Ok(CandidateDisposition::Preserved {
                    code: "workspace_cleanup_remove_failed",
                    detail: error.to_string(),
                })
            }
        };
        if let Err(error) = git(&canonical_repo_root, &["worktree", "remove", &remove_path]) {
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
    // CF-CLO-R2: the remote branch is deleted only after the exact local branch
    // is gone, and only when the remote still publishes that exact name. A
    // remote failure keeps the workspace row, so the next pass resumes here.
    if let Some(remote) = choose_remote(&canonical_repo_root) {
        match remote_branch_exists(&canonical_repo_root, &remote, &candidate.branch_name) {
            Ok(false) => {}
            Ok(true) => {
                if let Err(error) = git(
                    &canonical_repo_root,
                    &["push", &remote, "--delete", &candidate.branch_name],
                ) {
                    return Ok(CandidateDisposition::Preserved {
                        code: "workspace_cleanup_remote_branch_failed",
                        detail: format!(
                            "远端分支 {} 删除失败: {error}",
                            candidate.branch_name
                        ),
                    });
                }
                if remote_branch_exists(&canonical_repo_root, &remote, &candidate.branch_name)
                    .unwrap_or(true)
                {
                    return Ok(CandidateDisposition::Preserved {
                        code: "workspace_cleanup_remote_branch_failed",
                        detail: format!(
                            "远端分支 {} 在删除后仍然存在",
                            candidate.branch_name
                        ),
                    });
                }
            }
            Err(error) => {
                return Ok(CandidateDisposition::Preserved {
                    code: "workspace_cleanup_remote_branch_failed",
                    detail: format!("无法确认远端分支状态: {error}"),
                })
            }
        }
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
    run_cleanup_pass_with(
        pool,
        workspace_container,
        process_instance,
        &OfflineCanonicalPrObserver,
    )
    .await
}

/// Cleanup pass with an explicit canonical-PR observer. The offline entry point
/// above passes [`OfflineCanonicalPrObserver`], so deterministic local callers
/// keep the exact previous behaviour; the background supervisor injects the
/// remote-backed observer so an externally-merged PR is settled too (CF-CLO-R1).
/// Read-only progress reconciliation is independent of mutation takeover. A
/// failed intent check must not hide a PR opened/armed by later session steps.
pub(crate) async fn reconcile_parked_delivery_progress(
    pool: &SqlitePool,
    observer: &dyn CanonicalPrObserver,
    owner: &str,
) -> Result<()> {
    reconcile_parked_delivery_progress_at(pool, observer, owner, Utc::now().timestamp_millis())
        .await
}

/// M114/R2+R3+R4+R6: the same reconciliation with an explicit clock (so the 60s
/// cleanup cadence can be simulated without sleeping) plus the budget guards
/// that stop the pass from re-asking about deliveries that already concluded.
pub(crate) async fn reconcile_parked_delivery_progress_at(
    pool: &SqlitePool,
    observer: &dyn CanonicalPrObserver,
    owner: &str,
    now: i64,
) -> Result<()> {
    let has_runs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='delivery_runs'",
    ).fetch_one(pool).await?;
    if has_runs == 0 { return Ok(()); }
    let observation_columns = ensure_pr_observation_columns(pool).await?;
    // R6: one free `gh api rate_limit` reading at most every five minutes. The
    // endpoint consumes no core quota, so quota awareness cannot itself become
    // a cost.
    if crate::agent::delivery::github_api_budget::rate_limit_refresh_due() {
        if let Some(snapshot) = observer.observe_rate_limit().await {
            crate::agent::delivery::github_api_budget::record_gh_call(
                crate::agent::delivery::github_api_budget::CALLER_RATE_LIMIT,
            );
            crate::agent::delivery::github_api_budget::record_rate_limit(snapshot);
        }
    }
    let policy = crate::agent::delivery::github_api_budget::current_rate_limit_policy();
    if crate::agent::delivery::github_api_budget::record_policy_transition(policy) {
        tracing::info!(
            status = %crate::agent::delivery::github_api_budget::status_line(),
            "background canonical-PR observation policy"
        );
    }
    if !policy.allows_observation() {
        // Below 5% of the shared user budget: stay out of GitHub's way
        // entirely. No conclusion changes; the question is only deferred.
        return Ok(());
    }
    let terminal_cutoff = now
        .saturating_sub(crate::agent::delivery::github_api_budget::terminal_observation_window_ms());
    if observation_columns {
        // R2: persist the conclusion for terminal deliveries outside the
        // observation window, so they stop consuming candidate slots and are
        // never queried again.
        sqlx::query(
            "UPDATE delivery_runs SET pr_observation_concluded_at=?
             WHERE pr_observation_concluded_at IS NULL
               AND COALESCE(pr_observation_last_state,'') <> 'open'
               AND COALESCE(last_observed_at,0) < ?
               AND objective_id IN (
                   SELECT id FROM objectives WHERE status IN ('failed','cancelled','completed'))",
        ).bind(now).bind(terminal_cutoff).execute(pool).await?;
    }
    let candidates = if observation_columns {
        sqlx::query_as::<_, CleanupCandidate>(
            "SELECT w.id, w.objective_id, w.repo_identity, w.repo_root, w.git_common_dir,
                    COALESCE(w.worktree_path,'') AS worktree_path,
                    COALESCE(w.worktree_identity,'') AS worktree_identity, w.branch_name,
                    COALESCE(w.head_sha,'') AS head_sha
             FROM execution_workspaces w JOIN objectives o ON o.id=w.objective_id
             WHERE (o.status IN ('running','active','waiting_system','waiting_core_input','cancelled','failed')
                    OR (o.status='completed' AND EXISTS (
                        SELECT 1 FROM delivery_runs broken WHERE broken.objective_id=o.id
                        AND broken.status IN ('failed','cancelled','platform_incident'))))
               AND w.state IN ('active','delivering','cleanup_pending','closed','incident')
               AND EXISTS (SELECT 1 FROM delivery_runs r WHERE r.objective_id=o.id
                   AND r.canonical_pr_number IS NOT NULL
                   AND (r.lease_expires_at IS NULL OR r.lease_expires_at<=?)
                   AND r.pr_observation_concluded_at IS NULL
                   AND (r.pr_observation_next_at IS NULL OR r.pr_observation_next_at<=?)
                   AND (o.status NOT IN ('failed','cancelled','completed')
                        OR r.pr_observation_last_state='open'
                        OR COALESCE(r.last_observed_at,0)>=?))
             ORDER BY w.updated_at LIMIT 64",
        ).bind(now).bind(now).bind(terminal_cutoff).fetch_all(pool).await?
    } else {
        sqlx::query_as::<_, CleanupCandidate>(
            "SELECT w.id, w.objective_id, w.repo_identity, w.repo_root, w.git_common_dir,
                    COALESCE(w.worktree_path,'') AS worktree_path,
                    COALESCE(w.worktree_identity,'') AS worktree_identity, w.branch_name,
                    COALESCE(w.head_sha,'') AS head_sha
             FROM execution_workspaces w JOIN objectives o ON o.id=w.objective_id
             WHERE (o.status IN ('running','active','waiting_system','waiting_core_input','cancelled','failed')
                    OR (o.status='completed' AND EXISTS (
                        SELECT 1 FROM delivery_runs broken WHERE broken.objective_id=o.id
                        AND broken.status IN ('failed','cancelled','platform_incident'))))
               AND w.state IN ('active','delivering','cleanup_pending','closed','incident')
               AND EXISTS (SELECT 1 FROM delivery_runs r WHERE r.objective_id=o.id
                   AND r.canonical_pr_number IS NOT NULL
                   AND (r.lease_expires_at IS NULL OR r.lease_expires_at<=?))
             ORDER BY w.updated_at LIMIT 64",
        ).bind(now).fetch_all(pool).await?
    };
    let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('objectives')")
        .fetch_all(pool).await?;
    // R4: resolve every due reference first, then ask the provider once per
    // repository instead of once per record.
    let mut pending: Vec<(CleanupCandidate, CanonicalPrReference)> = Vec::new();
    for candidate in candidates {
        if let Some(reference) =
            load_canonical_pr_reference_with_identity(pool, &candidate, false).await?
        {
            pending.push((candidate, reference));
        }
    }
    let mut observations: std::collections::HashMap<usize, Result<CanonicalPrState>> =
        std::collections::HashMap::new();
    if !pending.is_empty() {
        let mut by_root: std::collections::BTreeMap<String, Vec<usize>> =
            std::collections::BTreeMap::new();
        for (index, (candidate, _)) in pending.iter().enumerate() {
            by_root.entry(candidate.repo_root.clone()).or_default().push(index);
        }
        for (repo_root, indexes) in by_root {
            let requests: Vec<PrObservationRequest> = indexes
                .iter()
                .map(|index| PrObservationRequest {
                    pr_number: pending[*index].1.canonical_pr_number,
                    expected_head_sha: pending[*index].1.canonical_head_sha.clone(),
                })
                .collect();
            let observed = observer.observe_many(Path::new(&repo_root), &requests).await;
            crate::agent::delivery::github_api_budget::record_gh_call(
                crate::agent::delivery::github_api_budget::CALLER_OBSERVE,
            );
            for (index, request) in indexes.iter().zip(requests.iter()) {
                let state = match observed.get(&request.pr_number) {
                    Some(Ok(state)) => Ok(state.clone()),
                    Some(Err(error)) => Err(anyhow!("{error}")),
                    None => Ok(CanonicalPrState::Unknown(
                        "batched observation omitted this PR".into(),
                    )),
                };
                observations.insert(*index, state);
            }
        }
    }
    for (index, (candidate, reference)) in pending.iter().enumerate() {
        // Read-only remote truth does not grant replay or cleanup authority.
        let mut state = observations
            .remove(&index)
            .unwrap_or_else(|| Ok(CanonicalPrState::Unknown("not observed".into())));
        let mut reference = reference.clone();
        let previous = load_observation_state(pool, &reference.run_id, observation_columns).await?;
        // M115/R2: the queue's strict sync (or GitHub's update-branch) rewrites
        // the PR head into a merge commit. Before that was treated as an
        // unknown head, so the canonical head froze at the session's commit and
        // M108's CI wake never fired on the new head (field PR #728: 57 minutes
        // of silence). Prove the provenance first; only a merge of *this*
        // delivery with the default branch may continue it.
        let mut head_advanced = false;
        let mut advanced_head: Option<String> = None;
        let advanced_state = match &state {
            Ok(CanonicalPrState::HeadAdvanced { head_sha, queued }) => {
                Some((head_sha.clone(), *queued))
            }
            _ => None,
        };
        if let Some((head_sha, queued)) = advanced_state {
            match observer
                .observe_updated_head(
                    Path::new(&candidate.repo_root),
                    reference.canonical_pr_number,
                    &reference.canonical_head_sha,
                    &head_sha,
                )
                .await
            {
                UpdatedHeadVerdict::SameDelivery {
                    synced_from_default_branch,
                } => {
                    let advanced = sqlx::query(
                        "UPDATE delivery_runs SET canonical_head_sha=?, last_observed_at=?, updated_at=?
                         WHERE id=? AND claim_epoch=? AND expected_head_sha=? AND canonical_head_sha=?
                           AND canonical_pr_number=? AND (lease_expires_at IS NULL OR lease_expires_at<=?)",
                    )
                    .bind(&head_sha)
                    .bind(now)
                    .bind(now)
                    .bind(&reference.run_id)
                    .bind(reference.claim_epoch)
                    .bind(&reference.expected_head_sha)
                    .bind(&reference.canonical_head_sha)
                    .bind(reference.canonical_pr_number)
                    .bind(now)
                    .execute(pool)
                    .await?;
                    if advanced.rows_affected() != 1 {
                        // Takeover or a second observer moved the row; never
                        // guess which head is canonical.
                        continue;
                    }
                    sqlx::query(
                        "INSERT INTO delivery_run_events (id,run_id,event_kind,stage,status,detail_json,process_instance,created_at)
                         VALUES (?,?,'canonical_pr_head_advanced','pr_open','waiting',?,?,?)",
                    )
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(&reference.run_id)
                    .bind(
                        serde_json::json!({
                            "pr_number": reference.canonical_pr_number,
                            "pr_url": reference.canonical_pr_url,
                            "previous_head_sha": reference.canonical_head_sha,
                            "head_sha": head_sha,
                            "synced_from_default_branch": synced_from_default_branch,
                            "queued": queued,
                        })
                        .to_string(),
                    )
                    .bind(owner)
                    .bind(now)
                    .execute(pool)
                    .await?;
                    // Only the observed head moves. `expected_head_sha` stays
                    // the session's own commit, so resuming the delivery does
                    // not require the local clone to know the merge commit
                    // (R3) — the wake prompt tells it to fetch it first.
                    reference.canonical_head_sha = head_sha.clone();
                    advanced_head = Some(head_sha);
                    head_advanced = true;
                    state = Ok(if queued {
                        CanonicalPrState::Queued
                    } else {
                        CanonicalPrState::Open
                    });
                }
                UpdatedHeadVerdict::Foreign(reason) => {
                    state = Ok(CanonicalPrState::Unknown(format!(
                        "PR head moved to {head_sha}: {reason}"
                    )));
                }
            }
        }
        if previous.concluded_at.is_some() {
            continue;
        }
        let objective_status: String = sqlx::query_scalar("SELECT status FROM objectives WHERE id=?")
            .bind(&candidate.objective_id).fetch_one(pool).await?;
        // R2: durable second guard — outside every window in which the
        // conclusion could still change, record the conclusion instead of
        // asking again.
        if observation_columns
            && !crate::agent::delivery::github_api_budget::should_observe(
                &objective_status,
                previous.last_observed_at,
                previous.last_state.as_deref(),
                now,
            )
        {
            conclude_or_schedule(
                pool,
                &reference.run_id,
                now,
                previous.last_state.as_deref().unwrap_or("unknown"),
                true,
                &previous,
                policy,
            )
            .await?;
            continue;
        }
        let state_label = match &state {
            Ok(CanonicalPrState::Merged { .. }) => "merged",
            Ok(CanonicalPrState::ClosedUnmerged) => "closed",
            Ok(CanonicalPrState::Open) => "open",
            Ok(CanonicalPrState::Queued) => "queued",
            Ok(CanonicalPrState::HeadAdvanced { .. })
            | Ok(CanonicalPrState::Unknown(_))
            | Err(_) => "unknown",
        };
        let unchanged = previous
            .last_state
            .as_deref()
            .is_some_and(|last| last == state_label);
        let stable_now = if unchanged { previous.stable_count + 1 } else { 0 };
        // R2: a merged PR is a conclusion. A *closed* PR is too, but only for a
        // terminal objective — a live wait can still see its PR reopened and
        // merged, so that path keeps the (bounded) backoff and concludes at the
        // stable limit instead of going silent on the first closed answer.
        let conclude = matches!(&state, Ok(CanonicalPrState::Merged { .. }))
            || (matches!(&state, Ok(CanonicalPrState::ClosedUnmerged))
                && (crate::agent::delivery::github_api_budget::is_terminal_objective_status(
                    &objective_status,
                ) || previous.min_delay_ms > 0))
            || stable_now >= crate::agent::delivery::github_api_budget::OBSERVATION_STABLE_LIMIT as i64;
        let (stage, status, reached, merged) = match &state {
            Ok(CanonicalPrState::Open) => ("pr_open", "waiting", "pr_open", false),
            Ok(CanonicalPrState::Queued) => ("merge_queued", "waiting", "merge_queued", false),
            Ok(CanonicalPrState::Merged { merge_sha }) if !merge_sha.trim().is_empty() =>
                ("complete", "completed", "merged", true),
            _ => {
                // Unknown/closed/head drift never proves success or authorises
                // deletion; it is remembered and revisited on a backoff.
                conclude_or_schedule(
                    pool,
                    &reference.run_id,
                    now,
                    state_label,
                    conclude,
                    &previous,
                    policy,
                )
                .await?;
                continue;
            }
        };
        if matches!(objective_status.as_str(), "failed" | "completed") && !merged {
            // Historical terminal settlements need positive merge evidence.
            conclude_or_schedule(
                pool,
                &reference.run_id,
                now,
                state_label,
                conclude,
                &previous,
                policy,
            )
            .await?;
            continue;
        }
        if objective_status == "cancelled" && !merged {
            conclude_or_schedule(
                pool,
                &reference.run_id,
                now,
                state_label,
                conclude,
                &previous,
                policy,
            )
            .await?;
            continue;
        }
        // M108/R4: an open PR whose checks already concluded must hand the
        // waiting session a decision. The previous pass re-wrote
        // `delivered_awaiting_ci`, cleared `next_observation_at` and dropped the
        // attention prompt, so a finished red CI (#713: check-rust panic beside
        // the expected trust-root refusal) sat unread until a human transcribed
        // it. `classify_ci_checks` excludes the trust-root refusal per check, so
        // any ordinary failure still wakes the session.
        //
        // M114/R4: only ask for CI checks when a session is actually waiting on
        // them. Terminal objectives (failed/cancelled/completed) can still hold
        // an open PR worth observing, but no live wait depends on its checks, so
        // the extra REST call is skipped.
        let ci_decision = if (matches!(&state, Ok(CanonicalPrState::Open))
            || (head_advanced && matches!(&state, Ok(CanonicalPrState::Queued))))
            && matches!(objective_status.as_str(), "running" | "active" | "waiting_system")
        {
            let reference_label = format!("PR #{}", reference.canonical_pr_number);
            crate::agent::delivery::github_api_budget::record_gh_call(
                crate::agent::delivery::github_api_budget::CALLER_OBSERVE_CI,
            );
            match observer
                .observe_ci_checks(
                    Path::new(&candidate.repo_root),
                    &reference.canonical_head_sha,
                )
                .await
            {
                Some(checks) => {
                    let outcome = crate::agent::objective::classify_ci_checks(&checks, 0);
                    Some((
                        outcome.failure_code().to_string(),
                        crate::agent::objective::ci_outcome_wake_prompt(&reference_label, &outcome),
                    ))
                }
                None => Some((
                    crate::agent::objective::DELIVERED_AWAITING_CI.to_string(),
                    crate::agent::objective::delivered_awaiting_ci_message(&reference_label),
                )),
            }
        } else {
            None
        };
        // M115/R3: a queue-synced head is a merge commit the session's clone may
        // not know. Every non-green decision therefore tells it to start from
        // that head instead of committing on the old one.
        let ci_decision = ci_decision.map(|(code, prompt)| match advanced_head.as_deref() {
            Some(head) if code != crate::agent::objective::DELIVERED_CI_GREEN => (
                code,
                format!(
                    "{prompt}\n\n{}",
                    crate::agent::objective::advanced_head_rebase_note(head)
                ),
            ),
            _ => (code, prompt),
        });
        let sql_text = |value: &str| format!("'{}'", value.replace('\'', "''"));
        let wait_failure_code = ci_decision
            .as_ref()
            .map(|(code, _)| code.as_str())
            .unwrap_or(crate::agent::objective::DELIVERED_AWAITING_CI);
        let wait_attention = ci_decision.as_ref().map(|(code, prompt)| {
            serde_json::json!({
                "kind": code,
                "prompt": prompt,
                "missing_inputs": [],
                "attempted_routes": [],
                "decision_options": [],
                "recommended_option": null,
                "irreversible": false,
                "no_safe_default_reason": null,
                "resume_cursor": null,
            })
            .to_string()
        });
        let wait_next_observation_at = crate::agent::objective::bounded_next_observation_at(
            now,
            now.saturating_add(crate::agent::objective::DELIVERED_CI_REOBSERVE_MS),
        );
        // Positive merge evidence settles delivery-only acceptance even after
        // a real stop. This read-only settlement never resumes stopped work or
        // removes its durable stop fence.
        let run_columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('delivery_runs')")
            .fetch_all(pool).await?;
        let has = |column: &str| run_columns.iter().any(|name| name == column);
        // Historical live inference can be corrected only from the original
        // authorized merged request, never from merge evidence alone.
        let request = if merged {
            sqlx::query_scalar::<_, String>(
                "SELECT m.content FROM objectives o JOIN sessions s ON s.id=o.session_id
                 JOIN messages m ON m.id=o.root_turn_id AND m.session_id=s.id
                 JOIN delivery_runs r ON r.id=o.delivery_run_id AND r.objective_id=o.id
                 WHERE o.id=? AND o.kind='live' AND o.requested_acceptance='live_verification'
                   AND s.delivery_authorized=1 AND m.role='user'
                   AND r.requested_ceiling='through_merge'",
            ).bind(&candidate.objective_id).fetch_optional(pool).await?
        } else { None };
        let repair = request.as_deref().is_some_and(|content| {
            content.to_ascii_lowercase().contains("ceiling: merged")
                && !crate::commands::chat::explicitly_requests_live_acceptance(content)
        });
        let complete = merged && if columns.iter().any(|c| c == "kind") {
            let kind: String = sqlx::query_scalar("SELECT kind FROM objectives WHERE id=?")
                .bind(&candidate.objective_id).fetch_one(pool).await?;
            let requested: String = sqlx::query_scalar(
                "SELECT requested_ceiling FROM delivery_runs WHERE id=?",
            ).bind(&reference.run_id).fetch_one(pool).await?;
            let merged_target: bool = if columns.iter().any(|c| c == "requested_acceptance") {
                let target: String = sqlx::query_scalar("SELECT requested_acceptance FROM objectives WHERE id=?")
                    .bind(&candidate.objective_id).fetch_one(pool).await?;
                target == "merged"
            } else { false };
            (repair || matches!(kind.as_str(), "delivery" | "local_mutation"))
                && (merged_target || matches!(requested.as_str(), "pr_only" | "through_ci_green" | "through_merge"))
        } else { true };
        if matches!(objective_status.as_str(), "failed" | "cancelled") && !complete { continue; }
        let settle_merged = i32::from(complete);
        // M115/R2: a queue-synced head is genuinely new progress, so it must not
        // be swallowed by the "same stage/status twice" idempotence fence that
        // keeps repeated identical observations from spamming events.
        let progress_fence = if head_advanced {
            "1=1"
        } else {
            "NOT (stage=? AND reached_ceiling=? AND status IN ('waiting','completed'))"
        };
        let run_statement = format!(
            "UPDATE delivery_runs SET reached_ceiling=?{}{} WHERE id=? AND claim_epoch=? AND expected_head_sha=?
               AND canonical_head_sha=? AND canonical_pr_number=?
               AND (lease_expires_at IS NULL OR lease_expires_at<=?)
               AND {progress_fence}
               AND (reached_ceiling IN ('local','committed','pushed','pr_open')
                    OR (?='merge_queued' AND reached_ceiling='merge_queued')
                    OR (?='merged' AND (reached_ceiling NOT IN ('release_triggered','deployment_succeeded','live_verified') OR {settle_merged})))",
            if has("stage") { ", stage=?" } else { "" },
            if has("status") { format!(", status=?, wait_class=?, next_action=NULL, failure_signature=NULL{}{}{}", if has("failure_code") { ", failure_code=NULL" } else { "" }, if has("failure_class") { ", failure_class=NULL" } else { "" }, if has("next_action_authorized") && merged { ", next_action_authorized=0" } else { "" }) } else { String::new() },
        );
        let mut run_query = sqlx::query(&run_statement).bind(reached);
        if has("stage") { run_query = run_query.bind(stage); }
        if has("status") {
            run_query = run_query
                .bind(if complete { "completed" } else { "waiting" })
                .bind(if complete { "none" } else { "external_wait" });
        }
        let mut tx = pool.begin().await?;
        // Fence observations against a takeover that acquired a lease while we
        // were reading the provider; this path never grants mutation authority.
        let mut changed_query = run_query.bind(&reference.run_id).bind(reference.claim_epoch)
            .bind(&reference.expected_head_sha).bind(&reference.canonical_head_sha)
            .bind(reference.canonical_pr_number).bind(now);
        if !head_advanced {
            changed_query = changed_query.bind(stage).bind(reached);
        }
        let changed = changed_query.bind(reached).bind(reached).bind(reached)
            .execute(&mut *tx).await?;
        if changed.rows_affected() == 0 {
            if !repair && !merged {
                // M114b/R2: nothing new to write, but this observation must
                // still be remembered — the previous code dropped it, so a
                // record that already matched its own stage/status was asked
                // again on every tick.
                tx.rollback().await?;
                conclude_or_schedule(
                    pool,
                    &reference.run_id,
                    now,
                    state_label,
                    conclude,
                    &previous,
                    policy,
                )
                .await?;
                continue;
            }
            let fenced: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_runs WHERE id=? AND claim_epoch=? AND expected_head_sha=? AND canonical_head_sha=? AND canonical_pr_number=? AND (lease_expires_at IS NULL OR lease_expires_at<=?) AND reached_ceiling='merged'")
                .bind(&reference.run_id).bind(reference.claim_epoch).bind(&reference.expected_head_sha)
                .bind(&reference.canonical_head_sha).bind(reference.canonical_pr_number).bind(now)
                .fetch_one(&mut *tx).await?;
            if fenced != 1 { continue; }
        }
        let evidence = serde_json::json!({"pr_number": reference.canonical_pr_number,
            "pr_url": reference.canonical_pr_url, "head_sha": reference.canonical_head_sha,
            "reached_ceiling": reached,
            "merge_sha": match &state { Ok(CanonicalPrState::Merged { merge_sha }) => Some(merge_sha), _ => None },
        }).to_string();
        let event_id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO delivery_run_events (id,run_id,event_kind,stage,status,detail_json,process_instance,created_at)
            VALUES (?,?,'canonical_pr_progress',?,?,?,?,?)")
            .bind(&event_id).bind(&reference.run_id).bind(stage).bind(status).bind(&evidence)
            .bind(owner).bind(now).execute(&mut *tx).await?;
        // Historical/minimal schemas are supported without inventing columns.
        // Live/release objectives are not completed from merge evidence alone.
        // Delivery tool ceilings are not objective acceptance requirements.
        // Reuse the fenced observation's decision rather than classifying twice.
        let mut sets = vec![format!("status='{}'", if complete { "completed" } else { "waiting_system" })];
        if repair {
            sets.push("kind='delivery'".into());
            sets.push("requested_acceptance='merged'".into());
        }
        for (column, expression) in [
            ("revision", "revision+1".to_string()),
            ("decision_type", format!("'{}'", if complete { "complete" } else { "waiting" })),
            ("reached_acceptance", format!("'{reached}'")),
            ("failure_code", if complete { "NULL".into() } else { sql_text(wait_failure_code) }),
            ("cancellation_provenance", "NULL".into()),
            ("failure_signature", "NULL".into()), ("recovery_owner", "NULL".into()),
            ("remediation_id", "NULL".into()), ("lease_owner", "NULL".into()),
            ("lease_expires_at", "NULL".into()),
            ("next_observation_at", if complete { "NULL".into() } else { wait_next_observation_at.to_string() }),
            ("attention_request_json", if complete { "NULL".into() } else { wait_attention.as_deref().map(sql_text).unwrap_or_else(|| "NULL".into()) }),
            ("evidence_ref", format!("'delivery_run_events:{event_id}'")),
            ("completed_at", if complete { now.to_string() } else { "NULL".into() }),
            ("updated_at", now.to_string()), ("last_progress_at", now.to_string()),
        ] {
            if columns.iter().any(|c| c == column) { sets.push(format!("{column}={expression}")); }
        }
        let pointer = if columns.iter().any(|c| c == "delivery_run_id") {
            // Adopt only the still-latest canonical run for this objective.
            // A concurrent delivery or active executor must fence settlement.
            sets.push("delivery_run_id=?".into());
            " AND ?=(SELECT id FROM delivery_runs WHERE objective_id=objectives.id AND canonical_pr_number IS NOT NULL AND canonical_pr_url IS NOT NULL AND canonical_head_sha IS NOT NULL ORDER BY updated_at DESC, rowid DESC LIMIT 1) AND (lease_expires_at IS NULL OR lease_expires_at<=?) AND NOT COALESCE((delivery_run_id=? AND status=? AND reached_acceptance=? AND failure_code IS NULL),0)"
        } else { "" };
        if objective_status == "completed" {
            tx.commit().await?;
            // Acceptance is immutable; only delivery evidence may advance — but
            // the observation is still recorded so the record stops being asked
            // about once it has a conclusion.
            conclude_or_schedule(
                pool,
                &reference.run_id,
                now,
                state_label,
                conclude,
                &previous,
                policy,
            )
            .await?;
            continue;
        }
        let eligible_statuses = if complete {
            "'running','active','waiting_system','cancelled','failed'"
        } else {
            "'running','active','waiting_system'"
        };
        let statement = format!("UPDATE objectives SET {} WHERE id=? AND status IN ({eligible_statuses}){pointer}", sets.join(","));
        let mut query = sqlx::query(&statement);
        if !pointer.is_empty() { query = query.bind(&reference.run_id); }
        query = query.bind(&candidate.objective_id);
        if !pointer.is_empty() {
            query = query.bind(&reference.run_id).bind(now).bind(&reference.run_id)
                .bind(if complete { "completed" } else { "waiting_system" }).bind(reached);
        }
        if query.execute(&mut *tx).await?.rows_affected() != 1 {
            // M114b/R2: an objective that cannot adopt this progress right now
            // (for example it is parked on `waiting_core_input` or
            // `agent_action_required` and needs the user) must still record the
            // answer and the next due time. Before this, the `continue` threw
            // the observation away, so merged #680 and closed #704/#710 were
            // re-queried on every pass forever, and an open PR under a user wait
            // was re-queried on every single tick.
            tx.rollback().await?;
            conclude_or_schedule(
                pool,
                &reference.run_id,
                now,
                state_label,
                conclude,
                &previous,
                policy,
            )
            .await?;
            continue;
        }
        if complete { mark_objective_terminal_in_tx(&mut tx, &candidate.objective_id, now).await?; }
        let has_objective_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='objective_events'")
            .fetch_one(&mut *tx).await?;
        if has_objective_events != 0 {
            sqlx::query("INSERT INTO objective_events (id,objective_id,revision,event_type,status,decision_type,domain,failure_code,detail_json,created_at)
                SELECT ?,id,revision,'canonical_pr_progress',status,decision_type,domain,failure_code,?,? FROM objectives WHERE id=?")
                .bind(uuid::Uuid::new_v4().to_string()).bind(&evidence).bind(now)
                .bind(&candidate.objective_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        if let Ok(CanonicalPrState::Merged { merge_sha }) = &state {
            record_reconciled_merge_receipt(pool, &reference, merge_sha, owner).await?;
        }
        conclude_or_schedule(
            pool,
            &reference.run_id,
            now,
            state_label,
            conclude,
            &previous,
            policy,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn run_cleanup_pass_with(
    pool: &SqlitePool,
    workspace_container: &Path,
    process_instance: &str,
    observer: &dyn CanonicalPrObserver,
) -> Result<CleanupPassOutcome> {
    ensure_schema(pool).await?;
    let _guard = ALLOCATION_LOCK.lock().await;
    reconcile_parked_delivery_progress(pool, observer, process_instance).await?;
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
                Ok(None) => {
                    // CF-CLO-R1/R3/R4: no committed local receipt. Ask the
                    // read-only observer whether the canonical PR was merged
                    // externally; never delete on an unknown answer.
                    resolve_unrecorded_merge(
                        pool,
                        &candidate,
                        observer,
                        workspace_container,
                        &owner,
                    )
                    .await?
                }
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
        if let Err(error) = reclaim_dead_repo_locks(&pool).await {
            tracing::warn!(%error, "startup repository lock reclamation failed; automatic retry on acquisition");
        }
        loop {
            let sessions: Result<Vec<String>, _> = sqlx::query_scalar(
                "SELECT session_id FROM execution_workspaces
                 WHERE session_id IS NOT NULL AND state IN ('active','allocating','delivering')
                 GROUP BY session_id HAVING COUNT(*)>1",
            ).fetch_all(&pool).await;
            if let Ok(sessions) = sessions {
                for session in sessions {
                    if let Err(error) = repair_session_split_under_repo_lock(&pool, &session, &process_instance).await {
                        tracing::warn!(%error, "split workspace recovery preserved originals; retrying later");
                    }
                }
            }
            match run_cleanup_pass_with(
                &pool,
                &workspace_container,
                &process_instance,
                &RemoteCanonicalPrObserver,
            )
            .await
            {
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

// ── CF-BLD-R2/R8: build-cache ownership and the reclamation supervisor ───────

/// Which managed workspaces have ended, which cache directories have no
/// workspace row at all, and which tasks can still continue.
///
/// R2 reclaims a cache as soon as its task ends. "Ended" is read from the
/// workspace row *and* its objective, rather than guessed from the directory
/// names, because a task that can still be resumed must keep its cache — that
/// is the whole difference between bounding the caches and throwing away work
/// someone still needs.
///
/// The owner id is the workspace directory's path relative to the container.
/// The live layout nests a repo bucket above the workspace
/// (`execution-workspaces/<repo-prefix>/<workspace-prefix>`), so the id has two
/// components; using only the first would have merged every cache under one
/// repo into a single owner.
pub async fn cache_ownership(
    pool: &SqlitePool,
    container: &Path,
) -> Result<(Vec<String>, Vec<String>, Vec<String>, Vec<String>)> {
    let rows = sqlx::query(
        "SELECT w.worktree_path AS worktree_path, w.state AS state, o.status AS objective_status,
         MAX(COALESCE(o.updated_at, 0), COALESCE(o.last_progress_at, 0),
             COALESCE((SELECT MAX(created_at) FROM messages WHERE session_id = o.session_id), 0),
             COALESCE((SELECT MAX(t.created_at) FROM tool_calls t JOIN messages m ON m.id = t.message_id WHERE m.session_id = o.session_id), 0),
             COALESCE((SELECT MAX(updated_at) FROM chat_turn_state WHERE session_id = o.session_id), 0),
             COALESCE((SELECT MAX(CAST((julianday(created_at) - 2440587.5) * 86400000 AS INTEGER)) FROM model_usage_events WHERE session_id = o.session_id), 0)) AS last_activity_ms
         FROM execution_workspaces w
         LEFT JOIN objectives o ON o.id = w.objective_id",
    )
    .fetch_all(pool)
    .await?;
    let mut ended: Vec<String> = Vec::new();
    let mut alive: Vec<String> = Vec::new();
    let mut failed_idle: Vec<String> = Vec::new();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let idle_ms = crate::build_cache::configured_failed_idle_ms();
    for row in rows {
        let worktree_path: String = row.get("worktree_path");
        let state: String = row.get("state");
        let objective_status: Option<String> = row.get("objective_status");
        let Some(owner) =
            crate::build_cache::owner_from_workspace_path(Path::new(&worktree_path), container)
        else {
            continue;
        };
        if objective_status.as_deref() == Some("failed") {
            if crate::build_cache::failed_cache_is_idle(
                objective_status.as_deref(), row.get("last_activity_ms"), now_ms, idle_ms,
            ) {
                failed_idle.push(owner);
            } else {
                alive.push(owner);
            }
        } else if crate::build_cache::workspace_is_ended(&state, objective_status.as_deref()) {
            ended.push(owner);
        } else {
            alive.push(owner);
        }
    }
    // A workspace directory with no row at all is a task that never finished
    // writing its own record; it is reclaimed only once the long idle rule
    // allows it. Derived from the same workspace discovery as the scan, so the
    // ids line up on the nested layout too.
    let orphans = crate::build_cache::workspace_cache_owners(container)
        .into_iter()
        .filter(|owner| !alive.contains(owner) && !ended.contains(owner) && !failed_idle.contains(owner))
        .collect();
    Ok((ended, orphans, alive, failed_idle))
}

/// CF-BLD-R2/R8: reclaim the build caches of finished tasks and the long-idle
/// orphan backlog, then hold the total under the ceiling. Runs on the same
/// cadence as workspace closeout, so "the task ended" and "its cache is gone"
/// stay one decision instead of two hand-synchronised ones.
pub(crate) fn spawn_build_cache_supervisor(pool: SqlitePool, container: PathBuf) {
    tauri::async_runtime::spawn(async move {
        tracing::info!(
            poll_ms = CLEANUP_POLL_INTERVAL.as_millis(),
            "build cache supervisor started"
        );
        let mut first_pass = true;
        loop {
            let trigger = crate::build_cache::sweep_trigger(first_pass);
            first_pass = false;
            match cache_ownership(&pool, &container).await {
                Ok((ended, orphans, alive, failed_idle)) => {
                    let working = container.clone();
                    let sweep = tauri::async_runtime::spawn_blocking(move || {
                        let log = crate::build_cache::AuditLog::new(
                            crate::build_cache::audit_log_path(&working),
                        );
                        crate::build_cache::maintain(
                            &crate::build_cache::MaintenanceRequest {
                                container: &working,
                                ended_owners: &ended,
                                failed_idle_owners: &failed_idle,
                                orphan_owners: &orphans,
                                protected_owners: &alive,
                                idle_before_unix: crate::build_cache::now_unix().saturating_sub(crate::build_cache::ORPHAN_IDLE_SECS),
                                limit_bytes: crate::build_cache::configured_budget_bytes(),
                                trigger,
                            },
                            Some(&log),
                        )
                    })
                    .await;
                    match sweep {
                        Ok(outcome) if !outcome.removed.is_empty() => tracing::info!(
                            removed = outcome.removed.len(),
                            reclaimed_bytes = outcome.reclaimed_bytes,
                            "build cache reclaimed finished tasks"
                        ),
                        Ok(_) => {}
                        Err(error) => tracing::warn!(
                            %error,
                            "build cache sweep failed; retrying later"
                        ),
                    }
                }
                Err(error) => tracing::warn!(
                    %error,
                    "build cache ownership could not be read; retrying later"
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
    sqlx::query("CREATE TABLE IF NOT EXISTS execution_workspace_rescues (rescue_ref TEXT PRIMARY KEY, objective_id TEXT NOT NULL, phase TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)")
        .execute(pool).await.context("ensure workspace rescue journal")?;
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

async fn reclaim_dead_repo_locks(pool: &SqlitePool) -> Result<()> {
    let locks: Vec<(String, String)> = sqlx::query_as("SELECT repo_identity,lease_owner FROM execution_workspace_repo_locks").fetch_all(pool).await?;
    for (repo, owner) in locks {
        let Some((pid, token)) = owner.split_once(':') else { continue; };
        let Ok(pid) = pid.parse::<u32>() else { continue; };
        if !crate::storage::db::process_identity_is_live(pid, Some(token)) {
            sqlx::query("DELETE FROM execution_workspace_repo_locks WHERE repo_identity=? AND lease_owner=?")
                .bind(repo).bind(owner).execute(pool).await?;
        }
    }
    Ok(())
}

async fn acquire_repo_allocation_lock(
    pool: &SqlitePool,
    repo_identity: &str,
    process_instance: &str,
) -> Result<()> {
    let now = Utc::now().timestamp_millis();
    reclaim_dead_repo_locks(pool).await?;
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

/// Give a workspace parked in `incident` one chance to prove it is usable again.
///
/// `mark_incident` and `mark_identity_incident` park a workspace, and nothing
/// ever moved it out: `ATTACHABLE_STATES` excludes `incident` and no production
/// path rewrites that state. Every later setup for the Objective then failed on
/// the not-attachable guard below — silently, because a chat setup abort carries
/// no failure text of its own. Production 2026-09-21: an agent checked out a
/// different branch inside its managed worktree, and from then on the session
/// died on every attempt with nothing to read; recovering it meant editing the
/// row by hand.
///
/// Identity conflicts are usually external and reversible — a branch switched
/// back, a path restored. So re-run exactly the checks that park a workspace,
/// and return it to service only when ALL of them pass. One whose conflict is
/// still real fails a check and stays parked, exactly as before.
async fn recover_incident_if_identity_is_whole(
    pool: &SqlitePool,
    row: WorkspaceRow,
) -> WorkspaceRow {
    if row.state != "incident" {
        return row;
    }
    let Ok(canonical_path) = PathBuf::from(&row.worktree_path).canonicalize() else {
        return row;
    };
    let Ok(branch) = git(&canonical_path, &["branch", "--show-current"]) else {
        return row;
    };
    if branch != row.branch_name {
        return row;
    }
    let Ok((repo_identity, worktree_identity, head_sha)) = workspace_identity(&canonical_path)
    else {
        return row;
    };
    if repo_identity != row.repo_identity {
        return row;
    }
    if row
        .worktree_identity
        .as_deref()
        .is_some_and(|recorded| recorded != worktree_identity)
    {
        return row;
    }
    let now = Utc::now().timestamp_millis();
    let restored = sqlx::query(
        "UPDATE execution_workspaces
         SET state='active', failure_code=CASE WHEN failure_code='workspace_rescued' THEN failure_code END,
             failure_detail=CASE WHEN failure_code='workspace_rescued' THEN failure_detail END,
             worktree_identity=?, head_sha=?, updated_at=?
         WHERE objective_id=? AND state='incident'",
    )
    .bind(&worktree_identity)
    .bind(&head_sha)
    .bind(now)
    .bind(&row.objective_id)
    .execute(pool)
    .await;
    if !matches!(restored, Ok(ref result) if result.rows_affected() == 1) {
        return row;
    }
    tracing::info!(
        objective_id = %row.objective_id,
        branch = %row.branch_name,
        "managed workspace recovered from incident: identity checks pass again"
    );
    WorkspaceRow {
        state: "active".into(),
        worktree_identity: Some(worktree_identity),
        head_sha: Some(head_sha),
        ..row
    }
}

/// M151 R10: repair every parked incident workspace once, at startup.
///
/// Each incident is handed to the normal attach path, which self-heals a
/// switched-but-clean branch and rebuilds a vanished worktree from its branch.
/// Whatever still cannot be repaired keeps its incident row and, when the
/// incident belongs to a session, records a user-visible reason against its
/// objective so the session list stops hanging without explanation.
pub async fn heal_incident_workspaces(pool: &SqlitePool) -> Vec<(String, String)> {
    let rows: Vec<(String, String)> = match sqlx::query_as(
        "SELECT objective_id, COALESCE(failure_detail, '') FROM execution_workspaces
         WHERE state='incident' ORDER BY updated_at",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "startup: could not list incident workspaces");
            return Vec::new();
        }
    };
    let store = crate::agent::objective::ObjectiveStore::new(pool.clone());
    let mut unrepaired = Vec::new();
    for (objective_id, previous_detail) in rows {
        let state = match attach_existing(pool, &objective_id, "startup-self-heal").await {
            Ok(Some(row)) => row.state,
            Ok(None) => "missing".to_string(),
            Err(error) => {
                // Prefer the recorded incident detail: that is the actionable
                // sentence ("branch changed with uncommitted changes; start a
                // new session ..."). The attach failure is usually just the
                // state guard, which explains nothing to the user.
                let reason = if previous_detail.trim().is_empty() {
                    error.to_string()
                } else {
                    previous_detail.clone()
                };
                record_incident_reason(&store, &objective_id, &reason).await;
                unrepaired.push((objective_id, reason));
                continue;
            }
        };
        if state == "active" {
            tracing::info!(
                %objective_id,
                "startup: managed workspace self-healed out of incident"
            );
            continue;
        }
        record_incident_reason(&store, &objective_id, &previous_detail).await;
        unrepaired.push((objective_id, previous_detail));
    }
    unrepaired
}

async fn record_incident_reason(
    store: &crate::agent::objective::ObjectiveStore,
    objective_id: &str,
    detail: &str,
) {
    if detail.trim().is_empty() {
        return;
    }
    // A managed workspace only ever belongs to an interactive run, so the
    // reason is recorded on the chat rail the session list already shows.
    if let Err(error) = store
        .record_failure_detail(
            objective_id,
            crate::agent::objective::RecoveryDomain::Chat,
            "managed_workspace_incident",
            "managed_workspace_incident",
            detail,
        )
        .await
    {
        tracing::warn!(%objective_id, %error, "startup: could not record workspace incident reason");
    }
}

/// One-shot startup pass over incident managed workspaces.
pub fn spawn_incident_healer(pool: SqlitePool) {
    tauri::async_runtime::spawn(async move {
        let unrepaired = heal_incident_workspaces(&pool).await;
        if !unrepaired.is_empty() {
            tracing::warn!(
                count = unrepaired.len(),
                "startup: managed workspaces still in incident after self-heal"
            );
        }
    });
}

async fn reattach_inner(
    pool: &SqlitePool,
    row: WorkspaceRow,
    process_instance: &str,
) -> Result<ExecutionWorkspace> {
    // M161: repair parked workspaces before the state guard; dirty changes
    // become durable rescue objects before branch restoration.
    if row.state == "incident" {
        let path = PathBuf::from(&row.worktree_path);
        if path.exists() {
            if let Err(error) = restore_registered_branch(pool, &row, &path).await {
                tracing::warn!(
                    objective_id = %row.objective_id,
                    %error,
                    "incident repair: branch is not restorable yet"
                );
            }
        }
    }
    let row = recover_incident_if_identity_is_whole(pool, row).await;
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
        let add_path = external_path_str(&path)?;
        if let Err(error) = git(
            &repo_root,
            &[
                "worktree",
                "add",
                "-b",
                &row.branch_name,
                &add_path,
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
            // M151 R10: a managed worktree directory can vanish (cache cleanup,
            // user deletion, failed move). Its branch still carries every
            // commit, so the workspace is rebuilt from that branch instead of
            // leaving the session permanently incident.
            match rebuild_missing_worktree(&row) {
                Ok(path) => {
                    tracing::info!(
                        objective_id = %row.objective_id,
                        branch = %row.branch_name,
                        path = %path.display(),
                        "managed workspace self-healed: rebuilt missing worktree from its branch"
                    );
                    path
                }
                Err(rebuild_error) => {
                    let error = anyhow!(
                        "managed workspace path missing: {}: {error}; rebuild failed: {rebuild_error}",
                        path.display()
                    );
                    mark_identity_incident(pool, &row.objective_id, &error.to_string()).await;
                    return Err(error);
                }
            }
        }
    };
    let branch = match git(&canonical_path, &["branch", "--show-current"]) {
        Ok(branch) => branch,
        Err(error) => {
            mark_identity_incident(pool, &row.objective_id, &error.to_string()).await;
            return Err(error);
        }
    };
    let pending_rescue: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_workspace_rescues WHERE objective_id=? AND phase NOT IN ('applied','conflict')")
        .bind(&row.objective_id).fetch_one(pool).await?;
    if branch != row.branch_name || pending_rescue != 0 {
        match restore_registered_branch(pool, &row, &canonical_path).await {
            Ok(()) => {
                tracing::info!(
                    objective_id = %row.objective_id,
                    branch = %row.branch_name,
                    "managed workspace self-healed: switched back to its registered branch"
                );
            }
            Err(error) => {
                mark_identity_incident(pool, &row.objective_id, &error.to_string()).await;
                return Err(error);
            }
        }
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
             failure_code=CASE WHEN failure_code='workspace_rescued' THEN failure_code END,
             failure_detail=CASE WHEN failure_code='workspace_rescued' THEN failure_detail END,
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

/// Uncommitted edits (tracked or untracked) exist only in this worktree: no
/// PR, merge, or push can have captured them, so they always carry forward.
fn workspace_has_uncommitted_changes(worktree_path: &Path) -> bool {
    worktree_path.is_dir()
        && git(worktree_path, &["status", "--porcelain"])
            .is_ok_and(|status| !status.trim().is_empty())
}

/// Commits beyond the recorded base. Whether they still need carrying depends
/// on delivery, which `objective_work_is_already_delivered` settles.
fn workspace_has_commits_beyond_base(worktree_path: &Path, base_sha: &str) -> bool {
    worktree_path.is_dir()
        && git(worktree_path, &["rev-parse", "HEAD"]).is_ok_and(|head| head != base_sha)
}

/// Has this Objective's work actually reached the merged result? Only then is
/// there nothing left to carry forward.
///
/// U26 / CF-WSC-R1 + R5: U24 treated a *recorded* canonical PR number as
/// delivered, so an open, unmerged PR made the next message in the session
/// start a second checkout and open a duplicate PR. Unfinished work is
/// uncommitted edits, commits that were never pushed, and a PR that is still
/// open; only a merge (a proven merge receipt on the delivery record) means the
/// work is finished and the next message may start from the latest main.
async fn objective_work_is_merged(pool: &SqlitePool, objective_id: &str) -> Result<bool> {
    let has_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='delivery_runs'",
    )
    .fetch_one(pool)
    .await?;
    if has_table == 0 {
        return Ok(false);
    }
    let merged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_runs
         WHERE objective_id=? AND canonical_pr_number IS NOT NULL
           AND (reached_ceiling IN ('merged','release_triggered','deployment_succeeded','live_verified')
                OR status='merged')",
    )
    .bind(objective_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    Ok(merged > 0)
}

/// CF-WSC-R4: the code the fresh workspace carries when the previous message's
/// unfinished work could not be continued in place.
pub(crate) const PREVIOUS_WORK_NOT_CARRIED_FORWARD: &str = "previous_work_not_carried_forward";

/// What the continued Objective inherited from the session's previous
/// Objective (CF-WSC-R1): either the workspace itself, or — when unfinished
/// work existed but could not be carried — the plain-language notice the
/// session must show instead of silently starting over (CF-WSC-R4).
#[derive(Debug, Default)]
struct PredecessorHandoff {
    inherited: Option<ExecutionWorkspace>,
    not_carried_forward: Option<ContinuationNotice>,
}

/// Unfinished work the continued Objective could not take over, kept as data so
/// the wording stays in one place and cannot drift from the guard that keeps
/// machine words out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContinuationNotice {
    location: String,
    reason: &'static str,
}

impl ContinuationNotice {
    fn new(location: &str, reason: &'static str) -> Self {
        Self {
            location: location.to_string(),
            reason,
        }
    }

    /// Plain words naming where the previous changes are and what happens next.
    /// A stored path that would itself trip the vocabulary guard is replaced
    /// rather than shown, so this can never leak an internal word.
    fn detail(&self) -> String {
        let location = if crate::agent::failure_summary::assert_no_internal_vocabulary(
            &self.location,
        )
        .is_ok()
        {
            self.location.clone()
        } else {
            "上一次任务留下的目录".to_string()
        };
        format!(
            "上一条消息留下的改动还在 {location}，这一次没能接着用（{}）。现在用的是基于最新主线开出的新目录，那些改动没有被删除；想接着用它们，说一声就行。",
            self.reason
        )
    }
}

/// CF-WSC-R4: record the notice on the workspace the session is about to use, so
/// the user hears where their work is instead of silently starting from an
/// empty checkout.
async fn record_continuation_notice(
    pool: &SqlitePool,
    objective_id: &str,
    notice: &ContinuationNotice,
) -> Result<()> {
    let updated = sqlx::query(
        "UPDATE execution_workspaces
         SET failure_code=?, failure_detail=?, updated_at=?
         WHERE objective_id=?",
    )
    .bind(PREVIOUS_WORK_NOT_CARRIED_FORWARD)
    .bind(notice.detail().chars().take(1000).collect::<String>())
    .bind(Utc::now().timestamp_millis())
    .bind(objective_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        bail!("managed workspace disappeared while recording the continuation notice");
    }
    Ok(())
}

/// U24 + U26 (CF-WSC-R1): a session whose previous Objective already ended must
/// continue in that Objective's workspace for as long as the workspace still
/// holds unfinished work.
///
/// Ending a task used to park its workspace for closeout while the next message
/// in the same session created a new Objective with a brand-new checkout —
/// silently dropping every uncommitted change, even though the UI promised the
/// opposite ("停止后续生成不会撤销已经完成的修改" and "直接回一句「继续」…就可以接着做").
/// U24 covered the stop path. U26 widens the same hand-off to *every* terminal
/// state (`completed`, `cancelled`, `failed`), because the reported case is a
/// task that finished its turn with work still staged and uncommitted, and
/// decides "is there anything left to carry" on whether the work reached a
/// *merge* rather than on whether some PR number is recorded: an open,
/// unmerged PR is unfinished work (CF-WSC-R1), and only a merged result lets
/// the next message start from the latest main (CF-WSC-R5).
///
/// The hand-off re-keys the existing row to the new Objective instead of
/// copying files: same worktree, same branch, same base, same recorded
/// canonical PR — uncommitted edits, commits, and untracked files all survive
/// with no merge step that could conflict or half-apply, and delivery keeps
/// working on the PR the session already has (CF-WSC-R3). Callers hold
/// `ALLOCATION_LOCK` and the repository allocation lock, which is exactly the
/// lock the cleanup pass takes, so either the cleanup sees a `cleanup_pending`
/// row for the ended Objective or it sees an `active` row for the new one —
/// never a workspace it may delete (CF-WSC-R2).
///
/// A clean workspace, one whose work already reached a merge, or an Objective
/// that never owned a workspace yields no hand-off: the caller provisions a
/// fresh checkout. When a workspace *does* hold unfinished work that can no
/// longer be continued in place, the caller receives a plain-language notice
/// instead of silently starting from an empty checkout (CF-WSC-R4).
async fn inherit_predecessor_locked(
    pool: &SqlitePool,
    request: &ExecutionWorkspaceRequest,
    repo_identity: &str,
) -> Result<PredecessorHandoff> {
    let Some(session_id) = request.session_id.as_deref() else {
        return Ok(PredecessorHandoff::default());
    };
    let candidates: Vec<WorkspaceRow> = sqlx::query_as::<_, WorkspaceRow>(
        "SELECT workspace.id, workspace.objective_id, workspace.session_id,
                workspace.repo_identity, workspace.repo_root, workspace.git_common_dir,
                workspace.worktree_path, workspace.worktree_identity,
                workspace.branch_name, workspace.base_ref, workspace.base_sha,
                workspace.head_sha, workspace.state, workspace.lease_owner
         FROM execution_workspaces AS workspace
         JOIN objectives ON objectives.id=workspace.objective_id
         WHERE workspace.session_id=?
           AND workspace.objective_id<>?
           AND workspace.repo_identity=?
           AND workspace.state IN ('allocating', 'active', 'delivering', 'cleanup_pending')
           AND objectives.status IN ('completed', 'cancelled', 'failed', 'legacy_orphan')
         ORDER BY workspace.updated_at DESC
         LIMIT 5",
    )
    .bind(session_id)
    .bind(&request.objective_id)
    .bind(repo_identity)
    .fetch_all(pool)
    .await?;

    let mut not_carried_forward: Option<ContinuationNotice> = None;
    for row in candidates {
        let worktree_path = Path::new(&row.worktree_path);
        // CF-WSC-R4: the row says this Objective left work here, but the
        // directory is gone, so nothing can be verified or carried. Never
        // pretend it was empty and never start again without a word.
        if !worktree_path.is_dir() {
            not_carried_forward.get_or_insert_with(|| {
                ContinuationNotice::new(
                    &row.worktree_path,
                    "那个目录已经不在磁盘上了",
                )
            });
            continue;
        }
        if row.worktree_identity.as_deref().is_none_or(str::is_empty)
            || row.head_sha.as_deref().is_none_or(str::is_empty)
        {
            not_carried_forward.get_or_insert_with(|| {
                ContinuationNotice::new(&row.worktree_path, "那个目录的记录不完整")
            });
            continue;
        }
        // Edits made after a PR was opened are on no PR yet, and a PR that is
        // still open has not captured anything that survived on main: delivery
        // only excuses committed work that actually merged, never the rest.
        let carries_work = workspace_has_uncommitted_changes(worktree_path)
            || (workspace_has_commits_beyond_base(worktree_path, &row.base_sha)
                && !objective_work_is_merged(pool, &row.objective_id).await?);
        if !carries_work {
            continue;
        }
        let now = Utc::now().timestamp_millis();
        let handed_over = sqlx::query(
            "UPDATE execution_workspaces
             SET objective_id=?, state='active', failure_code=NULL, failure_detail=NULL,
                 lease_owner=?, lease_expires_at=?, updated_at=?
             WHERE id=? AND objective_id=?
               AND state IN ('allocating', 'active', 'delivering', 'cleanup_pending')",
        )
        .bind(&request.objective_id)
        .bind(&request.process_instance)
        .bind(now + 120_000)
        .bind(now)
        .bind(&row.id)
        .bind(&row.objective_id)
        .execute(pool)
        .await?;
        if handed_over.rows_affected() != 1 {
            // Lost the race with a cleanup pass or another process: never
            // attach to a workspace somebody else owns, but say so plainly
            // instead of quietly starting over (CF-WSC-R4).
            not_carried_forward.get_or_insert_with(|| {
                ContinuationNotice::new(
                    &row.worktree_path,
                    "那个目录刚刚被别的步骤接管了",
                )
            });
            continue;
        }
        let inherited = load_workspace(pool, &request.objective_id)
            .await?
            .ok_or_else(|| anyhow!("inherited managed workspace disappeared during hand-off"))?;
        tracing::info!(
            objective_id = %request.objective_id,
            inherited_from = %row.objective_id,
            inherited_state = %row.state,
            branch = %inherited.branch_name,
            "continued Objective inherited the previous Objective's managed workspace"
        );
        return Ok(PredecessorHandoff {
            inherited: Some(reattach_inner(pool, inherited, &request.process_instance).await?),
            not_carried_forward: None,
        });
    }
    Ok(PredecessorHandoff {
        inherited: None,
        not_carried_forward,
    })
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
        let add_path = external_path_str(&worktree_path)?;
        git(
            &observed.root,
            &[
                "worktree",
                "add",
                "-b",
                &branch_name,
                &add_path,
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

/// Preserve the complete index/worktree (including untracked files) without
/// changing the user's index or branch. Every recovery write has a rescue ref.
fn rescue_snapshot(row: &WorkspaceRow) -> Result<String> {
    let path = Path::new(&row.worktree_path);
    let index = tempfile::tempdir()?;
    let index_path = index.path().join("index");
    let run = |args: &[&str]| -> Result<String> {
        let output = Command::new("git").no_window().args(args).current_dir(path)
            .env("GIT_INDEX_FILE", &index_path)
            .env("GIT_AUTHOR_NAME", "CodeFactory Recovery")
            .env("GIT_AUTHOR_EMAIL", "recovery@codefactory.invalid")
            .env("GIT_COMMITTER_NAME", "CodeFactory Recovery")
            .env("GIT_COMMITTER_EMAIL", "recovery@codefactory.invalid")
            .output()?;
        if !output.status.success() {
            bail!("workspace rescue failed: {}", String::from_utf8_lossy(&output.stderr));
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    };
    run(&["read-tree", "HEAD"])?;
    run(&["add", "-A"])?;
    let tree = run(&["write-tree"])?;
    let head = git(path, &["rev-parse", "HEAD"])?;
    let snapshot = run(&["commit-tree", &tree, "-p", &head, "-m", "preserve split workspace"])?;
    // Never overwrite the first rescue point on retry.
    let rescue = format!("refs/codefactory/rescue/{}", row.id);
    if git(path, &["rev-parse", "--verify", &rescue]).is_err() {
        git(path, &["update-ref", &rescue, &snapshot])?;
    }
    Ok(snapshot)
}

/// M163/R2: startup and allocation share this recovery path. No directories or
/// branches are removed; conflicts leave all originals and rescue refs intact.
async fn repair_session_split_locked(pool: &SqlitePool, session: &str) -> Result<()> {
    let mut rows: Vec<WorkspaceRow> = sqlx::query_as(
        "SELECT id, objective_id, session_id, repo_identity, repo_root, git_common_dir,
                worktree_path, worktree_identity, branch_name, base_ref, base_sha,
                head_sha, state, lease_owner FROM execution_workspaces
         WHERE session_id=? AND state IN ('active','allocating','delivering')
         ORDER BY created_at ASC, id ASC",
    ).bind(session).fetch_all(pool).await?;
    if rows.len() < 2 { return Ok(()); }
    for row in &rows {
        let path = Path::new(&row.worktree_path);
        if !path.exists() {
            rebuild_missing_worktree(row)?;
            let (_, identity, _) = workspace_identity(path)?;
            sqlx::query("UPDATE execution_workspaces SET worktree_identity=? WHERE id=?")
                .bind(identity).bind(&row.id).execute(pool).await?;
        }
    }
    // Reload identities after rebuilding stale worktree registrations.
    for row in &mut rows {
        row.worktree_identity = sqlx::query_scalar("SELECT worktree_identity FROM execution_workspaces WHERE id=?")
            .bind(&row.id).fetch_one(pool).await?;
    }
    let repo = rows[0].repo_identity.clone();
    for row in &rows {
        let path = Path::new(&row.worktree_path);
        let (actual_repo, identity, _) = workspace_identity(path)?;
        if actual_repo != row.repo_identity || row.worktree_identity.as_deref() != Some(identity.as_str())
            || git(path, &["branch", "--show-current"])? != row.branch_name {
            bail!("split workspace identity changed; preserving originals");
        }
    }
    if rows.iter().any(|row| row.repo_identity != repo) {
        bail!("split session spans repositories; preserving every workspace");
    }
    let dirty: Vec<usize> = rows.iter().enumerate().filter_map(|(i, row)| {
        let path = Path::new(&row.worktree_path);
        (workspace_has_uncommitted_changes(path) || workspace_has_commits_beyond_base(path, &row.base_sha)).then_some(i)
    }).collect();
    let keep = match dirty.first().copied() {
        Some(index) => index,
        None => {
            // M169/R3: when none of the split workspaces has local work, the
            // current one is the workspace the delivery runs in (it owns the
            // canonical PR) — not merely the oldest row.
            let pr_owner: Option<String> = sqlx::query_scalar(
                "SELECT objective_id FROM execution_workspaces
                 WHERE session_id=? AND state IN ('active','allocating','delivering')
                   AND COALESCE(canonical_pr_number, 0) > 0
                 ORDER BY updated_at DESC, id DESC LIMIT 1",
            )
            .bind(session)
            .fetch_optional(pool)
            .await?;
            pr_owner
                .as_ref()
                .and_then(|owner| rows.iter().position(|row| &row.objective_id == owner))
                .unwrap_or(0)
        }
    };
    let keeper = rows.remove(keep);
    let mut snapshot = rescue_snapshot(&keeper)?;
    // Capture all branches before changing any files or durable state.
    let mut snapshots = Vec::new();
    for row in &rows { snapshots.push(rescue_snapshot(row)?); }
    let path = Path::new(&keeper.worktree_path);
    for donor in &snapshots {
        let tree = git(path, &["merge-tree", "--write-tree", &snapshot, donor])?;
        let tree = tree.lines().next().ok_or_else(|| anyhow!("missing recovered tree"))?;
        // Use commit-tree solely as a merge input; the managed branch is unchanged.
        let output = Command::new("git").no_window().current_dir(path)
            .args(["-c", "user.name=CodeFactory Recovery", "-c", "user.email=recovery@codefactory.invalid",
                "commit-tree", tree, "-p", &snapshot, "-p", donor, "-m", "combine split workspace rescue"])
            .output()?;
        if !output.status.success() { bail!("cannot create recovered merge snapshot"); }
        snapshot = String::from_utf8(output.stdout)?.trim().to_string();
    }
    // A clean donor must not erase a keeper's untracked edits when checking
    // out the combined tree. All content is already durable in rescue refs.
    git(path, &["read-tree", "--reset", "-u", &snapshot])?;
    // Leave recovered edits unstaged, preserving the original branch identity.
    git(path, &["reset", "--mixed", "HEAD"])?;
    let mut tx = pool.begin().await?;
    for row in &rows {
        sqlx::query("UPDATE execution_workspaces SET state='closed', lease_owner=NULL, lease_expires_at=NULL, closed_at=?, updated_at=? WHERE id=?")
            .bind(Utc::now().timestamp_millis()).bind(Utc::now().timestamp_millis()).bind(&row.id).execute(&mut *tx).await?;
        let has_turns: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_turn_state'").fetch_one(&mut *tx).await?;
        if has_turns > 0 {
            sqlx::query("UPDATE chat_turn_state SET objective_id=? WHERE objective_id=? AND session_id=?")
                .bind(&keeper.objective_id).bind(&row.objective_id).bind(session).execute(&mut *tx).await?;
        }
        let full_objectives: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('objectives') WHERE name='requires_user_action'")
            .fetch_one(&mut *tx).await?;
        if full_objectives > 0 {
            // System-owned closure: the supervisor already folded this duplicate
            // objective into the keeper, so the record is closed automatically and
            // never becomes a manual handoff waiting for the user.
            sqlx::query("UPDATE objectives SET status='legacy_orphan', decision_type='platform_incident', requires_user_action=0, lease_owner=NULL, lease_expires_at=NULL, recovery_owner=NULL, remediation_id=NULL, next_observation_at=NULL WHERE id=?")
                .bind(&row.objective_id).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE objectives SET status='legacy_orphan' WHERE id=?")
                .bind(&row.objective_id).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn repair_session_split_under_repo_lock(
    pool: &SqlitePool,
    session: &str,
    process: &str,
) -> Result<()> {
    let now = Utc::now().timestamp();
    if !super::workspace_recovery_budget::due(pool, session, now).await? { return Ok(()); }
    let result = async {
        let repo: Option<String> = sqlx::query_scalar("SELECT repo_identity FROM execution_workspaces WHERE session_id=? AND state IN ('active','allocating','delivering') LIMIT 1")
            .bind(session).fetch_optional(pool).await?;
        if let Some(repo) = repo {
            acquire_repo_allocation_lock(pool, &repo, process).await?;
            let result = repair_session_split_locked(pool, session).await;
            release_repo_allocation_lock(pool, &repo, process).await;
            result?;
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    match result {
        Ok(()) => super::workspace_recovery_budget::clear(pool, session).await?,
        Err(error) => {
            let attempts = super::workspace_recovery_budget::fail(pool, session, now, &error.to_string()).await?;
            tracing::warn!(session, attempts, %error, "split workspace recovery failed; bounded automatic retry");
            if attempts >= 3 {
                // M186/R3: asking the user and cancelling the wake are one write.
                super::thin_derive::ask_user_and_cancel(pool, session, &format!("项目工作区无法恢复：{error}。请确认项目目录或远端登记分支仍可访问。")).await?;
            }
        }
    }
    Ok(())
}

pub(crate) async fn settle_missing_terminal_workspace(pool: &SqlitePool, session: &str) -> Result<bool> {
    super::workspace_recovery_budget::ensure(pool).await?;
    if super::thin_derive::unanswered_user(pool, session).await? { return Ok(false); }
    let has: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='thin_external_observations'").fetch_one(pool).await?;
    if has == 0 { return Ok(false); }
    let reason: Option<String> = sqlx::query_scalar("SELECT completed_reason FROM thin_external_observations WHERE session_id=? AND (completed_reason LIKE '%已合并%' OR completed_reason LIKE '%已关闭但未合并%')")
        .bind(session).fetch_optional(pool).await?.flatten();
    let Some(reason) = reason else { return Ok(false); };
    let paths: Vec<String> = sqlx::query_scalar("SELECT worktree_path FROM execution_workspaces WHERE session_id=?").bind(session).fetch_all(pool).await?;
    if paths.is_empty() || paths.iter().any(|path| Path::new(path).exists()) { return Ok(false); }
    // A recovery receipt belongs to the current task only; a later user turn
    // must be able to start new work in the same conversation.
    let stale: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages'").fetch_one(pool).await?;
    if stale > 0 {
        let newer: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id=? AND role='user' AND created_at>COALESCE((SELECT updated_at FROM thin_session_state WHERE session_id=?),0)")
            .bind(session).bind(session).fetch_one(pool).await?;
        if newer > 0 { return Ok(false); }
    }
    let reason = format!("{reason}；登记工作区已删除，收尾已无需工作区，系统完成会话结算（不代表重新执行了 §8 文件清理）。");
    sqlx::query("INSERT INTO workspace_recovery_budget(session_id,settled_reason) VALUES (?,?) ON CONFLICT(session_id) DO UPDATE SET settled_reason=excluded.settled_reason")
        .bind(session).bind(&reason).execute(pool).await?;
    // M186/R3: the terminal state and the wake cancellation commit together.
    super::thin_derive::settle_ended_and_cancel(pool, session, &reason, "本会话记录").await?;
    Ok(true)
}

pub(crate) async fn repair_session_split(pool: &SqlitePool, session: &str, process: &str) -> Result<()> {
    // Cheap gate first: admission runs this on every turn, so a normal session
    // must not pay for schema work or a write transaction.
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_workspaces
         WHERE session_id=? AND state IN ('active','allocating','delivering')",
    )
    .bind(session)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(0);
    if active < 2 {
        return Ok(());
    }
    ensure_schema(pool).await?;
    let _guard = ALLOCATION_LOCK.lock().await;
    repair_session_split_under_repo_lock(pool, session, process).await
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
        // Inheriting needs no fresh base, so a flaky `git fetch` must not stop
        // a task that already ended from continuing in its own workspace.
        let handoff = inherit_predecessor_locked(pool, &request, &repo_identity).await?;
        if let Some(inherited) = handoff.inherited {
            return Ok(inherited);
        }
        let objective_id = request.objective_id.clone();
        let observed = refresh_source_base(seed)?;
        let workspace = allocate_new_locked(pool, request, observed).await?;
        if let Some(notice) = handoff.not_carried_forward {
            record_continuation_notice(pool, &objective_id, &notice).await?;
        }
        Ok(workspace)
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
        external_path, external_path_str, heal_incident_workspaces,
        latest_for_session, mark_objective_terminal_in_tx, release_repo_allocation_lock,
        run_cleanup_pass, verify_objective_workspace, ExecutionWorkspaceRequest,
        PREVIOUS_WORK_NOT_CARRIED_FORWARD,
    };
    use crate::util::no_window::NoWindow;
    use sqlx::SqlitePool;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    #[tokio::test]
    async fn failed_cache_ownership_resets_on_each_persisted_activity_source() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        for sql in [
            "CREATE TABLE execution_workspaces(worktree_path TEXT, state TEXT, objective_id TEXT)",
            "CREATE TABLE objectives(id TEXT, session_id TEXT, status TEXT, updated_at INTEGER, last_progress_at INTEGER)",
            "CREATE TABLE messages(id TEXT, session_id TEXT, created_at INTEGER)",
            "CREATE TABLE tool_calls(message_id TEXT, created_at INTEGER)",
            "CREATE TABLE chat_turn_state(session_id TEXT, updated_at INTEGER)",
            "CREATE TABLE model_usage_events(session_id TEXT, created_at TEXT)",
        ] { sqlx::query(sql).execute(&pool).await.unwrap(); }
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("repo/task");
        std::fs::create_dir_all(workspace.join(".codefactory-cache/cargo-target-test")).unwrap();
        sqlx::query("INSERT INTO execution_workspaces VALUES (?, 'active', 'obj')")
            .bind(workspace.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let old = chrono::Utc::now().timestamp_millis() - 25 * 3_600_000;
        sqlx::query("INSERT INTO objectives VALUES ('obj', 'session', 'failed', ?, NULL)")
            .bind(old).execute(&pool).await.unwrap();
        let ownership = super::cache_ownership(&pool, temp.path()).await.unwrap();
        assert_eq!(ownership.3, vec!["repo/task"]);
        assert!(ownership.1.is_empty());
        sqlx::query("INSERT INTO messages VALUES ('message', 'session', ?)")
            .bind(old).execute(&pool).await.unwrap();
        for activity in [
            "UPDATE messages SET created_at = ?",
            "INSERT INTO tool_calls VALUES ('message', ?)",
            "INSERT INTO chat_turn_state VALUES ('session', ?)",
            "UPDATE objectives SET last_progress_at = ?",
        ] {
            sqlx::query(activity).bind(chrono::Utc::now().timestamp_millis()).execute(&pool).await.unwrap();
            let ownership = super::cache_ownership(&pool, temp.path()).await.unwrap();
            assert!(ownership.3.is_empty(), "{activity}");
            assert_eq!(ownership.2, vec!["repo/task"]);
            sqlx::query("UPDATE messages SET created_at = ?").bind(old).execute(&pool).await.unwrap();
            sqlx::query("DELETE FROM tool_calls").execute(&pool).await.unwrap();
            sqlx::query("DELETE FROM chat_turn_state").execute(&pool).await.unwrap();
            sqlx::query("UPDATE objectives SET last_progress_at = NULL").execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO model_usage_events VALUES ('session', ?)")
            .bind(chrono::Utc::now().to_rfc3339()).execute(&pool).await.unwrap();
        assert!(super::cache_ownership(&pool, temp.path()).await.unwrap().3.is_empty());
        sqlx::query("DELETE FROM model_usage_events").execute(&pool).await.unwrap();
        for status in ["active", "completed", "cancelled"] {
            sqlx::query("UPDATE objectives SET status = ?").bind(status).execute(&pool).await.unwrap();
            let ownership = super::cache_ownership(&pool, temp.path()).await.unwrap();
            assert!(ownership.3.is_empty());
            if status == "active" { assert_eq!(ownership.2, vec!["repo/task"]); }
        }
    }

    /// Read a file as text with line endings normalized to LF: Windows CI
    /// checks worktrees out with core.autocrlf, so dirty files can round-trip
    /// as CRLF through git without the content itself changing.
    fn text(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
    }

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

    /// M114/R3: successive scripted polls must advance the clock. Before the
    /// backoff gate a test could call `reconcile` in a tight loop and observe
    /// every time — that tight loop IS the field bug (17 PRs, 43 `gh pr view`
    /// per minute). Each call here represents the next scheduled cleanup tick.
    async fn reconcile_next_poll(
        pool: &SqlitePool,
        observer: &dyn super::CanonicalPrObserver,
        owner: &str,
    ) -> anyhow::Result<()> {
        use std::sync::atomic::{AtomicI64, Ordering};
        static NEXT_OFFSET_MS: AtomicI64 = AtomicI64::new(0);
        let offset = NEXT_OFFSET_MS.fetch_add(61_000, Ordering::SeqCst);
        super::reconcile_parked_delivery_progress_at(
            pool,
            observer,
            owner,
            chrono::Utc::now().timestamp_millis() + offset,
        )
        .await
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
        let has_revision: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('objectives') WHERE name='revision'")
            .fetch_one(pool).await.unwrap();
        if has_revision == 0 {
            sqlx::query("UPDATE objectives SET status='completed' WHERE id=?")
                .bind(&workspace.objective_id).execute(pool).await.unwrap();
        }
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

    /// Production 2026-09-21: an agent checked out a different branch inside its
    /// managed worktree. The identity check parked the workspace in `incident`,
    /// and nothing ever moved it out — so every later chat setup for that
    /// Objective aborted with no failure text at all, and the session burned
    /// generation after generation until the row was edited by hand.
    ///
    /// Once the branch is back, the workspace is whole and must return to
    /// service on its own.
    #[tokio::test]
    async fn an_incident_workspace_recovers_once_its_identity_is_whole_again() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-incident-recovers").await;
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-incident-recovers", "process-a"),
        )
        .await
        .unwrap();

        // An older build (or a crash mid-heal) parked it while the worktree
        // itself is intact and clean.
        park_as_incident(&pool, "objective-incident-recovers", "branch changed").await;

        let recovered = attach_existing(&pool, "objective-incident-recovers", "process-b")
            .await
            .expect("a workspace whose identity is whole again must return to service")
            .expect("the recovered workspace must be handed back");
        assert_eq!(recovered.branch_name, workspace.branch_name);
        let state: String = sqlx::query_scalar(
            "SELECT state FROM execution_workspaces WHERE objective_id=?",
        )
        .bind("objective-incident-recovers")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "active");
    }

    #[tokio::test]
    async fn m161_dirty_incident_is_rescued_and_continues() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-incident-stays").await;
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-incident-stays", "process-a"),
        ).await.unwrap();
        let path = &workspace.worktree_path;
        // Byte-preservation assertions must not inherit host Git CRLF conversion.
        git(path, &["config", "core.autocrlf", "false"]);
        let registered_head = git(path, &["rev-parse", "HEAD"]);
        git(path, &["checkout", "-b", "still-wrong-branch"]);
        std::fs::write(path.join("base.txt"), "staged\n").unwrap();
        git(path, &["add", "base.txt"]);
        std::fs::write(path.join("base.txt"), "unstaged\n").unwrap();
        std::fs::write(path.join("half-done.txt"), [0, 255, 3]).unwrap();
        std::fs::remove_file(path.join("latest.txt")).unwrap();
        park_as_incident(&pool, &workspace.objective_id, "branch changed").await;
        let recovered = attach_existing(&pool, &workspace.objective_id, "process-b")
            .await.unwrap().unwrap();
        assert_eq!(recovered.state, "active");
        assert_eq!(git(path, &["branch", "--show-current"]), workspace.branch_name);
        assert_eq!(git(path, &["rev-parse", "HEAD"]), registered_head);
        assert_eq!(git(path, &["show", ":base.txt"]), "staged");
        assert_eq!(std::fs::read(path.join("base.txt")).unwrap(), b"unstaged\n");
        assert_eq!(std::fs::read(path.join("half-done.txt")).unwrap(), [0, 255, 3]);
        assert!(!path.join("latest.txt").exists());
        let refs = git(path, &["for-each-ref", "--format=%(refname)", "refs/codefactory/rescue/"]);
        assert!(refs.starts_with("refs/codefactory/rescue/objective-incident-stays/"));
        assert_eq!(git(path, &["show", &format!("{refs}^3:half-done.txt")]).as_bytes(), &[0, 239, 191, 189, 3]);
        attach_existing(&pool, &workspace.objective_id, "process-c").await.unwrap();
        assert_eq!(git(path, &["for-each-ref", "--format=%(refname)", "refs/codefactory/rescue/"]), refs);
    }

    #[tokio::test]
    async fn m161_conflict_preserves_rescue_and_registered_branch() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-conflict").await;
        let workspace = allocate_or_attach(&pool,
            request_for(&root, &container, "objective-conflict", "process-a")).await.unwrap();
        sqlx::query("CREATE TABLE messages(id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, created_at INTEGER)").execute(&pool).await.unwrap();
        let path = &workspace.worktree_path;
        git(path, &["config", "core.autocrlf", "false"]);
        std::fs::write(path.join("base.txt"), "registered\n").unwrap();
        git(path, &["add", "base.txt"]);
        git(path, &["commit", "-m", "registered change"]);
        let head = git(path, &["rev-parse", "HEAD"]);
        git(path, &["checkout", "-b", "foreign", &workspace.base_sha]);
        std::fs::write(path.join("base.txt"), "foreign edit\n").unwrap();
        std::fs::write(path.join("untracked.txt"), "precious\n").unwrap();
        park_as_incident(&pool, &workspace.objective_id, "branch changed").await;
        assert!(heal_incident_workspaces(&pool).await.is_empty());
        assert_eq!(workspace_state(&pool, &workspace.objective_id).await, "active");
        assert_eq!(git(path, &["branch", "--show-current"]), workspace.branch_name);
        assert_eq!(git(path, &["rev-parse", "HEAD"]), head);
        assert_eq!(std::fs::read(path.join("base.txt")).unwrap(), b"registered\n");
        assert!(git(path, &["status", "--porcelain"]).is_empty());
        let rescue = git(path, &["for-each-ref", "--format=%(refname)", "refs/codefactory/rescue/"]);
        assert_eq!(git(path, &["show", &format!("{rescue}:base.txt")]), "foreign edit");
        assert_eq!(git(path, &["show", &format!("{rescue}^3:untracked.txt")]), "precious");
        let notice: String = sqlx::query_scalar("SELECT failure_detail FROM execution_workspaces WHERE objective_id=?")
            .bind(&workspace.objective_id).fetch_one(&pool).await.unwrap();
        assert!(notice.contains(&rescue), "{notice}");
        assert!(notice.contains("可以继续"), "{notice}");
        attach_existing(&pool, &workspace.objective_id, "process-c").await.unwrap();
        let again: String = sqlx::query_scalar("SELECT failure_detail FROM execution_workspaces WHERE objective_id=?")
            .bind(&workspace.objective_id).fetch_one(&pool).await.unwrap();
        assert_eq!(again, notice);
        let messages: Vec<String> = sqlx::query_scalar("SELECT content FROM messages WHERE role='assistant'").fetch_all(&pool).await.unwrap();
        assert_eq!(messages, vec![notice]);
    }

    #[tokio::test]
    async fn m161_interrupted_saved_rescue_resumes_without_new_snapshot() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-interrupted").await;
        let workspace = allocate_or_attach(&pool, request_for(&root, &container, "objective-interrupted", "process-a")).await.unwrap();
        let path = &workspace.worktree_path;
        git(path, &["config", "core.autocrlf", "false"]);
        git(path, &["checkout", "-b", "foreign-interrupted"]);
        std::fs::write(path.join("new.txt"), "retained\n").unwrap();
        let reference = "refs/codefactory/rescue/objective-interrupted/synthetic";
        git(path, &["stash", "push", "-u", "-m", &format!("codefactory-rescue:{reference}")]);
        git(path, &["update-ref", reference, "refs/stash"]);
        sqlx::query("INSERT INTO execution_workspace_rescues VALUES (?,?,'saved',1,1)").bind(reference).bind(&workspace.objective_id).execute(&pool).await.unwrap();
        park_as_incident(&pool, &workspace.objective_id, "interrupted repair").await;
        attach_existing(&pool, &workspace.objective_id, "process-b").await.unwrap();
        assert_eq!(std::fs::read(path.join("new.txt")).unwrap(), b"retained\n");
        assert_eq!(git(path, &["branch", "--show-current"]), workspace.branch_name);
        let phases: Vec<String> = sqlx::query_scalar("SELECT phase FROM execution_workspace_rescues").fetch_all(&pool).await.unwrap();
        assert_eq!(phases, vec!["applied"]);
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
    async fn m175_authorized_dispatch_writes_only_in_managed_workspace() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let before = git(&root, &["status", "--porcelain"]);
        for (index, wording) in ["开始实施：修复合成问题。", "修复合成问题。\n\n# 模板\n只读审查一下，不要修改代码；不要推送。"].iter().enumerate() {
            let (contract, _) = crate::agent::resolve_turn_delivery_authorization(
                None, wording, crate::agent::TurnIntent::default(), true,
            );
            assert_eq!(contract.mode, crate::agent::AgentMode::Execute);
            assert_eq!(contract.capability, crate::agent::TurnCapability::Deliver);
            let workspace = allocate_or_attach(&pool, request(&root, &container, "synthetic-m175")).await.unwrap();
            assert_ne!(workspace.worktree_path, root);
            assert!(workspace.worktree_path.starts_with(container.canonicalize().unwrap()));
            let name = format!("synthetic-m175-{index}.txt");
            std::fs::write(workspace.worktree_path.join(&name), "synthetic managed output").unwrap();
            assert_eq!(std::fs::read_to_string(workspace.worktree_path.join(&name)).unwrap(), "synthetic managed output");
            assert!(!root.join(&name).exists());
        }
        assert_eq!(git(&root, &["status", "--porcelain"]), before);
    }

    #[tokio::test]
    async fn m184_dead_process_lock_is_reclaimed_immediately() {
        let pool = pool_with_objective("objective-dirty-root").await;
        let mut child = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
        if cfg!(windows) { child.args(["/C", "exit 0"]); } else { child.args(["-c", "exit 0"]); }
        let mut child = child.spawn().unwrap();
        let owner = format!("{}:synthetic-start", child.id());
        child.wait().unwrap();
        acquire_repo_allocation_lock(&pool, "synthetic-dead-repo", &owner).await.unwrap();
        acquire_repo_allocation_lock(&pool, "synthetic-dead-repo", "replacement").await.unwrap();
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

    #[test]
    fn external_paths_drop_the_windows_verbatim_prefix_before_reaching_git() {
        assert_eq!(
            external_path(Path::new(r"\\?\C:\Users\me\work")).to_string_lossy(),
            r"C:\Users\me\work"
        );
        assert_eq!(
            external_path(Path::new(r"\\?\UNC\server\share\work")).to_string_lossy(),
            r"\\server\share\work"
        );
        assert_eq!(
            external_path(Path::new("/tmp/plain/work")).to_string_lossy(),
            "/tmp/plain/work"
        );
    }

    #[tokio::test]
    async fn reattach_restores_a_switched_branch_and_continues() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        git(
            &workspace.worktree_path,
            &["checkout", "-q", "-b", "unexpected-branch"],
        );

        let recovered = attach_existing(&pool, "objective-dirty-root", "process-b")
            .await
            .unwrap()
            .expect("a switched but clean worktree is recoverable");

        assert_eq!(recovered.state, "active");
        let branch = git(&workspace.worktree_path, &["branch", "--show-current"]);
        assert_eq!(branch, workspace.branch_name);
        assert_eq!(recovered.worktree_path, workspace.worktree_path);
    }

    #[tokio::test]
    async fn reattach_never_discards_uncommitted_work_on_a_switched_branch() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        git(
            &workspace.worktree_path,
            &["checkout", "-q", "-b", "unexpected-branch"],
        );
        let dirty = Path::new(&workspace.worktree_path).join("uncommitted.txt");
        std::fs::write(&dirty, "keep me").unwrap();

        let recovered = attach_existing(&pool, "objective-dirty-root", "process-b")
            .await.unwrap().unwrap();
        assert_eq!(recovered.state, "active");
        assert_eq!(std::fs::read(&dirty).unwrap(), b"keep me");
        assert_eq!(git(&workspace.worktree_path, &["branch", "--show-current"]), workspace.branch_name);
        assert_eq!(workspace_state(&pool, "objective-dirty-root").await, "active");
    }

    #[tokio::test]
    async fn reattach_rebuilds_a_deleted_worktree_from_its_branch() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        std::fs::remove_dir_all(&workspace.worktree_path).unwrap();
        assert!(!Path::new(&workspace.worktree_path).exists());
        // Windows canonicalization stores `\\?\C:\...`, which git rejects
        // ("could not create leading directories ... Invalid argument").
        assert!(
            !external_path_str(Path::new(&workspace.worktree_path))
                .unwrap()
                .starts_with(r"\\?\"),
            "git must never receive a verbatim path"
        );

        let recovered = attach_existing(&pool, "objective-dirty-root", "process-b")
            .await
            .unwrap()
            .expect("a vanished worktree is rebuilt from its branch");

        assert_eq!(recovered.state, "active");
        assert_eq!(recovered.worktree_path, workspace.worktree_path);
        let branch = git(&workspace.worktree_path, &["branch", "--show-current"]);
        assert_eq!(branch, workspace.branch_name);
        let head = git(&workspace.worktree_path, &["rev-parse", "HEAD"]);
        assert_eq!(head, workspace.base_sha);
    }

    #[tokio::test]
    async fn startup_heal_brings_a_parked_workspace_back_to_active() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-dirty-root").await;
        let workspace = allocate_or_attach(&pool, request(&root, &container, "process-a"))
            .await
            .unwrap();
        git(
            &workspace.worktree_path,
            &["checkout", "-q", "-b", "unexpected-branch"],
        );
        park_as_incident(&pool, "objective-dirty-root", "branch changed").await;

        let unrepaired = heal_incident_workspaces(&pool).await;

        assert!(unrepaired.is_empty(), "{unrepaired:?}");
        let state = workspace_state(&pool, "objective-dirty-root").await;
        assert_eq!(state, "active");
        let branch = git(&workspace.worktree_path, &["branch", "--show-current"]);
        assert_eq!(branch, workspace.branch_name);
    }

    #[tokio::test]
    async fn m161_startup_heals_seven_dirty_incidents_once() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("synthetic-startup-0").await;
        let mut workspaces = Vec::new();
        for index in 0..7 {
            let objective = format!("synthetic-startup-{index}");
            if index != 0 {
                sqlx::query("INSERT INTO objectives(id) VALUES (?)").bind(&objective).execute(&pool).await.unwrap();
            }
            let workspace = allocate_or_attach(&pool, request_for(&root, &container, &objective, "process-a")).await.unwrap();
            git(&workspace.worktree_path, &["checkout", "-b", &format!("foreign-{index}")]);
            std::fs::write(workspace.worktree_path.join("uncommitted.txt"), format!("keep {index}")).unwrap();
            park_as_incident(&pool, &objective, "branch changed with uncommitted changes").await;
            workspaces.push(workspace);
        }
        assert!(heal_incident_workspaces(&pool).await.is_empty());
        for (index, workspace) in workspaces.iter().enumerate() {
            assert_eq!(workspace_state(&pool, &workspace.objective_id).await, "active");
            assert_eq!(git(&workspace.worktree_path, &["branch", "--show-current"]), workspace.branch_name);
            assert_eq!(std::fs::read_to_string(workspace.worktree_path.join("uncommitted.txt")).unwrap(), format!("keep {index}"));
            attach_existing(&pool, &workspace.objective_id, "process-b").await.unwrap();
        }
        assert!(heal_incident_workspaces(&pool).await.is_empty());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_workspace_rescues WHERE phase='applied'").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 7);
    }

    async fn park_as_incident(pool: &SqlitePool, objective_id: &str, detail: &str) {
        sqlx::query(
            "UPDATE execution_workspaces
             SET state='incident', failure_code='workspace_identity_conflict', failure_detail=?
             WHERE objective_id=?",
        )
        .bind(detail)
        .bind(objective_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn workspace_state(pool: &SqlitePool, objective_id: &str) -> String {
        sqlx::query_scalar("SELECT state FROM execution_workspaces WHERE objective_id=?")
            .bind(objective_id)
            .fetch_one(pool)
            .await
            .unwrap()
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
        let state: (String, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT state, failure_code, lease_owner, failure_detail
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind(&workspace.objective_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state.0, "cleanup_pending");
        assert_eq!(state.1.as_deref(), Some("workspace_cleanup_dirty"));
        assert_eq!(state.2, None);
        let detail = state.3.unwrap_or_default();
        assert!(detail.contains("没提交"), "{detail}");
        assert!(
            detail.contains(&workspace.worktree_path.display().to_string()),
            "the kept path must be visible: {detail}"
        );
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

    async fn pool_with_objectives(objective_ids: &[&str]) -> SqlitePool {
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
        for objective_id in objective_ids {
            sqlx::query("INSERT INTO objectives(id) VALUES (?)")
                .bind(objective_id)
                .execute(&pool)
                .await
                .unwrap();
        }
        ensure_schema(&pool).await.unwrap();
        pool
    }

    fn request_for_session(
        root: &Path,
        container: &Path,
        objective_id: &str,
        session_id: &str,
        process_instance: &str,
    ) -> ExecutionWorkspaceRequest {
        ExecutionWorkspaceRequest {
            objective_id: objective_id.into(),
            session_id: Some(session_id.into()),
            source_cwd: root.to_path_buf(),
            workspace_container: container.to_path_buf(),
            process_instance: process_instance.into(),
        }
    }

    /// Exactly what the stop path does: the Objective becomes `cancelled` and
    /// its workspace is handed to the closeout lifecycle in the same
    /// transaction.
    async fn cancel_objective(pool: &SqlitePool, objective_id: &str) {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("UPDATE objectives SET status='cancelled' WHERE id=?")
            .bind(objective_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        mark_objective_terminal_in_tx(
            &mut tx,
            objective_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    async fn workspace_state_of(pool: &SqlitePool, objective_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT state FROM execution_workspaces WHERE objective_id=?")
            .bind(objective_id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    /// Exactly what the completion path does: the Objective becomes `completed`
    /// and its workspace is handed to the closeout lifecycle in the same
    /// transaction.
    async fn complete_objective(pool: &SqlitePool, objective_id: &str) {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("UPDATE objectives SET status='completed' WHERE id=?")
            .bind(objective_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        mark_objective_terminal_in_tx(
            &mut tx,
            objective_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    async fn fail_objective(pool: &SqlitePool, objective_id: &str) {
        sqlx::query("UPDATE objectives SET status='failed' WHERE id=?")
            .bind(objective_id)
            .execute(pool)
            .await
            .unwrap();
    }

    fn commit_file(worktree: &Path, name: &str, contents: &str) {
        std::fs::write(worktree.join(name), contents).unwrap();
        git(worktree, &["add", name]);
        git(worktree, &["commit", "-m", name]);
    }

    /// The canonical PR a previous delivery opened: recorded on the workspace,
    /// with no merge receipt on `delivery_runs`, i.e. still open.
    async fn record_open_pr(pool: &SqlitePool, objective_id: &str, number: i64) {
        sqlx::query(
            "UPDATE execution_workspaces
             SET canonical_pr_number=?, canonical_pr_url=?
             WHERE objective_id=?",
        )
        .bind(number)
        .bind(format!("https://example.invalid/pull/{number}"))
        .bind(objective_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn workspace_pr_number(pool: &SqlitePool, objective_id: &str) -> Option<i64> {
        let recorded: i64 = sqlx::query_scalar(
            "SELECT COALESCE(canonical_pr_number, 0) FROM execution_workspaces
             WHERE objective_id=?",
        )
        .bind(objective_id)
        .fetch_one(pool)
        .await
        .unwrap();
        (recorded > 0).then_some(recorded)
    }

    async fn workspace_failure(pool: &SqlitePool, objective_id: &str) -> (String, String) {
        sqlx::query_as::<_, (String, String)>(
            "SELECT COALESCE(failure_code, ''), COALESCE(failure_detail, '')
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind(objective_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn m184_terminal_missing_workspace_settles_without_git() {
        let pool = pool_with_objectives(&["synthetic-terminal"]).await;
        sqlx::query("CREATE TABLE sessions(id TEXT PRIMARY KEY)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO sessions VALUES ('synthetic-session')").execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE thin_external_observations(session_id TEXT PRIMARY KEY, completed_reason TEXT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations VALUES ('synthetic-session','PR #42 已合并')").execute(&pool).await.unwrap();
        let (_temp, root, _remote, container) = init_repo();
        let workspace = allocate_or_attach(&pool, request_for_session(&root, &container, "synthetic-terminal", "synthetic-session", "synthetic")).await.unwrap();
        std::fs::remove_dir_all(&workspace.worktree_path).unwrap();
        assert!(super::settle_missing_terminal_workspace(&pool, "synthetic-session").await.unwrap());
        let state = crate::agent::thin_state::load(&pool, "synthetic-session").await.unwrap().unwrap();
        assert_eq!(state.state, crate::agent::thin_state::SessionState::Ended);
        assert!(state.reason.unwrap().contains("收尾已无需工作区"));
        assert!(!workspace.worktree_path.exists());
    }

    #[tokio::test]
    async fn m184_missing_split_worktrees_are_rebuilt() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["missing-a", "missing-b"]).await;
        let a = allocate_or_attach(&pool, request_for_session(&root, &container, "missing-a", "missing-session", "a")).await.unwrap();
        let b = allocate_or_attach(&pool, request_for_session(&root, &container, "missing-b", "other", "b")).await.unwrap();
        std::fs::remove_dir_all(&a.worktree_path).unwrap();
        std::fs::remove_dir_all(&b.worktree_path).unwrap();
        sqlx::query("UPDATE execution_workspaces SET session_id='missing-session'").execute(&pool).await.unwrap();
        super::repair_session_split(&pool, "missing-session", "restart").await.unwrap();
        assert!(a.worktree_path.join("latest.txt").is_file());
        assert!(b.worktree_path.join("latest.txt").is_file());
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM execution_workspaces WHERE state='active'").fetch_one(&pool).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn m163_split_recovery_preserves_both_dirty_workspaces() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["split-a", "split-b"]).await;
        let a = allocate_or_attach(&pool, request_for_session(&root, &container, "split-a", "split-session", "old-a")).await.unwrap();
        // Simulate a historical second admission: allocate under another session,
        // then restore the persisted split shape before startup recovery.
        let b = allocate_or_attach(&pool, request_for_session(&root, &container, "split-b", "other-session", "old-b")).await.unwrap();
        std::fs::write(a.worktree_path.join("first.txt"), "first edit\n").unwrap();
        std::fs::write(b.worktree_path.join("second.txt"), "second edit\n").unwrap();
        sqlx::query("UPDATE execution_workspaces SET session_id='split-session', created_at=CASE objective_id WHEN 'split-a' THEN 1 ELSE 2 END")
            .execute(&pool).await.unwrap();
        super::repair_session_split(&pool, "split-session", "restart").await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM execution_workspaces WHERE state='active'")
            .fetch_one(&pool).await.unwrap(), 1);
        // Windows CI checks these worktrees out with core.autocrlf, so a dirty
        // file's line endings legitimately come back as CRLF. The property under
        // test is that the edit survives recovery, not that git kept LF bytes,
        // so compare text with line endings normalized.
        assert_eq!(text(&a.worktree_path.join("first.txt")), "first edit\n");
        assert_eq!(text(&b.worktree_path.join("second.txt")), "second edit\n");
        let rescue = format!("refs/codefactory/rescue/{}", b.id);
        assert_eq!(
            git(&root, &["show", &format!("{rescue}:second.txt")]).replace("\r\n", "\n"),
            "second edit"
        );
        super::repair_session_split(&pool, "split-session", "restart").await.unwrap();
        assert_eq!(workspace_state_of(&pool, "split-b").await.as_deref(), Some("closed"));
    }

    /// M169/R3: a session that already split keeps the workspace its PR lives
    /// in, even when none of the copies has local edits — the empty duplicate is
    /// the one that must close.
    #[tokio::test]
    async fn split_recovery_keeps_the_workspace_that_owns_the_pull_request() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["pr-split-a", "pr-split-b"]).await;
        let _a = allocate_or_attach(
            &pool,
            request_for_session(&root, &container, "pr-split-a", "pr-split-session", "p-a"),
        )
        .await
        .unwrap();
        let b = allocate_or_attach(
            &pool,
            request_for_session(&root, &container, "pr-split-b", "other-session", "p-b"),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE execution_workspaces SET session_id='pr-split-session'")
            .execute(&pool)
            .await
            .unwrap();
        record_open_pr(&pool, "pr-split-b", 861).await;

        super::repair_session_split(&pool, "pr-split-session", "restart")
            .await
            .unwrap();

        assert_eq!(
            workspace_state_of(&pool, "pr-split-b").await.as_deref(),
            Some("active"),
            "the PR workspace must stay current"
        );
        assert_eq!(
            workspace_state_of(&pool, "pr-split-a").await.as_deref(),
            Some("closed"),
            "the empty duplicate must close"
        );
        let active_path: String = sqlx::query_scalar(
            "SELECT worktree_path FROM execution_workspaces
             WHERE session_id='pr-split-session' AND state='active'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            active_path,
            b.worktree_path.to_string_lossy().to_string(),
            "the surviving mutable workspace must be the PR workspace"
        );
    }

    /// U24 (a): the user stops a task that has uncommitted work, then sends
    /// another message in the same session. The next Objective must continue in
    /// the same worktree/branch instead of starting over in an empty checkout.
    #[tokio::test]
    async fn continuing_after_a_stop_inherits_the_uncommitted_workspace() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-stop-a", "objective-stop-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-stop-a", "process-a"),
        )
        .await
        .unwrap();
        std::fs::write(workspace_a.worktree_path.join("draft.txt"), "half done\n").unwrap();
        cancel_objective(&pool, "objective-stop-a").await;

        let workspace_b = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-stop-b", "process-b"),
        )
        .await
        .unwrap();

        assert_eq!(
            workspace_b.worktree_path, workspace_a.worktree_path,
            "the continued Objective must reuse the stopped Objective's worktree"
        );
        assert_eq!(workspace_b.branch_name, workspace_a.branch_name);
        assert_eq!(workspace_b.objective_id, "objective-stop-b");
        assert_eq!(
            std::fs::read_to_string(workspace_b.worktree_path.join("draft.txt")).unwrap(),
            "half done\n",
            "the uncommitted change must be inside the continued workspace"
        );
        assert_eq!(
            workspace_state_of(&pool, "objective-stop-b").await.as_deref(),
            Some("active")
        );
        assert_eq!(
            workspace_state_of(&pool, "objective-stop-a").await,
            None,
            "the workspace belongs to the continued Objective now"
        );
    }

    /// M169: a settled turn (`legacy_orphan`) whose delivery is still open is
    /// the same task. If any entry does create the next Objective, it must still
    /// inherit the one managed workspace and its canonical PR — otherwise the
    /// delivery continues on a branch nobody edited.
    #[tokio::test]
    async fn a_legacy_orphan_with_an_open_delivery_hands_its_workspace_forward() {
        let (_temp, root, _remote, container) = init_repo();
        let pool =
            pool_with_objectives(&["objective-orphan-a", "objective-orphan-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for_session(&root, &container, "objective-orphan-a", "orphan-session", "p-a"),
        )
        .await
        .unwrap();
        std::fs::write(workspace_a.worktree_path.join("draft.txt"), "unfinished fix\n").unwrap();
        record_open_pr(&pool, "objective-orphan-a", 861).await;

        // Exactly what the settled chat turn does.
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("UPDATE objectives SET status='legacy_orphan' WHERE id='objective-orphan-a'")
            .execute(&mut *tx)
            .await
            .unwrap();
        mark_objective_terminal_in_tx(
            &mut tx,
            "objective-orphan-a",
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let workspace_b = allocate_or_attach(
            &pool,
            request_for_session(&root, &container, "objective-orphan-b", "orphan-session", "p-b"),
        )
        .await
        .unwrap();

        assert_eq!(
            workspace_b.worktree_path, workspace_a.worktree_path,
            "an open delivery keeps its worktree even after the turn settled"
        );
        assert_eq!(workspace_b.branch_name, workspace_a.branch_name);
        assert_eq!(
            workspace_pr_number(&pool, "objective-orphan-b").await,
            Some(861),
            "the canonical PR belongs to the continued task"
        );
        assert_eq!(
            std::fs::read_to_string(workspace_b.worktree_path.join("draft.txt")).unwrap(),
            "unfinished fix\n"
        );
    }

    /// U24 (b) regression guard: after a *failure* the same Objective is
    /// reactivated and keeps its workspace; a different Objective must not
    /// steal it, and the failure path must not be re-routed through the
    /// stop-and-inherit rule.
    #[tokio::test]
    async fn a_failed_objective_keeps_its_workspace_and_others_do_not_inherit_it() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-failed-a", "objective-failed-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-failed-a", "process-a"),
        )
        .await
        .unwrap();
        std::fs::write(workspace_a.worktree_path.join("draft.txt"), "failed mid-flight\n").unwrap();
        sqlx::query("UPDATE objectives SET status='failed' WHERE id='objective-failed-a'")
            .execute(&pool)
            .await
            .unwrap();

        let resumed = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-failed-a", "process-b"),
        )
        .await
        .unwrap();
        assert_eq!(resumed.worktree_path, workspace_a.worktree_path);
        assert_eq!(resumed.objective_id, "objective-failed-a");

        let workspace_b = allocate_or_attach(
            &pool,
            request_for_session(
                &root,
                &container,
                "objective-failed-b",
                "session-unrelated",
                "process-c",
            ),
        )
        .await
        .unwrap();
        assert_ne!(
            workspace_b.worktree_path, workspace_a.worktree_path,
            "another session must not inherit a failed Objective's workspace"
        );
        assert_eq!(
            std::fs::read_to_string(workspace_a.worktree_path.join("draft.txt")).unwrap(),
            "failed mid-flight\n"
        );
    }

    /// U24 (c): the ended Objective's work already reached a merge, so there is
    /// nothing to carry over — the next Objective gets a clean workspace.
    #[tokio::test]
    async fn continuing_after_a_delivered_stop_starts_fresh() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-delivered-a", "objective-delivered-b"]).await;

        // (c1) merged: the closeout already proved the merge.
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-delivered-a", "process-a"),
        )
        .await
        .unwrap();
        std::fs::write(workspace_a.worktree_path.join("shipped.txt"), "shipped\n").unwrap();
        git(&workspace_a.worktree_path, &["add", "shipped.txt"]);
        git(&workspace_a.worktree_path, &["commit", "-m", "shipped"]);
        record_terminal_merge_receipt(&pool, &workspace_a).await;

        let workspace_b = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-delivered-b", "process-b"),
        )
        .await
        .unwrap();
        assert_ne!(workspace_b.worktree_path, workspace_a.worktree_path);

    }

    /// U26 / CF-WSC-R1 + R3. This case deliberately replaces the U24 assertion
    /// that a recorded PR number meant "already delivered": a PR that is open
    /// and unmerged is unfinished work, so the next message in the session
    /// continues in the same workspace, on the same branch, and keeps working
    /// toward the same PR instead of opening a duplicate.
    #[tokio::test]
    async fn continuing_after_an_open_pr_keeps_the_same_workspace_and_pr() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-open-pr-a", "objective-open-pr-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-open-pr-a", "process-a"),
        )
        .await
        .unwrap();
        commit_file(&workspace_a.worktree_path, "pr.txt", "on a PR\n");
        record_open_pr(&pool, "objective-open-pr-a", 7).await;
        cancel_objective(&pool, "objective-open-pr-a").await;

        let workspace_b = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-open-pr-b", "process-b"),
        )
        .await
        .unwrap();

        assert_eq!(
            workspace_b.worktree_path, workspace_a.worktree_path,
            "an open, unmerged PR is unfinished work and must not be abandoned"
        );
        assert_eq!(
            workspace_b.branch_name, workspace_a.branch_name,
            "the same branch is what keeps delivery pointing at the same PR"
        );
        assert_eq!(
            workspace_pr_number(&pool, "objective-open-pr-b").await,
            Some(7),
            "the PR the session already has must travel with the workspace"
        );
    }

    /// U24 (c3, review): a PR captures only what was committed and pushed. An
    /// edit made after the PR opened lives in this worktree alone, so stopping
    /// and continuing must carry it forward instead of starting fresh.
    #[tokio::test]
    async fn continuing_after_a_stop_keeps_edits_made_after_the_pr_opened() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-pr-edit-a", "objective-pr-edit-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-pr-edit-a", "process-a"),
        )
        .await
        .unwrap();
        std::fs::write(workspace_a.worktree_path.join("on-pr.txt"), "delivered\n").unwrap();
        git(&workspace_a.worktree_path, &["add", "on-pr.txt"]);
        git(&workspace_a.worktree_path, &["commit", "-m", "delivered"]);
        sqlx::query(
            "UPDATE execution_workspaces
             SET canonical_pr_number=9, canonical_pr_url='https://example.invalid/pull/9'
             WHERE objective_id='objective-pr-edit-a'",
        )
        .execute(&pool)
        .await
        .unwrap();
        std::fs::write(
            workspace_a.worktree_path.join("review-fix.txt"),
            "made after the PR opened\n",
        )
        .unwrap();
        cancel_objective(&pool, "objective-pr-edit-a").await;

        let workspace_b = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-pr-edit-b", "process-b"),
        )
        .await
        .unwrap();
        assert_eq!(
            workspace_b.worktree_path, workspace_a.worktree_path,
            "an uncommitted edit is on no PR yet and must not be dropped"
        );
        assert!(workspace_b.worktree_path.join("review-fix.txt").is_file());
    }

    /// U24 (d): the stopped Objective left nothing behind, so a clean workspace
    /// is provisioned instead of reusing an empty worktree.
    #[tokio::test]
    async fn continuing_after_a_clean_stop_starts_fresh() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-clean-a", "objective-clean-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-clean-a", "process-a"),
        )
        .await
        .unwrap();
        cancel_objective(&pool, "objective-clean-a").await;

        let workspace_b = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-clean-b", "process-b"),
        )
        .await
        .unwrap();

        assert_ne!(workspace_b.worktree_path, workspace_a.worktree_path);
        assert_eq!(
            workspace_state_of(&pool, "objective-clean-a").await.as_deref(),
            Some("cleanup_pending"),
            "an empty stopped workspace stays with the closeout lifecycle"
        );
    }

    /// U24 (e): once the workspace is handed to the continued Objective, the
    /// cleanup pass must not close it out from under the running turn.
    #[tokio::test]
    async fn cleanup_does_not_touch_a_workspace_handed_to_a_continued_objective() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-hand-a", "objective-hand-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-hand-a", "process-a"),
        )
        .await
        .unwrap();
        let draft = workspace_a.worktree_path.join("draft.txt");
        std::fs::write(&draft, "still working\n").unwrap();
        cancel_objective(&pool, "objective-hand-a").await;

        let workspace_b = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-hand-b", "process-b"),
        )
        .await
        .unwrap();
        assert_eq!(workspace_b.worktree_path, workspace_a.worktree_path);

        let outcome = run_cleanup_pass(&pool, &container, "cleanup-process").await.unwrap();

        assert_eq!(outcome.scanned, 0);
        assert_eq!(outcome.closed, 0);
        assert_eq!(outcome.preserved, 0);
        assert!(workspace_b.worktree_path.join("draft.txt").is_file());
        assert_eq!(std::fs::read_to_string(&draft).unwrap(), "still working\n");
        assert_eq!(
            workspace_state_of(&pool, "objective-hand-b").await.as_deref(),
            Some("active")
        );
    }

    /// U24 (f): two sessions continuing at the same time each inherit their own
    /// stopped workspace — never the other session's.
    #[tokio::test]
    async fn concurrent_continues_never_inherit_another_sessions_workspace() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&[
            "objective-one-a",
            "objective-one-b",
            "objective-two-a",
            "objective-two-b",
        ])
        .await;
        let workspace_one_a = allocate_or_attach(
            &pool,
            request_for_session(&root, &container, "objective-one-a", "session-one", "process-a"),
        )
        .await
        .unwrap();
        let workspace_two_a = allocate_or_attach(
            &pool,
            request_for_session(&root, &container, "objective-two-a", "session-two", "process-b"),
        )
        .await
        .unwrap();
        std::fs::write(workspace_one_a.worktree_path.join("one.txt"), "session one\n").unwrap();
        std::fs::write(workspace_two_a.worktree_path.join("two.txt"), "session two\n").unwrap();
        cancel_objective(&pool, "objective-one-a").await;
        cancel_objective(&pool, "objective-two-a").await;

        let (one_b, two_b) = tokio::join!(
            allocate_or_attach(
                &pool,
                request_for_session(
                    &root,
                    &container,
                    "objective-one-b",
                    "session-one",
                    "process-c"
                ),
            ),
            allocate_or_attach(
                &pool,
                request_for_session(
                    &root,
                    &container,
                    "objective-two-b",
                    "session-two",
                    "process-d"
                ),
            ),
        );
        let one_b = one_b.unwrap();
        let two_b = two_b.unwrap();

        assert_eq!(one_b.worktree_path, workspace_one_a.worktree_path);
        assert_eq!(two_b.worktree_path, workspace_two_a.worktree_path);
        assert_ne!(one_b.worktree_path, two_b.worktree_path);
        assert!(one_b.worktree_path.join("one.txt").is_file());
        assert!(!one_b.worktree_path.join("two.txt").exists());
        assert!(two_b.worktree_path.join("two.txt").is_file());
        assert!(!two_b.worktree_path.join("one.txt").exists());
    }

    /// A read-only observer answering a scripted sequence, so the closeout
    /// decision is exercised without a live provider.
    struct ScriptedPrObserver {
        answers:
            std::sync::Mutex<std::collections::VecDeque<anyhow::Result<super::CanonicalPrState>>>,
    }

    impl ScriptedPrObserver {
        fn new(answers: Vec<anyhow::Result<super::CanonicalPrState>>) -> Self {
            Self {
                answers: std::sync::Mutex::new(answers.into_iter().collect()),
            }
        }
    }

    /// U26 / CF-WSC-R1: table-driven over every terminal state × every kind of
    /// unfinished work. Ending the task in any of these states must not end the
    /// job: the next message in the same session continues in that workspace.
    #[tokio::test]
    async fn a_continued_session_carries_unfinished_work_in_every_terminal_state() {
        for ending in ["completed", "failed", "stopped"] {
            for work in ["staged", "unpushed", "open_pr"] {
                let (_temp, root, _remote, container) = init_repo();
                let previous = format!("objective-{ending}-{work}-a");
                let next = format!("objective-{ending}-{work}-b");
                let pool = pool_with_objectives(&[previous.as_str(), next.as_str()]).await;
                let workspace_a = allocate_or_attach(
                    &pool,
                    request_for(&root, &container, &previous, "process-a"),
                )
                .await
                .unwrap();
                match work {
                    "staged" => {
                        std::fs::write(
                            workspace_a.worktree_path.join("staged.rs"),
                            "// staged but not committed\n",
                        )
                        .unwrap();
                        git(&workspace_a.worktree_path, &["add", "staged.rs"]);
                    }
                    "unpushed" => {
                        commit_file(&workspace_a.worktree_path, "unpushed.rs", "// never pushed\n");
                    }
                    _ => {
                        commit_file(&workspace_a.worktree_path, "on-pr.rs", "// on an open PR\n");
                        record_open_pr(&pool, &previous, 11).await;
                    }
                }
                match ending {
                    "completed" => complete_objective(&pool, &previous).await,
                    "failed" => fail_objective(&pool, &previous).await,
                    _ => cancel_objective(&pool, &previous).await,
                }

                let workspace_b = allocate_or_attach(
                    &pool,
                    request_for(&root, &container, &next, "process-b"),
                )
                .await
                .unwrap();

                assert_eq!(
                    workspace_b.worktree_path, workspace_a.worktree_path,
                    "{ending} + {work}: the next message must continue in the same workspace"
                );
                assert_eq!(
                    workspace_b.branch_name, workspace_a.branch_name,
                    "{ending} + {work}: on the same branch"
                );
                assert_eq!(
                    workspace_state_of(&pool, &previous).await,
                    None,
                    "{ending} + {work}: the workspace now belongs to the continued Objective"
                );
                match work {
                    "staged" => {
                        assert_eq!(
                            std::fs::read_to_string(
                                workspace_b.worktree_path.join("staged.rs")
                            )
                            .unwrap(),
                            "// staged but not committed\n",
                            "{ending}: the staged change is still inside the continued workspace"
                        );
                        assert!(
                            git(&workspace_b.worktree_path, &["status", "--porcelain"])
                                .contains("staged.rs"),
                            "{ending}: the change is still staged, not committed"
                        );
                    }
                    "unpushed" => {
                        assert!(workspace_b.worktree_path.join("unpushed.rs").is_file());
                        assert!(
                            git(&workspace_b.worktree_path, &["log", "--oneline", "-1"])
                                .contains("unpushed.rs"),
                            "{ending}: the unpushed commit is still the workspace HEAD"
                        );
                    }
                    _ => {}
                }
                assert_eq!(
                    workspace_pr_number(&pool, &next).await,
                    (work == "open_pr").then_some(11),
                    "{ending} + {work}: the PR the session already has travels with it"
                );
            }
        }
    }

    #[async_trait::async_trait]
    impl super::CanonicalPrObserver for ScriptedPrObserver {
        async fn observe(
            &self,
            _repo_root: &Path,
            _pr_number: i64,
            _pr_url: &str,
            _expected_head_sha: &str,
        ) -> anyhow::Result<super::CanonicalPrState> {
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(super::CanonicalPrState::Unknown(
                    "scripted observer exhausted".into(),
                )))
        }
    }

    /// CF-CLO-R1 external path: the delivery record knows the canonical PR, but
    /// there is no committed local merge receipt because the PR was merged in
    /// the GitHub web UI.
    async fn record_open_canonical_pr(
        pool: &SqlitePool,
        workspace: &super::ExecutionWorkspace,
        pr_number: i64,
        requested_ceiling: &str,
    ) -> String {
        crate::agent::delivery_run::ensure_schema(pool)
            .await
            .unwrap();
        let head_sha = git(&workspace.worktree_path, &["rev-parse", "HEAD"]);
        let now = chrono::Utc::now().timestamp_millis();
        let has_revision: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('objectives') WHERE name='revision'")
            .fetch_one(pool).await.unwrap();
        if has_revision == 0 {
            sqlx::query("UPDATE objectives SET status='completed' WHERE id=?")
                .bind(&workspace.objective_id).execute(pool).await.unwrap();
        }
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
             VALUES (?, ?, 'objective', ?, ?, ?, ?, ?, ?, ?, ?, ?,
                     'pr_open', 'pr_open', 'running', ?, ?, 'test', 'test',
                     'test-process', ?, ?, 1, 1)",
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
        .bind(requested_ceiling)
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        head_sha
    }

    async fn workspace_cleanup_cell(
        pool: &SqlitePool,
        objective_id: &str,
    ) -> (String, Option<String>, Option<String>) {
        sqlx::query_as(
            "SELECT state, failure_code, failure_detail
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind(objective_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// Publish the workspace branch on the bare remote the same way a real
    /// delivery does (`origin` receives the ref), so the remote-branch arm of
    /// closeout has something to delete.
    fn publish_workspace_branch(root: &Path, remote: &Path, branch: &str) {
        git(
            remote,
            &[
                "fetch",
                root.to_str().unwrap(),
                &format!("refs/heads/{branch}:refs/heads/{branch}"),
            ],
        );
    }

    /// CF-CLO-R1 (external merge) + CF-CLO-R2 (remove worktree, local branch,
    /// and the remote branch once it is merged and contained).
    #[tokio::test]
    async fn externally_merged_pr_is_settled_and_cleaned_up() {
        let (_temp, root, remote, container) = init_repo();
        let pool = pool_with_objective("objective-external-merge").await;
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-external-merge", "process-a"),
        )
            .await
            .unwrap();
        std::fs::write(workspace.worktree_path.join("change.txt"), "delivered\n").unwrap();
        git(&workspace.worktree_path, &["add", "change.txt"]);
        git(
            &workspace.worktree_path,
            &["commit", "-m", "delivered change"],
        );
        publish_workspace_branch(&root, &remote, &workspace.branch_name);
        record_open_canonical_pr(&pool, &workspace, 77, "through_merge").await;

        // Red before the fix: an externally merged PR has no local merge
        // receipt, so the offline pass can only preserve it and the session
        // stays "not done" (the reported U22 symptom).
        let offline = super::run_cleanup_pass(&pool, &container, "cleanup-offline")
            .await
            .unwrap();
        assert_eq!(offline.closed, 0);
        assert_eq!(offline.preserved, 1);
        assert!(workspace.worktree_path.is_dir());
        let (state, code, _) = workspace_cleanup_cell(&pool, &workspace.objective_id).await;
        assert_eq!(state, "cleanup_pending");
        assert_eq!(code.as_deref(), Some("workspace_cleanup_merge_unproven"));
        // Simulate the next scheduled poll after the retry backoff.
        sqlx::query(
            "UPDATE execution_workspaces SET failure_code=NULL, updated_at=?
             WHERE objective_id=?",
        )
        .bind(chrono::Utc::now().timestamp_millis() - 600_000)
        .bind(&workspace.objective_id)
        .execute(&pool)
        .await
        .unwrap();

        let observer = ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged {
            merge_sha: "externally-merged-sha".into(),
        })]);

        let outcome = super::run_cleanup_pass_with(&pool, &container, "cleanup-a", &observer)
            .await
            .unwrap();
        assert_eq!(outcome.closed, 1);
        assert_eq!(outcome.preserved, 0);
        assert!(!workspace.worktree_path.exists());
        assert!(!super::branch_exists(&root, &workspace.branch_name));
        assert!(!super::remote_branch_exists(&root, "origin", &workspace.branch_name).unwrap());
        let (state, _, _) = workspace_cleanup_cell(&pool, &workspace.objective_id).await;
        assert_eq!(state, "closed");
        // CF-CLO-R1: the delivery record now shows merged, with the merge commit.
        let (ceiling, evidence): (String, String) = sqlx::query_as(
            "SELECT delivery_runs.reached_ceiling, delivery_mutation_intents.evidence_json
             FROM delivery_runs
             JOIN delivery_mutation_intents ON delivery_mutation_intents.run_id=delivery_runs.id
             WHERE delivery_runs.objective_id=?
               AND delivery_mutation_intents.rung='provider_pr_merge'",
        )
        .bind(&workspace.objective_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(ceiling, "merged");
        assert!(evidence.contains("externally-merged-sha"), "{evidence}");
    }

    #[tokio::test]
    async fn m102_reconcile_never_reopens_terminal() {
        for initial in ["completed", "failed", "cancelled", "waiting_system"] {
            for merged in [false, true] {
                for reached in ["pr_open", "release_triggered"] {
                    let (_temp, root, _remote, container) = init_repo();
                    let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
                    crate::agent::objective::ObjectiveStore::new(pool.clone()).create(
                        crate::agent::objective::CreateObjective {
                            id: "synthetic-m102".into(), kind: crate::agent::objective::ObjectiveKind::Delivery,
                            session_id: None, root_turn_id: None, domain: crate::agent::objective::RecoveryDomain::Delivery,
                            requested_acceptance: "merged".into(), created_surface: "synthetic-test".into(),
                        },
                    ).await.unwrap();
                    let workspace = allocate_or_attach(&pool, request_for(&root, &container, "synthetic-m102", "process-a")).await.unwrap();
                    record_open_canonical_pr(&pool, &workspace, 102, "through_release").await;
                    sqlx::query("UPDATE objectives SET recovery_owner=NULL, remediation_id=NULL, lease_owner=NULL, lease_expires_at=NULL, status=?, decision_type=?, completed_at=1, evidence_ref='synthetic-acceptance', reached_acceptance='merged', cancellation_provenance=?, delivery_run_id=(SELECT id FROM delivery_runs WHERE objective_id=objectives.id)")
                        .bind(initial).bind(match initial { "completed" => "complete", "cancelled" => "cancelled", "failed" => "failed_internal", _ => "waiting" })
                        .bind(if initial == "cancelled" { Some("explicit_cancel") } else { None }).execute(&pool).await.unwrap();
                    sqlx::query("UPDATE delivery_runs SET reached_ceiling=?, stage='live', status='failed', lease_owner=NULL, lease_expires_at=NULL").bind(reached).execute(&pool).await.unwrap();
                    let before: i64 = sqlx::query_scalar("SELECT revision FROM objectives").fetch_one(&pool).await.unwrap();
                    for _ in 0..2 {
                        let state = if merged { super::CanonicalPrState::Merged { merge_sha: "synthetic-m102-merge".into() } } else { super::CanonicalPrState::Open };
                        reconcile_next_poll(&pool, &ScriptedPrObserver::new(vec![Ok(state)]), "m102").await.unwrap();
                        let (status, revision): (String, i64) = sqlx::query_as("SELECT status,revision FROM objectives").fetch_one(&pool).await.unwrap();
                        let advances = (merged && initial != "completed")
                            || (!merged && initial == "waiting_system" && reached == "pr_open");
                        assert_eq!(status, if merged { "completed" } else { initial }, "initial={initial} merged={merged} reached={reached}");
                        assert_eq!(revision, before + i64::from(advances));
                        let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objective_events WHERE event_type='canonical_pr_progress'").fetch_one(&pool).await.unwrap();
                        assert_eq!(events, i64::from(advances), "every state transition needs exactly one event");
                    }
                }
            }
        }
    }

    /// M114 (R2/R3/R4/R7): a scheduled cleanup pass must not re-query every
    /// historical delivery. 20 failed/cancelled records far outside the
    /// observation window plus 2 active open PRs: the historical records are
    /// never observed, and the active ones are reached on a backoff schedule
    /// (1m → 2m → 4m …) instead of once per 60s tick.
    ///
    /// Field evidence: 60s sampling caught 43 `gh pr view` calls/minute across
    /// 17 PRs, 15 of which belonged to deliveries that had ended months
    /// earlier — that is what exhausted the 5000/h user budget.
    #[tokio::test]
    async fn m114_historical_delivery_records_are_not_reobserved() {
        struct CountingObserver {
            observed: std::sync::Mutex<Vec<i64>>,
            batches: std::sync::atomic::AtomicU32,
        }
        #[async_trait::async_trait]
        impl super::CanonicalPrObserver for CountingObserver {
            async fn observe(
                &self,
                _repo_root: &Path,
                pr_number: i64,
                _pr_url: &str,
                _expected_head_sha: &str,
            ) -> anyhow::Result<super::CanonicalPrState> {
                self.observed.lock().unwrap().push(pr_number);
                Ok(super::CanonicalPrState::Open)
            }

            /// R4: the pass must batch per repository, not per record.
            async fn observe_many(
                &self,
                _repo_root: &Path,
                requests: &[super::PrObservationRequest],
            ) -> std::collections::HashMap<i64, anyhow::Result<super::CanonicalPrState>> {
                self.batches
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut out = std::collections::HashMap::new();
                for request in requests {
                    self.observed.lock().unwrap().push(request.pr_number);
                    out.insert(request.pr_number, Ok(super::CanonicalPrState::Open));
                }
                out
            }
        }

        let (_temp, root, _remote, container) = init_repo();
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        ensure_schema(&pool).await.unwrap();
        crate::agent::objective::ObjectiveStore::new(pool.clone())
            .create(crate::agent::objective::CreateObjective {
                id: "m114-template".into(),
                kind: crate::agent::objective::ObjectiveKind::Delivery,
                session_id: None,
                root_turn_id: None,
                domain: crate::agent::objective::RecoveryDomain::Delivery,
                requested_acceptance: "merged".into(),
                created_surface: "synthetic-test".into(),
            })
            .await
            .unwrap();
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "m114-template", "process-a"),
        )
        .await
        .unwrap();
        record_open_canonical_pr(&pool, &workspace, 900, "through_merge").await;

        // Copy a valid workspace + delivery-run row for each synthetic
        // objective, replacing only the identity/number columns. This keeps the
        // fixture on the real schema without allocating 22 git worktrees.
        async fn copy_row(
            pool: &SqlitePool,
            table: &str,
            source_id: &str,
            replacements: &[(&str, String)],
        ) {
            let cols: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT name FROM pragma_table_info('{table}')"
            ))
            .fetch_all(pool)
            .await
            .unwrap();
            let projections: Vec<String> = cols
                .iter()
                .map(|col| {
                    if replacements.iter().any(|(name, _)| name == col) {
                        "?".to_string()
                    } else {
                        format!("\"{col}\"")
                    }
                })
                .collect();
            let statement = format!(
                "INSERT INTO {table} ({}) SELECT {} FROM {table} WHERE id=?",
                cols.iter()
                    .map(|col| format!("\"{col}\""))
                    .collect::<Vec<_>>()
                    .join(","),
                projections.join(",")
            );
            let mut query = sqlx::query(&statement);
            for col in &cols {
                if let Some((_, value)) = replacements.iter().find(|(name, _)| *name == col.as_str())
                {
                    query = query.bind(value.clone());
                }
            }
            query.bind(source_id).execute(pool).await.unwrap();
        }

        let now = chrono::Utc::now().timestamp_millis();
        let long_ago = now - 30 * 24 * 60 * 60 * 1000;
        let mut historical = Vec::new();
        let mut active = Vec::new();
        for index in 0..22 {
            let objective_id = format!("m114-objective-{index}");
            crate::agent::objective::ObjectiveStore::new(pool.clone())
                .create(crate::agent::objective::CreateObjective {
                    id: objective_id.clone(),
                    kind: crate::agent::objective::ObjectiveKind::Delivery,
                    session_id: None,
                    root_turn_id: None,
                    domain: crate::agent::objective::RecoveryDomain::Delivery,
                    requested_acceptance: "merged".into(),
                    created_surface: "synthetic-test".into(),
                })
                .await
                .unwrap();
            let run_id = format!("m114-run-{index}");
            let pr_number = 1000 + index as i64;
            copy_row(
                &pool,
                "execution_workspaces",
                &workspace.id,
                &[
                    ("id", format!("m114-workspace-{index}")),
                    ("objective_id", objective_id.clone()),
                    ("branch_name", format!("m114-branch-{index}")),
                    ("worktree_identity", format!("m114-worktree-{index}")),
                    ("worktree_path", root.join(format!("m114-{index}")).to_string_lossy().into_owned()),
                    ("state", "active".into()),
                    ("updated_at", now.to_string()),
                ],
            )
            .await;
            copy_row(
                &pool,
                "delivery_runs",
                &format!("delivery-{}", workspace.id),
                &[
                    ("id", run_id),
                    ("objective_id", objective_id.clone()),
                    ("canonical_pr_number", pr_number.to_string()),
                    ("updated_at", now.to_string()),
                    ("last_observed_at", now.to_string()),
                ],
            )
            .await;
            if index < 20 {
                let status = if index % 2 == 0 { "failed" } else { "cancelled" };
                if status == "cancelled" {
                    sqlx::query(
                        "UPDATE objectives SET status='cancelled', decision_type='cancelled',
                             completed_at=?, cancellation_provenance='explicit_cancel' WHERE id=?",
                    )
                    .bind(long_ago)
                    .bind(&objective_id)
                    .execute(&pool)
                    .await
                    .unwrap();
                } else {
                    sqlx::query(
                        "UPDATE objectives SET status='failed', decision_type='failed_internal',
                             completed_at=?, failure_code='delivery_internal_failure' WHERE id=?",
                    )
                    .bind(long_ago)
                    .bind(&objective_id)
                    .execute(&pool)
                    .await
                    .unwrap();
                }
                sqlx::query("UPDATE delivery_runs SET last_observed_at=?, updated_at=?, status='failed' WHERE objective_id=?")
                    .bind(long_ago)
                    .bind(long_ago)
                    .bind(&objective_id)
                    .execute(&pool)
                    .await
                    .unwrap();
                historical.push(pr_number);
            } else {
                sqlx::query("UPDATE objectives SET status='active' WHERE id=?")
                    .bind(&objective_id)
                    .execute(&pool)
                    .await
                    .unwrap();
                active.push(pr_number);
            }
        }

        let observer = CountingObserver {
            observed: std::sync::Mutex::new(Vec::new()),
            batches: std::sync::atomic::AtomicU32::new(0),
        };
        for _ in 0..60 {
            reconcile_next_poll(&pool, &observer, "m114").await.unwrap();
        }

        // R4: one batched lookup per repository per due round — not one call
        // per candidate, and never a second per-PR lookup.
        let batches = observer
            .batches
            .load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            batches <= 8,
            "60 ticks must batch the due candidates (1m→2m→4m→…), made {batches} batched lookups"
        );

        let observed = observer.observed.lock().unwrap().clone();
        let historical_seen: Vec<i64> = observed
            .iter()
            .copied()
            .filter(|number| historical.contains(number))
            .collect();
        assert!(
            historical_seen.is_empty(),
            "historical deliveries must never be re-observed, saw {historical_seen:?}"
        );
        // 60 ticks × 22 candidates would be 1320 observations before the fix.
        // The budget allows the two active PRs on a backoff schedule, not per
        // tick: at most ~6 observations each inside one simulated hour.
        assert!(
            observed.len() <= 24,
            "60 cleanup ticks must stay inside the observation budget, made {} observations",
            observed.len()
        );
        // The 20 historical rows are durably concluded, not merely skipped.
        let concluded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_runs WHERE id LIKE 'm114-run-%' AND pr_observation_concluded_at IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        assert!(
            concluded >= 20,
            "historical records must persist a conclusion, only {concluded} did"
        );
    }

    /// M114b (R2/R3/R7): the four delivery states the orchestrator sampled —
    /// `mutation_permit|waiting` under a `waiting_core_input` objective,
    /// `base_sync|agent_action_required`, `takeover_reconciliation|
    /// platform_incident`, and an ordinary running delivery — must not keep
    /// re-asking a concluded PR. Before the fix those objectives were not part
    /// of the reconcile candidate set at all, so merged PR #680 and closed
    /// #704/#710 were queried four times a minute forever.
    #[tokio::test]
    async fn m114b_user_waiting_deliveries_stop_polling_concluded_prs() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct CountingObserver {
            calls: AtomicU32,
            observed: std::sync::Mutex<Vec<i64>>,
        }

        impl CountingObserver {
            fn state_for(&self, pr_number: i64) -> super::CanonicalPrState {
                match pr_number {
                    2004 => super::CanonicalPrState::Open,
                    number if number % 2 == 0 => super::CanonicalPrState::Merged {
                        merge_sha: format!("merge-{number}"),
                    },
                    _ => super::CanonicalPrState::ClosedUnmerged,
                }
            }
        }

        #[async_trait::async_trait]
        impl super::CanonicalPrObserver for CountingObserver {
            async fn observe(
                &self,
                _repo_root: &Path,
                pr_number: i64,
                _pr_url: &str,
                _expected_head_sha: &str,
            ) -> anyhow::Result<super::CanonicalPrState> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.observed.lock().unwrap().push(pr_number);
                Ok(self.state_for(pr_number))
            }

            async fn observe_many(
                &self,
                _repo_root: &Path,
                requests: &[super::PrObservationRequest],
            ) -> std::collections::HashMap<i64, anyhow::Result<super::CanonicalPrState>> {
                let mut out = std::collections::HashMap::new();
                for request in requests {
                    self.calls.fetch_add(1, Ordering::SeqCst);
                    self.observed.lock().unwrap().push(request.pr_number);
                    out.insert(request.pr_number, Ok(self.state_for(request.pr_number)));
                }
                out
            }
        }

        async fn copy_row(
            pool: &SqlitePool,
            table: &str,
            source_id: &str,
            replacements: &[(&str, String)],
            id_column: &str,
        ) -> Vec<String> {
            let cols: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT name FROM pragma_table_info('{table}')"
            ))
            .fetch_all(pool)
            .await
            .unwrap();
            let projections: Vec<String> = cols
                .iter()
                .map(|col| {
                    if replacements.iter().any(|(name, _)| name == col) {
                        "?".to_string()
                    } else {
                        format!("\"{col}\"")
                    }
                })
                .collect();
            let statement = format!(
                "INSERT INTO {table} ({}) SELECT {} FROM {table} WHERE \"{id_column}\"=?",
                cols.iter()
                    .map(|col| format!("\"{col}\""))
                    .collect::<Vec<_>>()
                    .join(","),
                projections.join(",")
            );
            let mut query = sqlx::query(&statement);
            for col in &cols {
                if let Some((_, value)) = replacements.iter().find(|(name, _)| *name == col.as_str())
                {
                    query = query.bind(value.clone());
                }
            }
            query.bind(source_id).execute(pool).await.unwrap();
            cols
        }

        let (_temp, root, _remote, container) = init_repo();
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        ensure_schema(&pool).await.unwrap();
        crate::agent::objective::ObjectiveStore::new(pool.clone())
            .create(crate::agent::objective::CreateObjective {
                id: "m114b-template".into(),
                kind: crate::agent::objective::ObjectiveKind::Delivery,
                session_id: None,
                root_turn_id: None,
                domain: crate::agent::objective::RecoveryDomain::Delivery,
                requested_acceptance: "merged".into(),
                created_surface: "synthetic-test".into(),
            })
            .await
            .unwrap();
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "m114b-template", "process-a"),
        )
        .await
        .unwrap();
        record_open_canonical_pr(&pool, &workspace, 950, "through_merge").await;

        let now = chrono::Utc::now().timestamp_millis();
        // (pr, objective status, run stage, run status, run wait_class,
        //  objective requires_user_action)
        let states: [(i64, &str, &str, &str, &str, i64); 5] = [
            (2000, "waiting_core_input", "mutation_permit", "waiting", "external_wait", 1),
            (2001, "waiting_core_input", "base_sync", "agent_action_required", "external_wait", 1),
            (2002, "waiting_core_input", "takeover_reconciliation", "platform_incident", "delivery_identity_conflict", 1),
            (2003, "active", "pr_open", "waiting", "external_wait", 0),
            (2004, "waiting_core_input", "pr_open", "waiting", "external_wait", 1),
        ];
        for (index, (pr_number, objective_status, stage, run_status, wait_class, user_action)) in
            states.iter().enumerate()
        {
            let objective_id = format!("m114b-objective-{index}");
            crate::agent::objective::ObjectiveStore::new(pool.clone())
                .create(crate::agent::objective::CreateObjective {
                    id: objective_id.clone(),
                    kind: crate::agent::objective::ObjectiveKind::Delivery,
                    session_id: None,
                    root_turn_id: None,
                    domain: crate::agent::objective::RecoveryDomain::Delivery,
                    requested_acceptance: "merged".into(),
                    created_surface: "synthetic-test".into(),
                })
                .await
                .unwrap();
            let run_id = format!("m114b-run-{index}");
            copy_row(
                &pool,
                "execution_workspaces",
                &workspace.id,
                &[
                    ("id", format!("m114b-workspace-{index}")),
                    ("objective_id", objective_id.clone()),
                    ("branch_name", format!("m114b-branch-{index}")),
                    ("worktree_identity", format!("m114b-worktree-{index}")),
                    ("worktree_path", root.join(format!("m114b-{index}")).to_string_lossy().into_owned()),
                    ("state", "active".into()),
                    ("updated_at", now.to_string()),
                ],
                "id",
            )
            .await;
            copy_row(
                &pool,
                "delivery_runs",
                &format!("delivery-{}", workspace.id),
                &[
                    ("id", run_id),
                    ("objective_id", objective_id.clone()),
                    ("canonical_pr_number", pr_number.to_string()),
                    ("stage", stage.to_string()),
                    ("status", run_status.to_string()),
                    ("wait_class", wait_class.to_string()),
                    ("next_action", "await_ci".to_string()),
                    ("next_action_authorized", "0".to_string()),
                    ("updated_at", now.to_string()),
                    ("last_observed_at", now.to_string()),
                ],
                "id",
            )
            .await;
            sqlx::query(
                "UPDATE objectives SET status=?, decision_type=?, requires_user_action=?,
                     next_observation_at=NULL WHERE id=?",
            )
            .bind(objective_status)
            .bind(if *objective_status == "waiting_core_input" {
                "core_input_required"
            } else {
                "waiting"
            })
            .bind(user_action)
            .bind(&objective_id)
            .execute(&pool)
            .await
            .unwrap();
        }

        let observer = CountingObserver {
            calls: AtomicU32::new(0),
            observed: std::sync::Mutex::new(Vec::new()),
        };
        let started = chrono::Utc::now().timestamp_millis();
        for _ in 0..60 {
            reconcile_next_poll(&pool, &observer, "m114b").await.unwrap();
        }

        // R3: a concluded PR is asked once, then never again — and the four
        // field states all stay inside the budget instead of 60 queries each.
        let observed = observer.observed.lock().unwrap().clone();
        for pr_number in [2000i64, 2001, 2002] {
            let count = observed.iter().filter(|seen| **seen == pr_number).count();
            assert_eq!(count, 1, "PR #{pr_number} must be queried exactly once, saw {count}");
        }
        // A closed PR on a *live* objective keeps the M114 backoff (a reopen can
        // still change the answer), but must stay far below one call per tick.
        let live_closed = observed.iter().filter(|seen| **seen == 2003).count();
        assert!(
            live_closed <= 8,
            "a live delivery must back off instead of polling each tick, saw {live_closed}"
        );
        let concluded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_runs WHERE id LIKE 'm114b-run-%'
               AND pr_observation_concluded_at IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        assert_eq!(
            concluded, 3,
            "the three PRs under user-waiting objectives must persist a conclusion"
        );
        // R2: the open PR under a user-waiting objective is re-observed at the
        // 30-minute floor at most, not 60 times.
        let open_count = observed.iter().filter(|seen| **seen == 2004).count();
        assert!(
            open_count <= 2,
            "open PR under a user wait is re-observed on the 30-minute floor only, saw {open_count}"
        );
        let next_at: Option<i64> = sqlx::query_scalar(
            "SELECT pr_observation_next_at FROM delivery_runs WHERE id='m114b-run-4'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            next_at.is_some_and(|value| value - started
                >= crate::agent::delivery::github_api_budget::USER_WAIT_OBSERVATION_MIN_DELAY_MS),
            "a user-waiting objective must back off at least 30 minutes, got {next_at:?}"
        );
        let total = observer.calls.load(Ordering::SeqCst);
        assert!(
            total <= 16,
            "60 ticks across 5 records must stay inside the call budget, made {total}"
        );
    }

    #[tokio::test]
    async fn m84_historical_misclassified_live_settles_once_at_merged() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        crate::agent::objective::ObjectiveStore::new(pool.clone()).create(
            crate::agent::objective::CreateObjective {
                id: "synthetic-m84".into(), kind: crate::agent::objective::ObjectiveKind::Live,
                session_id: None, root_turn_id: None, domain: crate::agent::objective::RecoveryDomain::Delivery,
                requested_acceptance: "live_verification".into(), created_surface: "synthetic-test".into(),
            },
        ).await.unwrap();
        sqlx::query("INSERT INTO sessions (id,title,cwd,model_id,created_at,updated_at,delivery_authorized) VALUES ('synthetic-m84-session','synthetic','/synthetic','synthetic',1,1,1)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO messages (id,session_id,role,content,created_at) VALUES ('synthetic-m84-root','synthetic-m84-session','user','授权交付（ceiling: merged）：按 truthful-delivery-state.md 修复',1)").execute(&pool).await.unwrap();
        sqlx::query("UPDATE objectives SET session_id='synthetic-m84-session', root_turn_id='synthetic-m84-root' WHERE id='synthetic-m84'").execute(&pool).await.unwrap();
        let workspace = allocate_or_attach(&pool, request_for(&root, &container, "synthetic-m84", "process-a")).await.unwrap();
        record_open_canonical_pr(&pool, &workspace, 184, "through_merge").await;
        sqlx::query("UPDATE objectives SET status='waiting_system', decision_type='waiting', delivery_run_id=(SELECT id FROM delivery_runs WHERE objective_id=objectives.id)").execute(&pool).await.unwrap();
        sqlx::query("UPDATE execution_workspaces SET lease_owner=NULL, lease_expires_at=NULL").execute(&pool).await.unwrap();
        sqlx::query("UPDATE delivery_runs SET stage='complete',reached_ceiling='merged',status='waiting' WHERE objective_id='synthetic-m84'").execute(&pool).await.unwrap();
        let observer = ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged { merge_sha: "synthetic-m84-merge".into() })]);
        reconcile_next_poll(&pool, &observer, "synthetic-observer").await.unwrap();
        let settled: (String, String, String, i64) = sqlx::query_as("SELECT status,kind,requested_acceptance,revision FROM objectives").fetch_one(&pool).await.unwrap();
        assert_eq!(settled.0, "completed");
        assert_eq!(settled.1, "delivery");
        assert_eq!(settled.2, "merged");
        let observer = ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged { merge_sha: "synthetic-m84-merge".into() })]);
        reconcile_next_poll(&pool, &observer, "synthetic-observer").await.unwrap();
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM objectives").fetch_one(&pool).await.unwrap();
        assert_eq!(revision, settled.3);
    }

    #[tokio::test]
    async fn m72_misclassified_cancel_with_merged_pr_is_completed() {
        let (_temp, root, remote, container) = init_repo();
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        crate::agent::objective::ObjectiveStore::new(pool.clone()).create(
            crate::agent::objective::CreateObjective {
                id: "synthetic-m72-merged".into(), kind: crate::agent::objective::ObjectiveKind::Delivery,
                session_id: None, root_turn_id: None, domain: crate::agent::objective::RecoveryDomain::Delivery,
                requested_acceptance: "delivery_receipt".into(), created_surface: "synthetic-test".into(),
            },
        ).await.unwrap();
        let workspace = allocate_or_attach(&pool,
            request_for(&root, &container, "synthetic-m72-merged", "process-a"),
        ).await.unwrap();
        std::fs::write(workspace.worktree_path.join("change.txt"), "synthetic delivery\n").unwrap();
        git(&workspace.worktree_path, &["add", "change.txt"]);
        git(&workspace.worktree_path, &["commit", "-m", "fix: synthetic M72"]);
        publish_workspace_branch(&root, &remote, &workspace.branch_name);
        record_open_canonical_pr(&pool, &workspace, 92, "through_merge").await;
        sqlx::query("UPDATE objectives SET status='cancelled', decision_type='cancelled', completed_at=1, cancellation_provenance='explicit_cancel', delivery_run_id=(SELECT id FROM delivery_runs WHERE objective_id=objectives.id)")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE execution_workspaces SET state='cleanup_pending', lease_owner=NULL, lease_expires_at=NULL")
            .execute(&pool).await.unwrap();
        let observer = ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged {
            merge_sha: "synthetic-m72-merge".into(),
        })]);
        let outcome = super::run_cleanup_pass_with(&pool, &container, "m72-observer", &observer).await.unwrap();
        let (status, provenance, acceptance): (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT status, cancellation_provenance, reached_acceptance FROM objectives",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "completed");
        assert!(provenance.is_none());
        assert_eq!(acceptance.as_deref(), Some("merged"));
        assert_eq!(outcome.closed, 1);
        assert!(!workspace.worktree_path.exists());
    }

    #[tokio::test]
    async fn m74_remote_merge_repairs_failed_cancelled_and_incident_with_stale_workspace() {
        for (index, (status, reached)) in [("failed", "pr_open"), ("cancelled", "ci_green"), ("platform_incident", "pr_open"), ("failed", "merged"), ("cancelled", "merged"), ("stopped", "pr_open")].iter().enumerate() {
            let (_temp, root, _remote, container) = init_repo();
            let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
            crate::agent::objective::ObjectiveStore::new(pool.clone()).create(
                crate::agent::objective::CreateObjective {
                    id: "synthetic-m74".into(), kind: crate::agent::objective::ObjectiveKind::Delivery,
                    session_id: None, root_turn_id: None, domain: crate::agent::objective::RecoveryDomain::Delivery,
                    requested_acceptance: "delivery_receipt".into(), created_surface: "synthetic-test".into(),
                },
            ).await.unwrap();
            let workspace = allocate_or_attach(&pool,
                request_for(&root, &container, "synthetic-m74", "process-a"),
            ).await.unwrap();
            record_open_canonical_pr(&pool, &workspace, 174 + index as i64, "through_merge").await;
            sqlx::query("UPDATE delivery_runs SET status=?, stage='takeover_reconciliation', reached_ceiling=?, failure_signature='takeover_reconciliation:delivery_identity_conflict', lease_owner=NULL, lease_expires_at=NULL")
                .bind(status).bind(reached).execute(&pool).await.unwrap();
            sqlx::query("UPDATE objectives SET status=?, decision_type=CASE WHEN ?='cancelled' THEN 'cancelled' ELSE 'failed_internal' END, cancellation_provenance=CASE WHEN ?='cancelled' THEN 'explicit_cancel' ELSE NULL END, completed_at=1, failure_code='technical_recovery_exhausted', delivery_run_id=(SELECT id FROM delivery_runs WHERE objective_id=objectives.id)")
                .bind(if index == 5 { "cancelled" } else { "failed" })
                .bind(if index == 5 { "cancelled" } else { "failed" })
                .bind(if index == 5 { "cancelled" } else { "failed" }).execute(&pool).await.unwrap();
            if index == 5 {
                sqlx::query("INSERT INTO sessions (id,title,cwd,model_id,created_at,updated_at) VALUES ('synthetic-stopped-session','synthetic','/tmp/synthetic','synthetic',1,1)").execute(&pool).await.unwrap();
                sqlx::query("UPDATE objectives SET session_id='synthetic-stopped-session'").execute(&pool).await.unwrap();
                sqlx::query("INSERT INTO chat_session_cancel_intents (session_id,status,requested_at,updated_at) VALUES ('synthetic-stopped-session','requested',1,1)").execute(&pool).await.unwrap();
                crate::agent::stop_fence::record_stop(&pool, "synthetic-stopped-session", "orchestrator_stop", Some(179)).await.unwrap();
            }
            // Local identity is no longer suitable for mutation replay. In the
            // last case closeout has already removed the worktree altogether.
            sqlx::query("UPDATE execution_workspaces SET head_sha='stale-local-head', worktree_identity='stale-local-identity', state=?, lease_owner=NULL, lease_expires_at=NULL")
                .bind(if index == 2 { "closed" } else { "delivering" }).execute(&pool).await.unwrap();
            if index == 2 {
                git(&root, &["worktree", "remove", workspace.worktree_path.to_str().unwrap()]);
            }
            let before: (String, String, i64) = sqlx::query_as(
                "SELECT o.status,r.status,o.revision FROM objectives o JOIN delivery_runs r ON r.objective_id=o.id",
            ).fetch_one(&pool).await.unwrap();
            reconcile_next_poll(&pool,
                &ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Unknown("offline".into()))]), "m74-restart",
            ).await.unwrap();
            let unknown: (String, String, i64) = sqlx::query_as(
                "SELECT o.status,r.status,o.revision FROM objectives o JOIN delivery_runs r ON r.objective_id=o.id",
            ).fetch_one(&pool).await.unwrap();
            assert_eq!(unknown, before, "R9: uncertainty must leave history unchanged");
            for _ in 0..2 {
                reconcile_next_poll(&pool,
                    &ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged { merge_sha: "synthetic-m74-merge".into() })]), "m74-restart",
                ).await.unwrap();
                let repaired: (String, Option<String>, String, String, Option<String>, i64) = sqlx::query_as(
                    "SELECT o.status,o.failure_code,r.status,r.reached_ceiling,r.failure_signature,o.revision FROM objectives o JOIN delivery_runs r ON r.objective_id=o.id",
                ).fetch_one(&pool).await.unwrap();
                assert_eq!(repaired.0, "completed", "R8: repair historical objective failure");
                assert_eq!(repaired.1, None);
                assert_eq!(repaired.2, "completed", "R7: local drift is not a platform incident");
                assert_eq!(repaired.3, "merged");
                assert_eq!(repaired.4, None);
                assert_eq!(repaired.5, before.2 + 1, "R9: repeated merge observation is idempotent");
                let plan = crate::agent::delivery_run::plan_startup_recovery(&pool,
                    &crate::agent::delivery_run::ProcessIdentity::new("m74b-recovery", "test", "test"),
                    chrono::Utc::now().timestamp_millis(), 60_000).await.unwrap();
                assert!(plan.claimed.is_empty(), "R7: merged settlement must bypass mutation takeover");
                if index == 5 {
                    let fences: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivery_stop_fences WHERE session_id='synthetic-stopped-session'").fetch_one(&pool).await.unwrap();
                    assert_eq!(fences, 1, "read-only settlement must preserve the stop fence");
                }
            }
        }
    }

    #[tokio::test]
    async fn m74c_settles_merged_pr_after_the_queue_moved_the_head_and_the_workspace_is_gone() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        crate::agent::objective::ObjectiveStore::new(pool.clone()).create(
            crate::agent::objective::CreateObjective {
                id: "synthetic-m74c".into(), kind: crate::agent::objective::ObjectiveKind::Delivery,
                session_id: None, root_turn_id: None, domain: crate::agent::objective::RecoveryDomain::Delivery,
                requested_acceptance: "delivery_receipt".into(), created_surface: "synthetic-test".into(),
            },
        ).await.unwrap();
        let workspace = allocate_or_attach(&pool,
            request_for(&root, &container, "synthetic-m74c", "process-a"),
        ).await.unwrap();
        record_open_canonical_pr(&pool, &workspace, 644, "through_merge").await;
        // The merge queue synced the branch before merging and closeout already
        // removed the worktree, so the local head is no longer the PR head.
        sqlx::query("UPDATE delivery_runs SET stage='takeover_reconciliation', status='failed', reached_ceiling='merge_queued', expected_head_sha='pinned-before-queue', canonical_head_sha='pinned-before-queue', failure_signature='takeover_reconciliation:delivery_identity_conflict', lease_owner=NULL, lease_expires_at=NULL")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE execution_workspaces SET worktree_identity=NULL, head_sha=NULL, state='closed', lease_owner=NULL, lease_expires_at=NULL")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE objectives SET status='failed', decision_type='failed_internal', completed_at=1, failure_code='technical_recovery_exhausted', delivery_run_id=(SELECT id FROM delivery_runs WHERE objective_id=objectives.id)")
            .execute(&pool).await.unwrap();
        // The remote is authoritative: GitHub already merged PR #644.
        reconcile_next_poll(&pool,
            &ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged { merge_sha: "synthetic-m74c-merge".into() })]), "m74c-restart",
        ).await.unwrap();
        let settled: (String, Option<String>, String, String) = sqlx::query_as(
            "SELECT o.status,o.failure_code,r.status,r.reached_ceiling FROM objectives o JOIN delivery_runs r ON r.objective_id=o.id",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(settled.0, "completed", "a merged PR settles even after the queue moved the head");
        assert_eq!(settled.1, None);
        assert_eq!(settled.2, "completed");
        assert_eq!(settled.3, "merged");
    }

    /// M108/R4: an open PR whose checks already concluded. The observer reports
    /// the real check list so the reconcile pass makes the same decision the
    /// delivery tool makes, instead of re-parking the objective silently.
    struct SyntheticCiObserver {
        checks: Vec<crate::agent::objective::CiCheckState>,
    }

    #[async_trait::async_trait]
    impl super::CanonicalPrObserver for SyntheticCiObserver {
        async fn observe(
            &self,
            _repo_root: &Path,
            _pr_number: i64,
            _pr_url: &str,
            _expected_head_sha: &str,
        ) -> anyhow::Result<super::CanonicalPrState> {
            Ok(super::CanonicalPrState::Open)
        }

        async fn observe_ci_checks(
            &self,
            _repo_root: &Path,
            _head_sha: &str,
        ) -> Option<Vec<crate::agent::objective::CiCheckState>> {
            Some(self.checks.clone())
        }
    }

    fn ci_state(
        job: &str,
        conclusion: &str,
        detail: Option<&str>,
    ) -> crate::agent::objective::CiCheckState {
        crate::agent::objective::CiCheckState {
            job: job.into(),
            status: "completed".into(),
            conclusion: Some(conclusion.into()),
            detail: detail.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn m108_a_finished_ci_decision_wakes_the_parked_delivery() {
        use crate::agent::objective::{
            CiCheckState, DELIVERED_CI_FAILED, DELIVERED_CI_GREEN, DELIVERED_CI_TRUST_ROOT_ONLY,
        };
        let cases: Vec<(Vec<CiCheckState>, &str, &str)> = vec![
            (
                vec![
                    ci_state(
                        "check-rust",
                        "failure",
                        Some("test windows_worktree_target_is_shared ... FAILED"),
                    ),
                    ci_state(
                        "scenario-gate-pr",
                        "failure",
                        Some("trust roots cannot self-modify"),
                    ),
                ],
                DELIVERED_CI_FAILED,
                "check-rust",
            ),
            (
                vec![
                    ci_state("check-rust", "success", None),
                    ci_state(
                        "scenario-gate-pr",
                        "failure",
                        Some("trust roots cannot self-modify"),
                    ),
                ],
                DELIVERED_CI_TRUST_ROOT_ONLY,
                "等编排方放行",
            ),
            (
                vec![
                    ci_state("check-rust", "success", None),
                    ci_state("scenario-gate-pr", "success", None),
                ],
                DELIVERED_CI_GREEN,
                "merge-queue: armed",
            ),
        ];
        for (checks, expected_code, expected_prompt) in cases {
            let (_temp, root, _remote, container) = init_repo();
            let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
            crate::agent::objective::ObjectiveStore::new(pool.clone())
                .create(crate::agent::objective::CreateObjective {
                    id: "synthetic-m108".into(),
                    kind: crate::agent::objective::ObjectiveKind::Delivery,
                    session_id: None,
                    root_turn_id: None,
                    domain: crate::agent::objective::RecoveryDomain::Delivery,
                    requested_acceptance: "merged".into(),
                    created_surface: "synthetic-test".into(),
                })
                .await
                .unwrap();
            let workspace = allocate_or_attach(
                &pool,
                request_for(&root, &container, "synthetic-m108", "process-a"),
            )
            .await
            .unwrap();
            record_open_canonical_pr(&pool, &workspace, 713, "through_merge").await;
            // The session is parked exactly as the field incident left it.
            sqlx::query("UPDATE objectives SET status='waiting_system', decision_type='waiting', failure_code='delivered_awaiting_ci', next_observation_at=NULL, attention_request_json=NULL")
                .execute(&pool).await.unwrap();
            reconcile_next_poll(
                &pool,
                &SyntheticCiObserver { checks },
                "m108-observer",
            )
            .await
            .unwrap();
            let (code, next, attention): (Option<String>, Option<i64>, Option<String>) =
                sqlx::query_as(
                    "SELECT failure_code,next_observation_at,attention_request_json FROM objectives WHERE id='synthetic-m108'",
                )
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(
                code.as_deref(),
                Some(expected_code),
                "the parked delivery must take the CI decision, not stay delivered_awaiting_ci"
            );
            assert!(
                next.is_some_and(|at| at > 0),
                "a concluded CI must schedule the wake instead of clearing next_observation_at"
            );
            let prompt = attention.unwrap_or_default();
            assert!(
                prompt.contains(expected_prompt),
                "{expected_code}: the wake input must carry `{expected_prompt}`: {prompt}"
            );
            if expected_code == DELIVERED_CI_FAILED {
                assert!(
                    prompt.contains("windows_worktree_target_is_shared"),
                    "the failing test name must reach the next turn: {prompt}"
                );
            }
        }
    }

    /// M115/R2+R3+R4: GitHub's update-branch / the merge queue rewrite a PR head
    /// into a merge commit whose parents are the session's head and the default
    /// branch. That is the same delivery: the canonical head follows it, the CI
    /// of the NEW head is read, and a red run wakes the parked session with the
    /// queue-head instructions. A head whose parents are not ours stays closed.
    struct QueueUpdatedHeadObserver {
        queue_head: String,
        known_head: String,
        same_delivery: bool,
        checks: Vec<crate::agent::objective::CiCheckState>,
        ci_heads: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl super::CanonicalPrObserver for QueueUpdatedHeadObserver {
        async fn observe(
            &self,
            _repo_root: &Path,
            _pr_number: i64,
            _pr_url: &str,
            _expected_head_sha: &str,
        ) -> anyhow::Result<super::CanonicalPrState> {
            Ok(super::CanonicalPrState::HeadAdvanced {
                head_sha: self.queue_head.clone(),
                queued: true,
            })
        }

        async fn observe_updated_head(
            &self,
            _repo_root: &Path,
            _pr_number: i64,
            known_head_sha: &str,
            actual_head_sha: &str,
        ) -> super::UpdatedHeadVerdict {
            if self.same_delivery
                && known_head_sha == self.known_head
                && actual_head_sha == self.queue_head
            {
                super::UpdatedHeadVerdict::SameDelivery {
                    synced_from_default_branch: "main".into(),
                }
            } else {
                super::UpdatedHeadVerdict::Foreign(
                    "parents do not include the session head".into(),
                )
            }
        }

        async fn observe_ci_checks(
            &self,
            _repo_root: &Path,
            head_sha: &str,
        ) -> Option<Vec<crate::agent::objective::CiCheckState>> {
            self.ci_heads.lock().unwrap().push(head_sha.to_string());
            Some(self.checks.clone())
        }
    }

    #[tokio::test]
    async fn m115_a_queue_updated_head_is_the_same_delivery_and_wakes_on_ci_failure() {
        use crate::agent::objective::DELIVERED_CI_FAILED;
        for same_delivery in [true, false] {
            let (_temp, root, _remote, container) = init_repo();
            let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
            crate::agent::objective::ObjectiveStore::new(pool.clone())
                .create(crate::agent::objective::CreateObjective {
                    id: "synthetic-m115".into(),
                    kind: crate::agent::objective::ObjectiveKind::Delivery,
                    session_id: None,
                    root_turn_id: None,
                    domain: crate::agent::objective::RecoveryDomain::Delivery,
                    requested_acceptance: "merged".into(),
                    created_surface: "synthetic-test".into(),
                })
                .await
                .unwrap();
            let workspace = allocate_or_attach(
                &pool,
                request_for(&root, &container, "synthetic-m115", "process-a"),
            )
            .await
            .unwrap();
            let session_head = git(&workspace.worktree_path, &["rev-parse", "HEAD"]);
            record_open_canonical_pr(&pool, &workspace, 728, "through_merge").await;
            // The session is parked exactly as the field incident left it.
            sqlx::query("UPDATE objectives SET status='waiting_system', decision_type='waiting', failure_code='delivered_awaiting_ci', next_observation_at=NULL, attention_request_json=NULL")
                .execute(&pool).await.unwrap();
            let queue_head = "aacde6dd00000000000000000000000000000000".to_string();
            let observer = QueueUpdatedHeadObserver {
                queue_head: queue_head.clone(),
                known_head: session_head.clone(),
                same_delivery,
                checks: vec![ci_state(
                    "check-rust",
                    "failure",
                    Some("test windows_worktree_target_is_shared ... FAILED"),
                )],
                ci_heads: std::sync::Mutex::new(Vec::new()),
            };
            reconcile_next_poll(&pool, &observer, "m115-observer")
                .await
                .unwrap();
            let (canonical, expected): (Option<String>, String) = sqlx::query_as(
                "SELECT canonical_head_sha, expected_head_sha FROM delivery_runs WHERE objective_id='synthetic-m115'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            let (code, attention): (Option<String>, Option<String>) = sqlx::query_as(
                "SELECT failure_code, attention_request_json FROM objectives WHERE id='synthetic-m115'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if same_delivery {
                assert_eq!(
                    canonical.as_deref(),
                    Some(queue_head.as_str()),
                    "the canonical head must follow the queue-synced merge commit"
                );
                assert_eq!(
                    expected, session_head,
                    "R3: the session's own head stays the identity; only the observed head moves"
                );
                assert_eq!(
                    code.as_deref(),
                    Some(DELIVERED_CI_FAILED),
                    "a red CI on the new head must wake the session"
                );
                let prompt = attention.unwrap_or_default();
                assert!(
                    prompt.contains("windows_worktree_target_is_shared"),
                    "the failing test name must reach the next turn: {prompt}"
                );
                assert!(
                    prompt.contains(&queue_head),
                    "R3: the wake must name the queue head: {prompt}"
                );
                assert!(
                    prompt.contains("git fetch"),
                    "R3: the wake must tell the session to fetch the new head: {prompt}"
                );
                assert_eq!(
                    *observer.ci_heads.lock().unwrap(),
                    vec![queue_head.clone()],
                    "CI must be read for the advanced head, not the stale one"
                );
            } else {
                assert_eq!(
                    canonical.as_deref(),
                    Some(session_head.as_str()),
                    "a stranger head must not move the canonical head"
                );
                assert_ne!(
                    code.as_deref(),
                    Some(DELIVERED_CI_FAILED),
                    "a stranger head must not produce a repair decision"
                );
                assert!(
                    observer.ci_heads.lock().unwrap().is_empty(),
                    "a stranger head must not be observed for CI"
                );
            }
        }
    }

    #[tokio::test]
    async fn lagged_latest_delivery_run_settles_remote_merge() {
        for reached in ["pr_open", "ci_green", "merged"] {
            let (_temp, root, _remote, container) = init_repo();
            let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
            crate::agent::objective::ObjectiveStore::new(pool.clone()).create(
                crate::agent::objective::CreateObjective {
                    id: "synthetic-lagged".into(), kind: crate::agent::objective::ObjectiveKind::Delivery,
                    session_id: None, root_turn_id: None, domain: crate::agent::objective::RecoveryDomain::Delivery,
                    requested_acceptance: "merged".into(), created_surface: "synthetic-test".into(),
                },
            ).await.unwrap();
            let workspace = allocate_or_attach(&pool,
                request_for(&root, &container, "synthetic-lagged", "process-a"),
            ).await.unwrap();
            record_open_canonical_pr(&pool, &workspace, 755, "through_merge").await;
            sqlx::query("UPDATE delivery_runs SET id='older-run', status='completed', updated_at=1")
                .execute(&pool).await.unwrap();
            sqlx::query("UPDATE objectives SET status='waiting_system', decision_type='waiting', failure_code='delivered_awaiting_ci', delivery_run_id='older-run'")
                .execute(&pool).await.unwrap();
            record_open_canonical_pr(&pool, &workspace, 755, "through_merge").await;
            sqlx::query("UPDATE delivery_runs SET reached_ceiling=?, stage=CASE WHEN ?='merged' THEN 'complete' ELSE 'pr_open' END, status=CASE WHEN ?='merged' THEN 'completed' ELSE 'waiting' END WHERE id<>'older-run'")
                .bind(reached).bind(reached).bind(reached).execute(&pool).await.unwrap();
            for state in [super::CanonicalPrState::Unknown("offline".into()), super::CanonicalPrState::ClosedUnmerged] {
                reconcile_next_poll(&pool, &ScriptedPrObserver::new(vec![Ok(state)]), "lagged-test").await.unwrap();
                let status: String = sqlx::query_scalar("SELECT status FROM objectives").fetch_one(&pool).await.unwrap();
                assert_eq!(status, "waiting_system", "only positive merge evidence settles");
            }
            let before: i64 = sqlx::query_scalar("SELECT revision FROM objectives").fetch_one(&pool).await.unwrap();
            for _ in 0..2 {
                reconcile_next_poll(&pool, &ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged { merge_sha: "synthetic-merge".into() })]), "lagged-test").await.unwrap();
                let actual: (String, String, String, i64) = sqlx::query_as("SELECT o.status,o.reached_acceptance,r.reached_ceiling,o.revision FROM objectives o JOIN delivery_runs r ON r.id=o.delivery_run_id")
                    .fetch_one(&pool).await.unwrap();
                assert_eq!(actual, ("completed".into(), "merged".into(), "merged".into(), before + 1), "latest {reached} run must settle once despite a stale objective pointer");
            }
        }
    }

    #[tokio::test]
    async fn m71_failed_takeover_tracks_open_queued_and_external_merge() {
        let (_temp, root, remote, container) = init_repo();
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        crate::agent::objective::ObjectiveStore::new(pool.clone()).create(
            crate::agent::objective::CreateObjective {
                id: "synthetic-m71".into(), kind: crate::agent::objective::ObjectiveKind::Delivery,
                session_id: None, root_turn_id: None, domain: crate::agent::objective::RecoveryDomain::Delivery,
                requested_acceptance: "delivery_receipt".into(), created_surface: "synthetic-test".into(),
            },
        ).await.unwrap();
        let workspace = allocate_or_attach(
            &pool, request_for(&root, &container, "synthetic-m71", "process-a"),
        ).await.unwrap();
        std::fs::write(workspace.worktree_path.join("change.txt"), "delivered\n").unwrap();
        git(&workspace.worktree_path, &["add", "change.txt"]);
        git(&workspace.worktree_path, &["commit", "-m", "fix: synthetic delivery"]);
        publish_workspace_branch(&root, &remote, &workspace.branch_name);
        record_open_canonical_pr(&pool, &workspace, 91, "pr_only").await;
        // The old takeover failed before other session steps opened and armed the PR.
        sqlx::query("UPDATE delivery_runs SET stage='takeover_mutation_reconciliation', status='platform_incident', wait_class='external_state_uncertain', failure_signature='synthetic unresolved intent', lease_owner=NULL, lease_expires_at=NULL")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE objectives SET status='waiting_system', decision_type='waiting', failure_code='run_blocked', delivery_run_id=(SELECT id FROM delivery_runs WHERE objective_id=objectives.id)").execute(&pool).await.unwrap();
        sqlx::query("UPDATE execution_workspaces SET state='delivering', lease_owner=NULL, lease_expires_at=NULL")
            .execute(&pool).await.unwrap();
        // M114: the observation bookkeeping columns are added on demand.
        super::ensure_pr_observation_columns(&pool).await.unwrap();
        for (observed, expected) in [
            (super::CanonicalPrState::Open, "pr_open"),
            (super::CanonicalPrState::Queued, "merge_queued"),
        ] {
            let observer = ScriptedPrObserver::new(vec![Ok(observed)]);
            // M114/R3: represent the next scheduled poll (the pass itself now
            // refuses to re-ask inside the backoff window).
            sqlx::query("UPDATE delivery_runs SET pr_observation_next_at=NULL")
                .execute(&pool).await.unwrap();
            super::run_cleanup_pass_with(&pool, &container, "m71-observer", &observer).await.unwrap();
            let (stage, status, reached): (String, String, String) = sqlx::query_as(
                "SELECT stage, status, reached_ceiling FROM delivery_runs",
            ).fetch_one(&pool).await.unwrap();
            assert_eq!(stage, expected);
            assert_eq!(status, "waiting");
            assert_eq!(reached, expected);
            let (decision, failure, acceptance): (String, Option<String>, Option<String>) = sqlx::query_as(
                "SELECT decision_type, failure_code, reached_acceptance FROM objectives",
            ).fetch_one(&pool).await.unwrap();
            assert_eq!(decision, "waiting");
            assert_eq!(failure.as_deref(), Some("delivered_awaiting_ci"));
            assert_eq!(acceptance.as_deref(), Some(expected));
            assert!(workspace.worktree_path.exists());
        }
        // An identical observation is not new progress; head drift / offline
        // uncertainty cannot erase the queue handoff or manufacture incidents.
        let revision: i64 = sqlx::query_scalar("SELECT progress_revision FROM delivery_runs").fetch_one(&pool).await.unwrap();
        for state in [super::CanonicalPrState::Queued, super::CanonicalPrState::Open,
            super::CanonicalPrState::Unknown("synthetic network outage".into())] {
            super::run_cleanup_pass_with(&pool, &container, "m71-observer", &ScriptedPrObserver::new(vec![Ok(state)]))
                .await.unwrap();
            let actual: i64 = sqlx::query_scalar("SELECT progress_revision FROM delivery_runs").fetch_one(&pool).await.unwrap();
            assert_eq!(actual, revision);
        }
        let observer = ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged {
            merge_sha: "synthetic-m71-merge".into(),
        })]);
        // M114/R3: the next scheduled poll (outside the backoff window).
        sqlx::query("UPDATE delivery_runs SET pr_observation_next_at=NULL")
            .execute(&pool).await.unwrap();
        let outcome = super::run_cleanup_pass_with(&pool, &container, "m71-observer", &observer).await.unwrap();
        assert_eq!(outcome.closed, 1);
        let status: String = sqlx::query_scalar("SELECT status FROM objectives").fetch_one(&pool).await.unwrap();
        assert_eq!(status, "completed");
        let (stage, status, reached): (String, String, String) = sqlx::query_as(
            "SELECT stage, status, reached_ceiling FROM delivery_runs",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!((stage.as_str(), status.as_str(), reached.as_str()), ("complete", "completed", "merged"));
        assert!(!workspace.worktree_path.exists());
        assert!(!super::branch_exists(&root, &workspace.branch_name));
        assert!(!super::remote_branch_exists(&root, "origin", &workspace.branch_name).unwrap());
    }

    /// A merge alone is not acceptance for a live/release objective: the record
    /// advances to merged, but the objective is not completed and nothing on
    /// disk is deleted until real live verification settles it.
    #[tokio::test]
    async fn m71_external_merge_never_fakes_live_acceptance() {
        let (_temp, root, remote, container) = init_repo();
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        crate::agent::objective::ObjectiveStore::new(pool.clone()).create(
            crate::agent::objective::CreateObjective {
                id: "synthetic-m71-live".into(), kind: crate::agent::objective::ObjectiveKind::Delivery,
                session_id: None, root_turn_id: None, domain: crate::agent::objective::RecoveryDomain::Delivery,
                requested_acceptance: "live_verified".into(), created_surface: "synthetic-test".into(),
            },
        ).await.unwrap();
        let workspace = allocate_or_attach(
            &pool, request_for(&root, &container, "synthetic-m71-live", "process-a"),
        ).await.unwrap();
        std::fs::write(workspace.worktree_path.join("live.txt"), "delivered\n").unwrap();
        git(&workspace.worktree_path, &["add", "live.txt"]);
        git(&workspace.worktree_path, &["commit", "-m", "fix: synthetic live delivery"]);
        publish_workspace_branch(&root, &remote, &workspace.branch_name);
        record_open_canonical_pr(&pool, &workspace, 92, "through_release").await;
        sqlx::query("UPDATE delivery_runs SET status='platform_incident', stage='takeover_mutation_reconciliation', lease_owner=NULL, lease_expires_at=NULL")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE objectives SET status='waiting_system', decision_type='waiting', delivery_run_id=(SELECT id FROM delivery_runs WHERE objective_id=objectives.id)")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE execution_workspaces SET state='delivering', lease_owner=NULL, lease_expires_at=NULL")
            .execute(&pool).await.unwrap();

        let observer = ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Merged {
            merge_sha: "synthetic-m71-live-merge".into(),
        })]);
        let outcome = super::run_cleanup_pass_with(&pool, &container, "m71-observer", &observer)
            .await.unwrap();
        let probe: (String, String, String) = sqlx::query_as(
            "SELECT objectives.status, delivery_runs.status, delivery_runs.requested_ceiling
             FROM objectives JOIN delivery_runs ON delivery_runs.objective_id=objectives.id",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!((outcome.closed, probe.0.as_str(), probe.1.as_str(), probe.2.as_str()),
            (0, "waiting_system", "waiting", "through_release"));
        let (status, reached): (String, String) = sqlx::query_as(
            "SELECT objectives.status, delivery_runs.reached_ceiling FROM objectives
             JOIN delivery_runs ON delivery_runs.objective_id=objectives.id",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "waiting_system", "a merge is not live acceptance");
        assert_eq!(reached, "merged");
        assert!(workspace.worktree_path.exists(), "nothing is deleted without live acceptance");
        assert!(super::branch_exists(&root, &workspace.branch_name));
    }

    /// CF-CLO-R4 + CF-CLO-R3: a PR closed without merging deletes nothing and
    /// says in plain words where the work stays.
    #[tokio::test]
    async fn externally_closed_unmerged_pr_keeps_everything_and_says_where() {
        let (_temp, root, remote, container) = init_repo();
        let pool = pool_with_objective("objective-closed-pr").await;
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-closed-pr", "process-a"),
        )
            .await
            .unwrap();
        std::fs::write(workspace.worktree_path.join("change.txt"), "delivered\n").unwrap();
        git(&workspace.worktree_path, &["add", "change.txt"]);
        git(
            &workspace.worktree_path,
            &["commit", "-m", "delivered change"],
        );
        publish_workspace_branch(&root, &remote, &workspace.branch_name);
        record_open_canonical_pr(&pool, &workspace, 88, "through_merge").await;

        let observer =
            ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::ClosedUnmerged)]);
        let outcome =
            super::run_cleanup_pass_with(&pool, &container, "cleanup-a", &observer)
                .await
                .unwrap();

        assert_eq!(outcome.closed, 0);
        assert_eq!(outcome.preserved, 1);
        assert!(workspace.worktree_path.is_dir());
        assert!(workspace.worktree_path.join("change.txt").is_file());
        assert!(super::branch_exists(&root, &workspace.branch_name));
        let remote_heads = git(&root, &["ls-remote", "--heads", "origin"]);
        assert!(
            super::remote_branch_exists(&root, "origin", &workspace.branch_name).unwrap(),
            "remote heads after push: {remote_heads:?}"
        );
        let (state, code, detail) = workspace_cleanup_cell(&pool, &workspace.objective_id).await;
        assert_eq!(state, "cleanup_pending");
        assert_eq!(code.as_deref(), Some("workspace_cleanup_pr_closed_unmerged"));
        let detail = detail.unwrap();
        assert!(detail.contains("PR #88 已关闭但未合并"), "{detail}");
        assert!(
            detail.contains(workspace.worktree_path.to_str().unwrap()),
            "{detail}"
        );
    }

    /// CF-CLO-R3: an unobservable merge state never deletes anything.
    #[tokio::test]
    async fn unproven_merge_state_is_never_deleted() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-unknown-pr").await;
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-unknown-pr", "process-a"),
        )
            .await
            .unwrap();
        record_open_canonical_pr(&pool, &workspace, 99, "through_merge").await;

        let observer = ScriptedPrObserver::new(vec![Ok(super::CanonicalPrState::Unknown(
            "network down".into(),
        ))]);
        let outcome =
            super::run_cleanup_pass_with(&pool, &container, "cleanup-a", &observer)
                .await
                .unwrap();

        assert_eq!(outcome.closed, 0);
        assert_eq!(outcome.preserved, 1);
        assert!(workspace.worktree_path.is_dir());
        assert!(super::branch_exists(&root, &workspace.branch_name));
        let (state, code, _) = workspace_cleanup_cell(&pool, &workspace.objective_id).await;
        assert_eq!(state, "cleanup_pending");
        assert_eq!(code.as_deref(), Some("workspace_cleanup_merge_unproven"));
    }

    /// CF-CLO-R7: an interrupted closeout (network failure during observation)
    /// leaves no half-finished state and resumes at the next pass.
    #[tokio::test]
    async fn interrupted_closeout_resumes_and_completes_next_pass() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objective("objective-interrupted").await;
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-interrupted", "process-a"),
        )
            .await
            .unwrap();
        std::fs::write(workspace.worktree_path.join("change.txt"), "delivered\n").unwrap();
        git(&workspace.worktree_path, &["add", "change.txt"]);
        git(
            &workspace.worktree_path,
            &["commit", "-m", "delivered change"],
        );
        record_open_canonical_pr(&pool, &workspace, 55, "through_merge").await;

        let observer = ScriptedPrObserver::new(vec![
            Err(anyhow::anyhow!("network outage during observation")),
            Ok(super::CanonicalPrState::Merged {
                merge_sha: "merged-after-restart".into(),
            }),
        ]);

        // First pass: observation fails. Nothing is deleted and no partial state
        // is left behind.
        let interrupted = super::run_cleanup_pass_with(&pool, &container, "cleanup-a", &observer)
            .await
            .unwrap();
        assert_eq!(interrupted.closed, 0);
        assert_eq!(interrupted.preserved, 1);
        assert!(workspace.worktree_path.is_dir());
        assert!(super::branch_exists(&root, &workspace.branch_name));
        let (state, code, detail) =
            workspace_cleanup_cell(&pool, &workspace.objective_id).await;
        assert_eq!(state, "cleanup_pending");
        assert_eq!(code.as_deref(), Some("workspace_cleanup_merge_unproven"));
        assert!(detail.unwrap().contains("network outage during observation"));

        // Simulate the next scheduled poll after the retry backoff: the same
        // pass resumes and completes, with no manual repair.
        sqlx::query(
            "UPDATE execution_workspaces SET failure_code=NULL, updated_at=?
             WHERE objective_id=?",
        )
        .bind(chrono::Utc::now().timestamp_millis() - 600_000)
        .bind(&workspace.objective_id)
        .execute(&pool)
        .await
        .unwrap();
        let finished = super::run_cleanup_pass_with(&pool, &container, "cleanup-b", &observer)
            .await
            .unwrap();
        assert_eq!(finished.closed, 1);
        assert!(!workspace.worktree_path.exists());
        assert!(!super::branch_exists(&root, &workspace.branch_name));
        let (state, _, _) = workspace_cleanup_cell(&pool, &workspace.objective_id).await;
        assert_eq!(state, "closed");
    }
    /// U26 / CF-WSC-R5: once the work reached a merge, the next message starts
    /// from the latest main instead of dragging the merged branch along.
    #[tokio::test]
    async fn continuing_after_a_merge_starts_from_the_latest_main() {
        let (temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-merged-a", "objective-merged-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-merged-a", "process-a"),
        )
        .await
        .unwrap();
        commit_file(&workspace_a.worktree_path, "merged.rs", "// merged\n");
        record_terminal_merge_receipt(&pool, &workspace_a).await;

        // main moves on while the user is away
        let seed = temp.path().join("seed");
        commit_file(&seed, "after-merge.txt", "moved on\n");
        git(&seed, &["push", "origin", "main"]);
        let latest_main = git(&seed, &["rev-parse", "origin/main"]);

        let workspace_b = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-merged-b", "process-b"),
        )
        .await
        .unwrap();

        assert_ne!(workspace_b.worktree_path, workspace_a.worktree_path);
        assert_eq!(
            workspace_b.base_sha, latest_main,
            "the fresh workspace must be based on the latest main"
        );
        assert!(
            !workspace_b.worktree_path.join("merged.rs").exists(),
            "the merged branch must not be dragged into the fresh workspace"
        );
        assert!(workspace_b.worktree_path.join("after-merge.txt").is_file());
    }

    /// U26 / CF-WSC-R4: when the previous workspace really is gone, the session
    /// says in plain words where the changes are instead of silently starting
    /// from an empty checkout.
    #[tokio::test]
    async fn unfinished_work_that_cannot_be_continued_says_where_it_is() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-gone-a", "objective-gone-b"]).await;
        let workspace_a = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-gone-a", "process-a"),
        )
        .await
        .unwrap();
        std::fs::write(workspace_a.worktree_path.join("staged.rs"), "// staged\n").unwrap();
        git(&workspace_a.worktree_path, &["add", "staged.rs"]);
        cancel_objective(&pool, "objective-gone-a").await;
        let lost = workspace_a.worktree_path.to_string_lossy().into_owned();
        std::fs::remove_dir_all(&workspace_a.worktree_path).unwrap();

        let workspace_b = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-gone-b", "process-b"),
        )
        .await
        .unwrap();

        assert_ne!(
            workspace_b.worktree_path, workspace_a.worktree_path,
            "a workspace that is really gone cannot be continued in place"
        );
        let (code, detail) = workspace_failure(&pool, "objective-gone-b").await;
        assert_eq!(code, PREVIOUS_WORK_NOT_CARRIED_FORWARD);
        assert!(
            detail.contains(&lost),
            "the notice must name where the previous work is: {detail}"
        );
        assert!(
            detail.contains("没有被删除"),
            "the notice must say the changes were kept: {detail}"
        );
        crate::agent::failure_summary::assert_no_internal_vocabulary(&detail)
            .expect("the notice is shown to the user and must be plain language");
    }

    /// U26 / CF-WSC-R2: a workspace whose PR is still open is unfinished, so the
    /// cleanup pass preserves it — and the user can still continue in it.
    #[tokio::test]
    async fn cleanup_preserves_a_workspace_whose_pr_is_still_open() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-open-pr-cleanup"]).await;
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-open-pr-cleanup", "process-a"),
        )
        .await
        .unwrap();
        commit_file(&workspace.worktree_path, "on-pr.rs", "// on an open PR\n");
        complete_objective(&pool, "objective-open-pr-cleanup").await;
        record_open_pr(&pool, "objective-open-pr-cleanup", 23).await;

        let outcome = run_cleanup_pass(&pool, &container, "cleanup-process")
            .await
            .unwrap();

        assert_eq!(outcome.closed, 0, "an open PR is not a merged result");
        assert_eq!(outcome.preserved, 1);
        assert!(workspace.worktree_path.is_dir());
        assert_eq!(
            workspace_state_of(&pool, "objective-open-pr-cleanup")
                .await
                .as_deref(),
            Some("cleanup_pending"),
            "the workspace stays available for the continuing session"
        );
    }

    /// U26 / CF-WSC-R2: uncommitted work is never cleaned up, even after the
    /// closeout lifecycle has claimed the workspace.
    #[tokio::test]
    async fn cleanup_preserves_a_workspace_with_uncommitted_work() {
        let (_temp, root, _remote, container) = init_repo();
        let pool = pool_with_objectives(&["objective-dirty-cleanup"]).await;
        let workspace = allocate_or_attach(
            &pool,
            request_for(&root, &container, "objective-dirty-cleanup", "process-a"),
        )
        .await
        .unwrap();
        std::fs::write(workspace.worktree_path.join("staged.rs"), "// staged\n").unwrap();
        git(&workspace.worktree_path, &["add", "staged.rs"]);
        complete_objective(&pool, "objective-dirty-cleanup").await;

        let outcome = run_cleanup_pass(&pool, &container, "cleanup-process")
            .await
            .unwrap();

        assert_eq!(outcome.closed, 0);
        assert_eq!(outcome.preserved, 1);
        assert_eq!(
            std::fs::read_to_string(workspace.worktree_path.join("staged.rs")).unwrap(),
            "// staged\n"
        );
    }
}
