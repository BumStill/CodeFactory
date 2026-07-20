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

use serde::Serialize;

use crate::config::settings::{DeliveryCeiling, MergeMethod};
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
    /// Human summary the agent echoes to the user.
    pub summary: String,
}

impl DeliveryOutcome {
    fn blocked_at(mut self, step: StepResult) -> Self {
        let msg = step.detail.clone();
        self.steps.push(step);
        self.final_state = "blocked".into();
        self.summary = msg;
        self
    }
}

/// Options for a single delivery call (from the agent tool). All optional so
/// the model can invoke `deliver_changes` with no arguments in the common case.
#[derive(Debug, Clone, Default)]
pub struct DeliverOpts {
    pub title: Option<String>,
    pub body: Option<String>,
    /// A per-call ceiling; clamped to at most the user's configured ceiling.
    pub requested_ceiling: Option<DeliveryCeiling>,
    pub extra_excludes: Vec<String>,
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

/// Portable remote operations (token+REST). Implemented by `GithubRemote`;
/// stubbed in tests so the state machine is exercised without a network. Uses
/// native async-fn-in-trait with generic (static) dispatch — no `async_trait`
/// dependency, no dynamic dispatch.
pub trait DeliveryRemote {
    /// Return the existing open PR for `head`, or open a new one. Idempotent:
    /// callers rely on this never double-opening.
    fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> impl std::future::Future<Output = Result<(u64, String), String>>;
    fn ci_status(&self, sha: &str) -> impl std::future::Future<Output = Result<CiStatus, String>>;
    fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
    ) -> impl std::future::Future<Output = Result<(), String>>;
    fn trigger_release(&self) -> impl std::future::Future<Output = Result<String, String>>;
}

// ── Local git helper ────────────────────────────────────────────────────────

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .no_window()
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

/// Repo context resolved once at the start of delivery.
#[derive(Debug, Clone)]
pub struct RepoContext {
    pub root: PathBuf,
    pub branch: String,
    pub default_branch: String,
}

pub fn resolve_repo(cwd: &Path, default_branch_hint: Option<&str>) -> Result<RepoContext, String> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "not a git repository".to_string())?;
    let root = PathBuf::from(root);
    let branch = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        return Err("detached HEAD — check out a branch before delivering".into());
    }
    // Prefer the remote's default branch; fall back to a hint or common names.
    let default_branch = git(
        &root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .and_then(|s| s.rsplit('/').next().map(|s| s.to_string()))
    .or_else(|| default_branch_hint.map(|s| s.to_string()))
    .unwrap_or_else(|| "main".to_string());
    Ok(RepoContext {
        root,
        branch,
        default_branch,
    })
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
    Command::new("git")
        .no_window()
        .arg("-C")
        .arg(root)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false)
}

fn branch_is_ahead_of(root: &Path, base: &str, branch: &str) -> bool {
    // rev-list base..branch — nonzero count means the branch has commits to push.
    git(
        root,
        &["rev-list", "--count", &format!("origin/{base}..{branch}")],
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

// ── The state machine ───────────────────────────────────────────────────────

/// Run delivery up to the effective ceiling. `remote` is `None` when no git
/// remote token is configured — local steps still run and the PR step reports a
/// clear, non-looping blocker.
pub async fn deliver<R: DeliveryRemote>(
    cwd: &Path,
    configured_ceiling: DeliveryCeiling,
    merge_method: MergeMethod,
    ci_timeout_secs: u32,
    opts: &DeliverOpts,
    remote: Option<&R>,
    default_branch_hint: Option<&str>,
) -> DeliveryOutcome {
    let ceiling = match opts.requested_ceiling {
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
        summary: String::new(),
    };

    if ceiling == DeliveryCeiling::Off {
        outcome.final_state = "noop".into();
        outcome.summary = "交付已关闭(delivery_ceiling = off)。".into();
        outcome
            .steps
            .push(StepResult::skipped("policy", "delivery ceiling is Off"));
        return outcome;
    }

    // ── Resolve repo ────────────────────────────────────────────────────────
    let repo = match resolve_repo(cwd, default_branch_hint) {
        Ok(r) => r,
        Err(e) => return outcome.blocked_at(StepResult::blocked("repo", e)),
    };
    outcome.branch = Some(repo.branch.clone());
    if repo.branch == repo.default_branch {
        return outcome.blocked_at(StepResult::blocked(
            "repo",
            format!(
                "当前在默认分支 {} 上,不能从默认分支向自身开 PR;请先切到功能分支。",
                repo.default_branch
            ),
        ));
    }

    // ── Commit (noise-safe) ─────────────────────────────────────────────────
    let staged = match stage_scoped(&repo.root, &opts.extra_excludes) {
        Ok(s) => s,
        Err(e) => {
            return outcome.blocked_at(StepResult::blocked("commit", format!("暂存失败: {e}")))
        }
    };
    if has_staged_changes(&repo.root) {
        let msg = generate_commit_message(&repo.root, &repo.branch, opts.title.as_deref());
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
    if !branch_is_ahead_of(&repo.root, &repo.default_branch, &repo.branch)
        && outcome.steps.iter().all(|s| s.status == "skipped")
    {
        outcome.final_state = "noop".into();
        outcome.summary = "没有需要交付的改动。".into();
        return outcome;
    }

    // ── Push ────────────────────────────────────────────────────────────────
    match git(&repo.root, &["push", "-u", "origin", &repo.branch]) {
        Ok(_) => outcome.steps.push(StepResult::ok(
            "push",
            format!("推送 {} 到 origin", repo.branch),
        )),
        Err(e) => {
            return outcome.blocked_at(StepResult::blocked(
                "push",
                format!("推送失败: {e}。请确认已配置该远端的 git 凭据(或在设置里配置远端 token)。"),
            ))
        }
    }

    if ceiling.rank() < DeliveryCeiling::PrOnly.rank() {
        return finish(outcome, &repo.branch, ceiling);
    }

    // ── Open (or reuse) PR ──────────────────────────────────────────────────
    let Some(remote) = remote else {
        return outcome.blocked_at(StepResult::blocked("pr", NO_TOKEN_PR_MESSAGE));
    };
    let title = opts.title.clone().unwrap_or_else(|| {
        generate_commit_message(&repo.root, &repo.branch, None)
            .lines()
            .next()
            .unwrap_or(&repo.branch)
            .to_string()
    });
    let body = opts.body.clone().unwrap_or_else(|| {
        "由 CodeFactory 自动交付。\n\n🤖 Generated with CodeFactory".to_string()
    });
    let (pr_number, pr_url) = match remote
        .open_or_get_pr(&title, &body, &repo.branch, &repo.default_branch)
        .await
    {
        Ok(v) => v,
        Err(e) => return outcome.blocked_at(StepResult::blocked("pr", format!("开 PR 失败: {e}"))),
    };
    outcome.pr_number = Some(pr_number);
    outcome.pr_url = Some(pr_url.clone());
    outcome
        .steps
        .push(StepResult::ok("pr", format!("PR #{pr_number}: {pr_url}")));

    if ceiling.rank() < DeliveryCeiling::ThroughCiGreen.rank() {
        return finish(outcome, &repo.branch, ceiling);
    }

    // ── Wait for CI ─────────────────────────────────────────────────────────
    let sha = outcome.commit_sha.clone().unwrap_or_default();
    match wait_for_ci(remote, &sha, ci_timeout_secs).await {
        CiStatus::Success | CiStatus::None => outcome.steps.push(StepResult::ok("ci", "CI 通过")),
        CiStatus::Failure(d) => {
            return outcome.blocked_at(StepResult::blocked("ci", format!("CI 未通过: {d}")))
        }
        CiStatus::Pending => {
            return outcome.blocked_at(StepResult::blocked(
                "ci",
                format!("CI 在 {ci_timeout_secs}s 内仍未出结论;稍后重新调用交付即可从此处续跑。"),
            ))
        }
    }

    if ceiling.rank() < DeliveryCeiling::ThroughMerge.rank() {
        return finish(outcome, &repo.branch, ceiling);
    }

    // ── Merge ───────────────────────────────────────────────────────────────
    if let Err(e) = remote.merge_pr(pr_number, merge_method).await {
        return outcome.blocked_at(StepResult::blocked(
            "merge",
            format!("合并失败: {e}(可能受分支保护/必需评审限制)。"),
        ));
    }
    outcome.steps.push(StepResult::ok(
        "merge",
        format!("已 {} 合并 PR #{pr_number}", merge_method.as_str()),
    ));

    if ceiling.rank() < DeliveryCeiling::ThroughRelease.rank() {
        return finish(outcome, &repo.branch, ceiling);
    }

    // ── Release (deliberate) ────────────────────────────────────────────────
    match remote.trigger_release().await {
        Ok(detail) => outcome.steps.push(StepResult::ok("release", detail)),
        Err(e) => {
            return outcome.blocked_at(StepResult::blocked(
                "release",
                format!("发布触发失败: {e}(令牌可能缺少 workflow 权限)。"),
            ))
        }
    }

    finish(outcome, &repo.branch, ceiling)
}

/// Blocked-at-PR message when no remote token is configured. Carries the fix
/// path AND the model-behavior contract: surface it to the user and wait —
/// retrying deliver_changes cannot succeed until a token exists. (The app's
/// only historical deliver_changes call died exactly here.)
pub const NO_TOKEN_PR_MESSAGE: &str = "已提交并推送,但未配置远端 token,无法开 PR。\
请把这句原样告诉用户:在设置→远程仓库里为该仓库配置 GitHub 访问令牌(需 repo 权限),\
配置完成后回复继续即可自动完成 PR/CI/合并/发布。在用户确认配置完成之前,不要再调用 deliver_changes。";

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
    use crate::config::settings::GitProvider;
    if settings.delivery_ceiling == DeliveryCeiling::Off {
        return None;
    }
    let owner_repo = origin_url.and_then(parse_owner_repo)?;
    let has_github_remote = settings
        .git_remotes
        .iter()
        .any(|r| matches!(r.provider, GitProvider::Github));
    Some(if has_github_remote {
        format!(
            "\n\n# Delivery capability\n\
             Repo {owner_repo} has GitHub credentials configured; delivery ceiling = {}. \
             Code work ends by calling deliver_changes once tests are green — it carries the \
             work up to that ceiling automatically.",
            ceiling_label(settings.delivery_ceiling)
        )
    } else {
        format!(
            "\n\n# Delivery capability (BROKEN — surface early)\n\
             The delivery chain for {owner_repo} cannot open a PR: no GitHub token is \
             configured (设置→远程仓库). If this task involves delivering code, say so in your \
             FIRST reply — give the user that settings path and ask them to configure a token \
             (repo scope) — and do NOT call deliver_changes until they confirm it is configured. \
             Local work (tests, edits, commits) can proceed in the meantime."
        )
    })
}

/// Wrapper reading the cwd's `origin` URL; see [`delivery_readiness_from_origin`].
pub fn delivery_readiness_note(
    cwd: &Path,
    settings: &crate::config::settings::Settings,
) -> Option<String> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).ok()?;
    let origin = git(Path::new(&root), &["remote", "get-url", "origin"]).ok();
    delivery_readiness_from_origin(origin.as_deref(), settings)
}

fn finish(mut outcome: DeliveryOutcome, branch: &str, ceiling: DeliveryCeiling) -> DeliveryOutcome {
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
    // "Delivered" at a partial ceiling must say WHY merge/release didn't
    // happen — a bare "已交付" reads as "it stopped short again".
    if outcome.final_state == "delivered"
        && ceiling.rank() < DeliveryCeiling::ThroughRelease.rank()
    {
        outcome.summary.push_str(&format!(
            "\n已到达配置的交付上限({});未执行其后的合并/发布属预期行为。\
             要让交付自动进行到发布,请在设置→交付里把上限调整为 through_release。",
            ceiling_label(ceiling)
        ));
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
    let u = url.trim().trim_end_matches(".git");
    let after_host = if let Some(idx) = u.find("github.com") {
        &u[idx + "github.com".len()..]
    } else {
        return None;
    };
    let path = after_host.trim_start_matches([':', '/']);
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        Some(format!("{}/{}", parts[0], parts[1]))
    } else {
        None
    }
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
    let origin = git(Path::new(&root), &["remote", "get-url", "origin"]).ok()?;
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
    let default_branch = git(
        Path::new(&root),
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .and_then(|s| s.rsplit('/').next().map(String::from))
    .unwrap_or_else(|| "main".to_string());

    Some(GithubRemote {
        client,
        repo: owner_repo,
        default_branch,
        release_workflow: "auto-release.yml".to_string(),
    })
}

impl DeliveryRemote for GithubRemote {
    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<(u64, String), String> {
        // Idempotency: reuse an existing open PR for this head branch.
        if let Ok(prs) = crate::git_remote::github::list_prs(&self.client, &self.repo, "open").await
        {
            if let Some(pr) = prs.into_iter().find(|p| p.head_branch == head) {
                return Ok((pr.number, pr.url));
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
        Ok((pr.number, pr.url))
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

    async fn merge_pr(&self, number: u64, method: MergeMethod) -> Result<(), String> {
        crate::git_remote::github::merge_pr(&self.client, &self.repo, number, method.as_str()).await
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
        // The app's only historical deliver_changes call blocked exactly here
        // (git_remotes was never configured). The message must carry the fix
        // path AND forbid blind retries — retrying cannot succeed until the
        // user configures a token.
        assert!(NO_TOKEN_PR_MESSAGE.contains("设置→远程仓库"));
        assert!(NO_TOKEN_PR_MESSAGE.contains("不要再调用 deliver_changes"));
    }

    #[test]
    fn readiness_note_warns_before_work_when_github_origin_has_no_token() {
        // The broken chain must be surfaced in the model's FIRST reply, not
        // discovered after the work is done.
        let settings = crate::config::settings::Settings::default();
        let note = delivery_readiness_from_origin(
            Some("git@github.com:BumStill/CodeFactory.git"),
            &settings,
        )
        .expect("github origin without a token must produce a warning note");
        assert!(note.contains("FIRST reply"));
        assert!(note.contains("设置→远程仓库"));
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
                token_ref: None,
                token: "t".into(),
                default_repo: Some("BumStill/CodeFactory".into()),
            });
        let note = delivery_readiness_from_origin(
            Some("https://github.com/BumStill/CodeFactory.git"),
            &settings,
        )
        .expect("configured remote must produce a capability note");
        assert!(note.contains("pr_only"));
        assert!(note.contains("deliver_changes"));
    }

    #[test]
    fn readiness_note_stays_silent_when_off_or_not_github() {
        let settings = crate::config::settings::Settings::default();
        assert!(delivery_readiness_from_origin(None, &settings).is_none());
        assert!(
            delivery_readiness_from_origin(Some("https://gitlab.com/x/y.git"), &settings)
                .is_none()
        );

        let mut off = crate::config::settings::Settings::default();
        off.delivery_ceiling = DeliveryCeiling::Off;
        assert!(delivery_readiness_from_origin(
            Some("https://github.com/BumStill/CodeFactory.git"),
            &off
        )
        .is_none());
    }

    #[test]
    fn delivered_summary_names_the_ceiling_boundary() {
        // "Delivered" at pr_only must say WHY there was no merge/release and
        // how to raise the ceiling — the user reads a bare "已交付" as "it
        // stopped short again".
        let outcome = DeliveryOutcome {
            steps: vec![StepResult::ok("pr", "opened")],
            branch: Some("b".into()),
            commit_sha: None,
            pr_url: Some("https://github.com/x/y/pull/1".into()),
            pr_number: Some(1),
            final_state: "delivered".into(),
            summary: String::new(),
        };
        let done = finish(outcome.clone(), "b", DeliveryCeiling::PrOnly);
        assert!(done.summary.contains("交付上限"));
        assert!(done.summary.contains("through_release"));

        let full = finish(outcome, "b", DeliveryCeiling::ThroughRelease);
        assert!(!full.summary.contains("交付上限"));
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
    }
    impl DeliveryRemote for StubRemote {
        async fn open_or_get_pr(
            &self,
            _t: &str,
            _b: &str,
            _h: &str,
            _base: &str,
        ) -> Result<(u64, String), String> {
            Ok(self
                .existing_pr
                .clone()
                .unwrap_or((7, "https://example/pr/7".into())))
        }
        async fn ci_status(&self, _sha: &str) -> Result<CiStatus, String> {
            Ok(self.ci.clone())
        }
        async fn merge_pr(&self, _n: u64, _m: MergeMethod) -> Result<(), String> {
            if self.merge_ok {
                Ok(())
            } else {
                Err("protected branch".into())
            }
        }
        async fn trigger_release(&self) -> Result<String, String> {
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
        git(&root, &["remote", "add", "origin", origin.to_str().unwrap()]).unwrap();
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
    async fn through_merge_stops_when_ci_fails() {
        let root = feature_branch_repo("cifail");
        let remote = StubRemote {
            ci: CiStatus::Failure("build red".into()),
            existing_pr: None,
            merge_ok: true,
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
    async fn no_remote_configured_blocks_at_pr_after_local_push() {
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
        // Local steps still succeeded; only the PR step is blocked.
        assert!(out
            .steps
            .iter()
            .any(|s| s.step == "commit" && s.status == "ok"));
        assert!(out
            .steps
            .iter()
            .any(|s| s.step == "push" && s.status == "ok"));
        assert!(out
            .steps
            .iter()
            .any(|s| s.step == "pr" && s.status == "blocked"));
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
}
