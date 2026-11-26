// SPDX-License-Identifier: Apache-2.0
//! M285 guard around the legacy thin delivery engine.

use serde_json::Value;

use super::{ExecCtx, ToolOutput};
use crate::config::settings::DeliveryCeiling;

mod delivery {
    pub(crate) use crate::tools::delivery::*;
}

#[path = "delivery_thin.rs"]
mod legacy;

#[cfg(test)]
pub(crate) fn m232_deliver_fixture(cwd: &std::path::Path) -> Value {
    legacy::m232_deliver_fixture(cwd)
}

async fn guarded_args(args: &Value, ctx: &ExecCtx, ceiling: DeliveryCeiling) -> crate::errors::Result<(Value, bool)> {
    let mut guarded = args.clone();
    let branch = match crate::agent::delivery::git(&ctx.cwd, &["branch", "--show-current"]) {
        Ok(branch) if !branch.trim().is_empty() => branch,
        _ => return Ok((guarded, false)),
    };
    let raw = match crate::agent::delivery::bounded_delivery_output(
        crate::agent::delivery::dev_command_in("gh", &ctx.cwd).current_dir(&ctx.cwd).args([
            "pr", "list", "--head", &branch, "--state", "open", "--json",
            "number,title,body,isDraft,labels",
        ]),
    ) {
        Ok(raw) => raw,
        Err(_) => return Ok((guarded, false)),
    };
    let prs: Vec<Value> = serde_json::from_str(&raw).unwrap_or_default();
    let Some(pr) = prs.first() else { return Ok((guarded, true)); };
    let is_draft = pr["isDraft"].as_bool().unwrap_or(false);
    let is_armed = pr["labels"].as_array().is_some_and(|labels| {
        labels.iter().any(|label| label["name"] == "merge-queue: armed")
    });
    let (prior_ceiling, newly_authorized) = if let (Some(db), Some(session)) = (&ctx.db, &ctx.session_id) {
        let prior = sqlx::query_scalar::<_, String>("SELECT ceiling FROM thin_delivery_refs WHERE session_id=?")
            .bind(session)
            .fetch_optional(db)
            .await?;
        let authorized = sqlx::query_scalar::<_, i64>("SELECT delivery_authorized FROM sessions WHERE id=?")
            .bind(session)
            .fetch_optional(db)
            .await?
            .unwrap_or(0) != 0;
        (prior, authorized)
    } else {
        (None, false)
    };
    let recorded_revocation = prior_ceiling
        .as_deref()
        .is_some_and(super::delivery_m285_policy::ceiling_records_revocation);
    if recorded_revocation && !newly_authorized {
        // 记录一次撤回事实（带时间戳），供心跳/重启后尊重
        if let (Some(db), Some(session)) = (&ctx.db, &ctx.session_id) {
            let _ = sqlx::query("CREATE TABLE IF NOT EXISTS thin_delivery_revocations(session_id TEXT PRIMARY KEY, pr_number INTEGER NOT NULL, revoked_at INTEGER NOT NULL, kind TEXT NOT NULL)").execute(db).await;
            let now = chrono::Utc::now().timestamp_millis();
            let number = pr["number"].as_i64().unwrap_or(0);
            let _ = sqlx::query("INSERT INTO thin_delivery_revocations(session_id,pr_number,revoked_at,kind) VALUES (?,?,?,?) ON CONFLICT(session_id) DO UPDATE SET pr_number=excluded.pr_number,revoked_at=excluded.revoked_at,kind=excluded.kind")
                .bind(session)
                .bind(number)
                .bind(now)
                .bind("recorded_revocation")
                .execute(db).await;
        }
        return Err(crate::errors::AppError::ToolArgument(format!(
            "合并授权此前已由用户撤回；当前上限为 {}。需要用户新的明确授权，系统不会重复打标。",
            ceiling.label()
        )));
    }
    let was_merge_authorized = prior_ceiling
        .as_deref()
        .is_some_and(super::delivery_m285_policy::ceiling_was_merge_authorized);
    if let Some(reason) = super::delivery_m285_policy::existing_pr_merge_refusal(
        is_draft, was_merge_authorized, is_armed, ceiling,
    ) {
        if let (Some(db), Some(session)) = (&ctx.db, &ctx.session_id) {
            let marker = format!("revoked:{}", prior_ceiling.as_deref().unwrap_or(ceiling.label()));
            sqlx::query("UPDATE thin_delivery_refs SET ceiling=? WHERE session_id=?")
                .bind(&marker)
                .bind(session)
                .execute(db)
                .await?;
            // 记录撤回事实与时间戳
            let _ = sqlx::query("CREATE TABLE IF NOT EXISTS thin_delivery_revocations(session_id TEXT PRIMARY KEY, pr_number INTEGER NOT NULL, revoked_at INTEGER NOT NULL, kind TEXT NOT NULL)").execute(db).await;
            let now = chrono::Utc::now().timestamp_millis();
            let number = pr["number"].as_i64().unwrap_or(0);
            sqlx::query("INSERT INTO thin_delivery_revocations(session_id,pr_number,revoked_at,kind) VALUES (?,?,?,?) ON CONFLICT(session_id) DO UPDATE SET pr_number=excluded.pr_number,revoked_at=excluded.revoked_at,kind=excluded.kind")
                .bind(session)
                .bind(number)
                .bind(now)
                .bind(if is_draft {"draft"} else {"armed_removed"})
                .execute(db).await?;
            sqlx::query("UPDATE sessions SET delivery_authorized=0 WHERE id=?")
                .bind(session)
                .execute(db)
                .await?;
        }
        return Err(crate::errors::AppError::ToolArgument(reason));
    }
    if pr["title"].as_str().is_some() || pr["body"].as_str().is_some() {
        // F6/F7: default to preserving remote title/body on PR reuse.
        let explicit_update = args
            .get("update_pr_body")
            .and_then(Value::as_str)
            .is_some_and(|mode| matches!(mode, "append" | "replace"));
        if explicit_update {
            // Allow an explicit evidence update: append or replace the body while keeping the title.
            if let Some(obj) = guarded.as_object_mut() {
                if let Some(local) = obj.get("body").and_then(Value::as_str) {
                    let remote = pr["body"].as_str().unwrap_or("");
                    let merged = if args["update_pr_body"].as_str() == Some("append") {
                        if remote.trim().is_empty() { local.to_string() } else { format!("{remote}\n\n{local}") }
                    } else {
                        // replace
                        local.to_string()
                    };
                    obj.insert("body".into(), Value::String(merged));
                }
                // Never change an existing title here; let legacy path improve only unusable titles.
                obj.remove("title");
            }
        } else if let Some(obj) = guarded.as_object_mut() {
            // No explicit update requested: strip both fields so the legacy path does not overwrite maintainer edits.
            obj.remove("title");
            obj.remove("body");
        }
    }
    Ok((guarded, true))
}

pub(super) async fn execute(
    args: &Value,
    ctx: &ExecCtx,
    authorized_ceiling: DeliveryCeiling,
    excludes: &[String],
) -> crate::errors::Result<ToolOutput> {
    let requested = args.get("ceiling").and_then(Value::as_str).and_then(DeliveryCeiling::from_label);
    let mut ceiling = super::delivery_m285_policy::effective_delivery_ceiling(authorized_ceiling, requested);
    let (guarded, pr_checked) = guarded_args(args, ctx, ceiling).await?;
    // F3：无法读取 PR 状态时不得打 armed/转 ready —— 本次执行降级为 pr_only
    if !pr_checked && ceiling.rank() >= DeliveryCeiling::ThroughMerge.rank() {
        ceiling = DeliveryCeiling::PrOnly;
    }
    legacy::execute(&guarded, ctx, ceiling, excludes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_merge_ceiling_marks_prior_authorization() {
        assert!(super::super::delivery_m285_policy::ceiling_was_merge_authorized("ThroughRelease"));
        assert!(!super::super::delivery_m285_policy::ceiling_was_merge_authorized("revoked:ThroughRelease"));
    }
}
