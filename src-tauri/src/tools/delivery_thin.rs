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
    fn reconcile_body(&mut self, _title: &str, body: &str, _urgency: Option<ReleaseUrgency>) -> Result<String, String> {
        Ok(body.to_string())
    }
}

struct Local<'a>(&'a Path);
impl Commands for Local<'_> {
    fn git(&mut self, args: &[&str]) -> Result<String, String> { delivery::git(self.0, args) }
    fn gh(&mut self, args: &[&str]) -> Result<String, String> {
        delivery::bounded_delivery_output(delivery::dev_command("gh").current_dir(self.0).args(args))
    }
    fn stage(&mut self, excludes: &[String]) -> Result<Vec<String>, String> { delivery::stage_scoped(self.0, excludes) }
    fn precheck_governance(&mut self) -> Result<(Vec<String>, u64), String> {
        delivery::precheck_thin_governance(self.0)
    }
    fn reconcile_body(&mut self, title: &str, body: &str, urgency: Option<ReleaseUrgency>) -> Result<String, String> {
        delivery::prepare_thin_pr_body(self.0, title, body, urgency)
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

fn run(commands: &mut impl Commands, args: &Value, ceiling: DeliveryCeiling, excludes: &[String], log: &mut Log) -> Result<Value, String> {
    if ceiling == DeliveryCeiling::Off { return Ok(json!({"state":"noop"})); }
    let branch = commands.git(&["branch", "--show-current"])?;
    if branch.is_empty() { return Err("detached HEAD cannot be delivered".into()); }
    if args.get("expect_branch").and_then(Value::as_str).is_some_and(|expected| expected != branch) {
        return Err(format!("expected branch does not match {branch}"));
    }
    log.branch = branch.clone();
    let paths = commands.stage(excludes)?;
    let title = args.get("title").and_then(Value::as_str).map(str::to_owned)
        .unwrap_or_else(|| format!("fix: {}", paths.first().map(String::as_str).unwrap_or(&branch)));
    if !paths.is_empty() {
        commands.git(&["commit", "-m", &title])?;
        log.step("commit", "created");
    } else { log.step("commit", "unchanged"); }
    let sha = commands.git(&["rev-parse", "HEAD"])?;
    // The committed tree is now the thing the body describes; derive the required
    // declarations and refuse a body CI would reject before touching the remote.
    let urgency = super::delivery::release_urgency_from_args(args)?;
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
        commands.git(&["push", "--set-upstream", "origin", &format!("HEAD:{remote_ref}")])?;
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
        "branch":branch,"commit_sha":sha,"ci":ci,"next_action":"请用后台任务监视 CI；由合并队列合并，不要直接合并。"}))
}

pub(super) async fn execute(args: &Value, ctx: &ExecCtx, ceiling: DeliveryCeiling, excludes: &[String]) -> crate::errors::Result<ToolOutput> {
    let mut log = Log::default();
    let result = run(&mut Local(&ctx.cwd), args, ceiling, excludes, &mut log);
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
mod tests {
    use super::*;
    #[derive(Default)]
    struct Forge {
        pushed: bool, pr: bool, armed: bool, fail_label: bool, merged: bool,
        pushes: usize, creates: usize, merges: usize,
        // CF-STAGE1-P1: proves the body contract runs between commit and push.
        governance_root: Option<std::path::PathBuf>, governance_error: Option<String>, governance_warnings: Vec<String>,
        fill_body: bool, reject_body: Option<String>,
        created_body: Option<String>, edited_body: Option<String>, events: Vec<&'static str>,
    }
    impl Commands for Forge {
        fn precheck_governance(&mut self) -> Result<(Vec<String>, u64), String> {
            self.events.push("governance");
            if let Some(root) = &self.governance_root { return delivery::precheck_thin_governance(root); }
            if let Some(error) = &self.governance_error { return Err(error.clone()); }
            Ok((self.governance_warnings.clone(), 12))
        }
        fn stage(&mut self, _: &[String]) -> Result<Vec<String>,String> { self.events.push("stage"); Ok(vec![]) }
        fn reconcile_body(&mut self, _title: &str, body: &str, _urgency: Option<ReleaseUrgency>) -> Result<String,String> {
            self.events.push("pr_body");
            if let Some(error) = self.reject_body.clone() { return Err(error); }
            Ok(if self.fill_body {
                format!("{body}\n\n## README contract\n\nREADME-Update: reviewed\nREADME-Update-Reason: thin delivery derives the required lines before pushing")
            } else { body.to_string() })
        }
        fn git(&mut self, a: &[&str]) -> Result<String,String> {
            Ok(match a[0] {
                "branch" => "feature".into(), "rev-parse" => "abc123".into(),
                "ls-remote" if self.pushed => "abc123\trefs/heads/feature".into(),
                "push" => { self.pushed = true; self.pushes += 1; self.events.push("push"); String::new() },
                _ => String::new(),
            })
        }
        fn gh(&mut self, a: &[&str]) -> Result<String,String> {
            let pr = json!({"number":42,"url":"https://forge.invalid/pr/42","title":"fix: thin","body":"synthetic","state":if self.merged {"MERGED"} else {"OPEN"},"headRefOid":"abc123"});
            match a[1] {
                "list" => Ok(if self.pr {json!([pr])} else {json!([])}.to_string()),
                "create" => {
                    self.pr=true; self.creates+=1;
                    self.created_body = recorded_body(a);
                    Ok(String::new())
                },
                "edit" if a.contains(&"--add-label") => {
                    if self.fail_label {return Err("label denied".into());}
                    self.armed=true; Ok(String::new())
                },
                "edit" => { self.edited_body = recorded_body(a); Ok(String::new()) },
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
}
