// SPDX-License-Identifier: Apache-2.0
//! Thin delivery: no durable identities, receipts, event stream, or CI wait.
use serde_json::{json, Value};
use std::path::Path;
use crate::agent::delivery;
use crate::agent::delivery::ReleaseUrgency;
use super::{ExecCtx, ToolOutput};
use crate::config::settings::DeliveryCeiling;

trait Commands {
    fn git(&mut self, args: &[&str]) -> Result<String, String>;
    fn gh(&mut self, args: &[&str]) -> Result<String, String>;
    fn stage(&mut self, excludes: &[String]) -> Result<Vec<String>, String>;
    /// CF-STAGE1-P1: the repository PR-body contract. Applied after the local
    /// commit and before the first push, so a body CI would reject is never
    /// pushed. Default passthrough keeps synthetic forges in memory.
    fn precheck_governance(&mut self) -> Result<(Vec<String>, u64), String> {
        Ok((Vec::new(), 0))
    }
    fn reconcile_body(&mut self, _title: &str, body: &str, urgency: Option<ReleaseUrgency>) -> Result<String, String> {
        Ok(delivery::append_release_urgency(body.to_string(), urgency))
    }
}

/// Production adapter only. Synthetic command fixtures cannot manufacture this
/// host-owned proof, and the proof is never read from tool arguments.
struct Local<'a> {
    root: &'a Path,
    scope: Option<&'a delivery::TaskDeliveryScope>,
    staged: bool,
    installed_head: Option<String>,
    stop: Option<(tokio::runtime::Handle, std::sync::Arc<dyn crate::agent::stop_fence::DeliveryStopGate>)>,
}
impl Local<'_> {
    fn check_stop(&self) -> Result<(), String> {
        if let Some((runtime, gate)) = &self.stop {
            if let Some(reason) = runtime.block_on(gate.stop_reason()) {
                return Err(format!("DELIVERY_STOPPED: {reason}"));
            }
        }
        Ok(())
    }
    fn dispatch_forge(&self, args: &[&str], dispatch: impl FnOnce() -> Result<String, String>) -> Result<String, String> {
        let read_only = matches!(args, ["pr", "list", ..] | ["pr", "view", ..] | ["pr", "checks", ..]
            | ["run", "list", ..] | ["run", "view", ..] | ["release", "view", ..]);
        if !read_only { self.verify_current()?; self.check_stop()?; }
        dispatch()
    }

    fn proof(&self) -> Result<&delivery::TaskDeliveryScope, String> {
        self.scope.ok_or_else(|| "TASK_OWNERSHIP_UNPROVEN: host proof is missing; no delivery mutation performed".into())
    }
    fn verify_current(&self) -> Result<(), String> {
        if let Some(expected) = self.installed_head.as_deref() {
            if delivery::git(self.root, &["rev-parse", "HEAD"])? != expected {
                return Err("installed hook commit HEAD changed before thin mutation".into());
            }
        }
        if self.staged { self.proof()?.verify_materialized(self.root) }
        else { self.proof()?.verify(self.root) }
    }
}
impl Commands for Local<'_> {
    fn git(&mut self, args: &[&str]) -> Result<String, String> {
        if args.first() == Some(&"commit") {
            return Err("TASK_DELIVERY_SCOPE_UNPROVEN: thin 自动提交尚无保留正常 hooks 的固定树原子事务；未提交、推送或覆盖暂存。".into());
        }
        let mutation = args.first() == Some(&"push");
        if mutation {
            self.verify_current()?;
            let expected = self.installed_head.as_ref().map(|head| format!("{head}:refs/heads/{}", self.proof().expect("proof checked").proven_branch()))
                .unwrap_or_else(|| self.proof().expect("proof checked").proven_push_refspec());
            let refspec = args.last().copied().unwrap_or_default();
            if refspec != expected {
                return Err("TASK_DELIVERY_SCOPE_UNPROVEN: push 必须使用已验证的固定提交 SHA 与目标分支，不能解析可变 HEAD 或更换目标。".into());
            }
        }
        if mutation { self.check_stop()?; }
        let result = delivery::git(self.root, args)?;
        if mutation { self.verify_current()?; }
        Ok(result)
    }
    fn gh(&mut self, args: &[&str]) -> Result<String, String> {
        self.dispatch_forge(args, || delivery::bounded_delivery_output(delivery::dev_command("gh").current_dir(self.root).args(args)))
    }

    fn stage(&mut self, excludes: &[String]) -> Result<Vec<String>, String> {
        let proof = self.proof()?;
        if self.installed_head.is_some() {
            proof.verify_materialized(self.root)?;
            return Ok(Vec::new());
        }
        proof.verify(self.root)?;
        if proof.requires_commit() {
            // Do not stage first and fail later: Git's ordinary commit reads a
            // mutable index, while commit-tree would bypass normal hooks. Until
            // thin has a guarded fixed-tree commit, preserve every original byte.
            return Err("TASK_DELIVERY_SCOPE_UNPROVEN: thin 自动提交尚无保留正常 hooks 的固定树原子事务；工作区和原始 index 保持不变，未提交或推送。".into());
        }
        let _ = excludes; // No files are staged for an already-committed tree.
        self.staged = true;
        self.verify_current()?;
        Ok(Vec::new())
    }
    fn precheck_governance(&mut self) -> Result<(Vec<String>, u64), String> {
        delivery::precheck_thin_governance(self.root)
    }
    fn reconcile_body(&mut self, title: &str, body: &str, urgency: Option<ReleaseUrgency>) -> Result<String, String> {
        delivery::prepare_thin_pr_body(self.root, title, body, urgency)
    }
}

#[derive(Default)]
struct Log { gh_calls: usize, steps: Vec<Value>, branch: String, pr_number: Option<u64> }
impl Log {
    fn gh(&mut self, commands: &mut impl Commands, args: &[&str]) -> Result<String, String> {
        self.gh_calls += 1;
        commands.gh(args)
    }
    fn step(&mut self, step: &str, result: &str) { self.steps.push(json!({"step":step,"result":result})); }
}

/// M132: a title that only restates the branch says nothing about the change.
/// The field defect was `fix: codefactory/objective-<hex>` reaching `main` and the
/// release notes because nothing refused it.
fn echoes_branch(title: &str, branch: &str) -> bool {
    let claim = title
        .split_once(':')
        .map_or(title.trim(), |(_, rest)| rest.trim());
    let branch = branch.trim();
    let claim_lower = claim.to_ascii_lowercase();
    claim.is_empty()
        || (!branch.is_empty() && (claim == branch || claim.contains(branch)))
        || claim.contains("codefactory/objective")
        || claim_lower.starts_with("objective ")
        || claim_lower.starts_with("objective-")
}

/// A Conventional Commit subject split into its kind, the markers that follow it
/// (`(scope)` and `!`) and the description the title talks about.
fn split_subject(title: &str) -> Option<(&str, &str, &str)> {
    let (head, description) = title.trim().split_once(':')?;
    let description = description.trim();
    if description.is_empty() {
        return None;
    }
    let head = head.trim();
    let kind_end = head.find(['(', '!']).unwrap_or(head.len());
    let kind = &head[..kind_end];
    matches!(
        kind,
        "feat" | "fix" | "chore" | "docs" | "refactor" | "test" | "ci" | "build" | "perf" | "style"
    )
    .then_some((kind, &head[kind_end..], description))
}

/// Conventional Commit subjects are what this repository releases from; a title
/// without one gives the squash subject and the release note nothing to work with.
fn conventional_subject(title: &str) -> bool {
    split_subject(title).is_some()
}

fn usable_title(title: &str, branch: &str) -> bool {
    let title = title.trim();
    let claim = title
        .split_once(':')
        .map_or(title, |(_, rest)| rest.trim());
    conventional_subject(title)
        && !echoes_branch(title, branch)
        && !delivery::is_managed_objective_placeholder_title(claim)
}

/// The branch's own commits, newest first — the fallback description of the work.
fn recent_subjects(commands: &mut impl Commands) -> Vec<String> {
    commands
        .git(&["log", "--format=%s", "-n", "20"])
        .map(|log| {
            log.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// M171 — the type a PR title carries decides whether the batch ships at all:
/// release cadence only starts a version from `feat`/`fix`. Taking it from the
/// newest commit let a repair whose last commit was `test:` reach main as a test
/// change (#854, #858, #828), so the type now comes from the whole change.
/// The revision the branch forked from: a branch-scoped range, never `main`'s own
/// history — a `feat:` commit on main must not turn this branch's repair into one.
fn base_revision(commands: &mut impl Commands) -> String {
    let default_branch = commands
        .git(&["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .and_then(|s| s.rsplit('/').next().map(|b| format!("origin/{b}")))
        .unwrap_or_else(|| "origin/main".to_string());
    commands
        .git(&["merge-base", &default_branch, "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(default_branch)
}

/// Everything this delivery changes: committed on the branch, staged, unstaged,
/// and untracked. Only the union can say what the PR really is.
fn changed_files(commands: &mut impl Commands) -> Vec<String> {
    let base = base_revision(commands);
    let mut files: Vec<String> = commands
        .git(&["diff", "--name-only", &base])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if let Ok(untracked) = commands.git(&["ls-files", "--others", "--exclude-standard"]) {
        files.extend(
            untracked
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
    }
    files.sort();
    files.dedup();
    files
}

fn is_doc_path(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    path.starts_with("docs/") || path.ends_with(".md")
}

/// Product code is anything a person can observe except the test suite and the
/// docs — configuration, scripts and manifests included, so the conservative
/// answer for them is `fix`, not a prefix that never releases.
fn is_test_path(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    path.starts_with("tests/") || path.starts_with("test/") || path.contains("/tests/")
}

/// An explicit `feat:`/`feat(scope):` declaration — in the branch's own commits
/// or in the diff itself (a spec written by this delivery) — is the only signal
/// that a change adds capability rather than repairing one.
fn declares_feature(commands: &mut impl Commands, subjects: &[String]) -> bool {
    if subjects
        .iter()
        .any(|subject| split_subject(subject).is_some_and(|(kind, _, _)| kind == "feat"))
    {
        return true;
    }
    let base = base_revision(commands);
    commands
        .git(&["diff", &base])
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim_start_matches(['+', '-']).trim())
        .any(|line| line.starts_with("feat:") || line.starts_with("feat("))
}

fn derived_kind(commands: &mut impl Commands, subjects: &[String], fallback: &str) -> String {
    let files = changed_files(commands);
    if files.iter().any(|path| !is_doc_path(path) && !is_test_path(path)) {
        return if declares_feature(commands, subjects) { "feat" } else { "fix" }.to_string();
    }
    if files.is_empty() {
        return fallback.to_string();
    }
    if files.iter().all(|path| is_doc_path(path)) { "docs" } else { "test" }.to_string()
}

/// Keep the description and the `(scope)`/`!` markers; replace only a type that
/// the actual change contradicts.
fn retype(title: &str, kind: &str) -> String {
    match split_subject(title) {
        Some((current, markers, description)) if current != kind => {
            format!("{kind}{markers}: {description}")
        }
        _ => title.trim().to_string(),
    }
}

/// An already-open PR may carry a title a human already corrected; never lose it.
fn open_pr_title(commands: &mut impl Commands, branch: &str) -> Option<String> {
    let listed = commands
        .gh(&["pr", "list", "--head", branch, "--state", "open", "--json", "number,title,state"])
        .ok()?;
    let prs: Vec<Value> = serde_json::from_str(&listed).ok()?;
    prs.iter()
        .find(|pr| pr["state"] == "OPEN")
        .and_then(|pr| pr["title"].as_str())
        .map(str::to_owned)
}

/// Resolve the PR title before anything is committed or pushed: the model's own
/// title, then this branch's first meaningful commit subject, then a title a
/// human already fixed on the open PR. The description comes from that source,
/// the type from what the whole delivery actually changes. Nothing usable means no
/// delivery at all.
fn resolve_title(commands: &mut impl Commands, args: &Value, branch: &str) -> Result<String, String> {
    let subjects = recent_subjects(commands);
    let chosen = args
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| usable_title(title, branch))
        .map(str::to_owned)
        .or_else(|| subjects.iter().find(|s| usable_title(s, branch)).cloned())
        .or_else(|| open_pr_title(commands, branch).filter(|title| usable_title(title, branch)));
    let Some(chosen) = chosen else {
        return Err(format!(
            "deliver_changes 缺少可用的 PR 标题：调用没有给 title，分支 {branch} 上也没有能用作标题的提交说明。\
请用 title 传一句描述本次改动的 Conventional Commit 标题（如 fix: 修复标题回落成分支名），\
本次未提交、未推送任何内容。"
        ));
    };
    // M171: the newest commit may be `test:` while the change is a repair.
    let kind = derived_kind(commands, &subjects, split_subject(&chosen).map(|(kind, _, _)| kind).unwrap_or("fix"));
    Ok(retype(&chosen, &kind))
}

fn preflight(commands: &mut impl Commands, args: &Value, ceiling: DeliveryCeiling) -> Result<Option<String>, String> {
    if ceiling == DeliveryCeiling::Off { return Ok(None); }
    let branch = commands.git(&["branch", "--show-current"])?;
    if branch.is_empty() { return Err("detached HEAD cannot be delivered".into()); }
    if args.get("expect_branch").and_then(Value::as_str).is_some_and(|expected| expected != branch) {
        return Err(format!("expect_branch 不匹配当前 objective 的交付分支 {branch}；未执行提交或推送。系统自动重试/自愈：在登记工作区核对 git status 和 git branch --show-current，使用 expect_branch={branch} 重试 deliver_changes；不得切回上一单已合并分支。"));
    }
    Ok(Some(branch))
}

fn run(commands: &mut impl Commands, args: &Value, ceiling: DeliveryCeiling, excludes: &[String], log: &mut Log) -> Result<Value, String> {
    let Some(branch) = preflight(commands, args, ceiling)? else { return Ok(json!({"state":"noop"})); };
    log.branch = branch.clone();
    // M132: resolve the title first — a delivery that cannot describe itself must
    // not write `fix: <branch>` into the commit history or onto the PR.
    let title = resolve_title(commands, args, &branch)?;
    // M147b: the urgency has to exist before the commit is written. The PR body
    // is rewritable (nobody can stop a later `gh pr edit --body`), the commit
    // message is not — so the trailer is persisted here as the durable copy the
    // merge queue falls back to when the body carries none.
    let urgency = super::delivery::release_urgency_from_args(args)?;
    let commit_message = delivery::append_release_urgency(title.clone(), urgency);
    let paths = commands.stage(excludes)?;
    if !paths.is_empty() {
        commands.git(&["commit", "-m", &commit_message])?;
        log.step("commit", "created");
    } else { log.step("commit", "unchanged"); }
    let sha = commands.git(&["rev-parse", "HEAD"])?;
    // The committed tree is now the thing the body describes; derive the required
    // declarations and refuse a body CI would reject before touching the remote.
    let requested_body = args.get("body").and_then(Value::as_str).unwrap_or("");
    let (warnings, elapsed_ms) = commands.precheck_governance()?;
    log.step("governance_precheck", &format!("{elapsed_ms} ms"));
    let mut checked_body = requested_body.to_string();
    if !warnings.is_empty() {
        let warning = format!("治理预检警告（校验器不可用，未执行部分校验）：\n{}", warnings.join("\n"));
        log.step("governance_warning", &warning);
        checked_body.push_str(&format!("\n\n{warning}\n"));
    }
    let body = commands.reconcile_body(&title, &checked_body, urgency)?;
    log.step("pr_body", if body == requested_body { "unchanged" } else { "reconciled" });
    let remote_ref = format!("refs/heads/{branch}");
    let remote = commands.git(&["ls-remote", "--heads", "origin", &remote_ref])?;
    let remote_sha = remote.split_whitespace().next();
    if remote_sha != Some(sha.as_str()) {
        if let Some(remote_sha) = remote_sha {
            commands.git(&["fetch", "origin", &branch])?;
            commands.git(&["merge-base", "--is-ancestor", remote_sha, "HEAD"])
                .map_err(|_| "remote branch diverged; refusing force push".to_string())?;
        }
        commands.git(&["push", "--set-upstream", "origin", &format!("{sha}:{remote_ref}")])?;
        log.step("push", "pushed");
    } else { log.step("push", "unchanged"); }
    let listed = log.gh(commands, &["pr", "list", "--head", &branch, "--state", "all", "--json", "number,url,title,body,state,headRefOid"])?;
    let prs: Vec<Value> = serde_json::from_str(&listed).map_err(|e| e.to_string())?;
    let existing = prs.iter().find(|pr| pr["state"] == "OPEN")
        .or_else(|| prs.iter().find(|pr| pr["state"] == "MERGED" && pr["headRefOid"].as_str() == Some(sha.as_str())));
    if let Some(pr) = existing.filter(|pr| pr["state"] == "MERGED") {
        log.pr_number = pr["number"].as_u64();
        log.step("pr", "already merged");
        return Ok(json!({"state":"delivered", "pr_url":pr["url"], "pr_number":pr["number"], "branch":branch, "commit_sha":sha}));
    }
    let pr = if let Some(pr) = existing {
        let number = pr["number"].as_u64().ok_or("PR number missing")?.to_string();
        // M132 R3: update only with a title that says something, so a re-delivery
        // can never turn a usable PR title back into the branch name.
        let keep_title = !usable_title(&title, &branch)
            && pr["title"].as_str().is_some_and(|existing| usable_title(existing, &branch));
        let title = if keep_title {
            pr["title"].as_str().unwrap_or_default().to_string()
        } else {
            title
        };
        if pr["title"].as_str() != Some(title.as_str()) || pr["body"].as_str() != Some(body.as_str()) {
            log.gh(commands, &["pr", "edit", &number, "--title", &title, "--body", &body])?;
        }
        log.step("pr", "reused");
        pr.clone()
    } else {
        log.gh(commands, &["pr", "create", "--head", &branch, "--title", &title, "--body", &body])?;
        let raw = log.gh(commands, &["pr", "view", &branch, "--json", "number,url"])?;
        log.step("pr", "created");
        serde_json::from_str(&raw).map_err(|e| e.to_string())?
    };
    let number = pr["number"].as_u64().ok_or("PR number missing")?;
    log.pr_number = Some(number);
    let number_arg = number.to_string();
    if ceiling.rank() >= DeliveryCeiling::ThroughMerge.rank() {
        let raw = log.gh(commands, &["pr", "view", &number_arg, "--json", "labels"])?;
        let labels: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if !labels["labels"].as_array().is_some_and(|labels| labels.iter().any(|l| l["name"] == "merge-queue: armed")) {
            log.gh(commands, &["pr", "edit", &number_arg, "--add-label", "merge-queue: armed"])?;
            log.step("queue", "armed");
        } else { log.step("queue", "already armed"); }
        // M126: GitHub only emits the `labeled` webhook when a label is NEW, so
        // re-arming an already-armed PR woke nothing — "arm once CI went green"
        // left the queue asleep until its next cron tick (2026-10-20 #768 sat
        // idle 20+ min on a CLEAN, armed PR). Dispatch the queue workflow so
        // "armed" means "the queue runs now". Best effort: a token without
        // actions:write keeps the scheduled sweep in charge, and the arm itself
        // already succeeded, so a refused dispatch must not fail delivery.
        match log.gh(commands, &["workflow", "run", "merge-queue.yml"]) {
            Ok(_) => log.step("queue", "dispatched"),
            Err(_) => log.step("queue", "dispatch unavailable"),
        }
    }
    let ci = log.gh(commands, &["pr", "view", &number_arg, "--json", "state,statusCheckRollup"])?;
    let ci: Value = serde_json::from_str(&ci).map_err(|e| e.to_string())?;
    log.step("ci", "observed once");
    let ceiling_note = args.get("ceiling").and_then(Value::as_str)
        .and_then(DeliveryCeiling::from_label)
        .filter(|requested| requested.rank() < ceiling.rank())
        .map(|_| format!("按本会话授权继续交付至 {}，不在较低的请求 ceiling 停止。", ceiling.label()));
    Ok(json!({"state":"waiting", "pr_url":pr["url"], "pr_number":number,
        "effective_ceiling":ceiling.label(), "ceiling_note":ceiling_note,
        "branch":branch,"commit_sha":sha,"ci":ci,
        // M149 R3：M142 起系统对已交付 PR 低频心跳监视（≥180 秒）。结果必须说出来，
        // 否则模型会去自建监视源（`gh --watch` 被拒后本回合就没有唤醒源）。
        "session_state":"waiting_wake",
        "system_watch":{"source":"system","pr_number":number,"min_interval_secs":180},
        "next_action":format!(
            "系统已在监视 PR #{number}（心跳间隔 ≥180 秒）：CI 失败、合并冲突、被合并队列退回或\
             合并成功时都会自动唤醒本会话，你可以结束本回合等待唤醒。不要自己轮询\
             （`gh --watch` 会被拒绝，也不需要 schedule_wakeup）；合并由合并队列完成，\
             不要直接合并。被唤醒后需要动作时再调用 deliver_changes。"
        )}))
}

fn session_release_args(args: &Value, session_urgency: &str) -> Value {
    let mut effective = args.clone();
    // Missing/null cannot erase the persisted immediate policy. Invalid explicit
    // strings remain errors in the governed argument parser.
    if effective.get("release_urgency").is_none_or(Value::is_null) {
        if matches!(session_urgency, "immediate" | "hold") {
            effective["release_urgency"] = json!(session_urgency);
        } else { effective.as_object_mut().map(|args| args.remove("release_urgency")); }
    }
    // A persisted hold is a structured user decision, not an agent-created gate.
    if session_urgency == "hold" && effective["release_urgency"] == "hold" {
        effective["hold_requested_by_user"] = json!("release_urgency: hold (session setting)");
    }
    effective
}

pub(super) async fn prepared_commit_message(args: &Value, ctx: &ExecCtx, scope: &delivery::TaskDeliveryScope) -> crate::errors::Result<String> {
    let mut effective = args.clone();
    if let (Some(db), Some(session)) = (&ctx.db, &ctx.session_id) {
        let urgency: String = sqlx::query_scalar("SELECT release_urgency FROM sessions WHERE id=?")
            .bind(session).fetch_one(db).await?;
        effective = session_release_args(args, &urgency);
    }
    let mut local = Local { root: &ctx.cwd, scope: Some(scope), staged: false, installed_head: None, stop: None };
    let branch = preflight(&mut local, &effective, DeliveryCeiling::PrOnly)
        .map_err(crate::errors::AppError::Other)?.ok_or_else(|| crate::errors::AppError::Other("local commit preflight is disabled".into()))?;
    let title = resolve_title(&mut local, &effective, &branch).map_err(crate::errors::AppError::Other)?;
    let urgency = super::delivery::release_urgency_from_args(&effective).map_err(crate::errors::AppError::Other)?;
    Ok(delivery::append_release_urgency(title, urgency))
}

pub(super) async fn execute(
    args: &Value, ctx: &ExecCtx, ceiling: DeliveryCeiling, excludes: &[String],
    scope: &delivery::TaskDeliveryScope, installed_head: Option<String>,
    gate: std::sync::Arc<dyn crate::agent::stop_fence::DeliveryStopGate>,
) -> crate::errors::Result<ToolOutput> {
    let mut effective_args = args.clone();
    if let (Some(db), Some(session)) = (&ctx.db, &ctx.session_id) {
        let urgency: String = sqlx::query_scalar("SELECT release_urgency FROM sessions WHERE id=?")
            .bind(session).fetch_one(db).await?;
        effective_args = session_release_args(args, &urgency);
    }
    let runtime = tokio::runtime::Handle::current();
    let root = ctx.cwd.clone();
    let scope = scope.clone();
    let excludes = excludes.to_vec();
    // All synchronous commands run outside the async runtime worker. Each
    // actual write re-reads the existing persistent stop authorities.
    let (result, log) = tokio::task::spawn_blocking(move || {
        let mut log = Log::default();
        let mut local = Local { root: &root, scope: Some(&scope), staged: installed_head.is_some(), installed_head, stop: Some((runtime, gate)) };
        let result = run(&mut local, &effective_args, ceiling, &excludes, &mut log);
        (result, log)
    }).await.map_err(|error| crate::errors::AppError::Other(format!("thin delivery worker failed: {error}")))?;
    if let (Some(db), Some(session), Some(number)) = (&ctx.db, &ctx.session_id, log.pr_number) {
        sqlx::query("INSERT INTO thin_delivery_refs (session_id,branch,pr_number,ceiling) VALUES (?,?,?,?) ON CONFLICT(session_id) DO UPDATE SET branch=excluded.branch,pr_number=excluded.pr_number,ceiling=excluded.ceiling")
            .bind(session).bind(&log.branch).bind(number as i64).bind(format!("{ceiling:?}")).execute(db).await?;
    }
    let mut metadata = json!({"delivery_mode":"thin","gh_calls":log.gh_calls,"steps":log.steps});
    let output = match result {
        Ok(value) => {
            metadata["delivery"] = value.clone();
            if value["state"] == "waiting" { ToolOutput::waiting(value.to_string()) }
            else { ToolOutput::ok(value.to_string()) }
        }
        Err(error) => { metadata["error"] = json!(error); ToolOutput::err(error) }
    };
    tracing::info!(session_id=?ctx.session_id, evidence=%metadata, "thin_delivery");
    Ok(output.with_metadata(metadata))
}

#[cfg(test)]
pub(super) fn task_scope_stage_fixture(
    cwd: &Path, scope: &delivery::TaskDeliveryScope,
) -> Result<Vec<String>, String> {
    Local { root: cwd, scope: Some(scope), staged: false, installed_head: None, stop: None }.stage(&[])
}

#[cfg(test)]
pub(super) fn task_scope_native_fixture(
    cwd: &Path, scope: &delivery::TaskDeliveryScope, args: &[&str],
) -> Result<String, String> {
    Local { root: cwd, scope: Some(scope), staged: true, installed_head: None, stop: None }.git(args)
}

#[cfg(test)]
pub(super) fn m232_deliver_fixture(cwd: &Path) -> Value {
    tests::deliver_real_git_fixture(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ownership_missing_proof_preserves_thin_index_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        delivery::git(root, &["init", "-q"]).unwrap();
        delivery::git(root, &["config", "user.name", "Synthetic"]).unwrap();
        delivery::git(root, &["config", "user.email", "synthetic@example.invalid"]).unwrap();
        std::fs::write(root.join("src.rs"), "base\n").unwrap();
        delivery::git(root, &["add", "src.rs"]).unwrap();
        delivery::git(root, &["commit", "-qm", "synthetic baseline"]).unwrap();
        let head = delivery::git(root, &["rev-parse", "HEAD"]).unwrap();
        let index = std::fs::read(root.join(".git/index")).unwrap();
        std::fs::write(root.join("src.rs"), "requested change\n").unwrap();
        std::fs::write(root.join("t1.py"), "unrelated source\n").unwrap();
        let result = Local { root, scope: None, staged: false, installed_head: None, stop: None }.stage(&[]);
        assert!(result.is_err(), "missing host proof must refuse staging: {result:?}");
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(delivery::git(root, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(std::fs::read_to_string(root.join("src.rs")).unwrap(), "requested change\n");
        assert_eq!(std::fs::read_to_string(root.join("t1.py")).unwrap(), "unrelated source\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn r2_thin_dirty_commit_runs_normal_hook_and_installs_exact_child() {
        use std::os::unix::fs::PermissionsExt;
        let (db, temp, root, objective_id) = delivery::r2_production_native_fixture().await;
        let common = delivery::git(&root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).unwrap();
        let hook = std::path::PathBuf::from(&common).join("hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\nset -eu\ngit symbolic-ref -q HEAD >/dev/null\nprintf hook-ran > \"$(git rev-parse --git-common-dir)/r2-hook-ran\"\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let scope = delivery::TaskDeliveryScope::load(&db, &objective_id, &root, &[]).await.unwrap();
        let parent = delivery::git(&root, &["rev-parse", "HEAD"]).unwrap();
        let origin_refs = delivery::git(&temp.path().join("origin.git"), &["show-ref"]).unwrap();
        let installed = super::super::delivery::task_scope_thin_commit_fixture(&db, &objective_id, &root, &scope)
            .await.expect("proven dirty content must commit through the durable hook transaction");
        let mut local = Local { root: &root, scope: Some(&scope), staged: true, installed_head: Some(installed), stop: None };
        assert!(local.stage(&[]).unwrap().is_empty(), "normal thin flow must not commit the child twice");
        assert_eq!(std::fs::read_to_string(std::path::PathBuf::from(common).join("r2-hook-ran")).unwrap(), "hook-ran");
        assert_eq!(delivery::git(&root, &["rev-parse", "HEAD^"]).unwrap(), parent);
        assert_eq!(delivery::git(&root, &["ls-tree", "-r", "--name-only", "HEAD"]).unwrap(), "base.txt\nowned.txt");
        assert_eq!(delivery::git(&root, &["show", "HEAD:base.txt"]).unwrap(), "owned edit");
        assert_eq!(delivery::git(&root, &["show", "HEAD:owned.txt"]).unwrap(), "owned new content");
        assert!(delivery::git(&root, &["status", "--porcelain"]).unwrap().is_empty());
        assert_eq!(delivery::git(&temp.path().join("origin.git"), &["show-ref"]).unwrap(), origin_refs);
        let owner: (Option<String>, String, i64) = sqlx::query_as("SELECT delivery_run_id, status, revision FROM objectives WHERE id=?")
            .bind(&objective_id).fetch_one(&db).await.unwrap();
        assert_eq!(owner, (None, "active".into(), 1), "local completion must atomically hand back the pointer without completing or advancing the Objective");
        let commits: i64 = sqlx::query_scalar("SELECT count(*) FROM delivery_mutation_intents WHERE status='committed' AND rung IN ('git_hook_commit_prepare','git_local_commit')")
            .fetch_one(&db).await.unwrap();
        assert_eq!(commits, 2);
    }

    include!("delivery_hook_commit_tests.rs");

    pub(super) fn deliver_real_git_fixture(cwd: &Path) -> Value {
        struct Fixture<'a> { cwd: &'a Path, forge: Forge }
        impl Commands for Fixture<'_> {
            fn git(&mut self, args: &[&str]) -> Result<String, String> {
                delivery::git(self.cwd, args)
            }
            fn gh(&mut self, args: &[&str]) -> Result<String, String> {
                self.forge.gh(args)
            }
            fn stage(&mut self, _: &[String]) -> Result<Vec<String>, String> {
                delivery::git(self.cwd, &["add", "second.txt"])?;
                Ok(vec!["second.txt".into()])
            }
        }
        let mut fixture = Fixture { cwd, forge: Forge { number: Some(43),
            branch: delivery::git(cwd, &["branch", "--show-current"]).unwrap(), ..Default::default() } };
        let mut log = Log::default();
        let result = run(&mut fixture, &json!({"title":"fix: deliver second task", "body":"Synthetic second task"}),
            DeliveryCeiling::PrOnly, &[], &mut log).unwrap();
        assert_eq!(fixture.forge.creates, 1);
        assert!(fixture.forge.edited_body.is_none(), "old PR must not be edited");
        assert_eq!(fixture.forge.merges, 0);
        assert_eq!(result["branch"], delivery::git(cwd, &["branch", "--show-current"]).unwrap());
        let sha = delivery::git(cwd, &["rev-parse", "HEAD"]).unwrap();
        let remote = delivery::git(cwd, &["ls-remote", "--heads", "origin", &format!("refs/heads/{}", log.branch)]).unwrap();
        assert_eq!(remote.split_whitespace().next(), Some(sha.as_str()));
        result
    }
    #[test]
    fn m232_wrong_branch_has_actionable_retry_before_side_effects() {
        let mut forge = Forge::default();
        let error = run(&mut forge, &json!({"expect_branch":"previous"}),
            DeliveryCeiling::PrOnly, &[], &mut Log::default()).unwrap_err();
        assert!(error.contains("expect_branch=feature") && error.contains("重试 deliver_changes"), "{error}");
        assert_eq!(forge.pushes, 0);
        assert_eq!(forge.creates, 0);
        assert!(forge.events.is_empty());
    }

    #[test]
    fn m137_session_policy_reaches_created_and_updated_pr_bodies() {
        for policy in ["immediate", "default", "hold"] {
            for existing in [false, true] {
                let args = session_release_args(&json!({"title":"fix: persist release policy", "body":"Synthetic summary"}), policy);
                let mut forge = Forge { pr: existing, ..Default::default() };
                let mut log = Log::default();
                run(&mut forge, &args, DeliveryCeiling::ThroughMerge, &[], &mut log).unwrap();
                let actual = if existing { forge.edited_body.unwrap() } else { forge.created_body.unwrap() };
                if policy == "default" { assert!(!actual.contains("Release-Urgency:")); }
                else { assert!(actual.ends_with(&format!("Release-Urgency: {policy}")), "{actual}"); }
            }
        }
        let coauthored = delivery::append_release_urgency("Summary\n\nCo-Authored-By: Synthetic <synthetic@example.invalid>".into(), Some(ReleaseUrgency::Immediate));
        assert_eq!(coauthored.rsplit("\n\n").next().unwrap(), "Co-Authored-By: Synthetic <synthetic@example.invalid>\nRelease-Urgency: immediate");
        let args = session_release_args(&json!({"release_urgency":null}), "immediate");
        assert_eq!(args["release_urgency"], "immediate");
        let args = session_release_args(&json!({"release_urgency":"immediate"}), "hold");
        assert_eq!(args["release_urgency"], "immediate");
        assert!(super::super::delivery::release_urgency_from_args(&session_release_args(&json!({"release_urgency":"hold"}), "default")).unwrap().is_none());
    }

    #[derive(Default)]
    struct Forge {
        branch: String, number: Option<u64>,
        pushed: bool, pr: bool, armed: bool, fail_label: bool, merged: bool,
        pushes: usize, creates: usize, merges: usize, dispatched: bool,
        // CF-STAGE1-P1: proves the body contract runs between commit and push.
        governance_root: Option<std::path::PathBuf>, governance_error: Option<String>, governance_warnings: Vec<String>,
        fill_body: bool, reject_body: Option<String>,
        created_body: Option<String>, edited_body: Option<String>, events: Vec<&'static str>,
        // M132: the title the forge actually received, and the commit subjects the
        // branch carries (`git log --format=%s`, newest first).
        subjects: Vec<String>, pr_title: Option<String>,
        changed_paths: String, spec_text: String,
        created_title: Option<String>, edited_title: Option<String>,
        // M147b: the commit message the local commit actually carried.
        commit_message: Option<String>,
    }
    impl Commands for Forge {
        fn precheck_governance(&mut self) -> Result<(Vec<String>, u64), String> {
            self.events.push("governance");
            if let Some(root) = &self.governance_root { return delivery::precheck_thin_governance(root); }
            if let Some(error) = &self.governance_error { return Err(error.clone()); }
            Ok((self.governance_warnings.clone(), 12))
        }
        fn stage(&mut self, _: &[String]) -> Result<Vec<String>,String> { self.events.push("stage"); Ok(vec!["synthetic.rs".into()]) }
        fn reconcile_body(&mut self, _title: &str, body: &str, urgency: Option<ReleaseUrgency>) -> Result<String,String> {
            self.events.push("pr_body");
            let body = delivery::append_release_urgency(body.to_string(), urgency);
            if let Some(error) = self.reject_body.clone() { return Err(error); }
            Ok(if self.fill_body {
                format!("{body}\n\n## README contract\n\nREADME-Update: reviewed\nREADME-Update-Reason: thin delivery derives the required lines before pushing")
            } else { body.to_string() })
        }
        fn git(&mut self, a: &[&str]) -> Result<String,String> {
            Ok(match a[0] {
                "branch" => "feature".into(), "rev-parse" => "abc123".into(),
                "ls-remote" if self.pushed => "abc123\trefs/heads/feature".into(),
                "log" => self.subjects.join("\n"),
                "symbolic-ref" => "origin/main".into(),
                "merge-base" => "synthetic-base".into(),
                "diff" => if a.contains(&"--name-only") { self.changed_paths.clone() } else { self.spec_text.clone() },
                "commit" => {
                    self.events.push("commit");
                    self.commit_message = a.iter().position(|arg| *arg == "-m")
                        .and_then(|index| a.get(index + 1)).map(|message| message.to_string());
                    String::new()
                },
                "push" => { self.pushed = true; self.pushes += 1; self.events.push("push"); String::new() },
                _ => String::new(),
            })
        }
        fn gh(&mut self, a: &[&str]) -> Result<String,String> {
            if !self.branch.is_empty() && a.contains(&"--head") {
                assert_eq!(a[a.iter().position(|arg| *arg == "--head").unwrap() + 1], self.branch);
            }
            let number = self.number.unwrap_or(42);
            let pr = json!({"number":number,"url":format!("https://forge.invalid/pr/{number}"),
                "title":self.pr_title.clone().unwrap_or_else(|| "fix: thin".to_string()),
                "body":"synthetic","state":if self.merged {"MERGED"} else {"OPEN"},"headRefOid":"abc123"});
            match a[1] {
                "list" => Ok(if self.pr {json!([pr])} else {json!([])}.to_string()),
                "create" => {
                    self.pr=true; self.creates+=1;
                    self.created_body = recorded_body(a);
                    self.created_title = recorded_title(a);
                    Ok(String::new())
                },
                "edit" if a.contains(&"--add-label") => {
                    if self.fail_label {return Err("label denied".into());}
                    self.armed=true; Ok(String::new())
                },
                "edit" => {
                    self.edited_body = recorded_body(a);
                    if let Some(title) = recorded_title(a) { self.pr_title = Some(title.clone()); self.edited_title = Some(title); }
                    Ok(String::new())
                },
                "run" => { self.dispatched = true; Ok(String::new()) },
                "view" if a.contains(&"labels") => Ok(json!({"labels":if self.armed {vec![json!({"name":"merge-queue: armed"})]} else {vec![]}}).to_string()),
                "view" if a.contains(&"number,url") => Ok(pr.to_string()),
                "view" => Ok(json!({"state":"OPEN","statusCheckRollup":[{"status":"IN_PROGRESS"}]}).to_string()),
                "merge" => {self.merges+=1; Err("must never merge".into())},
                _ => Err(format!("unexpected {a:?}")),
            }
        }
    }
    fn recorded_body(args: &[&str]) -> Option<String> {
        args.iter().position(|arg| *arg == "--body")
            .and_then(|index| args.get(index + 1))
            .map(|body| body.to_string())
    }
    fn recorded_title(args: &[&str]) -> Option<String> {
        args.iter().position(|arg| *arg == "--title")
            .and_then(|index| args.get(index + 1))
            .map(|title| title.to_string())
    }
    fn deliver(f: &mut Forge) -> Result<Value,String> {
        run(f, &json!({"title":"fix: thin","body":"synthetic"}), DeliveryCeiling::ThroughMerge, &[], &mut Log::default())
    }
    #[test]
    fn thin_explicit_lower_ceiling_reports_authorized_merge_requirement() {
        let mut forge = Forge::default();
        let result = run(&mut forge, &json!({"title":"fix: thin", "body":"synthetic", "ceiling":"through_ci_green"}),
            DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
        assert!(forge.armed);
        assert_eq!(result["effective_ceiling"], "through_merge");
        assert!(result["ceiling_note"].as_str().unwrap().contains("授权"));
    }

    #[test]
    fn pushed_then_interrupted_resumes_without_repush_or_duplicate_pr() {
        let mut forge = Forge { pushed:true, pushes:1, ..Default::default() };
        let first = deliver(&mut forge).unwrap();
        let second = deliver(&mut forge).unwrap();
        assert_eq!(first["pr_number"], second["pr_number"]);
        assert_eq!((forge.pushes,forge.creates,forge.merges), (1,1,0));
    }
    #[test]
    fn existing_pr_and_label_are_reused_while_ci_returns_immediately() {
        let mut forge = Forge { pushed:true, pr:true, armed:true, ..Default::default() };
        let start = std::time::Instant::now();
        let result = deliver(&mut forge).unwrap();
        assert_eq!(result["ci"]["statusCheckRollup"][0]["status"], "IN_PROGRESS");
        assert_eq!(result["state"], "waiting");
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!((forge.pushes,forge.creates,forge.merges), (0,0,0));
    }
    #[test]
    fn m149_waiting_result_says_the_system_already_watches_the_pr() {
        // M149 R3：M142 起系统对已交付 PR 低频心跳监视。结果文案必须说出来，
        // 否则模型又会自己去造监视源（`gh --watch`→被拒→没有唤醒源→回合结束）。
        let mut forge = Forge { pushed:true, pr:true, armed:true, ..Default::default() };
        let result = deliver(&mut forge).unwrap();
        assert_eq!(result["state"], "waiting");
        let next = result["next_action"].as_str().unwrap();
        assert!(next.contains("系统已在监视 PR #"), "{next}");
        assert!(next.contains("唤醒"), "{next}");
        assert!(next.contains("结束本回合"), "{next}");
        assert!(!next.contains("请用后台任务监视"), "{next}");
        assert_eq!(result["session_state"], "waiting_wake");
        assert_eq!(result["system_watch"]["source"], "system");
    }
    #[test]
    fn merged_same_head_is_delivered_without_new_pr_or_label() {
        let mut forge = Forge { pushed:true, pr:true, merged:true, fail_label:true, ..Default::default() };
        let result = deliver(&mut forge).unwrap();
        assert_eq!(result["state"], "delivered");
        assert_eq!((forge.pushes, forge.creates, forge.merges), (0,0,0));
    }
    #[test]
    fn thin_delivery_sends_the_reconciled_body_and_does_it_before_pushing() {
        let mut forge = Forge { fill_body: true, ..Default::default() };
        deliver(&mut forge).unwrap();
        let body = forge.created_body.clone().expect("pr create must carry the body");
        assert_eq!(body.matches("README-Update:").count(), 1, "{body}");
        assert!(body.contains("README-Update-Reason: "), "{body}");
        let reconcile = forge.events.iter().position(|event| *event == "pr_body").unwrap();
        let push = forge.events.iter().position(|event| *event == "push").unwrap();
        assert!(reconcile < push, "body contract must run before the first push: {:?}", forge.events);
    }

    #[test]
    fn body_contract_failure_never_pushes_or_opens_a_pr() {
        let mut forge = Forge {
            reject_body: Some("PR 正文未通过 README 契约 的 CI 校验，未推送任何提交：missing README-Update:".into()),
            ..Default::default()
        };
        let error = deliver(&mut forge).unwrap_err();
        assert!(error.contains("README-Update"), "{error}");
        assert_eq!((forge.pushes, forge.creates, forge.events.contains(&"push")), (0, 0, false));
    }

    #[test]
    fn governance_failure_never_pushes_then_corrected_delivery_pushes() {
        let mut forge = Forge { governance_error: Some("OBJECTIVE_MANUAL_TECHNICAL_HANDOFF synthetic.md:2 system-owned technical state".into()), ..Default::default() };
        assert!(deliver(&mut forge).unwrap_err().contains("synthetic.md:2"));
        assert_eq!((forge.pushes, forge.creates), (0, 0));
        forge.governance_error = None;
        deliver(&mut forge).unwrap();
        assert_eq!((forge.pushes, forge.creates), (1, 1));
    }

    #[test]
    fn unavailable_governance_warns_in_body_and_tool_steps_without_blocking() {
        let mut forge = Forge { governance_warnings: vec!["synthetic validator unavailable".into()], ..Default::default() };
        let mut log = Log::default();
        run(&mut forge, &json!({"title":"fix: synthetic"}), DeliveryCeiling::ThroughMerge, &[], &mut log).unwrap();
        assert_eq!(forge.pushes, 1);
        assert!(forge.created_body.unwrap().contains("治理预检警告"));
        assert!(log.steps.iter().any(|step| step["step"] == "governance_warning"));
    }

    #[test]
    fn real_governance_rule_blocks_thin_push_then_fixed_spec_pushes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let tools = root.join("tools/governance");
        std::fs::create_dir_all(&tools).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        std::fs::copy(repo.join("tools/governance/validate_objective_non_interruption_contract.py"), tools.join("validate_objective_non_interruption_contract.py")).unwrap();
        delivery::git(root, &["init"]).unwrap();
        delivery::git(root, &["config", "user.name", "Synthetic"]).unwrap();
        delivery::git(root, &["config", "user.email", "synthetic@example.invalid"]).unwrap();
        let specs = root.join("docs/specs");
        std::fs::create_dir_all(&specs).unwrap();
        let spec = specs.join("synthetic.md");
        std::fs::write(&spec, "# Synthetic\n").unwrap();
        delivery::git(root, &["add", "."]).unwrap();
        delivery::git(root, &["commit", "-m", "fixture"]).unwrap();
        delivery::git(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        std::fs::write(&spec, "# Requirements\n技术失败时用户必须点击重试按钮。\n").unwrap();
        delivery::git(root, &["add", "."]).unwrap();
        delivery::git(root, &["commit", "-m", "synthetic bad spec"]).unwrap();
        let mut forge = Forge { governance_root: Some(root.to_path_buf()), ..Default::default() };
        let error = deliver(&mut forge).unwrap_err();
        assert!(error.contains("OBJECTIVE_MANUAL_TECHNICAL_HANDOFF"), "{error}");
        assert!(error.contains("synthetic.md:2"), "{error}");
        assert_eq!((forge.pushes, forge.creates), (0, 0));
        std::fs::write(&spec, "# Requirements\n技术失败时系统自动重试并自愈。\n").unwrap();
        delivery::git(root, &["add", "."]).unwrap();
        delivery::git(root, &["commit", "-m", "synthetic fixed spec"]).unwrap();
        deliver(&mut forge).unwrap();
        assert_eq!((forge.pushes, forge.creates), (1, 1));
    }

    #[test]
    fn label_failure_returns_error_never_merges() {
        let mut forge = Forge { fail_label:true, ..Default::default() };
        assert_eq!(deliver(&mut forge).unwrap_err(), "label denied");
        assert_eq!(forge.merges, 0);
    }
    #[test]
    fn already_armed_rearm_still_wakes_the_queue() {
        // M126: an already-armed PR emits no `labeled` webhook, so "arm once CI
        // went green" must still dispatch the queue or the PR waits for cron.
        let mut forge = Forge { pushed:true, pr:true, armed:true, ..Default::default() };
        let _ = deliver(&mut forge).unwrap();
        assert!(forge.dispatched, "re-arming an armed PR must dispatch the queue");
    }

    #[test]
    fn m171_whole_diff_controls_title_and_commit() {
        for (paths, subjects, explicit, spec, expected) in [
            ("src/runtime.rs\ntests/runtime.rs", vec!["test: cover recovery", "fix: recover writes"], None, "", "fix: cover recovery"),
            ("tests/runtime.rs", vec!["test: cover recovery"], None, "", "test: cover recovery"),
            ("docs/guide.md", vec!["docs: explain recovery"], Some("fix: explain recovery"), "", "docs: explain recovery"),
            ("src/runtime.rs", vec!["test: cover recovery", "feat: add recovery"], Some("test: cover recovery"), "", "feat: cover recovery"),
            ("src/runtime.rs\ndocs/specs/feature.md", vec!["test: cover recovery"], None, "+feat: add recovery", "feat: cover recovery"),
            ("src/runtime.rs", vec!["test: cover recovery"], None, "", "fix: cover recovery"),
        ] {
            let mut forge = Forge { changed_paths: paths.into(), subjects: subjects.into_iter().map(str::to_owned).collect(), spec_text: spec.into(), ..Default::default() };
            let mut args = json!({"body":"synthetic", "release_urgency":"immediate"});
            if let Some(title) = explicit { args["title"] = json!(title); }
            run(&mut forge, &args, DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
            assert_eq!(forge.created_title.as_deref(), Some(expected), "{paths}");
            assert_eq!(forge.commit_message.as_deref(), Some(format!("{expected}\n\nRelease-Urgency: immediate").as_str()));
        }
    }

    // ---- M132: the PR title/body must describe the change, never the branch ----
    const BRANCH_ECHO: &str = "fix: codefactory/objective-8af68c03794255ec2beef940d3d42524";

    #[test]
    fn missing_title_uses_first_meaningful_commit_subject() {
        // R1: no `title` from the model, but the branch already carries a real
        // Conventional Commit — that subject is the title, not the branch name.
        let mut forge = Forge { subjects: vec![BRANCH_ECHO.to_string(),
            "feat: keep the merge queue awake after a green run".to_string()], ..Default::default() };
        let result = run(&mut forge, &json!({"body":"synthetic"}), DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
        assert_eq!(forge.created_title.as_deref(), Some("feat: keep the merge queue awake after a green run"));
        assert_eq!(result["state"], "waiting");
    }

    #[test]
    fn branch_echo_title_falls_back_to_the_commit_subject() {
        // R1: an explicit title that only echoes the branch describes nothing.
        let mut forge = Forge { subjects: vec!["fix: stop the queue waking on a stale head".to_string()],
            ..Default::default() };
        let result = run(&mut forge, &json!({"title": BRANCH_ECHO, "body":"synthetic"}),
            DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
        assert_eq!(forge.created_title.as_deref(), Some("fix: stop the queue waking on a stale head"));
        assert_ne!(result["state"], "delivered");
    }

    #[test]
    fn no_title_and_no_meaningful_subject_errors_without_push() {
        // R1: nothing usable to say → error before any commit or push.
        let mut forge = Forge { subjects: vec![BRANCH_ECHO.to_string()], ..Default::default() };
        let error = run(&mut forge, &json!({"body":"synthetic"}), DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap_err();
        assert!(error.contains("title") || error.contains("标题"), "{error}");
        assert_eq!((forge.pushes, forge.creates), (0, 0));
        assert!(!forge.pushed);
    }

    #[test]
    fn reused_pr_keeps_a_usable_title_instead_of_the_branch_name() {
        // R3: an open PR that already carries a real title is not downgraded.
        let mut forge = Forge { pr: true, pr_title: Some("fix: keep the merge queue awake".into()),
            subjects: vec![BRANCH_ECHO.to_string()], ..Default::default() };
        let result = run(&mut forge, &json!({"title": BRANCH_ECHO, "body":"synthetic"}),
            DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
        assert_eq!(forge.edited_title, None);
        assert_eq!(result["state"], "waiting");
    }

    #[test]
    fn better_title_updates_the_existing_pr() {
        // R3: when the candidate is a real subject, the open PR is updated.
        let mut forge = Forge { pr: true, pr_title: Some(BRANCH_ECHO.into()),
            subjects: vec!["feat: derive the PR title from the commit".to_string()], ..Default::default() };
        run(&mut forge, &json!({"body":"synthetic"}), DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
        assert_eq!(forge.edited_title.as_deref(), Some("feat: derive the PR title from the commit"));
    }

    // ---- M147b: the urgency trailer must survive a later PR-body rewrite ------
    // #818 carried `release_urgency=immediate` in its session row yet the merged
    // PR body had no `Release-Urgency:` trailer, so the batch was never cut. The
    // body was the only carrier; the commit message is the durable one.

    #[test]
    fn m147b_thin_commit_carries_the_release_urgency_trailer() {
        let mut forge = Forge::default();
        run(&mut forge, &json!({"title":"fix: thin","body":"synthetic","release_urgency":"immediate"}),
            DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
        let message = forge.commit_message.clone().expect("the delivery must create a commit");
        assert_eq!(message.lines().next(), Some("fix: thin"), "{message}");
        assert!(message.trim_end().ends_with("Release-Urgency: immediate"), "{message}");
        let body = forge.created_body.clone().expect("pr create must carry the body");
        assert!(body.contains("Release-Urgency: immediate"), "{body}");
    }

    #[test]
    fn m147b_thin_commit_without_urgency_carries_no_trailer() {
        let mut forge = Forge::default();
        run(&mut forge, &json!({"title":"fix: thin","body":"synthetic"}),
            DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
        let message = forge.commit_message.clone().expect("the delivery must create a commit");
        assert!(!message.contains("Release-Urgency"), "{message}");
    }

    #[test]
    fn m147b_hold_mandate_reaches_the_commit_trailer() {
        let mut forge = Forge::default();
        run(&mut forge, &json!({"title":"fix: thin","body":"synthetic","release_urgency":"hold",
                "hold_requested_by_user":"user asked to hold the batch"}),
            DeliveryCeiling::ThroughMerge, &[], &mut Log::default()).unwrap();
        let message = forge.commit_message.clone().expect("the delivery must create a commit");
        assert!(message.trim_end().ends_with("Release-Urgency: hold"), "{message}");
    }
}
