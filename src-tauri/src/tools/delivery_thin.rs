// SPDX-License-Identifier: Apache-2.0
//! Thin delivery: no durable identities, receipts, event stream, or CI wait.
use serde_json::{json, Value};
use std::path::Path;
use crate::agent::delivery;
use super::{ExecCtx, ToolOutput};
use crate::config::settings::DeliveryCeiling;

trait Commands {
    fn git(&mut self, args: &[&str]) -> Result<String, String>;
    fn gh(&mut self, args: &[&str]) -> Result<String, String>;
    fn stage(&mut self, excludes: &[String]) -> Result<Vec<String>, String>;
}

struct Local<'a>(&'a Path);
impl Commands for Local<'_> {
    fn git(&mut self, args: &[&str]) -> Result<String, String> { delivery::git(self.0, args) }
    fn gh(&mut self, args: &[&str]) -> Result<String, String> {
        delivery::bounded_delivery_output(delivery::dev_command("gh").current_dir(self.0).args(args))
    }
    fn stage(&mut self, excludes: &[String]) -> Result<Vec<String>, String> { delivery::stage_scoped(self.0, excludes) }
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
    let body = args.get("body").and_then(Value::as_str).unwrap_or("");
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
        if pr["title"].as_str() != Some(title.as_str()) || pr["body"].as_str() != Some(body) {
            log.gh(commands, &["pr", "edit", &number, "--title", &title, "--body", body])?;
        }
        log.step("pr", "reused");
        pr.clone()
    } else {
        log.gh(commands, &["pr", "create", "--head", &branch, "--title", &title, "--body", body])?;
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
    struct Forge { pushed: bool, pr: bool, armed: bool, fail_label: bool, merged: bool, pushes: usize, creates: usize, merges: usize }
    impl Commands for Forge {
        fn stage(&mut self, _: &[String]) -> Result<Vec<String>,String> { Ok(vec![]) }
        fn git(&mut self, a: &[&str]) -> Result<String,String> {
            Ok(match a[0] {
                "branch" => "feature".into(), "rev-parse" => "abc123".into(),
                "ls-remote" if self.pushed => "abc123\trefs/heads/feature".into(),
                "push" => { self.pushed = true; self.pushes += 1; String::new() },
                _ => String::new(),
            })
        }
        fn gh(&mut self, a: &[&str]) -> Result<String,String> {
            let pr = json!({"number":42,"url":"https://forge.invalid/pr/42","title":"fix: thin","body":"synthetic","state":if self.merged {"MERGED"} else {"OPEN"},"headRefOid":"abc123"});
            match a[1] {
                "list" => Ok(if self.pr {json!([pr])} else {json!([])}.to_string()),
                "create" => {self.pr=true; self.creates+=1; Ok(String::new())},
                "edit" if a.contains(&"--add-label") => {
                    if self.fail_label {return Err("label denied".into());}
                    self.armed=true; Ok(String::new())
                },
                "edit" => Ok(String::new()),
                "view" if a.contains(&"labels") => Ok(json!({"labels":if self.armed {vec![json!({"name":"merge-queue: armed"})]} else {vec![]}}).to_string()),
                "view" if a.contains(&"number,url") => Ok(pr.to_string()),
                "view" => Ok(json!({"state":"OPEN","statusCheckRollup":[{"status":"IN_PROGRESS"}]}).to_string()),
                "merge" => {self.merges+=1; Err("must never merge".into())},
                _ => Err(format!("unexpected {a:?}")),
            }
        }
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
    fn label_failure_returns_error_never_merges() {
        let mut forge = Forge { fail_label:true, ..Default::default() };
        assert_eq!(deliver(&mut forge).unwrap_err(), "label denied");
        assert_eq!(forge.merges, 0);
    }
}
