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

async fn guarded_args(args: &Value, ctx: &ExecCtx, ceiling: DeliveryCeiling) -> crate::errors::Result<Value> {
    let mut guarded = args.clone();
    let branch = match crate::agent::delivery::git(&ctx.cwd, &["branch", "--show-current"]) {
        Ok(branch) if !branch.trim().is_empty() => branch,
        _ => return Ok(guarded),
    };
    let raw = match crate::agent::delivery::bounded_delivery_output(
        crate::agent::delivery::dev_command("gh").current_dir(&ctx.cwd).args([
            "pr", "list", "--head", &branch, "--state", "open", "--json",
            "number,title,body,isDraft,labels",
        ]),
    ) {
        Ok(raw) => raw,
        Err(_) => return Ok(guarded),
    };
    let prs: Vec<Value> = serde_json::from_str(&raw).unwrap_or_default();
    let Some(pr) = prs.first() else { return Ok(guarded); };
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
                .bind(marker)
                .bind(session)
                .execute(db)
                .await?;
            sqlx::query("UPDATE sessions SET delivery_authorized=0 WHERE id=?")
                .bind(session)
                .execute(db)
                .await?;
        }
        return Err(crate::errors::AppError::ToolArgument(reason));
    }
    if let (Some(title), Some(body)) = (pr["title"].as_str(), pr["body"].as_str()) {
        let (title, body) = super::delivery_m285_policy::preserve_existing_pr_content(title, body);
        guarded["title"] = Value::String(title.to_string());
        guarded["body"] = Value::String(body.to_string());
    }
    Ok(guarded)
}

pub(super) async fn execute(
    args: &Value,
    ctx: &ExecCtx,
    authorized_ceiling: DeliveryCeiling,
    excludes: &[String],
) -> crate::errors::Result<ToolOutput> {
    let requested = args.get("ceiling").and_then(Value::as_str).and_then(DeliveryCeiling::from_label);
    let ceiling = super::delivery_m285_policy::effective_delivery_ceiling(authorized_ceiling, requested);
    let guarded = guarded_args(args, ctx, ceiling).await?;
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
