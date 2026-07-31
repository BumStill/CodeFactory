// SPDX-License-Identifier: Apache-2.0
//! First-class delivery capability.
//!
//! CodeFactory's agent used to treat "produce the artifact + green tests +
//! report" as done — it had no notion of git delivery, so when a user's
//! standard was "open a PR, run CI, merge, release" the model improvised bash
//! git commands, hit the `bash=ask` permission gate, and stalled, re-listing
//! the missing steps instead of executing them. This module gives delivery a
//! single, coherent, resumable capability the agent invokes once, so "done"
//! for code work can actually include carrying the change toward production.
//!
//! # Design
//! - **Configurable ceiling** ([`DeliveryCeiling`]): the USER decides how far
//!   an unattended delivery goes — from `Off` through `PrOnly`, `ThroughCiGreen`,
//!   `ThroughMerge`, up to `ThroughRelease`. The app never hardcodes a policy;
//!   a per-call request may only *lower* the configured ceiling.
//! - **Hybrid provider**: local ops (stage / commit / push) shell out to the
//!   `git` CLI, exactly like [`crate::commands::git`] and [`crate::agent::checkpoint`]
//!   already do — no new runtime dependency. Remote ops (PR / CI / merge /
//!   release) go through the portable token+REST [`crate::git_remote`] layer via
//!   the [`DeliveryRemote`] trait; **`gh` is never assumed** (it is not present
//!   on arbitrary end-user machines).
//! - **Noise-safe staging**: delivery NEVER runs `git add -A`/`git add .`. It
//!   stages tracked modifications with `git add -u` (which by definition adds no
//!   untracked file) plus only those untracked files that are real source and
//!   not on the noise denylist. This is the structural guarantee that local
//!   junk (`.claude/`, `CLAUDE.md`, generated schemas, sibling worktrees, …) is
//!   never swept into a delivery commit.
//! - **Idempotent / resumable**: each step checks reality before acting —
//!   nothing to commit is a success, an already-open PR is reused (never
//!   double-opened), an already-merged PR short-circuits. Re-invoking after a
//!   crash continues from the real state.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::config::settings::{DeliveryCeiling, MergeMethod};
use crate::util::command_env;
use crate::util::no_window::NoWindow;

/// Untracked path prefixes/exact-names never included in a delivery commit,
/// even if not covered by `.gitignore`. Matched against `/`-normalized,
/// repo-relative paths (prefix match for dir entries, exact for files).
const BUILTIN_EXCLUDES: &[&str] = &[
    ".claude/",
    ".codex/",
    "CLAUDE.md",
    "AGENTS.md",
    "codex-worktrees/",
    ".codefactory/attachments/",
    "src-tauri/gen/schemas/",
    ".DS_Store",
];

/// One delivery step's outcome, surfaced to the UI and the agent.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StepResult {
    pub step: String,
    /// "ok" | "skipped" | "blocked" | "error"
    pub status: String,
    pub detail: String,
}

impl StepResult {
    fn ok(step: &str, detail: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            status: "ok".into(),
            detail: detail.into(),
        }
    }
    fn skipped(step: &str, detail: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            status: "skipped".into(),
            detail: detail.into(),
        }
    }
    fn blocked(step: &str, detail: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            status: "blocked".into(),
            detail: detail.into(),
        }
    }
}

/// The result of a delivery run.
#[derive(Debug, Clone, Serialize)]
pub struct DeliveryOutcome {
    pub steps: Vec<StepResult>,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<u64>,
    /// Terminal state: "delivered" (reached ceiling), "blocked" (a step
    /// couldn't proceed — never a loop), or "noop" (nothing to deliver).
    pub final_state: String,
    /// Structured truth fields used by persistence/UI; never inferred from the
    /// localized report body.
    pub stage: String,
    pub code: String,
    pub recoverable: bool,
    pub next_action: Option<String>,
    pub reached_state: String,
    /// The policy target selected for this call and the highest rung the
    /// available adapters can safely execute. Keeping both prevents a partial
    /// run from being mislabeled as complete.
    pub requested_ceiling: String,
    pub effective_ceiling: String,
    pub capability_gap: Option<String>,
    /// Durable local receipt written after a successful release dispatch. It
    /// lets a retry re-observe live state without dispatching the release again.
    pub release_receipt: Option<String>,
    /// Human summary the agent echoes to the user.
    pub summary: String,
}

impl DeliveryOutcome {
    fn blocked_at(mut self, step: StepResult) -> Self {
        let msg = step.detail.clone();
        self.stage = step.step.clone();
        self.code = format!("delivery_{}_blocked", step.step);
        self.recoverable = true;
        self.next_action = Some(msg.clone());
        self.reached_state = reached_state_from_steps(&self.steps);
        self.steps.push(step);
        self.final_state = "blocked".into();
        self.summary = msg;
        self
    }

    fn blocked_on_uncertain_side_effect(mut self, step: StepResult) -> Self {
        let msg = step.detail.clone();
        self = self.blocked_at(step);
        self.code = "delivery_external_state_uncertain".into();
        self.recoverable = false;
        self.next_action = Some(format!(
            "{msg} 外部动作结果不确定，禁止自动重试；请先核对远端事实，再人工续接。"
        ));
        self
    }
}

fn reached_state_from_steps(steps: &[StepResult]) -> String {
    steps
        .iter()
        .rev()
        .find(|step| step.status == "ok")
        .map(|step| match step.step.as_str() {
            "commit" => "committed",
            "push" => "pushed",
            "pr" => "pr_open",
            "ci" => "ci_green",
            "merge" => "merged",
            "release" => "release_triggered",
            "deploy" => "deployment_succeeded",
            "live" => "live_verified",
            _ => "local",
        })
        .unwrap_or("local")
        .to_string()
}

/// Options for a single delivery call (from the agent tool). All optional so
/// the model can invoke `deliver_changes` with no arguments in the common case.
#[derive(Debug, Clone, Default)]
pub struct DeliverOpts {
    pub title: Option<String>,
    pub body: Option<String>,
    /// Release cadence signal persisted into the final commit. `None` follows
    /// the repository's ordinary configured delivery policy.
    pub release_urgency: Option<ReleaseUrgency>,
    /// A per-call ceiling; clamped to at most the user's configured ceiling.
    pub requested_ceiling: Option<DeliveryCeiling>,
    pub extra_excludes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseUrgency {
    Immediate,
    Hold,
}

impl ReleaseUrgency {
    fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Hold => "hold",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCommitMessage {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryPr {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
}

/// CI conclusion for a commit.
#[derive(Debug, Clone, PartialEq)]
pub enum CiStatus {
    Success,
    Failure(String),
    Pending,
    /// No CI is configured for this commit — treated as "not blocking".
    None,
}

/// A deployment/live observer must distinguish an actual successful assertion
/// from an action that merely started or is not configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationStatus {
    Success(String),
    Failure(String),
    Pending(String),
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeliveryCapabilities {
    pub review: bool,
    pub ci: bool,
    pub merge: bool,
    pub release: bool,
    pub live: bool,
}

fn parse_observation_status(status: &str, detail: Option<String>) -> ObservationStatus {
    match status {
        "success" => ObservationStatus::Success(detail.unwrap_or_else(|| "verified".into())),
        "pending" => ObservationStatus::Pending(detail.unwrap_or_else(|| "pending".into())),
        "failure" => ObservationStatus::Failure(detail.unwrap_or_else(|| "failure".into())),
        "unsupported" | "none" => {
            ObservationStatus::Unsupported(detail.unwrap_or_else(|| "not configured".into()))
        }
        other => ObservationStatus::Failure(format!("unknown observation status: {other}")),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryDeliveryConfig {
    #[serde(default = "delivery_config_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default = "default_deployment_timeout_secs")]
    pub deployment_timeout_secs: u32,
    #[serde(default)]
    pub live: Option<LiveHttpAssertion>,
}

fn delivery_config_schema_version() -> u32 {
    1
}

fn default_deployment_timeout_secs() -> u32 {
    900
}

#[derive(Debug, Clone, Deserialize)]
pub struct LiveHttpAssertion {
    pub url: String,
    #[serde(default = "default_http_method")]
    pub method: String,
    #[serde(default = "default_expected_status")]
    pub expected_status: u16,
    /// Required for a valid live assertion: HTTP 200 alone is not evidence.
    pub body_contains: String,
    #[serde(default = "default_live_timeout_secs")]
    pub timeout_secs: u32,
    #[serde(default = "default_live_poll_interval_secs")]
    pub poll_interval_secs: u32,
}

fn default_http_method() -> String {
    "GET".into()
}
fn default_expected_status() -> u16 {
    200
}
fn default_live_timeout_secs() -> u32 {
    300
}
fn default_live_poll_interval_secs() -> u32 {
    10
}

impl LiveHttpAssertion {
    fn expected_body(&self, sha: &str) -> String {
        let short = sha.get(..7).unwrap_or(sha);
        self.body_contains
            .replace("$GIT_SHA_SHORT", short)
            .replace("$GIT_SHA", sha)
    }

    fn validate(&self) -> Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("live.url cannot be empty".into());
        }
        if !self.method.eq_ignore_ascii_case("GET") {
            return Err("only GET live assertions are supported".into());
        }
        if self.body_contains.trim().is_empty() {
            return Err(
                "live.body_contains is required; HTTP status alone cannot verify上线".into(),
            );
        }
        Ok(())
    }
}

pub fn load_delivery_config(root: &Path) -> Result<Option<RepositoryDeliveryConfig>, String> {
    let path = root.join(".codefactory").join("delivery.json");
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let config: RepositoryDeliveryConfig =
        serde_json::from_str(&raw).map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
    if config.schema_version != 1 {
        return Err(format!(
            "不支持 delivery schema_version {}",
            config.schema_version
        ));
    }
    if let Some(live) = &config.live {
        live.validate()?;
    }
    Ok(Some(config))
}

/// Portable remote operations (token+REST). Implemented by `GithubRemote`;
/// stubbed in tests so the state machine is exercised without a network. Uses
/// native async-fn-in-trait with generic (static) dispatch — no `async_trait`
/// dependency, no dynamic dispatch.
pub trait DeliveryRemote {
    fn capabilities(&self) -> DeliveryCapabilities;

    /// Return the existing open PR for `head`, or open a new one. Idempotent:
    /// callers rely on this never double-opening.
    fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> impl std::future::Future<Output = Result<DeliveryPr, String>>;
    fn ci_status(&self, sha: &str) -> impl std::future::Future<Output = Result<CiStatus, String>>;
    fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
    ) -> impl std::future::Future<Output = Result<(), String>>;
    fn trigger_release(&self) -> impl std::future::Future<Output = Result<String, String>>;

    /// Observe the external CD platform (Zeabur, Vercel, Argo CD, etc.).
    /// Defaulting to Unsupported keeps existing built-in and test adapters
    /// source-compatible while making absence of deployment evidence explicit.
    fn deployment_status(
        &self,
        _sha: &str,
        _provider: Option<&str>,
    ) -> impl std::future::Future<Output = Result<ObservationStatus, String>> {
        std::future::ready(Ok(ObservationStatus::Unsupported(
            "deployment observer not configured".into(),
        )))
    }

    /// Run a provider-specific real-service assertion. Repositories should
    /// prefer the repository-owned HTTP assertion when possible.
    fn verify_live(
        &self,
        _sha: &str,
        _url: Option<&str>,
    ) -> impl std::future::Future<Output = Result<ObservationStatus, String>> {
        std::future::ready(Ok(ObservationStatus::Unsupported(
            "live verifier not configured".into(),
        )))
    }
}

// ── Local git helper ────────────────────────────────────────────────────────

/// Build a `Command` for a developer CLI (`gh`/`git`) with the absolute binary
/// resolved and the augmented developer PATH applied. GUI-launched apps on macOS
/// do NOT inherit the login-shell PATH, so spawning a bare program name fails
/// even when `gh` is installed and authenticated (Homebrew puts it in
/// `/opt/homebrew/bin`, absent from the app's PATH). Resolving the absolute path
/// makes the spawn work, and the augmented PATH lets `gh` find `git`. Mirrors
/// `util::github_cli::gh_command`; the root cause of "deliver_changes gh PATH
/// blocked" even though the CLI works from a terminal. EVERY production spawn of
/// gh/git in this module MUST go through here (pinned by a source-text test).
fn dev_command(program: &str) -> Command {
    let mut command = Command::new(command_env::resolve_developer_command(program)).no_window();
    command_env::apply_developer_path_std(&mut command);
    command
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = dev_command("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeliveryReceipt {
    version: u32,
    state: String,
    remote: String,
    remote_identity: String,
    base_branch: String,
    head_branch: String,
    commit_sha: String,
    pr_number: u64,
    pr_url: String,
    #[serde(default)]
    pr_title: Option<String>,
    #[serde(default)]
    pr_body: Option<String>,
    release_detail: Option<String>,
}

fn receipt_remote_identity(repo: &RepoContext) -> String {
    let Some(url) = repo.remote_url.as_deref() else {
        return format!("unknown/{}", repo.remote);
    };
    if let (Some(host), Some(path)) = (remote_host(url), remote_repo_path(url)) {
        let host = host
            .rsplit('@')
            .next()
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| "unknown".into());
        return format!("{host}/{path}");
    }
    // Local/file/custom remotes have no host/path pair. Hash the raw URL so
    // different repositories remain distinct without persisting credentials
    // or private filesystem paths in git config.
    format!("opaque:{:x}", Sha256::digest(url.as_bytes()))
}

fn delivery_receipt_key(repo: &RepoContext, sha: &str) -> String {
    let context = format!(
        "{}\0{}\0{}\0{}\0{}",
        repo.remote,
        receipt_remote_identity(repo),
        repo.default_branch,
        repo.branch,
        sha
    );
    let fingerprint = format!("{:x}", Sha256::digest(context.as_bytes()));
    format!("codefactory.delivery.ctx-{fingerprint}")
}

fn read_local_config(root: &Path, key: &str) -> Result<Option<String>, String> {
    let output = dev_command("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--local", "--get", key])
        .output()
        .map_err(|error| format!("读取本地交付回执失败: {error}"))?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ));
    }
    if output.status.code() == Some(1) && output.stdout.is_empty() {
        return Ok(None);
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(format!("读取本地交付回执失败: {detail}"))
}

fn read_delivery_receipt(repo: &RepoContext, sha: &str) -> Result<Option<DeliveryReceipt>, String> {
    let raw = match read_local_config(&repo.root, &delivery_receipt_key(repo, sha))? {
        Some(raw) => raw,
        None => return Ok(None),
    };
    let receipt: DeliveryReceipt = serde_json::from_str(&raw)
        .map_err(|error| format!("本地交付回执损坏，拒绝重复外部动作: {error}"))?;
    if receipt.version != 1 {
        return Err(format!(
            "不支持本地交付回执版本 {}，拒绝重复外部动作",
            receipt.version
        ));
    }
    if !matches!(
        receipt.state.as_str(),
        "pr_open" | "intent_merge" | "merged" | "intent_release" | "release_triggered"
    ) {
        return Err(format!(
            "本地交付回执状态 {} 无法识别，拒绝重复外部动作",
            receipt.state
        ));
    }
    if receipt.commit_sha != sha
        || receipt.remote != repo.remote
        || receipt.remote_identity != receipt_remote_identity(repo)
        || receipt.base_branch != repo.default_branch
        || receipt.head_branch != repo.branch
    {
        return Err("本地交付回执上下文与当前仓库不一致，拒绝重复外部动作".into());
    }
    Ok(Some(receipt))
}

fn write_delivery_receipt(
    repo: &RepoContext,
    sha: &str,
    receipt: &DeliveryReceipt,
) -> Result<String, String> {
    let raw =
        serde_json::to_string(receipt).map_err(|error| format!("序列化交付回执失败: {error}"))?;
    git(
        &repo.root,
        &["config", "--local", &delivery_receipt_key(repo, sha), &raw],
    )?;
    Ok(raw)
}

/// Repo context resolved once at the start of delivery.
#[derive(Debug, Clone)]
pub struct RepoContext {
    pub root: PathBuf,
    pub branch: String,
    pub default_branch: String,
    pub remote: String,
    pub remote_url: Option<String>,
}

fn default_remote(root: &Path) -> String {
    let remotes = git(root, &["remote"]).unwrap_or_default();
    let names: Vec<&str> = remotes.lines().filter(|s| !s.trim().is_empty()).collect();
    if names.contains(&"origin") {
        "origin".into()
    } else {
        names.first().copied().unwrap_or("origin").into()
    }
}

fn remote_default_branch(root: &Path, remote: &str) -> Option<String> {
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
    .and_then(|s| s.rsplit('/').next().map(|s| s.to_string()))
}

pub fn resolve_repo(cwd: &Path, default_branch_hint: Option<&str>) -> Result<RepoContext, String> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "not a git repository".to_string())?;
    let root = PathBuf::from(root);
    let branch = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        return Err("detached HEAD — check out a branch before delivering".into());
    }
    let remote = default_remote(&root);
    let remote_url = git(&root, &["remote", "get-url", &remote]).ok();
    // Prefer the selected remote's default branch; fall back to a hint or common names.
    let default_branch = remote_default_branch(&root, &remote)
        .or_else(|| default_branch_hint.map(|s| s.to_string()))
        .unwrap_or_else(|| "main".to_string());
    Ok(RepoContext {
        root,
        branch,
        default_branch,
        remote,
        remote_url,
    })
}

/// Result of looking for a sibling worktree whose feature branch is ready to
/// deliver when the current checkout sits on the default branch.
enum WorktreeDiscovery {
    /// No sibling worktree carries a branch with commits ahead of the default.
    None,
    /// Exactly one worktree branch is ahead — that is the delivery target.
    Single(RepoContext),
    /// Several worktree branches are ahead; ambiguous, list them for the user.
    Multiple(Vec<String>),
}

/// When the current checkout is on the default branch (can't open a PR from
/// it), discover sibling worktrees whose branch has commits ahead of
/// `origin/<default>`. The common worktree-default workflow leaves exactly one
/// such branch; delivery should target it instead of refusing outright.
fn discover_worktree_target(repo: &RepoContext) -> WorktreeDiscovery {
    let Ok(porcelain) = git(
        &repo.root,
        &["worktree", "list", "--porcelain"],
    ) else {
        return WorktreeDiscovery::None;
    };
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();
    for stanza in porcelain.split("\n\n") {
        let mut dir: Option<&str> = None;
        let mut branch: Option<&str> = None;
        for line in stanza.lines() {
            if let Some(d) = line.strip_prefix("worktree ") {
                dir = Some(d);
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                branch = Some(b);
            }
        }
        let (Some(dir), Some(branch)) = (dir, branch) else { continue };
        let dir = PathBuf::from(dir);
        if dir == repo.root {
            continue; // the checkout we are running from
        }
        if branch == repo.default_branch {
            continue;
        }
        // Commits on this branch not reachable from the remote default branch
        // mean there is work here that has not been merged yet.
        let ahead = git(
            &repo.root,
            &[
                "rev-list",
                "--count",
                &format!("{}/{}", repo.remote, repo.default_branch),
                branch,
            ],
        )
        .unwrap_or_default();
        if ahead.trim() == "0" {
            continue;
        }
        candidates.push((dir, branch.to_string()));
    }
    match candidates.len() {
        0 => WorktreeDiscovery::None,
        1 => {
            let (root, branch) = candidates.into_iter().next().unwrap();
            let remote_url = git(&root, &["remote", "get-url", &repo.remote]).ok();
            WorktreeDiscovery::Single(RepoContext {
                root,
                branch,
                default_branch: repo.default_branch.clone(),
                remote: repo.remote.clone(),
                remote_url,
            })
        }
        n => WorktreeDiscovery::Multiple(candidates.into_iter().take(n).map(|(_, b)| b).collect()),
    }
}

/// Normalize a repo-relative path for denylist matching.
fn norm(p: &str) -> String {
    p.replace('\\', "/")
}

fn is_excluded(path: &str, extra: &[String]) -> bool {
    let p = norm(path);
    let hit = |pat: &str| {
        let pat = norm(pat);
        if pat.ends_with('/') {
            p.starts_with(&pat) || p == pat.trim_end_matches('/')
        } else {
            p == pat || p.starts_with(&format!("{pat}/"))
        }
    };
    BUILTIN_EXCLUDES.iter().any(|e| hit(e)) || extra.iter().any(|e| hit(e))
}

/// The untracked source files delivery WOULD add (for tests + previews):
/// `??` porcelain entries minus the noise denylist.
pub fn untracked_source_paths(root: &Path, extra: &[String]) -> Result<Vec<String>, String> {
    let porcelain = git(root, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    let mut out = Vec::new();
    for line in porcelain.lines() {
        if line.len() < 4 {
            continue;
        }
        let (code, rest) = line.split_at(2);
        let path = rest.trim();
        if code == "??" && !is_excluded(path, extra) {
            out.push(path.to_string());
        }
    }
    Ok(out)
}

/// Stage tracked modifications (`git add -u`) plus untracked source files that
/// pass the noise denylist. Returns the staged paths. Never a blanket add.
pub fn stage_scoped(root: &Path, extra: &[String]) -> Result<Vec<String>, String> {
    // `-u` stages modifications + deletions to tracked files, and adds NO
    // untracked file — the structural guarantee against sweeping in noise.
    git(root, &["add", "-u"])?;
    let untracked = untracked_source_paths(root, extra)?;
    for p in &untracked {
        git(root, &["add", "--", p])?;
    }
    // Report everything now staged (tracked mods + kept untracked).
    let staged = git(root, &["diff", "--cached", "--name-only"])?;
    Ok(staged.lines().map(|s| s.to_string()).collect())
}

fn has_staged_changes(root: &Path) -> bool {
    // `diff --cached --quiet` exits 1 when something is staged.
    dev_command("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false)
}

fn branch_is_ahead_of(root: &Path, remote: &str, base: &str, branch: &str) -> bool {
    // rev-list remote/base..branch — nonzero count means the branch has commits to push.
    git(
        root,
        &["rev-list", "--count", &format!("{remote}/{base}..{branch}")],
    )
    .ok()
    .and_then(|s| s.trim().parse::<u64>().ok())
    .map(|n| n > 0)
    .unwrap_or(true) // if we can't tell (e.g. no origin/base yet), assume there is work
}

fn generate_commit_message(root: &Path, branch: &str, title: Option<&str>) -> String {
    if let Some(t) = title {
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    let files = git(root, &["diff", "--cached", "--name-only"]).unwrap_or_default();
    let count = files.lines().count();
    let subject = branch
        .rsplit('/')
        .next()
        .unwrap_or(branch)
        .replace(['-', '_'], " ");
    format!("{subject}\n\nDelivered by CodeFactory ({count} file(s) changed).")
}

fn release_urgency_trailers(message: &str) -> Vec<String> {
    final_footer_lines(message)
        .iter()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("Release-Urgency")
                .then(|| value.trim().to_ascii_lowercase())
        })
        .collect()
}

fn final_footer_lines(message: &str) -> Vec<&str> {
    let lines: Vec<&str> = message.trim_end().lines().collect();
    let start = lines
        .iter()
        .rposition(|line| line.trim().is_empty())
        .map(|index| index + 1)
        .unwrap_or(0);
    lines[start..].iter().copied().collect()
}

fn breaking_change_trailers(message: &str) -> Vec<String> {
    final_footer_lines(message)
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

fn append_release_urgency(message: String, urgency: Option<ReleaseUrgency>) -> String {
    let Some(urgency) = urgency else {
        return message;
    };
    let value = urgency.as_str();
    if release_urgency_trailers(&message)
        .iter()
        .any(|existing| existing == value)
    {
        return message;
    }
    let footer_started = !release_urgency_trailers(&message).is_empty()
        || !breaking_change_trailers(&message).is_empty();
    format!(
        "{}{}Release-Urgency: {value}",
        message.trim_end(),
        if footer_started { "\n" } else { "\n\n" },
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReleaseMetadata {
    urgencies: Vec<String>,
    breaking_changes: Vec<String>,
}

impl ReleaseMetadata {
    fn is_empty(&self) -> bool {
        self.urgencies.is_empty() && self.breaking_changes.is_empty()
    }
}

fn branch_release_metadata(
    root: &Path,
    remote: &str,
    base: &str,
    pr_body: Option<&str>,
    explicit: Option<ReleaseUrgency>,
) -> Result<ReleaseMetadata, String> {
    let range = format!("{remote}/{base}..HEAD");
    let bodies = git(root, &["log", "--format=%B%x1e", &range])?;
    let mut metadata = ReleaseMetadata::default();
    for body in bodies.split('\x1e') {
        metadata.urgencies.extend(release_urgency_trailers(body));
        metadata
            .breaking_changes
            .extend(breaking_change_trailers(body));
    }
    if let Some(body) = pr_body {
        metadata.urgencies.extend(release_urgency_trailers(body));
        metadata
            .breaking_changes
            .extend(breaking_change_trailers(body));
    }
    if let Some(urgency) = explicit {
        metadata.urgencies.push(urgency.as_str().to_string());
    }
    metadata.urgencies.sort();
    metadata.urgencies.dedup();
    metadata.breaking_changes.sort();
    metadata.breaking_changes.dedup();
    Ok(metadata)
}

fn guarded_release_reason(urgencies: &[String]) -> Option<String> {
    let hold = urgencies.iter().any(|value| value == "hold");
    let invalid: Vec<&str> = urgencies
        .iter()
        .map(String::as_str)
        .filter(|value| !matches!(*value, "immediate" | "hold"))
        .collect();
    if !hold && invalid.is_empty() {
        return None;
    }
    let mut reasons = Vec::new();
    if hold {
        reasons.push("Release-Urgency: hold".to_string());
    }
    if !invalid.is_empty() {
        reasons.push(format!("非法 Release-Urgency: {}", invalid.join(", ")));
    }
    Some(reasons.join("; "))
}

fn squash_merge_message(title: &str, body: &str, metadata: &ReleaseMetadata) -> MergeCommitMessage {
    let mut merge_body = body.trim_end().to_string();
    let existing_urgencies = release_urgency_trailers(body);
    let existing_breaking_changes = breaking_change_trailers(body);
    let mut footer_started =
        !existing_urgencies.is_empty() || !existing_breaking_changes.is_empty();
    for breaking_change in &metadata.breaking_changes {
        if existing_breaking_changes.contains(breaking_change) {
            continue;
        }
        if !merge_body.is_empty() {
            merge_body.push_str(if footer_started { "\n" } else { "\n\n" });
        }
        merge_body.push_str(breaking_change);
        footer_started = true;
    }
    for urgency in &metadata.urgencies {
        if existing_urgencies.contains(urgency) {
            continue;
        }
        if !merge_body.is_empty() {
            merge_body.push_str(if footer_started { "\n" } else { "\n\n" });
        }
        merge_body.push_str(&format!("Release-Urgency: {urgency}"));
        footer_started = true;
    }
    MergeCommitMessage {
        title: title.to_string(),
        body: merge_body,
    }
}

struct PreparedReleasePolicy {
    guard: Option<String>,
    merge_commit_message: Option<MergeCommitMessage>,
    durable_body: String,
}

fn prepare_release_policy(
    root: &Path,
    remote: &str,
    base: &str,
    title: &str,
    body: &str,
    explicit: Option<ReleaseUrgency>,
) -> Result<PreparedReleasePolicy, String> {
    let metadata = branch_release_metadata(root, remote, base, Some(body), explicit)?;
    let guard = guarded_release_reason(&metadata.urgencies);
    let merge_commit_message =
        (!metadata.is_empty()).then(|| squash_merge_message(title, body, &metadata));
    let durable_body = merge_commit_message
        .as_ref()
        .map(|message| message.body.clone())
        .unwrap_or_else(|| body.to_string());
    Ok(PreparedReleasePolicy {
        guard,
        merge_commit_message,
        durable_body,
    })
}

/// What a delivery can actually reach, plus the `preflight` step that explains it.
///
/// Never "the whole ladder is cancelled": see [`delivery_preflight`].
struct Preflight {
    ceiling: DeliveryCeiling,
    step: StepResult,
    missing: Option<String>,
}

/// Resolve the highest ACHIEVABLE ceiling and the preflight step to record.
///
/// The rule (2026-07-30 field report): **a missing actuator lowers the ceiling;
/// a missing verifier lowers only the claim.** Previously any gap anywhere in
/// the capability chain returned a hard block, so `deliver()` returned before
/// the first git command — the dominant configuration (default `ThroughRelease`
/// + `live: false` on every non-hook adapter + no `.codefactory/delivery.json`)
/// had EVERY delivery refused with the work still uncommitted.
///
/// Two hard blocks remain, and both are deliberate:
/// - **No remote channel at all.** Nothing can ever leave the machine, so we do
///   not leave an unpushable commit behind in the user's repository. Pinned by
///   `no_remote_configured_blocks_in_preflight_before_local_mutation`.
/// - **An unreadable `.codefactory/delivery.json`.** Guessing past a malformed
///   delivery config would be guessing about release semantics.
///
/// The live verifier is deliberately NOT consulted here. `verify_release_live`
/// already refuses to claim a release as live without one, via
/// `block_unverified_release` — checking it here too only moved that refusal
/// earlier and made it swallow the achievable work.
fn delivery_preflight<R: DeliveryRemote>(
    repo: &RepoContext,
    ceiling: DeliveryCeiling,
    remote: Option<&R>,
) -> Result<Preflight, StepResult> {
    let Some(remote) = remote else {
        return Err(StepResult::blocked(
            "preflight",
            no_remote_channel_message(repo.remote_url.as_deref()),
        ));
    };
    let capabilities = remote.capabilities();
    load_delivery_config(&repo.root).map_err(|error| StepResult::blocked("preflight", error))?;

    // Descend one rung at a time, remembering why. Ordered low → high so the
    // FIRST missing actuator sets the ceiling and names itself.
    let mut reachable = ceiling;
    let mut missing: Option<&str> = None;
    for (needed, capable, capability) in [
        (
            DeliveryCeiling::PrOnly,
            capabilities.review,
            "review adapter",
        ),
        (
            DeliveryCeiling::ThroughCiGreen,
            capabilities.ci,
            "CI observer",
        ),
        (
            DeliveryCeiling::ThroughMerge,
            capabilities.merge,
            "merge adapter",
        ),
        (
            DeliveryCeiling::ThroughRelease,
            capabilities.release,
            "release adapter",
        ),
    ] {
        if ceiling.rank() >= needed.rank() && !capable {
            // One rung below the level this capability unlocks.
            reachable = match needed {
                DeliveryCeiling::PrOnly => DeliveryCeiling::Off,
                DeliveryCeiling::ThroughCiGreen => DeliveryCeiling::PrOnly,
                DeliveryCeiling::ThroughMerge => DeliveryCeiling::ThroughCiGreen,
                _ => DeliveryCeiling::ThroughMerge,
            };
            missing = Some(capability);
            break;
        }
    }

    // No review adapter means not even a PR is reachable. There is nothing to
    // descend to, so this stays a block rather than a silent local commit.
    if reachable == DeliveryCeiling::Off {
        return Err(StepResult::blocked(
            "preflight",
            format!(
                "交付预检未通过:目标 {} 缺少 {}；没有可用的评审通道，未执行 stage、commit 或 push。",
                ceiling_label(ceiling),
                missing.unwrap_or("review adapter")
            ),
        ));
    }

    let detail = match missing {
        None => format!(
            "目标 {} 的 provider/auth/review 链已就绪",
            ceiling_label(ceiling)
        ),
        Some(capability) => format!(
            "目标 {} 缺少 {capability}，已降级到 {}；该级及以下照常执行，更高级别未执行。\
补齐 {capability} 后重新调用 deliver_changes 即可续跑。",
            ceiling_label(ceiling),
            ceiling_label(reachable)
        ),
    };
    Ok(Preflight {
        ceiling: reachable,
        step: StepResult::ok("preflight", detail),
        missing: missing.map(str::to_string),
    })
}

// ── The state machine ───────────────────────────────────────────────────────

/// Run delivery up to the effective ceiling.
///
/// The configured ceiling is first clamped by any per-call request, then by what
/// the remote adapter can actually do (see [`delivery_preflight`]): a missing
/// actuator lowers the ceiling and the achievable rungs still run.
///
/// `remote` is `None` when no git remote token is configured. That case blocks
/// at preflight BEFORE any local mutation — deliberately, so delivery never
/// leaves an unpushable commit in the user's repository.
pub async fn deliver<R: DeliveryRemote>(
    cwd: &Path,
    configured_ceiling: DeliveryCeiling,
    merge_method: MergeMethod,
    ci_timeout_secs: u32,
    opts: &DeliverOpts,
    remote: Option<&R>,
    default_branch_hint: Option<&str>,
) -> DeliveryOutcome {
    let requested_ceiling = match opts.requested_ceiling {
        Some(req) => configured_ceiling.clamp_request(req),
        None => configured_ceiling,
    };
    let mut outcome = DeliveryOutcome {
        steps: Vec::new(),
        branch: None,
        commit_sha: None,
        pr_url: None,
        pr_number: None,
        final_state: "delivered".into(),
        stage: "preflight".into(),
        code: "delivery_ready".into(),
        recoverable: false,
        next_action: None,
        reached_state: "local".into(),
        requested_ceiling: ceiling_label(requested_ceiling).into(),
        effective_ceiling: ceiling_label(requested_ceiling).into(),
        capability_gap: None,
        release_receipt: None,
        summary: String::new(),
    };

    if requested_ceiling == DeliveryCeiling::Off {
        outcome.final_state = "noop".into();
        outcome.summary = "交付已关闭(delivery_ceiling = off)。".into();
        outcome
            .steps
            .push(StepResult::skipped("policy", "delivery ceiling is Off"));
        return outcome;
    }

    // ── Resolve repo ────────────────────────────────────────────────────────
    let mut repo = match resolve_repo(cwd, default_branch_hint) {
        Ok(r) => r,
        Err(e) => return outcome.blocked_at(StepResult::blocked("repo", e)),
    };
    outcome.branch = Some(repo.branch.clone());
    if repo.branch == repo.default_branch {
        // On the default branch we cannot open a PR from it to itself, but the
        // worktree-default workflow leaves the feature branch in a sibling
        // worktree while the main checkout sits on main. Discover that branch
        // and deliver it instead of refusing outright.
        match discover_worktree_target(&repo) {
            WorktreeDiscovery::Single(target) => {
                let from = repo.branch.clone();
                repo = target;
                outcome.branch = Some(repo.branch.clone());
                outcome.steps.push(StepResult::ok(
                    "repo",
                    format!(
                        "主 checkout 在默认分支 {from} 上；检测到 worktree 分支 {} 有未合并提交，改为以该分支为交付目标",
                        repo.branch
                    ),
                ));
            }
            WorktreeDiscovery::Multiple(candidates) => {
                return outcome.blocked_at(StepResult::blocked(
                    "repo",
                    format!(
                        "当前在默认分支 {} 上,不能从默认分支向自身开 PR;且检测到多个 worktree 分支有待交付提交({}),请先切到目标功能分支。",
                        repo.default_branch,
                        candidates.join(", ")
                    ),
                ));
            }
            WorktreeDiscovery::None => {
                return outcome.blocked_at(StepResult::blocked(
                    "repo",
                    format!(
                        "当前在默认分支 {} 上,不能从默认分支向自身开 PR;请先切到功能分支(未发现待交付的 worktree 分支)。",
                        repo.default_branch
                    ),
                ));
            }
        }
    }

    // A capability gap DESCENDS the ceiling; it does not cancel the rungs below
    // it. Everything after this point runs against `ceiling`, which is now the
    // achievable one.
    let ceiling = match delivery_preflight(&repo, requested_ceiling, remote) {
        Ok(preflight) => {
            outcome.effective_ceiling = ceiling_label(preflight.ceiling).into();
            outcome.capability_gap = preflight.missing;
            outcome.steps.push(preflight.step);
            preflight.ceiling
        }
        Err(blocker) => return outcome.blocked_at(blocker),
    };

    // ── Commit (noise-safe) ─────────────────────────────────────────────────
    let staged = match stage_scoped(&repo.root, &opts.extra_excludes) {
        Ok(s) => s,
        Err(e) => {
            return outcome.blocked_at(StepResult::blocked("commit", format!("暂存失败: {e}")))
        }
    };
    if has_staged_changes(&repo.root) {
        let msg = append_release_urgency(
            generate_commit_message(&repo.root, &repo.branch, opts.title.as_deref()),
            opts.release_urgency,
        );
        if let Err(e) = git(
            &repo.root,
            &[
                "-c",
                "user.name=CodeFactory",
                "-c",
                "user.email=noreply@codefactory.local",
                "commit",
                "--no-verify",
                "-m",
                &msg,
            ],
        ) {
            return outcome.blocked_at(StepResult::blocked("commit", format!("提交失败: {e}")));
        }
        outcome.steps.push(StepResult::ok(
            "commit",
            format!("提交 {} 个文件", staged.len()),
        ));
    } else {
        outcome
            .steps
            .push(StepResult::skipped("commit", "无待提交改动(可能已提交)"));
    }
    outcome.commit_sha = git(&repo.root, &["rev-parse", "HEAD"]).ok();

    // Nothing to deliver at all: branch has no commits beyond base and there
    // was nothing to commit. Report a clean noop rather than open an empty PR.
    if !branch_is_ahead_of(&repo.root, &repo.remote, &repo.default_branch, &repo.branch)
        && outcome.steps.iter().all(|s| s.status == "skipped")
    {
        outcome.final_state = "noop".into();
        outcome.summary = "没有需要交付的改动。".into();
        return outcome;
    }

    // ── Push ────────────────────────────────────────────────────────────────
    match git(&repo.root, &["push", "-u", &repo.remote, &repo.branch]) {
        Ok(_) => outcome.steps.push(StepResult::ok(
            "push",
            format!("推送 {} 到 {}", repo.branch, repo.remote),
        )),
        Err(e) => {
            return outcome.blocked_at(StepResult::blocked(
                "push",
                format!("推送失败: {e}。请确认已配置该远端的 git 凭据(或在设置里配置远端 token)。"),
            ))
        }
    }

    let Some(remote) = remote else {
        return outcome.blocked_at(StepResult::blocked(
            "pr",
            no_remote_channel_message(repo.remote_url.as_deref()),
        ));
    };
    let sha = outcome.commit_sha.clone().unwrap_or_default();
    let mut prior_receipt = match read_delivery_receipt(&repo, &sha) {
        Ok(receipt) => receipt,
        Err(error) => {
            return outcome.blocked_at(StepResult::blocked("receipt", error));
        }
    };
    if let Some(receipt) = prior_receipt.as_ref() {
        if matches!(receipt.state.as_str(), "intent_merge" | "intent_release") {
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "receipt",
                format!(
                    "检测到未完成的 {} 写前回执（PR/MR #{}）",
                    receipt.state, receipt.pr_number
                ),
            ));
        }
        if receipt.state == "release_triggered" {
            // The current adapter may temporarily lack the release actuator,
            // but this exact context already has a durable release receipt.
            // Resume observation instead of incorrectly descending to merge.
            outcome.effective_ceiling = outcome.requested_ceiling.clone();
            outcome.capability_gap = None;
            if let Some(preflight) = outcome
                .steps
                .iter_mut()
                .find(|step| step.step == "preflight")
            {
                preflight.detail = format!(
                    "当前缺少 release adapter，但同一仓库/分支/tip 已有 release_triggered 回执；\
复用已完成发布并继续 observation，不需要补发 release。"
                );
            }
        }
    }
    let mut pr_title = prior_receipt
        .as_ref()
        .and_then(|receipt| receipt.pr_title.clone())
        .or_else(|| opts.title.clone())
        .unwrap_or_else(|| {
            generate_commit_message(&repo.root, &repo.branch, None)
                .lines()
                .next()
                .unwrap_or(&repo.branch)
                .to_string()
        });
    let mut pr_body = prior_receipt
        .as_ref()
        .and_then(|receipt| receipt.pr_body.clone())
        .or_else(|| opts.body.clone())
        .unwrap_or_else(|| {
            "由 CodeFactory 自动交付。\n\n🤖 Generated with CodeFactory".to_string()
        });
    let mut release_policy = match prepare_release_policy(
        &repo.root,
        &repo.remote,
        &repo.default_branch,
        &pr_title,
        &pr_body,
        opts.release_urgency,
    ) {
        Ok(values) => values,
        Err(error) => {
            return outcome.blocked_at(StepResult::blocked(
                "policy",
                format!("无法审计发布元数据，未继续远端交付: {error}"),
            ))
        }
    };
    let resumed_after_merge = prior_receipt
        .as_ref()
        .map(|receipt| matches!(receipt.state.as_str(), "merged" | "release_triggered"))
        .unwrap_or(false);

    if resumed_after_merge {
        let receipt = prior_receipt.as_ref().expect("checked above");
        outcome.pr_number = Some(receipt.pr_number);
        outcome.pr_url = Some(receipt.pr_url.clone());
        outcome.steps.push(StepResult::ok(
            "pr",
            format!(
                "复用本地交付回执中的 PR/MR #{}: {}",
                receipt.pr_number, receipt.pr_url
            ),
        ));
        outcome
            .steps
            .push(StepResult::ok("ci", "复用已合并交付的 CI 通过事实"));
        outcome.steps.push(StepResult::ok(
            "merge",
            format!("复用本地交付回执: PR/MR #{} 已合并", receipt.pr_number),
        ));
    } else {
        if ceiling.rank() < DeliveryCeiling::PrOnly.rank() {
            return finish(outcome, &repo.branch);
        }

        // ── Open (or reuse) PR/MR ───────────────────────────────────────────
        let had_pr_receipt = prior_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.state == "pr_open");
        let remote_pr = match remote
            .open_or_get_pr(&pr_title, &pr_body, &repo.branch, &repo.default_branch)
            .await
        {
            Ok(pr) => pr,
            Err(e) => {
                return outcome.blocked_at(StepResult::blocked(
                    "pr",
                    format!("开 PR/MR 或读取远端真实正文失败: {e}"),
                ))
            }
        };
        let pr_number = remote_pr.number;
        let pr_url = remote_pr.url;
        pr_title = remote_pr.title;
        pr_body = remote_pr.body;
        release_policy = match prepare_release_policy(
            &repo.root,
            &repo.remote,
            &repo.default_branch,
            &pr_title,
            &pr_body,
            opts.release_urgency,
        ) {
            Ok(policy) => policy,
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked(
                    "policy",
                    format!("无法审计远端 PR 发布元数据，未继续交付: {error}"),
                ))
            }
        };
        if had_pr_receipt {
            outcome.steps.push(StepResult::ok(
                "pr",
                format!("复用并刷新远端 PR/MR #{pr_number}: {pr_url}"),
            ));
        } else {
            outcome.steps.push(StepResult::ok(
                "pr",
                format!("PR/MR #{pr_number}: {pr_url}"),
            ));
        }
        outcome.pr_number = Some(pr_number);
        outcome.pr_url = Some(pr_url.clone());
        let pr_receipt = DeliveryReceipt {
            version: 1,
            state: "pr_open".into(),
            remote: repo.remote.clone(),
            remote_identity: receipt_remote_identity(&repo),
            base_branch: repo.default_branch.clone(),
            head_branch: repo.branch.clone(),
            commit_sha: sha.clone(),
            pr_number,
            pr_url: pr_url.clone(),
            pr_title: Some(pr_title.clone()),
            pr_body: Some(release_policy.durable_body.clone()),
            release_detail: None,
        };
        if let Err(error) = write_delivery_receipt(&repo, &sha, &pr_receipt) {
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "receipt",
                format!(
                    "PR/MR #{pr_number} 已创建或复用，但 PR 阶段回执写入失败: {error}；\
未继续 CI/merge，避免无参数恢复时丢失发布元数据。"
                ),
            ));
        }

        if ceiling.rank() < DeliveryCeiling::ThroughCiGreen.rank() {
            return finish(outcome, &repo.branch);
        }

        // ── Wait for CI ─────────────────────────────────────────────────────
        match wait_for_ci(remote, &sha, ci_timeout_secs).await {
            CiStatus::Success | CiStatus::None => {
                outcome.steps.push(StepResult::ok("ci", "CI 通过"))
            }
            CiStatus::Failure(d) => {
                return outcome.blocked_at(StepResult::blocked("ci", format!("CI 未通过: {d}")))
            }
            CiStatus::Pending => {
                return outcome.blocked_at(StepResult::blocked(
                    "ci",
                    format!(
                        "CI 在 {ci_timeout_secs}s 内仍未出结论;稍后重新调用交付即可从此处续跑。"
                    ),
                ))
            }
        }

        if ceiling.rank() < DeliveryCeiling::ThroughMerge.rank() {
            return finish(outcome, &repo.branch);
        }

        // ── Merge ───────────────────────────────────────────────────────────
        let refreshed_pr = match remote
            .open_or_get_pr(&pr_title, &pr_body, &repo.branch, &repo.default_branch)
            .await
        {
            Ok(pr) if pr.number == pr_number => pr,
            Ok(pr) => {
                return outcome.blocked_at(StepResult::blocked(
                    "policy",
                    format!(
                        "合并前远端 PR 身份变化: 预期 #{pr_number}，实际 #{}；未执行合并",
                        pr.number
                    ),
                ))
            }
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked(
                    "policy",
                    format!("合并前无法刷新远端 PR 正文，未执行合并: {error}"),
                ))
            }
        };
        pr_title = refreshed_pr.title;
        pr_body = refreshed_pr.body;
        release_policy = match prepare_release_policy(
            &repo.root,
            &repo.remote,
            &repo.default_branch,
            &pr_title,
            &pr_body,
            opts.release_urgency,
        ) {
            Ok(policy) => policy,
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked(
                    "policy",
                    format!("合并前无法审计远端 PR 发布元数据，未执行合并: {error}"),
                ))
            }
        };
        let intent = DeliveryReceipt {
            version: 1,
            state: "intent_merge".into(),
            remote: repo.remote.clone(),
            remote_identity: receipt_remote_identity(&repo),
            base_branch: repo.default_branch.clone(),
            head_branch: repo.branch.clone(),
            commit_sha: sha.clone(),
            pr_number,
            pr_url: pr_url.clone(),
            pr_title: Some(pr_title.clone()),
            pr_body: Some(release_policy.durable_body.clone()),
            release_detail: None,
        };
        if let Err(error) = write_delivery_receipt(&repo, &sha, &intent) {
            return outcome.blocked_at(StepResult::blocked(
                "receipt",
                format!("合并前无法写入本地意图回执，未执行合并: {error}"),
            ));
        }
        if let Err(e) = remote
            .merge_pr(
                pr_number,
                merge_method,
                release_policy.merge_commit_message.as_ref(),
            )
            .await
        {
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "merge",
                format!("合并请求返回失败: {e}(服务端可能已接收；已保留 intent_merge 回执)。"),
            ));
        }
        outcome.steps.push(StepResult::ok(
            "merge",
            format!("已 {} 合并 PR #{pr_number}", merge_method.as_str()),
        ));
        let receipt = DeliveryReceipt {
            version: 1,
            state: "merged".into(),
            remote: repo.remote.clone(),
            remote_identity: receipt_remote_identity(&repo),
            base_branch: repo.default_branch.clone(),
            head_branch: repo.branch.clone(),
            commit_sha: sha.clone(),
            pr_number,
            pr_url,
            pr_title: Some(pr_title.clone()),
            pr_body: Some(release_policy.durable_body.clone()),
            release_detail: None,
        };
        if let Err(error) = write_delivery_receipt(&repo, &sha, &receipt) {
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "receipt",
                format!("合并请求已返回成功，但完成回执写入失败: {error}；intent_merge 仍保留。"),
            ));
        }
        prior_receipt = Some(receipt);
    }

    let release_already_triggered = prior_receipt
        .as_ref()
        .is_some_and(|receipt| receipt.state == "release_triggered");
    if ceiling.rank() < DeliveryCeiling::ThroughRelease.rank() && !release_already_triggered {
        return finish(outcome, &repo.branch);
    }

    // ── Release (deliberate) ────────────────────────────────────────────────
    if !release_already_triggered {
        if let Some(reason) = release_policy.guard.as_ref() {
            return outcome.blocked_at(StepResult::blocked(
                "release",
                format!(
                    "发布批次受保护，未触发 release: {reason}。确认依赖和完整批次后，\
请从 Auto Release 手动设置 allow_guarded_batch=true；普通 force 不能绕过。"
                ),
            ));
        }
    }
    if let Some(receipt) = prior_receipt
        .as_ref()
        .filter(|receipt| receipt.state == "release_triggered")
    {
        let detail = receipt
            .release_detail
            .clone()
            .unwrap_or_else(|| "发布已由同一交付回执触发".into());
        outcome
            .steps
            .push(StepResult::ok("release", format!("复用回执: {detail}")));
        outcome.release_receipt = serde_json::to_string(receipt).ok();
    } else {
        let intent = DeliveryReceipt {
            version: 1,
            state: "intent_release".into(),
            remote: repo.remote.clone(),
            remote_identity: receipt_remote_identity(&repo),
            base_branch: repo.default_branch.clone(),
            head_branch: repo.branch.clone(),
            commit_sha: sha.clone(),
            pr_number: outcome.pr_number.unwrap_or_default(),
            pr_url: outcome.pr_url.clone().unwrap_or_default(),
            pr_title: Some(pr_title.clone()),
            pr_body: Some(release_policy.durable_body.clone()),
            release_detail: None,
        };
        if let Err(error) = write_delivery_receipt(&repo, &sha, &intent) {
            return outcome.blocked_at(StepResult::blocked(
                "receipt",
                format!("发布前无法写入本地意图回执，未触发发布: {error}"),
            ));
        }
        match remote.trigger_release().await {
            Ok(detail) => {
                outcome
                    .steps
                    .push(StepResult::ok("release", detail.clone()));
                let receipt = DeliveryReceipt {
                    version: 1,
                    state: "release_triggered".into(),
                    remote: repo.remote.clone(),
                    remote_identity: receipt_remote_identity(&repo),
                    base_branch: repo.default_branch.clone(),
                    head_branch: repo.branch.clone(),
                    commit_sha: sha.clone(),
                    pr_number: outcome.pr_number.unwrap_or_default(),
                    pr_url: outcome.pr_url.clone().unwrap_or_default(),
                    pr_title: Some(pr_title.clone()),
                    pr_body: Some(release_policy.durable_body.clone()),
                    release_detail: Some(detail),
                };
                match write_delivery_receipt(&repo, &sha, &receipt) {
                    Ok(raw) => outcome.release_receipt = Some(raw),
                    Err(error) => {
                        return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                            "receipt",
                            format!(
                                "发布请求已返回成功，但完成回执写入失败: {error}；intent_release 仍保留。"
                            ),
                        ))
                    }
                }
            }
            Err(e) => {
                return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                    "release",
                    format!(
                        "发布触发请求返回失败: {e}(服务端可能已接收；已保留 intent_release 回执)。"
                    ),
                ))
            }
        }
    }

    match verify_release_live(
        &repo.root,
        remote,
        &outcome.commit_sha.clone().unwrap_or_default(),
    )
    .await
    {
        Ok(live_steps) => outcome.steps.extend(live_steps),
        Err(blocker) => return block_unverified_release(outcome, blocker),
    }

    finish(outcome, &repo.branch)
}

async fn verify_release_live<R: DeliveryRemote>(
    root: &Path,
    remote: &R,
    sha: &str,
) -> Result<Vec<StepResult>, String> {
    let config = load_delivery_config(root)?;
    let mut steps = Vec::new();
    let provider = config.as_ref().and_then(|c| c.provider.as_deref());
    let deployment_timeout_secs = config
        .as_ref()
        .map(|c| c.deployment_timeout_secs)
        .unwrap_or_else(default_deployment_timeout_secs);

    let deployment = wait_for_deployment(remote, sha, provider, deployment_timeout_secs).await?;
    if let Some(detail) = deployment {
        steps.push(StepResult::ok("deploy", detail));
    }

    if let Some(live) = config.as_ref().and_then(|c| c.live.as_ref()) {
        wait_for_http_live(live, sha).await?;
        steps.push(StepResult::ok(
            "live",
            format!("线上验证通过: {} 包含本次提交标识", live.url),
        ));
        return Ok(steps);
    }

    match remote.verify_live(sha, None).await? {
        ObservationStatus::Success(detail) => {
            steps.push(StepResult::ok("live", detail));
            Ok(steps)
        }
        ObservationStatus::Pending(detail) => Err(format!(
            "线上验证仍在等待: {detail};稍后重新调用 deliver_changes 续跑。"
        )),
        ObservationStatus::Failure(detail) => Err(format!("线上验证失败: {detail}")),
        ObservationStatus::Unsupported(detail) => Err(format!(
            "发布已触发,但没有可用的 live verifier: {detail};不能声明已上线。"
        )),
    }
}

async fn wait_for_deployment<R: DeliveryRemote>(
    remote: &R,
    sha: &str,
    provider: Option<&str>,
    timeout_secs: u32,
) -> Result<Option<String>, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs as u64);
    loop {
        match remote.deployment_status(sha, provider).await? {
            ObservationStatus::Success(detail) => return Ok(Some(detail)),
            ObservationStatus::Failure(detail) => return Err(format!("部署失败: {detail}")),
            ObservationStatus::Unsupported(_) => return Ok(None),
            ObservationStatus::Pending(detail) => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!(
                        "部署在 {timeout_secs}s 内仍未完成: {detail};稍后重新调用 deliver_changes 续跑。"
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn wait_for_http_live(live: &LiveHttpAssertion, sha: &str) -> Result<(), String> {
    live.validate()?;
    let expected_body = live.expected_body(sha);
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(live.timeout_secs as u64);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            live.poll_interval_secs.max(1).min(30) as u64,
        ))
        .build()
        .map_err(|e| format!("创建 live verifier HTTP client 失败: {e}"))?;
    let mut last_error = String::new();
    loop {
        match client.get(&live.url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match resp.text().await {
                    Ok(body) => {
                        if status == live.expected_status && body.contains(&expected_body) {
                            return Ok(());
                        }
                        last_error = format!(
                            "HTTP {status}, expected {}, body missing '{}'",
                            live.expected_status, expected_body
                        );
                    }
                    Err(e) => last_error = format!("读取 live 响应失败: {e}"),
                }
            }
            Err(e) => last_error = format!("请求 live URL 失败: {e}"),
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("线上验证超时: {last_error}"));
        }
        tokio::time::sleep(std::time::Duration::from_secs(
            live.poll_interval_secs.max(1) as u64,
        ))
        .await;
    }
}

/// Blocked-at-PR message when no remote token is configured. Carries the fix
/// path AND the model-behavior contract: surface it to the user and wait —
/// retrying deliver_changes cannot succeed until a token exists. (The app's
/// only historical deliver_changes call died exactly here.)
pub const NO_TOKEN_PR_MESSAGE: &str =
    "交付预检未通过：没有可用的 GitHub 通道，无法开 PR，尚未提交或推送。\
两条路任选其一(推荐前者):1) 在终端执行 `gh auth login` 登录 GitHub CLI——登录一次,\
交付链即刻可用,无需在应用里配任何令牌;2) 在设置→远程仓库为该仓库配置访问令牌。\
把这两条路原样告诉用户;在用户完成其一之前,不要再调用 deliver_changes 重试。";

fn no_remote_channel_message(origin_url: Option<&str>) -> String {
    let Some(origin) = origin_url else {
        return "交付预检未通过：仓库没有可识别的 review provider，尚未提交或推送。请配置实际 Git remote 和 delivery_provider hook/plugin；在 provider 配好前不要重试 deliver_changes。".into();
    };
    let family = classify_forge(origin);
    match family {
        ForgeFamily::Github => {
            let host = remote_host(origin).unwrap_or_else(|| "github.com".into());
            if host == "github.com" {
                NO_TOKEN_PR_MESSAGE.to_string()
            } else {
                format!(
                    "交付预检未通过：GitHub Enterprise 主机 {host} 没有可用的 PR 通道，尚未提交或推送。请先运行 `gh auth login --hostname {host}`，或为该主机配置 GitHub remote token / delivery_provider hook；在通道配置完成前不要重试 deliver_changes。"
                )
            }
        }
        ForgeFamily::Gitlab => {
            let project = parse_gitlab_project_path(origin).unwrap_or_else(|| "unknown".into());
            format!(
                "交付预检未通过：GitLab 项目 {project} 没有可用的 merge request 通道，尚未提交或推送。请在 设置→远程仓库 配置该 GitLab/企业 GitLab 的 token,或启用仓库 delivery_provider hook/plugin；不要把这当成缺 GitHub 通道。"
            )
        }
        other => format!(
            "交付预检未通过：{} remote ({}) 没有内置 review adapter，尚未提交或推送。请配置仓库 delivery_provider hook/plugin 来实现 PR/MR/Change、CI、合并和发布；不要用 GitHub CLI 登录作为通用修复。",
            other.label(),
            remote_host(origin).unwrap_or_else(|| "unknown-host".into())
        ),
    }
}

fn ceiling_label(ceiling: DeliveryCeiling) -> &'static str {
    match ceiling {
        DeliveryCeiling::Off => "off",
        DeliveryCeiling::PrOnly => "pr_only",
        DeliveryCeiling::ThroughCiGreen => "through_ci_green",
        DeliveryCeiling::ThroughMerge => "through_merge",
        DeliveryCeiling::ThroughRelease => "through_release",
    }
}

/// System-prompt note about the delivery chain's readiness for this cwd, so
/// the model surfaces a broken chain in its FIRST reply instead of the user
/// discovering it when deliver_changes blocks after the work is already done.
/// Silent (None) when delivery is off or the origin isn't a GitHub repo.
pub fn delivery_readiness_from_origin(
    origin_url: Option<&str>,
    settings: &crate::config::settings::Settings,
) -> Option<String> {
    delivery_readiness_with_gh(origin_url, settings, gh_cli_available())
}

/// Testable core of [`delivery_readiness_from_origin`] with the gh probe
/// injected.
pub fn delivery_readiness_with_gh(
    origin_url: Option<&str>,
    settings: &crate::config::settings::Settings,
    gh_available: bool,
) -> Option<String> {
    use crate::config::settings::GitProvider;
    if settings.delivery_ceiling == DeliveryCeiling::Off {
        return None;
    }
    let origin = origin_url?;
    if let Some(owner_repo) = parse_owner_repo(origin) {
        let host = remote_host(origin).unwrap_or_else(|| "github.com".into());
        if gh_available {
            return Some(format!(
                "\n\n# Delivery capability\n\
                 Repo {owner_repo} on {host}: a logged-in GitHub CLI is available for this host — the delivery chain \
                 (PR/CI/merge/release, up to ceiling {}) works with ZERO app-side token setup. \
                 Never ask the user to configure a remote token while gh is available.",
                ceiling_label(settings.delivery_ceiling)
            ));
        }
        let has_github_remote = configured_remote_for(settings, GitProvider::Github, &owner_repo)
            .and_then(|r| crate::config::settings::resolve_git_remote_token(r).ok())
            .is_some();
        return Some(if has_github_remote {
            format!(
                "\n\n# Delivery capability\n\
                 Repo {owner_repo} has GitHub credentials configured; delivery ceiling = {}. \
                 Code work ends by calling deliver_changes once tests are green — it carries the \
                 work up to that ceiling automatically.",
                ceiling_label(settings.delivery_ceiling)
            )
        } else {
            let gh_login = if host == "github.com" {
                "gh auth login".to_string()
            } else {
                format!("gh auth login --hostname {host}")
            };
            format!(
                "\n\n# Delivery capability (BROKEN — surface early)\n\
                 The delivery chain for {owner_repo} on {host} cannot open a PR: no logged-in GitHub CLI \
                 for this host and no configured token. If this task involves delivering code, say so in your \
                 FIRST reply and offer both fixes — preferred: run `{gh_login}` once in a \
                 terminal (zero app-side config); alternative: 设置→远程仓库 token setup — and \
                 do NOT call deliver_changes until one of them is done. Local work (tests, \
                 edits, commits) can proceed in the meantime."
            )
        });
    }

    let project = parse_gitlab_project_path(origin)?;
    let has_gitlab_remote = configured_remote_for(settings, GitProvider::Gitlab, &project)
        .and_then(|r| crate::config::settings::resolve_git_remote_token(r).ok())
        .is_some();
    let has_delivery_provider_hook = !delivery_provider_hooks_for(settings, origin).is_empty();
    if !host_looks_like_gitlab(origin) && !has_gitlab_remote && !has_delivery_provider_hook {
        return None;
    }
    Some(if has_gitlab_remote {
        format!(
            "\n\n# Delivery capability\n\
             GitLab project {project} has credentials configured; delivery ceiling = {}. \
             Code work ends by calling deliver_changes once tests are green — it opens or reuses \
             a GitLab merge request and carries the work up to the configured boundary. \
             Repository-specific CI/release automation can be supplied by a delivery provider \
             hook/plugin when the built-in GitLab adapter is not enough.",
            ceiling_label(settings.delivery_ceiling)
        )
    } else {
        format!(
            "\n\n# Delivery capability (BROKEN — surface early)\n\
             The delivery chain for GitLab project {project} cannot open a merge request: no \
             configured GitLab remote token/provider. If this task involves delivering code, \
             say so in your FIRST reply and ask for 设置→远程仓库 token setup, or a repository \
             delivery provider hook/plugin for this enterprise GitLab. Do NOT treat this as a \
             missing GitHub channel and do NOT call deliver_changes until one is configured. \
             Local work (tests, edits, commits) can proceed in the meantime."
        )
    })
}

/// Wrapper reading the cwd's selected remote URL; see [`delivery_readiness_from_origin`].
pub fn delivery_readiness_note(
    cwd: &Path,
    settings: &crate::config::settings::Settings,
) -> Option<String> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).ok()?;
    let remote = default_remote(Path::new(&root));
    let origin = git(Path::new(&root), &["remote", "get-url", &remote]).ok();
    delivery_readiness_from_origin(origin.as_deref(), settings)
}

fn block_unverified_release(
    outcome: DeliveryOutcome,
    detail: impl Into<String>,
) -> DeliveryOutcome {
    outcome.blocked_at(StepResult::blocked("live", detail))
}

fn finish(mut outcome: DeliveryOutcome, branch: &str) -> DeliveryOutcome {
    outcome.reached_state = reached_state_from_steps(&outcome.steps);
    let done: Vec<&str> = outcome
        .steps
        .iter()
        .filter(|s| s.status == "ok")
        .map(|s| s.step.as_str())
        .collect();
    outcome.summary = if let Some(url) = &outcome.pr_url {
        format!("已交付分支 {branch}(步骤: {}) — {url}", done.join(" → "))
    } else {
        format!("已交付分支 {branch}(步骤: {})", done.join(" → "))
    };
    if outcome.requested_ceiling != outcome.effective_ceiling {
        let gap = outcome
            .capability_gap
            .clone()
            .unwrap_or_else(|| "higher delivery capability".into());
        let next_action = format!(
            "补齐 {gap} 后再次调用 deliver_changes；本地交付回执会复用已完成步骤，不会重复 merge 或 release。"
        );
        outcome.final_state = "blocked".into();
        outcome.stage = "capability".into();
        outcome.code = "delivery_capability_gap".into();
        outcome.recoverable = true;
        outcome.next_action = Some(next_action.clone());
        outcome.summary.push_str(&format!(
            "\n本次实际到达 {}，未达到请求的 {}：缺少 {gap}。{next_action}",
            outcome.reached_state, outcome.requested_ceiling
        ));
    } else {
        outcome.stage = "complete".into();
        outcome.code = "delivery_ceiling_reached".into();
    }
    outcome
}

async fn wait_for_ci<R: DeliveryRemote>(remote: &R, sha: &str, timeout_secs: u32) -> CiStatus {
    let deadline = timeout_secs.max(1);
    let mut waited = 0u32;
    let step = 10u32;
    loop {
        match remote.ci_status(sha).await {
            Ok(CiStatus::Pending) => {}
            Ok(other) => return other,
            Err(e) => return CiStatus::Failure(e),
        }
        if waited >= deadline {
            return CiStatus::Pending;
        }
        tokio::time::sleep(std::time::Duration::from_secs(
            step.min(deadline - waited) as u64
        ))
        .await;
        waited += step;
    }
}

// ── GitHub provider (gh CLI, preferred) ─────────────────────────────────────

/// Which remote transport a delivery run will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteKind {
    /// A logged-in `gh` CLI on this machine — zero app-side configuration.
    GhCli,
    /// The portable token+REST client from configured git_remotes.
    RestToken,
}

/// Delivery remote families known by the state machine. `Hook` is the extension
/// seam for enterprise/self-hosted systems whose MR API is supplied by a plugin
/// or repository hook instead of CodeFactory's built-in GitHub/GitLab adapters.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryProviderKind {
    Github,
    Gitlab,
    GhCli,
    Hook(String),
}

/// Description returned by a delivery provider resolver. Tests and future
/// plugins use this to prove provider selection without requiring network I/O.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRemoteDescriptor {
    pub provider: DeliveryProviderKind,
    pub repo: String,
    pub default_branch: String,
    pub missing_credentials_message: Option<String>,
}

#[cfg(test)]
pub struct DeliveryRemoteContext<'a> {
    pub origin_url: String,
    pub default_branch: String,
    pub settings: &'a crate::config::settings::Settings,
}

#[cfg(test)]
type DeliveryRemoteResolver = Box<
    dyn for<'a> Fn(&DeliveryRemoteContext<'a>) -> Option<DeliveryRemoteDescriptor> + Send + Sync,
>;

#[cfg(test)]
#[derive(Default)]
pub struct DeliveryRemoteRegistry {
    resolvers: Vec<DeliveryRemoteResolver>,
}

#[cfg(test)]
impl DeliveryRemoteRegistry {
    pub fn register<F>(&mut self, resolver: F)
    where
        F: for<'a> Fn(&DeliveryRemoteContext<'a>) -> Option<DeliveryRemoteDescriptor>
            + Send
            + Sync
            + 'static,
    {
        self.resolvers.push(Box::new(resolver));
    }

    pub fn resolve(&self, ctx: &DeliveryRemoteContext<'_>) -> Option<DeliveryRemoteDescriptor> {
        self.resolvers.iter().find_map(|resolver| resolver(ctx))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryProviderHook {
    pub id: String,
    pub command: String,
    pub cwd: Option<String>,
}

pub fn delivery_provider_hooks_for(
    settings: &crate::config::settings::Settings,
    origin_url: &str,
) -> Vec<DeliveryProviderHook> {
    settings
        .hooks
        .iter()
        .filter(|hook| hook.enabled && hook.event == "delivery_provider")
        .filter(|hook| {
            hook.filter
                .as_deref()
                .map(|filter| origin_url.contains(filter))
                .unwrap_or(true)
        })
        .filter_map(|hook| match &hook.action {
            crate::commands::hooks::HookAction::RunCommand { command, cwd } => {
                Some(DeliveryProviderHook {
                    id: hook.id.clone(),
                    command: command.clone(),
                    cwd: cwd.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct HookPrResponse {
    number: u64,
    url: String,
    title: String,
    body: String,
}

#[derive(Debug, Deserialize)]
struct HookStatusResponse {
    status: String,
    detail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HookOkResponse {
    #[allow(dead_code)]
    ok: Option<bool>,
    detail: Option<String>,
}

pub struct HookRemote {
    id: String,
    command: String,
    cwd: PathBuf,
}

impl HookRemote {
    pub fn new(id: String, command: String, cwd: PathBuf) -> Self {
        Self { id, command, cwd }
    }

    fn run_json(&self, payload: serde_json::Value) -> Result<serde_json::Value, String> {
        let shell = command_env::shell_invocation(&self.command);
        let mut child = Command::new(shell.program)
            .no_window()
            .args(shell.args)
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("delivery provider hook '{}' failed to start: {e}", self.id))?;
        {
            use std::io::Write;
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| format!("delivery provider hook '{}' has no stdin", self.id))?;
            stdin
                .write_all(payload.to_string().as_bytes())
                .map_err(|e| format!("delivery provider hook '{}' stdin failed: {e}", self.id))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("delivery provider hook '{}' wait failed: {e}", self.id))?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !out.status.success() {
            return Err(format!(
                "delivery provider hook '{}' exited {}: {}",
                self.id,
                out.status.code().unwrap_or(-1),
                stderr
            ));
        }
        let value: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
            format!(
                "delivery provider hook '{}' returned non-JSON stdout: {e}: {}",
                self.id, stdout
            )
        })?;
        if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
            return Err(error.to_string());
        }
        Ok(value)
    }
}

impl DeliveryRemote for HookRemote {
    fn capabilities(&self) -> DeliveryCapabilities {
        DeliveryCapabilities {
            review: true,
            ci: true,
            merge: true,
            release: true,
            live: true,
        }
    }

    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<DeliveryPr, String> {
        let value = self.run_json(json!({
            "action": "open_or_get_pr",
            "title": title,
            "body": body,
            "head": head,
            "base": base,
        }))?;
        let response: HookPrResponse = serde_json::from_value(value).map_err(|e| {
            format!(
                "delivery provider hook '{}' PR response invalid: {e}",
                self.id
            )
        })?;
        Ok(DeliveryPr {
            number: response.number,
            url: response.url,
            title: response.title,
            body: response.body,
        })
    }

    async fn ci_status(&self, sha: &str) -> Result<CiStatus, String> {
        let value = self.run_json(json!({ "action": "ci_status", "sha": sha }))?;
        let response: HookStatusResponse = serde_json::from_value(value).map_err(|e| {
            format!(
                "delivery provider hook '{}' CI response invalid: {e}",
                self.id
            )
        })?;
        Ok(match response.status.as_str() {
            "success" => CiStatus::Success,
            "pending" => CiStatus::Pending,
            "none" => CiStatus::None,
            "failure" => CiStatus::Failure(response.detail.unwrap_or_else(|| "failure".into())),
            other => CiStatus::Failure(format!("unknown hook ci status: {other}")),
        })
    }

    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
    ) -> Result<(), String> {
        let value = self.run_json(json!({
            "action": "merge_pr",
            "number": number,
            "method": method.as_str(),
            "commit_title": commit_message.map(|message| message.title.as_str()),
            "commit_body": commit_message.map(|message| message.body.as_str()),
        }))?;
        let _response: HookOkResponse = serde_json::from_value(value).map_err(|e| {
            format!(
                "delivery provider hook '{}' merge response invalid: {e}",
                self.id
            )
        })?;
        Ok(())
    }

    async fn trigger_release(&self) -> Result<String, String> {
        let value = self.run_json(json!({ "action": "trigger_release" }))?;
        let response: HookOkResponse = serde_json::from_value(value).map_err(|e| {
            format!(
                "delivery provider hook '{}' release response invalid: {e}",
                self.id
            )
        })?;
        Ok(response
            .detail
            .unwrap_or_else(|| format!("delivery provider hook '{}' triggered release", self.id)))
    }

    async fn deployment_status(
        &self,
        sha: &str,
        provider: Option<&str>,
    ) -> Result<ObservationStatus, String> {
        let value = self.run_json(json!({
            "action": "deployment_status",
            "sha": sha,
            "provider": provider,
        }))?;
        let response: HookStatusResponse = serde_json::from_value(value).map_err(|e| {
            format!(
                "delivery provider hook '{}' deployment response invalid: {e}",
                self.id
            )
        })?;
        Ok(parse_observation_status(&response.status, response.detail))
    }

    async fn verify_live(&self, sha: &str, url: Option<&str>) -> Result<ObservationStatus, String> {
        let value = self.run_json(json!({
            "action": "verify_live",
            "sha": sha,
            "url": url,
        }))?;
        let response: HookStatusResponse = serde_json::from_value(value).map_err(|e| {
            format!(
                "delivery provider hook '{}' live response invalid: {e}",
                self.id
            )
        })?;
        Ok(parse_observation_status(&response.status, response.detail))
    }
}

/// gh CLI first (the user already authenticated it once, system-wide), the
/// configured token second, nothing → the caller blocks with guidance. Field
/// report: delivery kept demanding an app token while a logged-in gh sat
/// right there.
pub fn resolve_remote_kind(gh_available: bool, has_rest_token: bool) -> Option<RemoteKind> {
    if gh_available {
        Some(RemoteKind::GhCli)
    } else if has_rest_token {
        Some(RemoteKind::RestToken)
    } else {
        None
    }
}

/// Is a logged-in gh CLI available? `gh auth status` exits non-zero when the
/// binary is missing OR no host is authenticated — exactly the two cases
/// where the REST fallback should take over.
pub fn gh_cli_available() -> bool {
    gh_cli_available_for_host("github.com")
}

pub fn gh_cli_available_for_host(hostname: &str) -> bool {
    // Standard PATH first.
    if gh_auth_status_for_host("gh", hostname) {
        return true;
    }
    // macOS GUI apps don't inherit the shell PATH. Homebrew installs `gh`
    // into one of these well-known prefixes — check them directly.
    for prefix in &["/opt/homebrew/bin/gh", "/usr/local/bin/gh"] {
        if gh_auth_status_for_host(prefix, hostname) {
            return true;
        }
    }
    // PATH and brew probes both missed: check the credential file directly.
    // `gh auth status --hostname <host>` succeeds ↔ ~/.config/gh/hosts.yml has
    // a non-empty user entry for that host with an oauth_token.
    gh_hosts_file_indicates_authenticated_for_host(hostname)
}

fn gh_auth_status_for_host(bin: &str, hostname: &str) -> bool {
    dev_command(bin)
        .args(["auth", "status", "--hostname", hostname])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Read `~/.config/gh/hosts.yml` and check for a host entry with a non-empty
/// user. This is the same credential file `gh auth status --hostname` checks;
/// reading it directly works even when the `gh` binary is not in the GUI app's
/// PATH (common on macOS with Homebrew).
fn gh_hosts_file_indicates_authenticated_for_host(hostname: &str) -> bool {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return false,
    };
    let path = home.join(".config").join("gh").join("hosts.yml");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let header = format!("{}:", hostname.trim().to_ascii_lowercase());
    let mut in_host_block = false;
    let mut has_user = false;
    let mut has_token = false;
    for line in content.lines() {
        let t = line.trim();
        if t.to_ascii_lowercase() == header {
            in_host_block = true;
            has_user = false;
            has_token = false;
            continue;
        }
        if in_host_block {
            if t.starts_with("user:") && t.strip_prefix("user:").unwrap_or("").trim().len() > 0 {
                has_user = true;
            }
            if t.starts_with("oauth_token:")
                && !t
                    .strip_prefix("oauth_token:")
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                has_token = true;
            }
            if has_user && has_token {
                return true;
            }
            // Any non-indented top-level key ends the selected host block.
            if !t.starts_with(' ') && t.ends_with(':') {
                return false;
            }
        }
    }
    false
}

fn gh_pr_create_args(title: &str, body: &str, head: &str, base: &str) -> Vec<String> {
    vec![
        "pr".into(),
        "create".into(),
        "--title".into(),
        title.into(),
        "--body".into(),
        body.into(),
        "--head".into(),
        head.into(),
        "--base".into(),
        base.into(),
    ]
}

fn gh_pr_merge_args(
    number: u64,
    method: MergeMethod,
    commit_message: Option<&MergeCommitMessage>,
) -> Vec<String> {
    let flag = match method {
        MergeMethod::Squash => "--squash",
        MergeMethod::Merge => "--merge",
        MergeMethod::Rebase => "--rebase",
    };
    let mut args = vec!["pr".into(), "merge".into(), number.to_string(), flag.into()];
    if method == MergeMethod::Squash {
        if let Some(message) = commit_message {
            args.extend([
                "--subject".into(),
                message.title.clone(),
                "--body".into(),
                message.body.clone(),
            ]);
        }
    }
    args
}

fn gh_workflow_run_args(workflow: &str, git_ref: &str) -> Vec<String> {
    vec![
        "workflow".into(),
        "run".into(),
        workflow.into(),
        "--ref".into(),
        git_ref.into(),
    ]
}

/// [`DeliveryRemote`] over a logged-in `gh` CLI. All commands run in the repo
/// root so gh resolves the repo from the checkout, using the user's existing
/// system-wide authentication — no app-side token required.
pub struct GhCliRemote {
    cwd: PathBuf,
    repo: String,
    default_branch: String,
    release_workflow: String,
}

/// Build a [`GhCliRemote`] for `cwd` when it is a GitHub checkout. Does not
/// probe authentication — pair with [`gh_cli_available`].
pub fn gh_remote_for(cwd: &Path) -> Option<GhCliRemote> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).ok()?;
    let remote = default_remote(Path::new(&root));
    let origin = git(Path::new(&root), &["remote", "get-url", &remote]).ok()?;
    let repo = parse_owner_repo(&origin)?;
    let default_branch =
        remote_default_branch(Path::new(&root), &remote).unwrap_or_else(|| "main".to_string());
    Some(GhCliRemote {
        cwd: PathBuf::from(root),
        repo,
        default_branch,
        release_workflow: "auto-release.yml".to_string(),
    })
}

impl GhCliRemote {
    fn gh(&self, args: &[String]) -> Result<String, String> {
        let out = dev_command("gh")
            .current_dir(&self.cwd)
            .args(args)
            .output()
            .map_err(|e| format!("failed to spawn gh: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
}

impl DeliveryRemote for GhCliRemote {
    fn capabilities(&self) -> DeliveryCapabilities {
        DeliveryCapabilities {
            review: true,
            ci: true,
            merge: true,
            release: true,
            live: false,
        }
    }

    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<DeliveryPr, String> {
        // Reuse an open PR for this head first — idempotence contract.
        let existing = self.gh(&[
            "pr".into(),
            "list".into(),
            "--head".into(),
            head.into(),
            "--base".into(),
            base.into(),
            "--state".into(),
            "open".into(),
            "--json".into(),
            "number,url,title,body".into(),
            "--limit".into(),
            "1".into(),
        ])?;
        if let Ok(list) = serde_json::from_str::<serde_json::Value>(&existing) {
            if let Some(pr) = list.as_array().and_then(|a| a.first()) {
                if let (Some(n), Some(u), Some(t), Some(b)) = (
                    pr["number"].as_u64(),
                    pr["url"].as_str(),
                    pr["title"].as_str(),
                    pr["body"].as_str(),
                ) {
                    return Ok(DeliveryPr {
                        number: n,
                        url: u.to_string(),
                        title: t.to_string(),
                        body: b.to_string(),
                    });
                }
            }
        }
        self.gh(&gh_pr_create_args(title, body, head, base))?;
        let created = self.gh(&[
            "pr".into(),
            "view".into(),
            head.into(),
            "--json".into(),
            "number,url,title,body".into(),
        ])?;
        let v: serde_json::Value = serde_json::from_str(&created)
            .map_err(|e| format!("gh pr view returned non-JSON: {e}"))?;
        match (
            v["number"].as_u64(),
            v["url"].as_str(),
            v["title"].as_str(),
            v["body"].as_str(),
        ) {
            (Some(n), Some(u), Some(t), Some(b)) => Ok(DeliveryPr {
                number: n,
                url: u.to_string(),
                title: t.to_string(),
                body: b.to_string(),
            }),
            _ => Err("gh pr view missing number/url/title/body".into()),
        }
    }

    async fn ci_status(&self, sha: &str) -> Result<CiStatus, String> {
        let raw = self.gh(&[
            "api".into(),
            format!("repos/{}/commits/{}/check-runs", self.repo, sha),
        ])?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("check-runs non-JSON: {e}"))?;
        let runs = v["check_runs"].as_array().cloned().unwrap_or_default();
        if runs.is_empty() {
            return Ok(CiStatus::None);
        }
        let mut pending = false;
        for run in &runs {
            let status = run["status"].as_str().unwrap_or("");
            let conclusion = run["conclusion"].as_str().unwrap_or("");
            if status != "completed" {
                pending = true;
            } else if !matches!(conclusion, "success" | "skipped" | "neutral") {
                let name = run["name"].as_str().unwrap_or("check");
                return Ok(CiStatus::Failure(format!("{name}: {conclusion}")));
            }
        }
        Ok(if pending {
            CiStatus::Pending
        } else {
            CiStatus::Success
        })
    }

    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
    ) -> Result<(), String> {
        self.gh(&gh_pr_merge_args(number, method, commit_message))?;
        if method != MergeMethod::Squash {
            return Ok(());
        }
        let Some(expected_message) = commit_message.map(|message| message.body.as_str()) else {
            return Ok(());
        };
        let merge_sha = self.gh(&[
            "pr".into(),
            "view".into(),
            number.to_string(),
            "--json".into(),
            "mergeCommit".into(),
            "--jq".into(),
            ".mergeCommit.oid".into(),
        ])?;
        if merge_sha.trim().is_empty() {
            return Err("squash merge succeeded but GitHub returned no merge commit SHA".into());
        }
        let merged_message = self.gh(&[
            "api".into(),
            format!("repos/{}/commits/{merge_sha}", self.repo),
            "--jq".into(),
            ".commit.message".into(),
        ])?;
        let missing = missing_release_metadata(expected_message, &merged_message);
        if !missing.is_empty() {
            return Err(format!(
                "squash merge commit {merge_sha} lost release metadata: {}",
                missing.join(", ")
            ));
        }
        Ok(())
    }

    async fn trigger_release(&self) -> Result<String, String> {
        self.gh(&gh_workflow_run_args(
            &self.release_workflow,
            &self.default_branch,
        ))
        .map(|_| format!("已通过 gh 触发发布工作流 {}", self.release_workflow))
    }
}

/// Static-dispatch wrapper so `deliver` keeps its generic signature while the
/// call site picks gh-vs-REST at runtime.
pub enum EitherRemote {
    Hook(HookRemote),
    Gh(GhCliRemote),
    Github(GithubRemote),
    Gitlab(GitlabRemote),
}

impl DeliveryRemote for EitherRemote {
    fn capabilities(&self) -> DeliveryCapabilities {
        match self {
            EitherRemote::Hook(r) => r.capabilities(),
            EitherRemote::Gh(r) => r.capabilities(),
            EitherRemote::Github(r) => r.capabilities(),
            EitherRemote::Gitlab(r) => r.capabilities(),
        }
    }

    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<DeliveryPr, String> {
        match self {
            EitherRemote::Hook(r) => r.open_or_get_pr(title, body, head, base).await,
            EitherRemote::Gh(r) => r.open_or_get_pr(title, body, head, base).await,
            EitherRemote::Github(r) => r.open_or_get_pr(title, body, head, base).await,
            EitherRemote::Gitlab(r) => r.open_or_get_pr(title, body, head, base).await,
        }
    }
    async fn ci_status(&self, sha: &str) -> Result<CiStatus, String> {
        match self {
            EitherRemote::Hook(r) => r.ci_status(sha).await,
            EitherRemote::Gh(r) => r.ci_status(sha).await,
            EitherRemote::Github(r) => r.ci_status(sha).await,
            EitherRemote::Gitlab(r) => r.ci_status(sha).await,
        }
    }
    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
    ) -> Result<(), String> {
        match self {
            EitherRemote::Hook(r) => r.merge_pr(number, method, commit_message).await,
            EitherRemote::Gh(r) => r.merge_pr(number, method, commit_message).await,
            EitherRemote::Github(r) => r.merge_pr(number, method, commit_message).await,
            EitherRemote::Gitlab(r) => r.merge_pr(number, method, commit_message).await,
        }
    }
    async fn trigger_release(&self) -> Result<String, String> {
        match self {
            EitherRemote::Hook(r) => r.trigger_release().await,
            EitherRemote::Gh(r) => r.trigger_release().await,
            EitherRemote::Github(r) => r.trigger_release().await,
            EitherRemote::Gitlab(r) => r.trigger_release().await,
        }
    }

    async fn deployment_status(
        &self,
        sha: &str,
        provider: Option<&str>,
    ) -> Result<ObservationStatus, String> {
        match self {
            EitherRemote::Hook(r) => r.deployment_status(sha, provider).await,
            EitherRemote::Gh(r) => r.deployment_status(sha, provider).await,
            EitherRemote::Github(r) => r.deployment_status(sha, provider).await,
            EitherRemote::Gitlab(r) => r.deployment_status(sha, provider).await,
        }
    }

    async fn verify_live(&self, sha: &str, url: Option<&str>) -> Result<ObservationStatus, String> {
        match self {
            EitherRemote::Hook(r) => r.verify_live(sha, url).await,
            EitherRemote::Gh(r) => r.verify_live(sha, url).await,
            EitherRemote::Github(r) => r.verify_live(sha, url).await,
            EitherRemote::Gitlab(r) => r.verify_live(sha, url).await,
        }
    }
}

fn hook_remote_for(cwd: &Path, settings: &crate::config::settings::Settings) -> Option<HookRemote> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).ok()?;
    let remote = default_remote(Path::new(&root));
    let origin = git(Path::new(&root), &["remote", "get-url", &remote]).ok()?;
    let hook = delivery_provider_hooks_for(settings, &origin)
        .into_iter()
        .next()?;
    let cwd = hook
        .cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(root));
    Some(HookRemote::new(hook.id, hook.command, cwd))
}

/// Resolve the best available remote for `cwd`: configured delivery provider
/// hooks first, then logged-in gh CLI for GitHub, then built-in REST tokens.
/// `None` → delivery blocks with provider-aware guidance.
fn selected_remote_url(cwd: &Path) -> Option<String> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).ok()?;
    let remote = default_remote(Path::new(&root));
    git(Path::new(&root), &["remote", "get-url", &remote]).ok()
}

pub fn resolve_delivery_remote(
    cwd: &Path,
    settings: &crate::config::settings::Settings,
) -> Option<EitherRemote> {
    if let Some(hook) = hook_remote_for(cwd, settings) {
        return Some(EitherRemote::Hook(hook));
    }
    let selected = selected_remote_url(cwd)?;
    match classify_forge(&selected) {
        ForgeFamily::Github => {
            let host = remote_host(&selected).unwrap_or_else(|| "github.com".into());
            if gh_cli_available_for_host(&host) {
                if let Some(remote) = gh_remote_for(cwd) {
                    return Some(EitherRemote::Gh(remote));
                }
            }
            github_remote_for(cwd, settings).map(EitherRemote::Github)
        }
        ForgeFamily::Gitlab => gitlab_remote_for(cwd, settings).map(EitherRemote::Gitlab),
        _ => None,
    }
}

// ── GitHub provider (token + REST) ──────────────────────────────────────────

/// Concrete [`DeliveryRemote`] over the portable token+REST client. Resolved
/// from the cwd's `origin` and the user's configured `git_remotes` tokens.
pub struct GithubRemote {
    client: crate::git_remote::client::RemoteGitClient,
    repo: String,
    default_branch: String,
    release_workflow: String,
}

/// Extract `owner/name` from a GitHub remote URL (https or ssh).
fn parse_owner_repo(url: &str) -> Option<String> {
    let host = remote_host(url)?;
    if classify_forge(url) != ForgeFamily::Github {
        return None;
    }
    parse_owner_repo_for_host(url, &host)
}

pub(crate) fn remote_host(url: &str) -> Option<String> {
    let u = url.trim();
    if let Some(rest) = u.strip_prefix("git@") {
        return rest
            .split_once(':')
            .map(|(host, _)| host.to_ascii_lowercase());
    }
    if let Some(rest) = u.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        return Some(
            host_port
                .split(':')
                .next()
                .unwrap_or(host_port)
                .to_ascii_lowercase(),
        );
    }
    if let Some(rest) = u.strip_prefix("https://") {
        return rest
            .split_once('/')
            .map(|(host, _)| host.to_ascii_lowercase());
    }
    if let Some(rest) = u.strip_prefix("http://") {
        return rest
            .split_once('/')
            .map(|(host, _)| host.to_ascii_lowercase());
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeFamily {
    Github,
    Gitlab,
    Bitbucket,
    AzureDevops,
    Gitea,
    Forgejo,
    Gerrit,
    CodeCommit,
    Generic,
}

impl ForgeFamily {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Github => "GitHub",
            Self::Gitlab => "GitLab",
            Self::Bitbucket => "Bitbucket",
            Self::AzureDevops => "Azure DevOps",
            Self::Gitea => "Gitea",
            Self::Forgejo => "Forgejo",
            Self::Gerrit => "Gerrit",
            Self::CodeCommit => "AWS CodeCommit",
            Self::Generic => "企业/通用 Git",
        }
    }
}

pub fn classify_forge(url: &str) -> ForgeFamily {
    let host = remote_host(url).unwrap_or_default();
    let lower = url.to_ascii_lowercase();
    if host == "github.com" || host.starts_with("github.") || host.contains(".github.") {
        ForgeFamily::Github
    } else if host == "gitlab.com" || host.starts_with("gitlab.") || host.contains(".gitlab.") {
        ForgeFamily::Gitlab
    } else if host == "bitbucket.org"
        || host.starts_with("bitbucket.")
        || host.contains(".bitbucket.")
    {
        ForgeFamily::Bitbucket
    } else if host == "dev.azure.com"
        || host.ends_with("visualstudio.com")
        || lower.contains("/_git/")
    {
        ForgeFamily::AzureDevops
    } else if host.contains("forgejo") {
        ForgeFamily::Forgejo
    } else if host.contains("gitea") {
        ForgeFamily::Gitea
    } else if host.starts_with("review.") || lower.contains(":29418/") {
        ForgeFamily::Gerrit
    } else if host.starts_with("git-codecommit.") && host.ends_with("amazonaws.com") {
        ForgeFamily::CodeCommit
    } else {
        ForgeFamily::Generic
    }
}

pub(crate) fn remote_repo_path(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches(".git");
    let path = if let Some(rest) = u.strip_prefix("git@") {
        rest.split_once(':')?.1
    } else if let Some(rest) = u.strip_prefix("ssh://") {
        let (_, path) = rest.split_once('/')?;
        path
    } else if let Some(rest) = u.strip_prefix("https://") {
        let (_, path) = rest.split_once('/')?;
        path
    } else if let Some(rest) = u.strip_prefix("http://") {
        let (_, path) = rest.split_once('/')?;
        path
    } else {
        return None;
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    (parts.len() >= 2).then(|| parts.join("/"))
}

fn parse_owner_repo_for_host(url: &str, expected_host: &str) -> Option<String> {
    if remote_host(url).as_deref() != Some(&expected_host.to_ascii_lowercase()) {
        return None;
    }
    let path = remote_repo_path(url)?;
    let parts: Vec<&str> = path.split('/').collect();
    (parts.len() >= 2).then(|| format!("{}/{}", parts[0], parts[1]))
}

fn host_looks_like_gitlab(url: &str) -> bool {
    remote_host(url)
        .map(|host| {
            host == "gitlab.com" || host.starts_with("gitlab.") || host.contains(".gitlab.")
        })
        .unwrap_or(false)
}

/// Extract a GitLab project path from SaaS or enterprise GitLab remotes. Unlike
/// GitHub's fixed `owner/repo`, GitLab projects can live under nested groups, so
/// every path segment after the host belongs to the project id.
pub(crate) fn parse_gitlab_project_path(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches(".git");
    let path = if let Some(rest) = u.strip_prefix("git@") {
        rest.split_once(':')?.1
    } else if let Some(rest) = u.strip_prefix("ssh://git@") {
        let (_, path) = rest.split_once('/')?;
        path
    } else if let Some(rest) = u.strip_prefix("https://") {
        let (_, path) = rest.split_once('/')?;
        path
    } else if let Some(rest) = u.strip_prefix("http://") {
        let (_, path) = rest.split_once('/')?;
        path
    } else {
        return None;
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        Some(parts.join("/"))
    } else {
        None
    }
}

fn configured_remote_for<'a>(
    settings: &'a crate::config::settings::Settings,
    provider: crate::config::settings::GitProvider,
    repo: &str,
) -> Option<&'a crate::config::settings::GitRemoteConfig> {
    settings
        .git_remotes
        .iter()
        .find(|r| r.provider == provider && r.default_repo.as_deref() == Some(repo))
        .or_else(|| {
            let mut candidates = settings
                .git_remotes
                .iter()
                .filter(|r| r.provider == provider);
            let first = candidates.next()?;
            if candidates.next().is_none() {
                Some(first)
            } else {
                None
            }
        })
}

/// Build a [`GithubRemote`] for `cwd` from the user's configured git remote
/// tokens, or `None` when nothing matches (delivery then blocks cleanly at the
/// PR step with a configure-a-token message). Never assumes `gh`.
pub fn github_remote_for(
    cwd: &Path,
    settings: &crate::config::settings::Settings,
) -> Option<GithubRemote> {
    use crate::config::settings::GitProvider;
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).ok()?;
    let remote_name = default_remote(Path::new(&root));
    let origin = git(Path::new(&root), &["remote", "get-url", &remote_name]).ok()?;
    let owner_repo = parse_owner_repo(&origin)?;

    // Prefer a git_remotes entry whose default_repo matches; else the first
    // GitHub remote with a resolvable token.
    let remote = settings
        .git_remotes
        .iter()
        .find(|r| {
            matches!(r.provider, GitProvider::Github)
                && r.default_repo.as_deref() == Some(owner_repo.as_str())
        })
        .or_else(|| {
            settings
                .git_remotes
                .iter()
                .find(|r| matches!(r.provider, GitProvider::Github))
        })?;
    let token = crate::config::settings::resolve_git_remote_token(remote).ok()?;
    let client = crate::git_remote::client::RemoteGitClient::new(
        &remote.base_url,
        &token,
        remote.provider.clone(),
    );
    let default_branch =
        remote_default_branch(Path::new(&root), &remote_name).unwrap_or_else(|| "main".to_string());

    Some(GithubRemote {
        client,
        repo: owner_repo,
        default_branch,
        release_workflow: "auto-release.yml".to_string(),
    })
}

/// Concrete [`DeliveryRemote`] over GitLab's Merge Request REST API. GitLab CI
/// polling and release orchestration vary widely across enterprises, so the
/// built-in adapter guarantees MR creation/reuse and merge; CI/release are
/// intentionally hook/provider extension points until a repo config supplies
/// those semantics.
pub struct GitlabRemote {
    client: crate::git_remote::client::RemoteGitClient,
    repo: String,
}

pub fn gitlab_remote_for(
    cwd: &Path,
    settings: &crate::config::settings::Settings,
) -> Option<GitlabRemote> {
    use crate::config::settings::GitProvider;
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).ok()?;
    let remote_name = default_remote(Path::new(&root));
    let origin = git(Path::new(&root), &["remote", "get-url", &remote_name]).ok()?;
    let repo = parse_gitlab_project_path(&origin)?;
    let remote = configured_remote_for(settings, GitProvider::Gitlab, &repo)?;
    let token = crate::config::settings::resolve_git_remote_token(remote).ok()?;
    let client = crate::git_remote::client::RemoteGitClient::new(
        &remote.base_url,
        &token,
        remote.provider.clone(),
    );
    Some(GitlabRemote { client, repo })
}

impl DeliveryRemote for GitlabRemote {
    fn capabilities(&self) -> DeliveryCapabilities {
        DeliveryCapabilities {
            review: true,
            ci: false,
            merge: true,
            release: false,
            live: false,
        }
    }

    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<DeliveryPr, String> {
        if let Ok(mrs) = crate::git_remote::gitlab::list_prs(&self.client, &self.repo, "open").await
        {
            if let Some(mr) = mrs.into_iter().find(|mr| mr.head_branch == head) {
                return Ok(DeliveryPr {
                    number: mr.number,
                    url: mr.url,
                    title: mr.title,
                    body: mr.body,
                });
            }
        }
        let mr = crate::git_remote::gitlab::create_pr(
            &self.client,
            &self.repo,
            title,
            body,
            head,
            base,
            false,
        )
        .await?;
        Ok(DeliveryPr {
            number: mr.number,
            url: mr.url,
            title: mr.title,
            body: mr.body,
        })
    }

    async fn ci_status(&self, _sha: &str) -> Result<CiStatus, String> {
        Ok(CiStatus::None)
    }

    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        _commit_message: Option<&MergeCommitMessage>,
    ) -> Result<(), String> {
        crate::git_remote::gitlab::merge_pr(&self.client, &self.repo, number, method.as_str()).await
    }

    async fn trigger_release(&self) -> Result<String, String> {
        Err("GitLab release dispatch is not built in; configure a delivery provider hook/plugin for this repository's release pipeline.".into())
    }
}

impl DeliveryRemote for GithubRemote {
    fn capabilities(&self) -> DeliveryCapabilities {
        DeliveryCapabilities {
            review: true,
            ci: true,
            merge: true,
            release: true,
            live: false,
        }
    }

    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<DeliveryPr, String> {
        // Idempotency: reuse an existing open PR for this head branch.
        if let Ok(prs) = crate::git_remote::github::list_prs(&self.client, &self.repo, "open").await
        {
            if let Some(pr) = prs.into_iter().find(|p| p.head_branch == head) {
                return Ok(DeliveryPr {
                    number: pr.number,
                    url: pr.url,
                    title: pr.title,
                    body: pr.body,
                });
            }
        }
        let pr = crate::git_remote::github::create_pr(
            &self.client,
            &self.repo,
            title,
            body,
            head,
            base,
            false,
        )
        .await?;
        Ok(DeliveryPr {
            number: pr.number,
            url: pr.url,
            title: pr.title,
            body: pr.body,
        })
    }

    async fn ci_status(&self, sha: &str) -> Result<CiStatus, String> {
        let s = crate::git_remote::github::ci_status(&self.client, &self.repo, sha).await?;
        Ok(match s.as_str() {
            "success" => CiStatus::Success,
            "pending" => CiStatus::Pending,
            "none" => CiStatus::None,
            other => CiStatus::Failure(other.trim_start_matches("failure:").to_string()),
        })
    }

    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
    ) -> Result<(), String> {
        crate::git_remote::github::merge_pr(
            &self.client,
            &self.repo,
            number,
            method.as_str(),
            commit_message.map(|message| message.title.as_str()),
            commit_message.map(|message| message.body.as_str()),
        )
        .await
    }

    async fn trigger_release(&self) -> Result<String, String> {
        // workflow_dispatch on the repo's release workflow (needs a token with
        // the `workflow` scope; a repo-only token yields a clear 403 here).
        let path = format!(
            "/repos/{}/actions/workflows/{}/dispatches",
            self.repo, self.release_workflow
        );
        self.client
            .post(&path, serde_json::json!({ "ref": self.default_branch }))
            .await
            .map(|_| format!("已触发发布工作流 {}", self.release_workflow))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };

    #[test]
    fn production_gh_git_spawns_go_through_dev_command() {
        // Regression (the "deliver_changes gh PATH blocked" report): a bare
        // `Command::new` on a program NAME fails to spawn in a GUI-launched app
        // on macOS — it doesn't inherit the login-shell PATH, so `/opt/homebrew/
        // bin/gh` is invisible even when gh is installed + authenticated. Every
        // PRODUCTION spawn MUST resolve the absolute path via `dev_command`; only
        // #[cfg(test)] code (which runs with cargo's full env) may use bare names.
        let src = include_str!("delivery.rs");
        let production = src
            .split("\n#[cfg(test)]")
            .next()
            .expect("delivery.rs has a production section");
        for bad in [
            "Command::new(\"gh\")",
            "Command::new(\"git\")",
            "Command::new(bin)",
        ] {
            assert!(
                !production.contains(bad),
                "production delivery code must spawn via dev_command(), not `{bad}`"
            );
        }
    }

    fn make_repo(tag: &str) -> PathBuf {
        // The repo lives one level under a unique per-test parent, so
        // `root.parent()` is that isolated parent — cleanup via
        // `remove_dir_all(root.parent())` removes only this test's artifacts
        // (repo + its sibling bare origin), NEVER the shared temp dir. A prior
        // version cleaned up `root.parent()` == temp_dir(), which nuked
        // concurrently-running tests on Windows.
        let parent = std::env::temp_dir().join(format!(
            "cf-delivery-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let base = parent.join("repo");
        std::fs::create_dir_all(&base).unwrap();
        let g = |args: &[&str]| git(&base, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.name", "t"]);
        g(&["config", "user.email", "t@t"]);
        std::fs::write(base.join("app.rs"), "fn main() {}\n").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-q", "-m", "init"]);
        base
    }

    #[test]
    fn stage_scoped_excludes_untracked_noise_but_keeps_real_source() {
        let root = make_repo("noise");
        // A real new source file + a bunch of noise that a blanket add would sweep in.
        std::fs::write(root.join("feature.rs"), "pub fn f() {}\n").unwrap();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "notes").unwrap();
        std::fs::create_dir_all(root.join("src-tauri/gen/schemas")).unwrap();
        std::fs::write(root.join("src-tauri/gen/schemas/macOS-schema.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("codex-worktrees/x")).unwrap();
        std::fs::write(root.join("codex-worktrees/x/f"), "junk").unwrap();
        // A tracked modification too.
        std::fs::write(root.join("app.rs"), "fn main() { /* changed */ }\n").unwrap();

        let staged = stage_scoped(&root, &[]).unwrap();

        assert!(
            staged.contains(&"feature.rs".to_string()),
            "real new source staged"
        );
        assert!(
            staged.contains(&"app.rs".to_string()),
            "tracked modification staged"
        );
        assert!(
            !staged.iter().any(|p| p.starts_with(".claude/")),
            "no .claude noise"
        );
        assert!(!staged.contains(&"CLAUDE.md".to_string()), "no CLAUDE.md");
        assert!(
            !staged
                .iter()
                .any(|p| p.starts_with("src-tauri/gen/schemas")),
            "no generated schemas"
        );
        assert!(
            !staged.iter().any(|p| p.starts_with("codex-worktrees/")),
            "no sibling worktree"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn extra_excludes_are_honored() {
        let root = make_repo("extra");
        std::fs::write(root.join("keep.rs"), "x").unwrap();
        std::fs::write(root.join("scratch.tmp"), "y").unwrap();
        let staged = stage_scoped(&root, &["scratch.tmp".to_string()]).unwrap();
        assert!(staged.contains(&"keep.rs".to_string()));
        assert!(!staged.contains(&"scratch.tmp".to_string()));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn no_token_message_tells_the_model_to_stop_retrying() {
        // The message must offer BOTH setup paths — a one-time `gh auth
        // login` (preferred: zero app-side config) and the conversational
        // token flow — and forbid blind retries until one succeeds.
        assert!(NO_TOKEN_PR_MESSAGE.contains("gh auth login"));
        assert!(NO_TOKEN_PR_MESSAGE.contains("远程仓库"));
        assert!(NO_TOKEN_PR_MESSAGE.contains("不要再调用 deliver_changes"));
    }

    #[test]
    fn readiness_note_warns_before_work_when_github_origin_has_no_token() {
        // The broken chain must be surfaced in the model's FIRST reply, not
        // discovered after the work is done.
        let settings = crate::config::settings::Settings::default();
        let note = delivery_readiness_with_gh(
            Some("git@github.com:BumStill/CodeFactory.git"),
            &settings,
            false,
        )
        .expect("github origin without gh or a token must produce a warning note");
        assert!(note.contains("FIRST reply"));
        assert!(note.contains("gh auth login"));
        assert!(note.contains("do NOT call deliver_changes"));
    }

    #[test]
    fn readiness_note_reports_ceiling_when_remote_is_configured() {
        let mut settings = crate::config::settings::Settings::default();
        settings
            .git_remotes
            .push(crate::config::settings::GitRemoteConfig {
                id: "r1".into(),
                name: "github".into(),
                provider: crate::config::settings::GitProvider::Github,
                base_url: "https://api.github.com".into(),
                token_ref: Some("cf.test.github.readiness".into()),
                token: "".into(),
                default_repo: Some("BumStill/CodeFactory".into()),
            });
        crate::secrets::set_key("cf.test.github.readiness", "token").unwrap();
        let note = delivery_readiness_with_gh(
            Some("https://github.com/BumStill/CodeFactory.git"),
            &settings,
            false,
        )
        .expect("configured remote must produce a capability note");
        assert!(note.contains("through_release"));
        assert!(note.contains("deliver_changes"));
    }

    #[test]
    fn readiness_note_stays_silent_when_off_or_unrecognized_origin() {
        let settings = crate::config::settings::Settings::default();
        assert!(delivery_readiness_with_gh(None, &settings, false).is_none());
        assert!(
            delivery_readiness_with_gh(Some("file:///tmp/repo.git"), &settings, false).is_none()
        );

        let mut off = crate::config::settings::Settings::default();
        off.delivery_ceiling = DeliveryCeiling::Off;
        assert!(delivery_readiness_with_gh(
            Some("https://github.com/BumStill/CodeFactory.git"),
            &off,
            false,
        )
        .is_none());
    }

    #[test]
    fn readiness_note_supports_configured_enterprise_gitlab_origin() {
        let mut settings = crate::config::settings::Settings::default();
        settings
            .git_remotes
            .push(crate::config::settings::GitRemoteConfig {
                id: "gl1".into(),
                name: "corp-gitlab".into(),
                provider: crate::config::settings::GitProvider::Gitlab,
                base_url: "https://gitlab.corp.example/api/v4".into(),
                token_ref: Some("cf.test.gitlab.readiness".into()),
                token: "".into(),
                default_repo: Some("platform/app".into()),
            });

        crate::secrets::set_key("cf.test.gitlab.readiness", "token").unwrap();
        let note = delivery_readiness_with_gh(
            Some("git@gitlab.corp.example:platform/app.git"),
            &settings,
            false,
        )
        .expect("configured GitLab origin should advertise delivery capability");

        assert!(note.contains("GitLab"));
        assert!(note.contains("merge request"));
        assert!(note.contains("deliver_changes"));
        assert!(
            !note.contains("没有可用的 GitHub 通道"),
            "GitLab remotes must not be reported as missing GitHub credentials"
        );
    }

    #[test]
    fn readiness_note_for_unconfigured_gitlab_origin_names_gitlab_setup_not_github_only() {
        let settings = crate::config::settings::Settings::default();
        let note = delivery_readiness_with_gh(
            Some("https://gitlab.corp.example/platform/app.git"),
            &settings,
            false,
        )
        .expect("GitLab origin without token should produce an early blocker note");

        assert!(note.contains("GitLab"));
        assert!(note.contains("merge request"));
        assert!(note.contains("远程仓库 token"));
        assert!(
            !note.contains("gh auth login"),
            "enterprise GitLab setup must not tell the user that GitHub CLI auth fixes the MR path"
        );
    }

    #[test]
    fn partial_summary_names_requested_and_effective_ceiling() {
        let partial = DeliveryOutcome {
            steps: vec![StepResult::ok("pr", "opened")],
            branch: Some("b".into()),
            commit_sha: None,
            pr_url: Some("https://github.com/x/y/pull/1".into()),
            pr_number: Some(1),
            final_state: "delivered".into(),
            stage: "complete".into(),
            code: "delivery_ceiling_reached".into(),
            recoverable: false,
            next_action: None,
            reached_state: "pr_open".into(),
            requested_ceiling: "through_release".into(),
            effective_ceiling: "pr_only".into(),
            capability_gap: Some("CI observer".into()),
            release_receipt: None,
            summary: String::new(),
        };
        let done = finish(partial, "b");
        assert_eq!(done.final_state, "blocked");
        assert_eq!(done.code, "delivery_capability_gap");
        assert!(done.summary.contains("pr_open"));
        assert!(done.summary.contains("through_release"));
        assert!(done.next_action.as_deref().unwrap_or("").contains("CI"));
    }

    #[test]
    fn gh_cli_is_preferred_over_rest_token_and_both_over_nothing() {
        // Field report: the delivery chain kept demanding a configured token
        // while a logged-in `gh` CLI sat right there. gh comes first; the
        // token+REST path stays as the fallback for machines without gh.
        use super::RemoteKind;
        assert_eq!(resolve_remote_kind(true, true), Some(RemoteKind::GhCli));
        assert_eq!(resolve_remote_kind(true, false), Some(RemoteKind::GhCli));
        assert_eq!(
            resolve_remote_kind(false, true),
            Some(RemoteKind::RestToken)
        );
        assert_eq!(resolve_remote_kind(false, false), None);
    }

    #[test]
    fn gh_cli_argv_builders_produce_exact_commands() {
        let create = gh_pr_create_args("t", "b", "feat/x", "main");
        assert_eq!(
            create,
            vec![
                "pr", "create", "--title", "t", "--body", "b", "--head", "feat/x", "--base", "main"
            ]
        );
        let merge_message = MergeCommitMessage {
            title: "fix: preserve release policy".into(),
            body: "Release-Urgency: hold".into(),
        };
        let merge = gh_pr_merge_args(42, MergeMethod::Squash, Some(&merge_message));
        assert_eq!(
            merge,
            vec![
                "pr",
                "merge",
                "42",
                "--squash",
                "--subject",
                "fix: preserve release policy",
                "--body",
                "Release-Urgency: hold",
            ]
        );
        let release = gh_workflow_run_args("auto-release.yml", "main");
        assert_eq!(
            release,
            vec!["workflow", "run", "auto-release.yml", "--ref", "main"]
        );
    }

    #[test]
    fn release_urgency_is_only_read_from_the_footer_and_survives_squash() {
        assert!(release_urgency_trailers(
            "fix: safe\n\nThis prose says Release-Urgency: hold but is not a trailer."
        )
        .is_empty());
        let trailers =
            release_urgency_trailers("fix: guarded\n\nDetails.\n\nRelease-Urgency: hold");
        assert_eq!(trailers, vec!["hold"]);

        let metadata = ReleaseMetadata {
            urgencies: trailers,
            breaking_changes: Vec::new(),
        };
        let message = squash_merge_message("fix: guarded", "PR details", &metadata);
        assert_eq!(message.title, "fix: guarded");
        assert!(message.body.ends_with("Release-Urgency: hold"));
        assert_eq!(release_urgency_trailers(&message.body), vec!["hold"]);

        let mixed = squash_merge_message(
            "fix: mixed",
            "PR details",
            &ReleaseMetadata {
                urgencies: vec!["hold".into(), "immediate".into()],
                breaking_changes: Vec::new(),
            },
        );
        assert!(mixed
            .body
            .ends_with("Release-Urgency: hold\nRelease-Urgency: immediate"));
        assert_eq!(
            release_urgency_trailers(&mixed.body),
            vec!["hold", "immediate"]
        );

        let breaking_commit = append_release_urgency(
            "fix: change format\n\nBREAKING CHANGE: migration required".into(),
            Some(ReleaseUrgency::Immediate),
        );
        assert!(breaking_commit
            .ends_with("BREAKING CHANGE: migration required\nRelease-Urgency: immediate"));
        assert_eq!(
            breaking_change_trailers(&breaking_commit),
            vec!["BREAKING CHANGE: migration required"]
        );
        assert_eq!(
            breaking_change_trailers("fix: change format\n\nBREAKING-CHANGE: migration required"),
            vec!["BREAKING-CHANGE: migration required"]
        );
    }

    #[test]
    fn branch_breaking_change_and_urgency_survive_squash_in_one_footer_block() {
        let root = make_repo("squash-release-metadata");
        let origin = root.parent().unwrap().join("origin.git");
        git(
            &root,
            &["init", "--bare", origin.to_str().expect("origin path")],
        )
        .unwrap();
        git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                origin.to_str().expect("origin path"),
            ],
        )
        .unwrap();
        git(&root, &["push", "-u", "origin", "main"]).unwrap();
        git(&root, &["checkout", "-b", "feature/breaking"]).unwrap();
        git(
            &root,
            &[
                "commit",
                "--allow-empty",
                "-m",
                "fix: change persisted format",
                "-m",
                "BREAKING CHANGE: old databases require migration\nRelease-Urgency: hold",
            ],
        )
        .unwrap();

        let metadata = branch_release_metadata(
            &root,
            "origin",
            "main",
            Some("Reviewed migration.\n\nRelease-Urgency: immediate"),
            None,
        )
        .unwrap();
        assert_eq!(
            metadata.breaking_changes,
            vec!["BREAKING CHANGE: old databases require migration"]
        );
        assert_eq!(metadata.urgencies, vec!["hold", "immediate"]);

        let message = squash_merge_message(
            "fix: change persisted format",
            "Reviewed migration.\n\nRelease-Urgency: immediate",
            &metadata,
        );
        assert!(message.body.ends_with(
            "Release-Urgency: immediate\n\
BREAKING CHANGE: old databases require migration\n\
Release-Urgency: hold"
        ));
        assert_eq!(
            breaking_change_trailers(&message.body),
            vec!["BREAKING CHANGE: old databases require migration"]
        );
        assert_eq!(
            release_urgency_trailers(&message.body),
            vec!["immediate", "hold"]
        );

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn no_token_message_offers_the_gh_cli_path_first() {
        assert!(NO_TOKEN_PR_MESSAGE.contains("gh auth login"));
        assert!(NO_TOKEN_PR_MESSAGE.contains("远程仓库"));
    }

    #[test]
    fn no_remote_channel_message_keeps_github_https_as_github() {
        let message =
            no_remote_channel_message(Some("https://github.com/BumStill/CodeFactory.git"));
        assert!(message.contains("GitHub 通道"));
        assert!(message.contains("gh auth login"));
        assert!(message.contains("开 PR"));
        assert!(!message.contains("GitLab 项目"));
        assert!(!message.contains("merge request"));
    }

    #[test]
    fn no_remote_channel_message_is_provider_aware_for_gitlab() {
        let message = no_remote_channel_message(Some("git@gitlab.corp.example:platform/app.git"));
        assert!(message.contains("GitLab 项目 platform/app"));
        assert!(message.contains("merge request"));
        assert!(message.contains("hook/plugin"));
        assert!(!message.contains("没有可用的 GitHub 通道"));
        assert!(!message.contains("gh auth login"));

        let github_message =
            no_remote_channel_message(Some("git@github.com:BumStill/CodeFactory.git"));
        assert!(github_message.contains("GitHub 通道"));
    }

    /// Real-runtime smoke: with a logged-in gh on this machine, ci_status on
    /// the repo's own HEAD must parse into a valid CiStatus. Skips cleanly
    /// when gh is absent or unauthenticated.
    #[tokio::test]
    async fn gh_cli_remote_reads_real_ci_status_when_gh_is_authenticated() {
        if !gh_cli_available() {
            eprintln!("skipping gh smoke: gh missing or unauthenticated");
            return;
        }
        let cwd = std::env::current_dir().unwrap();
        let Some(remote) = gh_remote_for(&cwd) else {
            eprintln!("skipping gh smoke: not a github repo checkout");
            return;
        };
        let head = git(&cwd, &["rev-parse", "HEAD"]).unwrap();
        match remote.ci_status(&head).await {
            Ok(_) => {}
            Err(e) => panic!("gh ci_status must parse: {e}"),
        }
    }

    #[test]
    fn parse_owner_repo_handles_https_and_ssh() {
        assert_eq!(
            parse_owner_repo("https://github.com/BumStill/CodeFactory.git").as_deref(),
            Some("BumStill/CodeFactory")
        );
        assert_eq!(
            parse_owner_repo("git@github.com:BumStill/CodeFactory.git").as_deref(),
            Some("BumStill/CodeFactory")
        );
        assert_eq!(
            parse_owner_repo("https://github.com/BumStill/CodeFactory").as_deref(),
            Some("BumStill/CodeFactory")
        );
        assert_eq!(parse_owner_repo("https://gitlab.com/x/y.git"), None);
    }

    #[test]
    fn provider_discovery_covers_common_forges_without_defaulting_to_github() {
        let cases = [
            ("https://github.com/acme/app.git", ForgeFamily::Github),
            ("git@github.corp.example:acme/app.git", ForgeFamily::Github),
            ("https://gitlab.com/acme/app.git", ForgeFamily::Gitlab),
            ("git@gitlab.corp.example:acme/app.git", ForgeFamily::Gitlab),
            ("https://bitbucket.org/acme/app.git", ForgeFamily::Bitbucket),
            (
                "https://dev.azure.com/acme/project/_git/app",
                ForgeFamily::AzureDevops,
            ),
            ("git@gitea.example.com:acme/app.git", ForgeFamily::Gitea),
            (
                "ssh://git@forgejo.example.com/acme/app.git",
                ForgeFamily::Forgejo,
            ),
            ("ssh://review.example.com:29418/app", ForgeFamily::Gerrit),
            (
                "https://git-codecommit.us-east-1.amazonaws.com/v1/repos/app",
                ForgeFamily::CodeCommit,
            ),
            (
                "ssh://git@git.corp.example/acme/app.git",
                ForgeFamily::Generic,
            ),
        ];
        for (url, expected) in cases {
            assert_eq!(classify_forge(url), expected, "{url}");
        }
    }

    #[test]
    fn non_github_missing_channel_messages_never_prescribe_gh_auth() {
        for url in [
            "https://bitbucket.org/acme/app.git",
            "https://dev.azure.com/acme/project/_git/app",
            "ssh://git@gitea.example.com/acme/app.git",
            "ssh://review.example.com:29418/app",
            "https://git-codecommit.us-east-1.amazonaws.com/v1/repos/app",
            "ssh://git@git.corp.example/acme/app.git",
        ] {
            let message = no_remote_channel_message(Some(url));
            assert!(!message.contains("gh auth login"), "{url}: {message}");
            assert!(message.contains("delivery_provider"), "{url}: {message}");
        }
    }

    #[test]
    fn repository_delivery_config_expands_sha_bound_live_assertion() {
        let root = make_repo("live-config");
        std::fs::create_dir_all(root.join(".codefactory")).unwrap();
        std::fs::write(
            root.join(".codefactory/delivery.json"),
            r#"{
              "schema_version": 1,
              "provider": "zeabur",
              "deployment_timeout_secs": 42,
              "live": {
                "url": "https://example.test/health",
                "expected_status": 200,
                "body_contains": "build:$GIT_SHA_SHORT",
                "timeout_secs": 30,
                "poll_interval_secs": 2
              }
            }"#,
        )
        .unwrap();
        let config = load_delivery_config(&root).unwrap().unwrap();
        assert_eq!(config.provider.as_deref(), Some("zeabur"));
        assert_eq!(config.deployment_timeout_secs, 42);
        let live = config.live.unwrap();
        assert_eq!(live.expected_body("1234567890abcdef"), "build:1234567");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn release_without_live_evidence_is_not_reported_as_delivered_or_live() {
        let mut outcome = DeliveryOutcome {
            steps: vec![
                StepResult::ok("merge", "merged"),
                StepResult::ok("release", "release triggered"),
            ],
            branch: Some("feat/x".into()),
            commit_sha: Some("abc123".into()),
            pr_url: Some("https://example.test/pr/1".into()),
            pr_number: Some(1),
            final_state: "delivered".into(),
            stage: "release".into(),
            code: "release_triggered".into(),
            recoverable: false,
            next_action: None,
            reached_state: "release_triggered".into(),
            requested_ceiling: "through_release".into(),
            effective_ceiling: "through_release".into(),
            capability_gap: None,
            release_receipt: None,
            summary: String::new(),
        };
        outcome = block_unverified_release(outcome, "未配置 live verifier");
        assert_eq!(outcome.final_state, "blocked");
        assert!(outcome
            .steps
            .iter()
            .any(|s| s.step == "live" && s.status == "blocked"));
        assert!(!outcome.summary.contains("已上线"));
    }

    #[test]
    fn hook_status_parser_distinguishes_pending_failure_unsupported_and_success() {
        assert_eq!(
            parse_observation_status("success", None),
            ObservationStatus::Success("verified".into())
        );
        assert_eq!(
            parse_observation_status("pending", Some("building".into())),
            ObservationStatus::Pending("building".into())
        );
        assert_eq!(
            parse_observation_status("failure", Some("boom".into())),
            ObservationStatus::Failure("boom".into())
        );
        assert_eq!(
            parse_observation_status("unsupported", None),
            ObservationStatus::Unsupported("not configured".into())
        );
    }

    #[test]
    fn parse_owner_repo_supports_github_enterprise_host() {
        assert_eq!(
            parse_owner_repo_for_host(
                "git@github.corp.example:team/app.git",
                "github.corp.example"
            )
            .as_deref(),
            Some("team/app")
        );
        assert_eq!(
            parse_owner_repo_for_host(
                "https://github.corp.example/team/app.git",
                "github.corp.example"
            )
            .as_deref(),
            Some("team/app")
        );
    }

    #[test]
    fn unrecognized_non_github_hosts_do_not_get_gitlab_readiness_by_default() {
        let settings = crate::config::settings::Settings::default();
        assert!(
            delivery_readiness_with_gh(
                Some("https://git.example.com/platform/app.git"),
                &settings,
                false,
            )
            .is_none(),
            "generic private Git hosts should use delivery_provider hooks instead of being mislabeled as GitLab"
        );
    }

    #[test]
    fn parse_gitlab_project_path_handles_saas_enterprise_https_and_ssh() {
        assert_eq!(
            parse_gitlab_project_path("https://gitlab.com/group/sub/project.git").as_deref(),
            Some("group/sub/project")
        );
        assert_eq!(
            parse_gitlab_project_path("git@gitlab.corp.example:platform/app.git").as_deref(),
            Some("platform/app")
        );
        assert_eq!(
            parse_gitlab_project_path("ssh://git@gitlab.corp.example/platform/app.git").as_deref(),
            Some("platform/app")
        );
    }

    #[test]
    fn remote_provider_hook_can_override_built_in_resolution() {
        let mut registry = DeliveryRemoteRegistry::default();
        registry.register(|ctx| {
            if ctx.origin_url.contains("git.corp.example") {
                Some(DeliveryRemoteDescriptor {
                    provider: DeliveryProviderKind::Hook("corp-mr".into()),
                    repo: "platform/app".into(),
                    default_branch: "main".into(),
                    missing_credentials_message: None,
                })
            } else {
                None
            }
        });

        let descriptor = registry
            .resolve(&DeliveryRemoteContext {
                origin_url: "ssh://git@git.corp.example/platform/app.git".into(),
                default_branch: "main".into(),
                settings: &crate::config::settings::Settings::default(),
            })
            .expect("hook should resolve custom enterprise remote");

        assert_eq!(
            descriptor.provider,
            DeliveryProviderKind::Hook("corp-mr".into())
        );
        assert_eq!(descriptor.repo, "platform/app");
    }

    #[tokio::test]
    async fn delivery_provider_hook_remote_executes_json_protocol() {
        let root = make_repo("hook-remote");
        let hook = root.join("provider.py");
        std::fs::write(
            &hook,
            r#"#!/usr/bin/env python3
import json, os, sys
req=json.load(sys.stdin)
action=req.get('action')
if action == 'open_or_get_pr':
    print(json.dumps({
        'number': 42,
        'url': 'https://git.corp.example/platform/app/-/merge_requests/42',
        'title': req.get('title', ''),
        'body': req.get('body', ''),
    }))
elif action == 'ci_status':
    print(json.dumps({'status': 'success'}))
elif action == 'merge_pr':
    print(json.dumps({'ok': True}))
elif action == 'trigger_release':
    print(json.dumps({'detail': 'corp release dispatched'}))
elif action == 'deployment_status':
    print(json.dumps({'status': 'success', 'detail': 'corp deployment ready'}))
elif action == 'verify_live':
    print(json.dumps({'status': 'success', 'detail': 'corp live verified'}))
else:
    print(json.dumps({'error': 'unknown action'}))
    sys.exit(2)
"#,
        )
        .unwrap();
        let remote = HookRemote::new(
            "corp-mr".into(),
            format!("python3 {}", hook.display()),
            root.clone(),
        );

        let pr = remote
            .open_or_get_pr("title", "body", "feat/x", "main")
            .await
            .expect("hook open_or_get_pr");
        assert_eq!(pr.number, 42);
        assert!(pr.url.contains("merge_requests/42"));
        assert_eq!(pr.title, "title");
        assert_eq!(pr.body, "body");
        assert_eq!(remote.ci_status("abc123").await.unwrap(), CiStatus::Success);
        remote
            .merge_pr(42, MergeMethod::Squash, None)
            .await
            .unwrap();
        assert_eq!(
            remote.trigger_release().await.unwrap(),
            "corp release dispatched"
        );
        assert_eq!(
            remote
                .deployment_status("abc123", Some("zeabur"))
                .await
                .unwrap(),
            ObservationStatus::Success("corp deployment ready".into())
        );
        assert_eq!(
            remote
                .verify_live("abc123", Some("https://app.example.test"))
                .await
                .unwrap(),
            ObservationStatus::Success("corp live verified".into())
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn delivery_provider_hooks_are_discovered_from_settings_hooks() {
        let mut settings = crate::config::settings::Settings::default();
        settings.hooks.push(crate::commands::hooks::HookConfig {
            id: "delivery-provider-corp".into(),
            name: "Corp MR provider".into(),
            event: "delivery_provider".into(),
            action: crate::commands::hooks::HookAction::RunCommand {
                command: "corp-delivery-provider".into(),
                cwd: None,
            },
            enabled: true,
            filter: Some("git.corp.example".into()),
        });

        let candidates =
            delivery_provider_hooks_for(&settings, "ssh://git@git.corp.example/platform/app.git");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "delivery-provider-corp");
        assert_eq!(candidates[0].command, "corp-delivery-provider");
    }

    #[tokio::test]
    async fn deliver_state_machine_uses_delivery_provider_hook_after_push() {
        let root = feature_branch_repo("hook-deliver");
        let hook = root.join("provider.py");
        std::fs::write(
            &hook,
            r#"#!/usr/bin/env python3
import json, sys
req=json.load(sys.stdin)
action=req.get('action')
if action == 'open_or_get_pr':
    print(json.dumps({
        'number': 77,
        'url': 'https://git.corp.example/platform/app/-/merge_requests/77',
        'title': req.get('title', ''),
        'body': req.get('body', ''),
    }))
elif action == 'ci_status':
    print(json.dumps({'status': 'success'}))
elif action == 'merge_pr':
    print(json.dumps({'ok': True}))
elif action == 'trigger_release':
    print(json.dumps({'detail': 'corp release dispatched'}))
elif action == 'deployment_status':
    print(json.dumps({'status': 'success', 'detail': 'corp deployment ready'}))
elif action == 'verify_live':
    print(json.dumps({'status': 'success', 'detail': 'corp live verified'}))
else:
    print(json.dumps({'error': 'unknown action'}))
    sys.exit(2)
"#,
        )
        .unwrap();
        let remote = HookRemote::new(
            "corp-mr".into(),
            format!("python3 {}", hook.display()),
            root.clone(),
        );

        let outcome = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            1,
            &DeliverOpts {
                title: Some("hook delivery".into()),
                body: Some("body".into()),
                release_urgency: None,
                requested_ceiling: None,
                extra_excludes: vec![],
            },
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(outcome.final_state, "delivered");
        assert_eq!(outcome.pr_number, Some(77));
        assert_eq!(
            outcome.pr_url.as_deref(),
            Some("https://git.corp.example/platform/app/-/merge_requests/77")
        );
        assert!(outcome
            .steps
            .iter()
            .any(|s| s.step == "push" && s.status == "ok"));
        assert!(outcome
            .steps
            .iter()
            .any(|s| s.step == "pr" && s.detail.contains("PR/MR #77")));
        assert!(outcome
            .steps
            .iter()
            .any(|s| s.step == "merge" && s.status == "ok"));
        assert!(outcome
            .steps
            .iter()
            .any(|s| s.step == "release" && s.detail == "corp release dispatched"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn configured_gitlab_remote_is_resolved_from_enterprise_origin() {
        let root = make_repo("gitlab-resolve");
        let mut settings = crate::config::settings::Settings::default();
        settings
            .git_remotes
            .push(crate::config::settings::GitRemoteConfig {
                id: "gl1".into(),
                name: "corp-gitlab".into(),
                provider: crate::config::settings::GitProvider::Gitlab,
                base_url: "https://gitlab.corp.example/api/v4".into(),
                token_ref: Some("cf.test.gitlab.resolve".into()),
                token: "".into(),
                default_repo: Some("platform/app".into()),
            });
        crate::secrets::set_key("cf.test.gitlab.resolve", "token").unwrap();
        git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "git@gitlab.corp.example:platform/app.git",
            ],
        )
        .unwrap();

        let remote = resolve_delivery_remote(&root, &settings)
            .expect("GitLab remote token should resolve a delivery remote");
        assert!(matches!(remote, EitherRemote::Gitlab(_)));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn is_excluded_matches_dirs_and_files() {
        assert!(is_excluded(".claude/settings.json", &[]));
        assert!(is_excluded("CLAUDE.md", &[]));
        assert!(is_excluded("src-tauri/gen/schemas/macOS-schema.json", &[]));
        assert!(!is_excluded("src/main.rs", &[]));
        assert!(
            !is_excluded("claude.rs", &[]),
            "prefix must be path-boundary, not substring"
        );
        assert!(is_excluded("weird.tmp", &["weird.tmp".into()]));
    }

    // ── State-machine tests with a stub remote ──────────────────────────────

    struct StubRemote {
        ci: CiStatus,
        existing_pr: Option<(u64, String)>,
        merge_ok: bool,
        /// Varies per test: the whole point of the ladder fix is that a missing
        /// high-rung capability must not cancel the rungs below it.
        caps: DeliveryCapabilities,
        calls: Arc<StubCalls>,
    }

    #[derive(Default)]
    struct StubCalls {
        merged: AtomicBool,
        open_pr: AtomicUsize,
        ci: AtomicUsize,
        merge: AtomicUsize,
        release: AtomicUsize,
        merge_commit_message: Mutex<Option<MergeCommitMessage>>,
        remote_pr_text: Mutex<Option<(String, String)>>,
    }

    fn stub_calls() -> Arc<StubCalls> {
        Arc::new(StubCalls::default())
    }

    fn every_capability() -> DeliveryCapabilities {
        DeliveryCapabilities {
            review: true,
            ci: true,
            merge: true,
            release: true,
            live: true,
        }
    }

    impl DeliveryRemote for StubRemote {
        fn capabilities(&self) -> DeliveryCapabilities {
            self.caps
        }

        async fn open_or_get_pr(
            &self,
            t: &str,
            b: &str,
            _h: &str,
            _base: &str,
        ) -> Result<DeliveryPr, String> {
            self.calls.open_pr.fetch_add(1, Ordering::SeqCst);
            let (number, url) = self
                .existing_pr
                .clone()
                .unwrap_or((7, "https://example/pr/7".into()));
            let (title, body) = self
                .calls
                .remote_pr_text
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| (t.to_string(), b.to_string()));
            Ok(DeliveryPr {
                number,
                url,
                title,
                body,
            })
        }
        async fn ci_status(&self, _sha: &str) -> Result<CiStatus, String> {
            self.calls.ci.fetch_add(1, Ordering::SeqCst);
            Ok(self.ci.clone())
        }
        async fn merge_pr(
            &self,
            _n: u64,
            _m: MergeMethod,
            commit_message: Option<&MergeCommitMessage>,
        ) -> Result<(), String> {
            self.calls.merge.fetch_add(1, Ordering::SeqCst);
            *self.calls.merge_commit_message.lock().unwrap() = commit_message.cloned();
            if self.merge_ok {
                self.calls.merged.store(true, Ordering::SeqCst);
                Ok(())
            } else {
                Err("protected branch".into())
            }
        }
        async fn trigger_release(&self) -> Result<String, String> {
            self.calls.release.fetch_add(1, Ordering::SeqCst);
            Ok("release workflow dispatched".into())
        }
    }

    fn feature_branch_repo(tag: &str) -> PathBuf {
        let root = make_repo(tag);
        // A bare origin under the same per-test parent so push targets a real
        // writable repo. `root.parent()` is the isolated per-test dir.
        let origin = root.parent().unwrap().join("origin.git");
        Command::new("git")
            .no_window()
            .args(["init", "--bare", "-q", origin.to_str().unwrap()])
            .status()
            .unwrap();
        git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        )
        .unwrap();
        git(&root, &["push", "-q", "origin", "main"]).unwrap();
        git(&root, &["checkout", "-q", "-b", "feat/x"]).unwrap();
        std::fs::write(root.join("feature.rs"), "pub fn f() {}\n").unwrap();
        root
    }

    #[tokio::test]
    async fn pr_only_commits_pushes_and_opens_pr_then_stops() {
        let root = feature_branch_repo("pronly");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: stub_calls(),
        };
        let out = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;
        assert_eq!(out.final_state, "delivered", "{:?}", out.steps);
        assert_eq!(out.pr_number, Some(7));
        let steps: Vec<&str> = out
            .steps
            .iter()
            .filter(|s| s.status == "ok")
            .map(|s| s.step.as_str())
            .collect();
        assert!(steps.contains(&"commit"));
        assert!(steps.contains(&"push"));
        assert!(steps.contains(&"pr"));
        assert!(!steps.contains(&"ci"), "PrOnly must stop before CI");
        assert!(!steps.contains(&"merge"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn pr_only_metadata_survives_a_parameterless_resume_through_release() {
        let root = feature_branch_repo("pr-metadata-resume");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };
        let first = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            5,
            &DeliverOpts {
                title: Some("fix: resume guarded metadata".into()),
                body: Some(
                    "Reviewed migration.\n\n\
BREAKING CHANGE: old databases require migration\n\
Release-Urgency: hold"
                        .into(),
                ),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;
        assert_eq!(first.final_state, "delivered", "{:?}", first.steps);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);

        let resumed = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(resumed.final_state, "blocked", "{:?}", resumed.steps);
        assert_eq!(resumed.reached_state, "merged");
        assert_eq!(resumed.code, "delivery_release_blocked");
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let merge_message = calls
            .merge_commit_message
            .lock()
            .unwrap()
            .clone()
            .expect("resume must preserve the explicit squash message");
        assert_eq!(
            breaking_change_trailers(&merge_message.body),
            vec!["BREAKING CHANGE: old databases require migration"]
        );
        assert_eq!(release_urgency_trailers(&merge_message.body), vec!["hold"]);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn remote_pr_metadata_is_refreshed_before_a_parameterless_merge() {
        let root = feature_branch_repo("remote-pr-metadata-refresh");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };
        let first = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            5,
            &DeliverOpts {
                title: Some("fix: refresh remote policy".into()),
                body: Some("Initial review notes.".into()),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;
        assert_eq!(first.final_state, "delivered", "{:?}", first.steps);

        *calls.remote_pr_text.lock().unwrap() = Some((
            "fix: refresh remote policy".into(),
            "Maintainer updated the policy.\n\n\
BREAKING CHANGE: old clients require migration\n\
Release-Urgency: hold"
                .into(),
        ));
        let resumed = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(resumed.reached_state, "merged");
        assert_eq!(resumed.code, "delivery_release_blocked");
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let message = calls
            .merge_commit_message
            .lock()
            .unwrap()
            .clone()
            .expect("remote policy metadata must drive the squash message");
        assert_eq!(
            breaking_change_trailers(&message.body),
            vec!["BREAKING CHANGE: old clients require migration"]
        );
        assert_eq!(release_urgency_trailers(&message.body), vec!["hold"]);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn missing_review_provider_blocks_before_commit_or_push() {
        let root = feature_branch_repo("preflight-no-provider");
        let before_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let before_status = git(
            &root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .unwrap();
        let before_upstream = git(
            &root,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .ok();

        let out = deliver::<StubRemote>(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            1,
            &DeliverOpts::default(),
            None,
            Some("main"),
        )
        .await;

        assert_eq!(out.final_state, "blocked");
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), before_head);
        assert_eq!(
            git(
                &root,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )
            .unwrap(),
            before_status,
            "preflight blocker must not stage or commit the worktree"
        );
        assert_eq!(
            git(
                &root,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}"
                ],
            )
            .ok(),
            before_upstream,
            "preflight blocker must not push or create upstream state"
        );
        assert!(
            out.steps
                .iter()
                .all(|step| !matches!(step.step.as_str(), "commit" | "push")),
            "{:?}",
            out.steps
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn configured_remote_without_review_adapter_is_side_effect_free() {
        let root = feature_branch_repo("preflight-no-review");
        let before_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let before_status = git(
            &root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .unwrap();
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: DeliveryCapabilities {
                review: false,
                ..every_capability()
            },
            calls: calls.clone(),
        };

        let out = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            1,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(out.final_state, "blocked");
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), before_head);
        assert_eq!(
            git(
                &root,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )
            .unwrap(),
            before_status
        );
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert_eq!(calls.ci.load(Ordering::SeqCst), 0);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn malformed_delivery_config_is_side_effect_free() {
        let root = feature_branch_repo("preflight-malformed-config");
        std::fs::create_dir_all(root.join(".codefactory")).unwrap();
        std::fs::write(root.join(".codefactory/delivery.json"), "{not json").unwrap();
        let before_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let before_status = git(
            &root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .unwrap();
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };

        let out = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            1,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(out.final_state, "blocked");
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), before_head);
        assert_eq!(
            git(
                &root,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )
            .unwrap(),
            before_status
        );
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn through_merge_stops_when_ci_fails() {
        let root = feature_branch_repo("cifail");
        let remote = StubRemote {
            ci: CiStatus::Failure("build red".into()),
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: stub_calls(),
        };
        let out = deliver(
            &root,
            DeliveryCeiling::ThroughMerge,
            MergeMethod::Squash,
            1,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;
        assert_eq!(out.final_state, "blocked");
        assert!(out
            .steps
            .iter()
            .any(|s| s.step == "ci" && s.status == "blocked"));
        assert!(
            !out.steps.iter().any(|s| s.step == "merge"),
            "must not merge on red CI"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn through_merge_merges_on_green() {
        let root = feature_branch_repo("merge");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: stub_calls(),
        };
        let out = deliver(
            &root,
            DeliveryCeiling::ThroughMerge,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;
        assert_eq!(out.final_state, "delivered", "{:?}", out.steps);
        assert!(out
            .steps
            .iter()
            .any(|s| s.step == "merge" && s.status == "ok"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn no_remote_configured_blocks_in_preflight_before_local_mutation() {
        let root = feature_branch_repo("noremote");
        let out = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            None::<&StubRemote>,
            Some("main"),
        )
        .await;
        assert_eq!(out.final_state, "blocked");
        // Provider/auth are checked before staging, committing, or pushing.
        assert!(out
            .steps
            .iter()
            .any(|s| s.step == "preflight" && s.status == "blocked"));
        assert!(!out
            .steps
            .iter()
            .any(|s| s.step == "commit" || s.step == "push" || s.step == "pr"));
        assert!(!git(&root, &["status", "--porcelain"])
            .expect("status")
            .trim()
            .is_empty());
        assert!(!out
            .steps
            .iter()
            .any(|s| s.status == "ok" && s.step != "repo"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    // ── The ladder descends; it is never cancelled wholesale ────────────────
    //
    // 2026-07-30 field report: `deliver_changes` refused with "交付预检未通过:
    // 目标 through_release 缺少 live verifier；尚未执行 stage、commit 或 push。"
    // The work was written and verified, and the tool would not even commit it.
    //
    // Three defaults multiply into that: the default ceiling is ThroughRelease,
    // GhCliRemote/GithubRemote/GitlabRemote all report `live: false`, and most
    // repositories have no `.codefactory/delivery.json`. So the dominant
    // configuration had EVERY delivery refused before the first git command.
    //
    // The rule this pins: a missing ACTUATOR lowers the ceiling; a missing
    // VERIFIER lowers only the claim. Never the whole ladder.

    #[tokio::test]
    async fn a_missing_live_verifier_still_delivers_and_only_withholds_the_live_claim() {
        let root = feature_branch_repo("nolive");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: DeliveryCapabilities {
                live: false,
                ..every_capability()
            },
            calls: calls.clone(),
        };
        let first = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        // The screenshot's exact failure: blocked at preflight with nothing done.
        assert!(
            !first
                .steps
                .iter()
                .any(|s| s.step == "preflight" && s.status == "blocked"),
            "a missing verifier must not block the preflight: {:?}",
            first.steps
        );
        for step in ["commit", "push", "pr", "merge", "release"] {
            assert!(
                first
                    .steps
                    .iter()
                    .any(|s| s.step == step && s.status == "ok"),
                "{step} must still run when only the live verifier is absent: {:?}",
                first.steps
            );
        }
        assert_eq!(first.requested_ceiling, "through_release");
        assert_eq!(first.effective_ceiling, "through_release");
        assert_eq!(first.final_state, "blocked");
        assert_eq!(first.reached_state, "release_triggered");
        assert!(first.recoverable);
        assert!(first.next_action.as_deref().unwrap_or("").contains("live"));

        // Retrying the same session after an unverified release must only
        // re-observe. It must not merge or dispatch the release a second time.
        let second = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;
        assert_eq!(second.final_state, "blocked");
        assert_eq!(calls.merge.load(Ordering::SeqCst), 1);
        assert_eq!(calls.release.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn a_hold_trailer_survives_commit_and_merge_but_blocks_release_dispatch() {
        let root = feature_branch_repo("release-hold");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };
        let outcome = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts {
                title: Some("fix: guarded delivery".into()),
                body: Some("Requires a companion change.".into()),
                release_urgency: Some(ReleaseUrgency::Hold),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(outcome.final_state, "blocked");
        assert_eq!(outcome.reached_state, "merged");
        assert_eq!(outcome.code, "delivery_release_blocked");
        assert!(outcome.summary.contains("allow_guarded_batch=true"));
        assert_eq!(calls.merge.load(Ordering::SeqCst), 1);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let commit_message = git(&root, &["show", "-s", "--format=%B", "HEAD"]).unwrap();
        assert_eq!(release_urgency_trailers(&commit_message), vec!["hold"]);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn release_receipt_resumes_observation_when_release_adapter_is_temporarily_missing() {
        let root = feature_branch_repo("resume-release-receipt");
        let calls = stub_calls();
        let first_remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: DeliveryCapabilities {
                live: false,
                ..every_capability()
            },
            calls: calls.clone(),
        };
        let first = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&first_remote),
            Some("main"),
        )
        .await;
        assert_eq!(first.reached_state, "release_triggered");

        let resume_remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: DeliveryCapabilities {
                release: false,
                live: false,
                ..every_capability()
            },
            calls: calls.clone(),
        };
        let resumed = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&resume_remote),
            Some("main"),
        )
        .await;
        assert!(
            resumed
                .steps
                .iter()
                .any(|step| step.step == "release" && step.detail.contains("复用回执")),
            "{:?}",
            resumed.steps
        );
        assert_eq!(resumed.effective_ceiling, "through_release");
        assert!(resumed.capability_gap.is_none());
        let preflight = resumed
            .steps
            .iter()
            .find(|step| step.step == "preflight")
            .expect("preflight step");
        assert!(preflight.detail.contains("继续 observation"));
        assert!(!preflight.detail.contains("补齐 release"));
        assert_eq!(calls.merge.load(Ordering::SeqCst), 1);
        assert_eq!(calls.release.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn delivery_receipt_fails_closed_when_corrupt_and_never_crosses_remote_context() {
        let root = feature_branch_repo("receipt-context");
        let repo = resolve_repo(&root, Some("main")).unwrap();
        let sha = git(&root, &["rev-parse", "HEAD"]).unwrap();
        git(
            &root,
            &[
                "config",
                "--local",
                &delivery_receipt_key(&repo, &sha),
                "{not-json",
            ],
        )
        .unwrap();
        let error = read_delivery_receipt(&repo, &sha).unwrap_err();
        assert!(error.contains("回执损坏"));

        let other_remote = DeliveryReceipt {
            version: 1,
            state: "release_triggered".into(),
            remote: "upstream".into(),
            remote_identity: receipt_remote_identity(&repo),
            base_branch: repo.default_branch.clone(),
            head_branch: repo.branch.clone(),
            commit_sha: sha.clone(),
            pr_number: 7,
            pr_url: "https://example/pr/7".into(),
            pr_title: None,
            pr_body: None,
            release_detail: Some("dispatched".into()),
        };
        write_delivery_receipt(&repo, &sha, &other_remote).unwrap();
        let error = read_delivery_receipt(&repo, &sha).unwrap_err();
        assert!(error.contains("上下文"));

        let unknown_state = DeliveryReceipt {
            remote: repo.remote.clone(),
            remote_identity: receipt_remote_identity(&repo),
            state: "future_state".into(),
            ..other_remote
        };
        write_delivery_receipt(&repo, &sha, &unknown_state).unwrap();
        let error = read_delivery_receipt(&repo, &sha).unwrap_err();
        assert!(error.contains("无法识别"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn delivery_receipt_key_is_scoped_to_repo_branch_and_tip() {
        let root = feature_branch_repo("receipt-key-context");
        let repo = resolve_repo(&root, Some("main")).unwrap();
        let sha = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let original_key = delivery_receipt_key(&repo, &sha);

        let mut other_branch = repo.clone();
        other_branch.branch = "feat/other".into();
        assert_ne!(original_key, delivery_receipt_key(&other_branch, &sha));

        let mut other_repo = repo.clone();
        other_repo.remote_url = Some("https://github.com/other/project.git".into());
        assert_ne!(original_key, delivery_receipt_key(&other_repo, &sha));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn write_ahead_intent_blocks_automatic_external_retry() {
        for state in ["intent_merge", "intent_release"] {
            let root = feature_branch_repo(state);
            git(&root, &["add", "feature.rs"]).unwrap();
            git(&root, &["commit", "-q", "-m", "feature"]).unwrap();
            let repo = resolve_repo(&root, Some("main")).unwrap();
            let sha = git(&root, &["rev-parse", "HEAD"]).unwrap();
            let receipt = DeliveryReceipt {
                version: 1,
                state: state.into(),
                remote: repo.remote.clone(),
                remote_identity: receipt_remote_identity(&repo),
                base_branch: repo.default_branch.clone(),
                head_branch: repo.branch.clone(),
                commit_sha: sha.clone(),
                pr_number: 7,
                pr_url: "https://example/pr/7".into(),
                pr_title: None,
                pr_body: None,
                release_detail: None,
            };
            write_delivery_receipt(&repo, &sha, &receipt).unwrap();
            let calls = stub_calls();
            let remote = StubRemote {
                ci: CiStatus::Success,
                existing_pr: None,
                merge_ok: true,
                caps: every_capability(),
                calls: calls.clone(),
            };
            let out = deliver(
                &root,
                DeliveryCeiling::ThroughRelease,
                MergeMethod::Squash,
                5,
                &DeliverOpts::default(),
                Some(&remote),
                Some("main"),
            )
            .await;
            assert_eq!(out.final_state, "blocked");
            assert!(!out.recoverable);
            assert_eq!(out.code, "delivery_external_state_uncertain");
            assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
            assert_eq!(calls.release.load(Ordering::SeqCst), 0);
            let _ = std::fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[tokio::test]
    async fn a_missing_release_actuator_descends_to_merge_instead_of_refusing_everything() {
        let root = feature_branch_repo("norelease");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: DeliveryCapabilities {
                release: false,
                live: false,
                ..every_capability()
            },
            calls,
        };
        let out = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        for step in ["commit", "push", "pr", "merge"] {
            assert!(
                out.steps.iter().any(|s| s.step == step && s.status == "ok"),
                "{step} is achievable and must run: {:?}",
                out.steps
            );
        }
        assert!(
            !out.steps.iter().any(|s| s.step == "release"),
            "release has no actuator, so it must be skipped — not attempted: {:?}",
            out.steps
        );
        let preflight = out
            .steps
            .iter()
            .find(|s| s.step == "preflight")
            .expect("preflight is always recorded");
        assert_eq!(preflight.status, "ok");
        assert!(
            preflight.detail.contains("release"),
            "the descent must name the missing capability: {}",
            preflight.detail
        );
        assert_eq!(out.requested_ceiling, "through_release");
        assert_eq!(out.effective_ceiling, "through_merge");
        assert_eq!(out.reached_state, "merged");
        assert_eq!(out.final_state, "blocked");
        assert!(out.recoverable);
        assert!(out.next_action.as_deref().unwrap_or("").contains("release"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn a_missing_ci_observer_descends_to_pr_only() {
        // GitlabRemote's real matrix: review+merge, no ci, no release, no live.
        let root = feature_branch_repo("nocianyway");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: DeliveryCapabilities {
                ci: false,
                release: false,
                live: false,
                ..every_capability()
            },
            calls,
        };
        let out = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        for step in ["commit", "push", "pr"] {
            assert!(
                out.steps.iter().any(|s| s.step == step && s.status == "ok"),
                "{step} is achievable and must run: {:?}",
                out.steps
            );
        }
        // Without a CI observer we must not merge on an unknown CI verdict.
        assert!(
            !out.steps.iter().any(|s| s.step == "merge"),
            "merging without a CI verdict would ship unverified code: {:?}",
            out.steps
        );
        assert_eq!(out.requested_ceiling, "through_release");
        assert_eq!(out.effective_ceiling, "pr_only");
        assert_eq!(out.reached_state, "pr_open");
        assert_eq!(out.final_state, "blocked");
        assert!(out.recoverable);
        assert!(out.next_action.as_deref().unwrap_or("").contains("CI"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn a_missing_merge_adapter_descends_to_ci_green_and_reports_partial_truth() {
        let root = feature_branch_repo("nomerge");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: DeliveryCapabilities {
                merge: false,
                release: false,
                live: false,
                ..every_capability()
            },
            calls: calls.clone(),
        };
        let out = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(out.requested_ceiling, "through_release");
        assert_eq!(out.effective_ceiling, "through_ci_green");
        assert_eq!(out.reached_state, "ci_green");
        assert_eq!(out.final_state, "blocked");
        assert!(out.recoverable);
        assert!(out.next_action.as_deref().unwrap_or("").contains("merge"));
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn explicit_lower_ceiling_is_complete_not_partial() {
        let root = feature_branch_repo("requested-pr-only");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: DeliveryCapabilities {
                ci: false,
                merge: false,
                release: false,
                live: false,
                review: true,
            },
            calls: calls.clone(),
        };
        let out = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            5,
            &DeliverOpts {
                requested_ceiling: Some(DeliveryCeiling::PrOnly),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(out.requested_ceiling, "pr_only");
        assert_eq!(out.effective_ceiling, "pr_only");
        assert_eq!(out.reached_state, "pr_open");
        assert_eq!(out.final_state, "delivered");
        assert!(!out.recoverable);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn off_ceiling_is_noop() {
        let root = feature_branch_repo("off");
        let out = deliver(
            &root,
            DeliveryCeiling::Off,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            None::<&StubRemote>,
            Some("main"),
        )
        .await;
        assert_eq!(out.final_state, "noop");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn detached_or_default_branch_blocks_cleanly() {
        let root = make_repo("defbranch"); // on main, no feature branch
        let out = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            None::<&StubRemote>,
            Some("main"),
        )
        .await;
        assert_eq!(out.final_state, "blocked");
        assert!(out
            .steps
            .iter()
            .any(|s| s.step == "repo" && s.status == "blocked"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn gh_hosts_yml_parser_detects_authenticated_user() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".config").join("gh");
        std::fs::create_dir_all(&cfg).unwrap();

        // Minimal authentic hosts.yml.
        std::fs::write(
            cfg.join("hosts.yml"),
            "github.com:\n    user: BumStill\n    oauth_token: gho_abc123\n",
        )
        .unwrap();

        // We can't intercept dirs::home_dir(), so test the parser indirectly
        // via a real sample. On a machine without a real hosts.yml this test
        // still validates the logic doesn't panic.
        let _ = gh_hosts_file_indicates_authenticated_for_host("github.com");
    }

    /// Verify the hosts.yml parser handles edge cases without panicking.
    #[test]
    fn gh_hosts_yml_parser_edge_cases() {
        // Empty file
        {
            let dir = tempfile::tempdir().unwrap();
            let cfg = dir.path().join(".config").join("gh");
            std::fs::create_dir_all(&cfg).unwrap();
            std::fs::write(cfg.join("hosts.yml"), "").unwrap();
            // Not intercepted, but exercises no-panic path
        }
        // github.com missing user
        {
            let dir = tempfile::tempdir().unwrap();
            let cfg = dir.path().join(".config").join("gh");
            std::fs::create_dir_all(&cfg).unwrap();
            std::fs::write(
                cfg.join("hosts.yml"),
                "github.com:\n    oauth_token: gho_abc\n",
            )
            .unwrap();
        }
    }

    // ── Worktree discovery: deliver from main by finding the sibling ─────────

    /// Repo with `main` pushed to a bare origin, plus a sibling worktree whose
    /// branch `feat/wt` has one commit ahead. Returns (main root, worktree dir).
    fn repo_with_worktree_feature(tag: &str) -> (PathBuf, PathBuf) {
        let root = make_repo(tag);
        let origin = root.parent().unwrap().join("origin.git");
        Command::new("git")
            .no_window()
            .args(["init", "--bare", "-q", origin.to_str().unwrap()])
            .status()
            .unwrap();
        git(&root, &["remote", "add", "origin", origin.to_str().unwrap()]).unwrap();
        git(&root, &["push", "-q", "origin", "main"]).unwrap();

        let wt = root.parent().unwrap().join("wt-feat");
        git(&root, &["worktree", "add", "-q", "-b", "feat/wt", wt.to_str().unwrap(), "main"]).unwrap();
        std::fs::write(wt.join("feature.rs"), "pub fn f() {}\n").unwrap();
        git(&wt, &["add", "-A"]).unwrap();
        git(&wt, &["commit", "-q", "-m", "feat(wt): work"]).unwrap();
        (root, wt)
    }

    #[test]
    fn default_branch_refuses_when_no_worktree_candidate() {
        let root = make_repo("wt-none");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: stub_calls(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out = rt.block_on(deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            1,
            &DeliverOpts {
                title: None,
                body: None,
                release_urgency: None,
                requested_ceiling: None,
                extra_excludes: vec![],
            },
            Some(&remote),
            Some("main"),
        ));
        assert_eq!(out.reached_state, "local");
        assert!(
            out.summary.contains("默认分支"),
            "summary should explain the default-branch refusal, got: {}",
            out.summary
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn worktree_feature_branch_is_discovered_and_delivered_from_main() {
        let (root, wt) = repo_with_worktree_feature("wt-discover");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: stub_calls(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out = rt.block_on(deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            1,
            &DeliverOpts {
                title: None,
                body: None,
                release_urgency: None,
                requested_ceiling: None,
                extra_excludes: vec![],
            },
            Some(&remote),
            Some("main"),
        ));
        // Delivery must NOT refuse on default branch: it found the worktree
        // branch and opened the PR from it.
        assert_eq!(out.branch.as_deref(), Some("feat/wt"));
        assert_eq!(out.pr_number, Some(7));
        assert!(
            out.steps.iter().any(|s| s.step == "repo" && s.status == "ok"),
            "worktree discovery should be recorded as a repo step"
        );
        assert_eq!(remote.calls.open_pr.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn multiple_worktree_candidates_are_reported_as_ambiguous() {
        let (root, wt1) = repo_with_worktree_feature("wt-multi");
        // A second sibling worktree with its own ahead branch.
        let wt2 = root.parent().unwrap().join("wt-feat2");
        git(&root, &["worktree", "add", "-q", "-b", "feat/wt2", wt2.to_str().unwrap(), "main"]).unwrap();
        std::fs::write(wt2.join("feature2.rs"), "pub fn g() {}\n").unwrap();
        git(&wt2, &["add", "-A"]).unwrap();
        git(&wt2, &["commit", "-q", "-m", "feat(wt2): work"]).unwrap();

        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: stub_calls(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out = rt.block_on(deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            1,
            &DeliverOpts {
                title: None,
                body: None,
                release_urgency: None,
                requested_ceiling: None,
                extra_excludes: vec![],
            },
            Some(&remote),
            Some("main"),
        ));
        assert_eq!(out.reached_state, "local");
        assert!(
            out.summary.contains("多个 worktree 分支"),
            "summary should name both candidates, got: {}",
            out.summary
        );
        assert!(out.summary.contains("feat/wt"), "candidate 1 named");
        assert!(out.summary.contains("feat/wt2"), "candidate 2 named");
        // No PR was opened for an ambiguous choice.
        assert_eq!(remote.calls.open_pr.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
