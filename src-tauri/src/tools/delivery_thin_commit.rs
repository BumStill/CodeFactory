// SPDX-License-Identifier: Apache-2.0
//! The durable owner here covers only a local hook/commit transaction. Thin
//! remains the owner of PR orchestration; recovery never invokes a forge.
use super::*;

pub(super) async fn prepare(
    args: &Value, ctx: &ExecCtx, scope: &delivery::TaskDeliveryScope, excludes: &[String],
    stop_gate: std::sync::Arc<dyn crate::agent::stop_fence::DeliveryStopGate>,
) -> Result<Option<String>> {
    if !scope.requires_commit() { return Ok(None); }
    let db = ctx.db.as_ref().ok_or_else(|| crate::errors::AppError::Other("thin commit requires persistent host identity".into()))?;
    let message = super::super::delivery_thin::prepared_commit_message(args, ctx, scope).await?;
    let run = prepare_durable_run_for_kind(args, ctx, DeliveryCeiling::PrOnly,
        Some(DeliveryCeiling::PrOnly), "thin_local_commit", Some(scope.target_tree_sha())).await?
        .ok_or_else(|| crate::errors::AppError::Other("thin local commit has no durable run".into()))?;
    let mut guard = DeliveryLeaseGuard { db: db.clone(), id: run.id.clone(), process: run.process.clone(), epoch: run.claim_epoch, released: false };
    let result = async {
        let unresolved = all_intents(db, &run.id).await?;
        if unresolved.iter().any(|i| i.status == "started" || i.status == "unknown") {
            return Err(crate::errors::AppError::Other("thin local transaction has an unresolved dispatch; reconcile it before replay, original files preserved".into()));
        }
        if unresolved.iter().filter(|intent| intent.rung == "git_hook_commit_prepare").count() > 1 {
            return Err(crate::errors::AppError::Other("ambiguous hook preparations in one local generation".into()));
        }
        for intent in unresolved.iter().filter(|intent| intent.rung == "git_hook_commit_prepare" && matches!(intent.status.as_str(), "committed" | "reconciled_committed")) {
            verify_recorded_hook_policy(&ctx.cwd, intent)?;
            let envelope: Value = serde_json::from_str(intent.evidence_json.as_deref().unwrap_or("null"))?;
            let result = envelope.get("committed_result").filter(|v| !v.is_null()).unwrap_or(&envelope);
            if result.get("plan").and_then(|v| v.get("message")).and_then(Value::as_str) != Some(message.as_str()) {
                return Err(crate::errors::AppError::Other("previous hook preparation must settle before changing the commit message; hooks were not replayed".into()));
            }
        }
        if !delivery_run::mark_delivery_claim_reconciled(db, &run.id, &run.process, run.claim_epoch,
            chrono::Utc::now().timestamp_millis()).await?
            && !delivery_run::verify_delivery_mutation_permit(db, &run.id, &run.process, run.claim_epoch,
                chrono::Utc::now().timestamp_millis()).await? {
            return Err(crate::errors::AppError::Other("thin local reconciliation lost its claim".into()));
        }
        let mut opts = DeliverOpts { mutation_permit: Some(run.mutation_permit(db)), extra_excludes: excludes.to_vec(), ..Default::default() };
        opts.stop_gate = Some(stop_gate);
        let head = await_delivery_with_lease_heartbeat(db, &run,
            delivery::commit_with_hooks(&ctx.cwd, scope, &opts, &message)).await?
            .map_err(crate::errors::AppError::Other)?;
        settle(db, &run).await?;
        Ok(Some(head))
    }.await;
    guard.release("thin_local_commit_return").await?;
    result
}

async fn settle(db: &sqlx::SqlitePool, run: &PreparedDurableRun) -> Result<()> {
    let (repo, _) = delivery::resolve_delivery_repo(&run.workspace_path, None, Some(&run.head_branch))
        .map_err(crate::errors::AppError::Other)?;
    let current = delivery::capture_delivery_identity(&repo).map_err(crate::errors::AppError::Other)?;
    let revision = build_delivery_identity_revision(run, &current.repo_identity, &current.worktree_identity,
        &current.head_sha, &current.change_set_digest)?;
    let observation = DeliveryObservation {
        head_branch: run.head_branch.clone(), stage: "thin_local_commit_complete".into(),
        status: "completed".into(), wait_class: None, next_action: None,
        reached_ceiling: "committed".into(), expected_head_sha: current.head_sha,
        canonical_pr_number: None, canonical_pr_url: None, canonical_head_sha: None,
        failure_signature: None, core_input: None, identity_revision: revision,
    };
    if !delivery_run::record_delivery_observation(db, &run.id, &run.process, run.claim_epoch,
        &observation, chrono::Utc::now().timestamp_millis(), DELIVERY_LEASE_TTL_MS).await? {
        return Err(crate::errors::AppError::Other("thin local settlement lost owner/epoch".into()));
    }
    Ok(())
}

pub(super) async fn recover(
    db: &sqlx::SqlitePool, claimed: &mut delivery_run::ClaimedRecovery, process: &ProcessIdentity,
) -> Result<()> {
    let (session, root): (Option<String>, Option<String>) = sqlx::query_as("SELECT session_id,root_turn_id FROM delivery_runs WHERE id=? AND run_kind='thin_local_commit'")
        .bind(&claimed.run_id).fetch_one(db).await?;
    let mut opts = DeliverOpts::default();
    if let Some(session) = session {
        opts.stop_gate = Some(super::thin_stop::ThinStopGate::new(db.clone(), session, claimed.objective_id.clone(), root));
        if let Some(reason) = opts.stop_gate.as_ref().unwrap().stop_reason().await {
            return Err(crate::errors::AppError::Other(reason));
        }
    }
    let all = all_intents(db, &claimed.run_id).await?;
    if all.iter().any(|i| i.rung == "git_hook_commit_prepare" && matches!(i.status.as_str(), "started" | "unknown")) {
        sqlx::query("UPDATE delivery_runs SET next_action_authorized=0, stage='thin_hook_result_unknown', wait_class='external_state_uncertain' WHERE id=? AND run_kind='thin_local_commit' AND lease_owner=? AND claim_epoch=?")
            .bind(&claimed.run_id).bind(&process.instance_id).bind(claimed.claim_epoch).execute(db).await?;
        return Err(crate::errors::AppError::Other("hook completion is unknown; no hook replay, local install, or remote action authorized".into()));
    }
    if all.iter().any(|i| !matches!(i.rung.as_str(), "git_hook_commit_prepare" | "git_local_commit"))
        || all.iter().filter(|i| i.rung == "git_hook_commit_prepare").count() > 1
        || all.iter().filter(|i| i.rung == "git_local_commit").count() > 1 {
        return Err(crate::errors::AppError::Other("ambiguous or nonlocal intents in a thin local generation; nothing replayed".into()));
    }
    let latest_hook = all.iter().find(|i| i.rung == "git_hook_commit_prepare");
    let mut intents = Vec::new();
    if let Some(hook) = latest_hook {
        intents.push(hook.clone());
        if matches!(hook.status.as_str(), "committed" | "reconciled_committed") {
            let envelope: Value = serde_json::from_str(hook.evidence_json.as_deref().ok_or_else(|| crate::errors::AppError::Other("hook result missing".into()))?)?;
            let result = envelope.get("committed_result").filter(|v| !v.is_null()).unwrap_or(&envelope);
            let head = result.get("head").and_then(Value::as_str).ok_or_else(|| crate::errors::AppError::Other("hook result head missing".into()))?;
            for local in all.iter().filter(|i| i.rung == "git_local_commit") {
                if let Some(raw) = started_delivery_mutation_evidence(db, &claimed.run_id, &local.intent_id).await? {
                    let evidence: LocalCommitIntentEvidence = serde_json::from_str(&raw)?;
                    if evidence.expected_head_sha == head { intents.push(local.clone()); break; }
                }
            }
        }
    }
    if intents.len() != all.len() {
        return Err(crate::errors::AppError::Other("local intent is not bound to the unique successful hook result".into()));
    }
    let had_local = intents.iter().any(|i| i.rung == "git_local_commit");
    let has_hook_success = intents.iter().any(|i| i.rung == "git_hook_commit_prepare" && matches!(i.status.as_str(), "committed" | "reconciled_committed"));
    if had_local && !has_hook_success {
        return Err(crate::errors::AppError::Other("local commit has no durable successful hook preparation".into()));
    }
    if had_local {
        reconcile_receipted_local_commit_head(db, claimed, process).await?;
        for intent in intents.iter().filter(|i| i.rung == "git_local_commit") {
            if !delivery_run::mark_delivery_mutation_intent_reconciled_committed(db, &intent.intent_id, process,
                claimed.claim_epoch, Some(&json!({"head_sha":claimed.expected_head_sha,"confirmation":"thin_local_commit_matches"}).to_string()),
                chrono::Utc::now().timestamp_millis()).await? {
                return Err(crate::errors::AppError::Other("thin local intent reconciliation lost epoch".into()));
            }
        }
    }
    let run = PreparedDurableRun {
        id: claimed.run_id.clone(), process: process.clone(), claim_epoch: claimed.claim_epoch,
        objective_id: claimed.objective_id.clone(), workspace_path: claimed.workspace_path.clone().into(),
        worktree_identity: claimed.worktree_identity.clone(), repo_identity: claimed.repo_identity.clone(),
        change_set_digest: claimed.change_set_digest.clone(), expected_head_sha: claimed.expected_head_sha.clone(),
        head_branch: claimed.head_branch.clone(),
    };
    if !had_local && has_hook_success {
        verify_recorded_hook_policy(&run.workspace_path, intents.iter().find(|i| i.rung == "git_hook_commit_prepare").unwrap())?;
        // Only a positively settled preparation may continue to the fixed CAS.
        let prepared = intents.iter().find(|i| i.rung == "git_hook_commit_prepare").unwrap();
        let envelope: Value = serde_json::from_str(prepared.evidence_json.as_deref().ok_or_else(|| crate::errors::AppError::Other("hook result missing".into()))?)?;
        let result = envelope.get("committed_result").filter(|v| !v.is_null()).unwrap_or(&envelope);
        if let Some(excludes) = result.get("plan").and_then(|v| v.get("excludes")) {
            opts.extra_excludes = serde_json::from_value(excludes.clone())?;
        }
        let message = result.get("plan").and_then(|v| v.get("message")).and_then(Value::as_str)
            .ok_or_else(|| crate::errors::AppError::Other("hook result message missing".into()))?;
        delivery_run::mark_delivery_claim_reconciled(db, &run.id, process, run.claim_epoch, chrono::Utc::now().timestamp_millis()).await?;
        opts.mutation_permit = Some(run.mutation_permit(db));
        let scope = delivery::TaskDeliveryScope::load(db, &run.objective_id, &run.workspace_path, &opts.extra_excludes).await.map_err(crate::errors::AppError::Other)?;
        delivery::commit_with_hooks(&run.workspace_path, &scope, &opts, message).await.map_err(crate::errors::AppError::Other)?;
    }
    if had_local || has_hook_success { settle(db, &run).await?; }
    // No hook dispatch exists: do not invent one during background recovery.
    // The thin foreground invocation may admit a fresh local preparation.
    delivery_run::release_delivery_lease(db, &run.id, process, run.claim_epoch,
        chrono::Utc::now().timestamp_millis(), "thin_local_recovery_return").await?;
    Ok(())
}

fn verify_recorded_hook_policy(root: &std::path::Path, intent: &delivery_run::DeliveryMutationIntent) -> Result<()> {
    let envelope: Value = serde_json::from_str(intent.evidence_json.as_deref().ok_or_else(|| crate::errors::AppError::Other("hook result missing".into()))?)?;
    let result = envelope.get("committed_result").filter(|v| !v.is_null()).unwrap_or(&envelope);
    let digest = result.get("hook_policy_digest").and_then(Value::as_str)
        .ok_or_else(|| crate::errors::AppError::Other("hook success lacks its frozen policy digest".into()))?;
    delivery::verify_hook_policy(root, digest).map_err(crate::errors::AppError::Other)
}

async fn all_intents(db: &sqlx::SqlitePool, run_id: &str) -> Result<Vec<delivery_run::DeliveryMutationIntent>> {
    Ok(sqlx::query_as::<_, delivery_run::DeliveryMutationIntent>(
        "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status, process_instance, evidence_json, started_at, updated_at FROM delivery_mutation_intents WHERE run_id=? ORDER BY started_at, intent_id",
    ).bind(run_id).fetch_all(db).await?)
}
