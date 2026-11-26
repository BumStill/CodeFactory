// SPDX-License-Identifier: Apache-2.0
//! Per actual host dispatch content evidence. No permission, retry, or business state.
use super::{
    shell_intent::{Entry, Manifest},
    FileObservationAdmission, ToolCall, ToolCtx, ToolExecutionStatus, ToolInvocationResult,
};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, SqlitePool};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    session: String,
    root: String,
    objective: String,
    revision: i64,
    workspace: String,
    base: String,
    repo: String,
    worktree: String,
    branch: String,
    run: String,
    mode: String,
    epoch: i64,
}
#[derive(Debug)]
pub(crate) struct Prepared {
    dispatch: String,
    scope: Scope,
    manifest: Manifest,
}
const SCHEMA: &str = include_str!("../../migrations/0027_workspace_content_provenance.sql");

pub(crate) async fn ensure(db: &SqlitePool) -> anyhow::Result<()> {
    let columns = sqlx::query("PRAGMA table_info(sessions)")
        .fetch_all(db)
        .await?;
    anyhow::ensure!(
        columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "permission_mode"),
        "content epoch requires real session permission_mode schema"
    );
    sqlx::raw_sql(SCHEMA).execute(db).await?;
    verify_epoch_trigger(db).await
}
async fn verify_epoch_trigger(db: &SqlitePool) -> anyhow::Result<()> {
    // CREATE IF NOT EXISTS must not silently accept a replaced ABA trigger.
    let actual:String=sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='trg_session_content_permission_epoch'").fetch_one(db).await?;
    let expected = SCHEMA
        .split("CREATE TRIGGER IF NOT EXISTS trg_session_content_permission_epoch")
        .nth(1)
        .and_then(|s| s.split("END;").next())
        .ok_or_else(|| anyhow::anyhow!("missing epoch trigger definition"))?;
    let normalize = |s: &str| {
        s.split_whitespace()
            .collect::<String>()
            .to_ascii_lowercase()
            .replace("ifnotexists", "")
            .trim_end_matches(';')
            .to_string()
    };
    anyhow::ensure!(
        normalize(&actual)
            == normalize(&format!(
                "CREATE TRIGGER trg_session_content_permission_epoch{expected}END"
            )),
        "content epoch trigger mismatch"
    );
    Ok(())
}
async fn scope_on(conn: &mut SqliteConnection, session: &str, root: &str) -> anyhow::Result<Scope> {
    let rows=sqlx::query("SELECT o.id AS objective,o.revision,w.id AS workspace,w.base_sha,w.repo_identity,
        w.worktree_identity,w.branch_name,c.run_instance_id,s.permission_mode,COALESCE(e.epoch,0) AS epoch
        FROM chat_turn_state t JOIN objectives o ON o.id=t.objective_id AND o.session_id=t.session_id
        JOIN chat_run_controls c ON c.root_turn_id=t.root_turn_id AND c.session_id=t.session_id
            AND c.objective_id=o.id AND c.objective_revision=o.revision
        JOIN execution_workspaces w ON w.objective_id=o.id AND w.session_id=t.session_id
        JOIN sessions s ON s.id=t.session_id
        LEFT JOIN session_content_epochs e ON e.session_id=s.id
        WHERE t.session_id=? AND t.root_turn_id=? AND o.status='active' AND c.status='active' AND c.cancel_requested_at IS NULL
            AND w.state IN ('active','delivering')
            AND NOT EXISTS(SELECT 1 FROM chat_session_cancel_intents cancel
                WHERE cancel.session_id=t.session_id AND cancel.status='requested') LIMIT 2")
        .bind(session).bind(root).fetch_all(conn).await?;
    anyhow::ensure!(
        rows.len() == 1,
        "content observation needs one exact active foreground run"
    );
    let row = &rows[0];
    Ok(Scope {
        session: session.into(),
        root: root.into(),
        objective: row.try_get("objective")?,
        revision: row.try_get("revision")?,
        workspace: row.try_get("workspace")?,
        base: row.try_get("base_sha")?,
        repo: row.try_get("repo_identity")?,
        worktree: row.try_get("worktree_identity")?,
        branch: row.try_get("branch_name")?,
        run: row.try_get("run_instance_id")?,
        mode: row.try_get("permission_mode")?,
        epoch: row.try_get("epoch")?,
    })
}
fn action(name: &str, args: &serde_json::Value, scope: &Scope) -> String {
    super::opaque_digest(&[
        "content_dispatch_v1",
        name,
        &super::canonical_json(args),
        &serde_json::to_string(scope).expect("scope string fields serialize"),
    ])
}
async fn verify_call(
    conn: &mut SqliteConnection,
    trace: &str,
    session: &str,
    name: &str,
    args: &serde_json::Value,
) -> anyhow::Result<()> {
    let row=sqlx::query("SELECT t.tool_name,t.arguments,m.session_id FROM tool_calls t JOIN messages m ON m.id=t.message_id WHERE t.id=?")
        .bind(trace).fetch_one(conn).await?;
    let stored: serde_json::Value = serde_json::from_str(&row.try_get::<String, _>("arguments")?)?;
    anyhow::ensure!(
        row.try_get::<String, _>("session_id")? == session
            && row.try_get::<String, _>("tool_name")? == name
            && super::canonical_json(&stored) == super::canonical_json(args),
        "content tool-call identity mismatch"
    );
    Ok(())
}
fn workspace_matches(
    scope: &Scope,
    w: &crate::agent::execution_workspace::ExecutionWorkspace,
) -> bool {
    scope.workspace == w.id
        && scope.base == w.base_sha
        && scope.repo == w.repo_identity
        && scope.worktree == w.worktree_identity
        && scope.branch == w.branch_name
        && w.session_id.as_deref() == Some(scope.session.as_str())
        && scope.objective == w.objective_id
}
/// None is absence of evidence, never a grant or a rejection of an existing tool.
pub(crate) async fn prepare(
    db: &SqlitePool,
    call: &ToolCall,
    args: &serde_json::Value,
    ctx: &ToolCtx,
) -> anyhow::Result<Option<Prepared>> {
    if ctx.task_id.is_some()
        || !matches!(
            call.function.name.as_str(),
            "write_file" | "edit_file" | "bash"
        )
    {
        return Ok(None);
    }
    let (Some(session), Some(root)) = (ctx.session_id.as_deref(), ctx.root_turn_id.as_deref())
    else {
        return Ok(None);
    };
    if ctx
        .trajectory_session_id
        .as_deref()
        .is_some_and(|id| id != session)
    {
        return Ok(None);
    }
    let FileObservationAdmission::Plan(plan) =
        super::prepare_file_observation(&call.function.name, args, &ctx.working_directory).await
    else {
        return Ok(None);
    };
    let locator: serde_json::Value = serde_json::from_str(&plan.safe_locator_json)?;
    let manifest = if let Some(value) = locator.get("shell_intent_v1") {
        serde_json::from_value::<Manifest>(value.clone())?
    } else {
        let path = locator
            .get("workspace_relative_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("native content locator absent"))?;
        Manifest {
            entries: vec![Entry {
                path: path.into(),
                before: plan.precondition_digest,
                after: plan.expected_postcondition_digest,
            }],
        }
    };
    anyhow::ensure!(manifest.valid(), "invalid content intent manifest");
    ensure(db).await?;
    let scope = {
        let mut conn = db.acquire().await?;
        scope_on(&mut conn, session, root).await?
    };
    let workspace = crate::agent::execution_workspace::verify_objective_workspace(
        db,
        &scope.objective,
        &ctx.working_directory,
    )
    .await?;
    anyhow::ensure!(
        workspace_matches(&scope, &workspace),
        "content workspace identity changed"
    );
    anyhow::ensure!(
        manifest.observe(&ctx.working_directory).as_deref()
            == Some(manifest.digest(false).as_str()),
        "content preimage changed before dispatch evidence"
    );
    let trace = crate::trajectory::trace_record_id(session, &call.id);
    let mut tx = db.begin().await?;
    anyhow::ensure!(
        scope_on(&mut tx, session, root).await? == scope,
        "content run scope changed before dispatch"
    );
    verify_call(&mut tx, &trace, session, &call.function.name, args).await?;
    let dispatch = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO workspace_content_intents(dispatch_id,objective_id,workspace_id,session_id,tool_call_id,run_instance_id,
        scope_json,action_fingerprint,manifest_json,precondition_digest,expected_digest,created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&dispatch).bind(&scope.objective).bind(&scope.workspace).bind(session).bind(trace).bind(&scope.run)
        .bind(serde_json::to_string(&scope)?).bind(action(&call.function.name,args,&scope)).bind(serde_json::to_string(&manifest)?)
        .bind(manifest.digest(false)).bind(manifest.digest(true)).bind(chrono::Utc::now().timestamp_millis())
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(Prepared {
        dispatch,
        scope,
        manifest,
    }))
}
pub(crate) async fn finish(
    db: &SqlitePool,
    prepared: Prepared,
    result: &ToolInvocationResult,
    ctx: &ToolCtx,
) -> anyhow::Result<()> {
    let success = result.status == ToolExecutionStatus::Done
        && !result.is_error
        && result.return_code.is_none_or(|code| code == 0);
    let epoch_trigger_ok = verify_epoch_trigger(db).await.is_ok();
    let observed = prepared.manifest.observe(&ctx.working_directory);
    let workspace_ok = crate::agent::execution_workspace::verify_objective_workspace(
        db,
        &prepared.scope.objective,
        &ctx.working_directory,
    )
    .await
    .is_ok_and(|workspace| workspace_matches(&prepared.scope, &workspace));
    let mut tx = db.begin().await?;
    let scope_ok = scope_on(&mut tx, &prepared.scope.session, &prepared.scope.root)
        .await
        .is_ok_and(|scope| scope == prepared.scope);
    let state = if !success || !scope_ok || !workspace_ok || !epoch_trigger_ok {
        "unknown"
    } else if observed.as_deref() == Some(prepared.manifest.digest(true).as_str()) {
        "applied"
    } else {
        "conflict"
    };
    sqlx::query("INSERT INTO workspace_content_results(dispatch_id,state,actual_success,observed_digest,finished_at) VALUES (?,?,?,?,?)")
        .bind(prepared.dispatch).bind(state).bind(success).bind(observed).bind(chrono::Utc::now().timestamp_millis())
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub(crate) async fn completed(
    db: &SqlitePool,
    workspace: &crate::agent::execution_workspace::ExecutionWorkspace,
) -> anyhow::Result<Vec<Manifest>> {
    let exists:i64=sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='workspace_content_intents'").fetch_one(db).await?;
    if exists == 0 {
        return Ok(vec![]);
    }
    let rows=sqlx::query("SELECT i.*,r.observed_digest,r.actual_success,r.finished_at,t.tool_name,t.arguments,m.session_id AS trajectory_session
        FROM workspace_content_intents i JOIN workspace_content_results r ON r.dispatch_id=i.dispatch_id
        JOIN tool_calls t ON t.id=i.tool_call_id JOIN messages m ON m.id=t.message_id
        WHERE i.objective_id=? AND i.workspace_id=? AND r.state='applied' AND r.actual_success=1 AND t.status='done'
        ORDER BY i.created_at,i.dispatch_id LIMIT 10001")
        .bind(&workspace.objective_id).bind(&workspace.id).fetch_all(db).await?;
    anyhow::ensure!(rows.len() <= 10000, "content dispatch budget exceeded");
    let mut manifests = Vec::new();
    for row in rows {
        let scope: Scope = serde_json::from_str(&row.try_get::<String, _>("scope_json")?)?;
        anyhow::ensure!(
            workspace_matches(&scope, workspace)
                && row.try_get::<String, _>("session_id")? == scope.session
                && row.try_get::<String, _>("trajectory_session")? == scope.session
                && row.try_get::<String, _>("run_instance_id")? == scope.run,
            "completed content scope mismatch"
        );
        let args: serde_json::Value =
            serde_json::from_str(&row.try_get::<String, _>("arguments")?)?;
        anyhow::ensure!(
            action(&row.try_get::<String, _>("tool_name")?, &args, &scope)
                == row.try_get::<String, _>("action_fingerprint")?,
            "content action mismatch"
        );
        let manifest: Manifest = serde_json::from_str(&row.try_get::<String, _>("manifest_json")?)?;
        anyhow::ensure!(
            manifest.valid()
                && manifest.digest(false) == row.try_get::<String, _>("precondition_digest")?
                && manifest.digest(true) == row.try_get::<String, _>("expected_digest")?
                && row
                    .try_get::<Option<String>, _>("observed_digest")?
                    .as_deref()
                    == Some(manifest.digest(true).as_str())
                && row.try_get::<i64, _>("finished_at")? >= row.try_get::<i64, _>("created_at")?,
            "completed content manifest mismatch"
        );
        manifests.push(manifest);
    }
    Ok(manifests)
}
