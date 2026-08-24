// SPDX-License-Identifier: Apache-2.0
//! `deliver_changes` agent tool — the single call that carries code work
//! through git delivery (commit → push → PR → CI → merge → release) up to the
//! user-configured [`DeliveryCeiling`]. This is the capability whose absence
//! made the agent stall at a green build, re-listing the missing PR instead of
//! opening one. The model invokes this instead of hand-running git in bash.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::future::Future;

#[cfg(test)]
use super::ToolExecutionStatus;
use super::{ExecCtx, ToolOutput};
use crate::agent::delivery::{
    self, DeliverOpts, DeliveryIdentitySnapshot, DeliveryMutationBegin,
    DeliveryMutationCommittedReceipt, DeliveryMutationIntentToken, DeliveryMutationPermit,
    DeliveryMutationPermitVerifier, DeliveryRemote, LocalCommitIntentEvidence, MergeObservation,
    OpenPrObservation, ReleaseUrgency,
};
use crate::agent::delivery_run::{
    self, CoreInputRequest, DeliveryIdentityRevision, DeliveryObservation, NewDeliveryRun,
    ProcessIdentity,
};
use crate::agent::objective::ObjectiveStore;
#[cfg(not(test))]
use crate::agent::objective::{CreateObjective, ObjectiveKind, RecoveryDomain};
use crate::config::settings::DeliveryCeiling;
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};
use crate::util::no_window::NoWindow;

const RECOVERY_SUPERVISOR_POLL_MS: u64 = 15_000;
const DELIVERY_LEASE_HEARTBEAT_MS: u64 = 20_000;
const DELIVERY_LEASE_TTL_MS: i64 = 90_000;

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "deliver_changes".into(),
            description: "Deliver the current code changes through git: stage only the real \
                changed source files (never local noise), commit, push, open (or reuse) a pull \
                request, and — depending on the user's configured delivery ceiling — wait for CI, \
                merge, and release. Call this after tests pass to carry code work to done; \
                do NOT hand-run git in bash and do NOT stop at a green build to describe a missing \
                PR. It generates or repairs the PR README decision block. Idempotent: when a \
                recoverable result supplies next_action, perform that action and call this again \
                to resume the SAME PR; the agent loop bounds retries. Returns the steps taken, the \
                PR URL, and a state (delivered / blocked / noop)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Optional PR/commit title. Defaults to a message derived from the branch + changed files." },
                    "body":  { "type": "string", "description": "Optional PR body." },
                    "release_urgency": {
                        "type": "string",
                        "enum": ["immediate", "hold"],
                        "description": "Optional release-cadence signal. `immediate` is yours to judge: use it when the rubric applies (main path broken, data loss, security bypass, a released version exposing the defect, the user said urgent, or a large self-contained capability just landed). `hold` is NOT yours to set — it stops the entire batch including other people's merges and only the user can clear it. Pass it only when the user explicitly asked to withhold the release, and quote their instruction in `hold_requested_by_user`; an unmandated `hold` is ignored and delivery proceeds normally."
                    },
                    "ceiling": {
                        "type": "string",
                        "enum": ["off", "pr_only", "through_ci_green", "through_merge", "through_release"],
                        "description": "Optional per-call ceiling. Clamped to at most the user's configured ceiling — a call can lower, never raise it. Use `through_ci_green` to WAIT for CI on the PR this branch already has: this tool polls with backoff (10s→60s) and is the supported way to wait. Never shell out to `gh pr checks --watch` or a tight `gh` polling loop — those refresh every 10s until CI ends, exhaust the shared GitHub quota, and the resulting 403s break unrelated releases."
                    },
                    "hold_requested_by_user": {
                        "type": "string",
                        "description": "Required to make `hold` take effect: quote the user's own instruction to withhold this release. Without it a `hold` is dropped, because gating a batch is the user's decision, not the agent's."
                    },
                    "expect_branch": {
                        "type": "string",
                        "description": "Optional guard: the branch you believe you are delivering. This tool has no branch argument — it delivers whatever branch the working directory is on. When resuming a specific PR, pass its head branch here; if the working directory is somewhere else the call stops before touching anything instead of opening a second PR for unrelated work."
                    },
                    "autonomous_completion": {
                        "type": "boolean",
                        "description": "Defaults true once deliver_changes is authorized, so technical waits survive restart and apply the recommended recovery without asking the user to continue. Set false only when the user explicitly limited unattended continuation; it never expands authority or bypasses release/test/signing gates."
                    }
                }
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Some(settings) = ctx.settings.clone() else {
        return Ok(ToolOutput::err("交付不可用:当前上下文没有可用的设置快照。"));
    };

    let title = args.get("title").and_then(Value::as_str).map(String::from);
    let body = args.get("body").and_then(Value::as_str).map(String::from);
    let release_urgency = match release_urgency_from_args(&args) {
        Ok(value) => value,
        Err(message) => return Ok(ToolOutput::err(message)),
    };
    let requested_ceiling = args
        .get("ceiling")
        .and_then(Value::as_str)
        .and_then(parse_ceiling);

    let mut opts = DeliverOpts {
        title,
        body,
        release_urgency,
        requested_ceiling,
        extra_excludes: settings.delivery_exclude_globs.clone(),
        expect_branch: args
            .get("expect_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from),
        expected_identity: None,
        mutation_permit: None,
    };

    let mut durable = match prepare_durable_run(
        &args,
        ctx,
        settings.delivery_ceiling,
        requested_ceiling,
    )
    .await
    {
        Ok(durable) => durable,
        Err(error) if error.to_string().contains("系统身份冲突") => {
            return Ok(ToolOutput::waiting(
                "交付目标身份不唯一；系统将只读核对 objective/worktree 映射，未执行任何交付副作用。",
            )
            .with_metadata(json!({
                "status": "recovering",
                "delivery_state": "waiting",
                "stage": "identity",
                "code": "delivery_identity_incident",
                "recoverable": true,
                "recovery_class": "agent_action_required",
                "decision_type": "system_owned",
                "requires_user_continue": false,
                "retry_after_ms": 1_000,
                "next_action": "只读核对当前 objective 所属 worktree/branch，再用 expect_branch 续接；不得询问用户选择技术身份。",
                "requested_ceiling": requested_ceiling.map(ceiling_label).unwrap_or_else(|| ceiling_label(settings.delivery_ceiling)),
                "reached_state": "local"
            })));
        }
        Err(error) if error.to_string().contains("active invocation") => {
            return Ok(ToolOutput::waiting(
                "同一 objective 已有一条交付执行持有 worktree lease；当前调用已附着等待，未并发执行 stage/commit/push。",
            )
            .with_metadata(json!({
                "status": "recovering",
                "delivery_state": "waiting",
                "stage": "lease",
                "code": "delivery_operation_already_running",
                "recoverable": true,
                "recovery_class": "agent_action_required",
                "decision_type": "system_owned",
                "requires_user_continue": false,
                "retry_after_ms": 1_000,
                "next_action": "attach_existing_delivery_run",
                "requested_ceiling": requested_ceiling.map(ceiling_label).unwrap_or_else(|| ceiling_label(settings.delivery_ceiling)),
                "reached_state": "local"
            })));
        }
        Err(error) => return Err(error),
    };
    if opts.expect_branch.is_none() {
        opts.expect_branch = durable.as_ref().map(|run| run.head_branch.clone());
    }
    opts.expected_identity = durable.as_ref().map(PreparedDurableRun::identity_snapshot);
    opts.mutation_permit = durable
        .as_ref()
        .and_then(|run| ctx.db.as_ref().map(|db| run.mutation_permit(db)));

    let remote = delivery::resolve_delivery_remote(&ctx.cwd, &settings);

    loop {
        let delivery_future = delivery::deliver(
            &ctx.cwd,
            settings.delivery_ceiling,
            settings.delivery_merge_method,
            settings.delivery_ci_timeout_secs,
            &opts,
            remote.as_ref(),
            None,
        );
        let outcome = if let (Some(db), Some(durable)) = (ctx.db.as_ref(), durable.as_ref()) {
            await_delivery_with_lease_heartbeat(db, durable, delivery_future).await?
        } else {
            delivery_future.await
        };

        if let (Some(db), Some(session_id)) = (ctx.db.as_ref(), ctx.session_id.as_deref()) {
            persist_delivery_ref(db, session_id, &outcome).await?;
        }
        if let (Some(db), Some(durable)) = (ctx.db.as_ref(), durable.as_mut()) {
            persist_durable_outcome(db, durable, &outcome).await?;
            opts.expected_identity = Some(durable.identity_snapshot());
        }

        if outcome.validate_contract().is_err() {
            return Ok(tool_output_for_outcome(&outcome));
        }

        if outcome.final_state != "waiting" {
            return Ok(tool_output_for_outcome(&outcome));
        }

        // A waiting delivery remains one in-flight tool call. The shared loop
        // keeps emitting its 30s heartbeat and can cancel by dropping this
        // future; no extra model request or user "continue" is required.
        let retry_after_ms = outcome.retry_after_ms.unwrap_or(30_000).clamp(1, 60_000);
        tokio::time::sleep(std::time::Duration::from_millis(retry_after_ms)).await;
    }
}

struct PreparedDurableRun {
    id: String,
    process: ProcessIdentity,
    claim_epoch: i64,
    objective_id: String,
    workspace_path: std::path::PathBuf,
    worktree_identity: String,
    repo_identity: String,
    change_set_digest: String,
    expected_head_sha: String,
    head_branch: String,
}

impl PreparedDurableRun {
    fn identity_snapshot(&self) -> DeliveryIdentitySnapshot {
        DeliveryIdentitySnapshot {
            repo_identity: self.repo_identity.clone(),
            worktree_identity: self.worktree_identity.clone(),
            head_sha: self.expected_head_sha.clone(),
            change_set_digest: self.change_set_digest.clone(),
        }
    }

    fn mutation_permit(&self, db: &sqlx::SqlitePool) -> DeliveryMutationPermit {
        DeliveryMutationPermit::new(std::sync::Arc::new(DurableDeliveryMutationPermit {
            db: db.clone(),
            run_id: self.id.clone(),
            process: self.process.clone(),
            claim_epoch: self.claim_epoch,
            pause_after_begin: None,
            pause_after_commit: None,
            pause_before_materialize: None,
        }))
    }

    #[cfg(not(test))]
    fn mutation_permit_stopping_after_begin(
        &self,
        db: &sqlx::SqlitePool,
        rung: &str,
        marker: std::path::PathBuf,
    ) -> DeliveryMutationPermit {
        DeliveryMutationPermit::new(std::sync::Arc::new(DurableDeliveryMutationPermit {
            db: db.clone(),
            run_id: self.id.clone(),
            process: self.process.clone(),
            claim_epoch: self.claim_epoch,
            pause_after_begin: Some((rung.into(), marker)),
            pause_after_commit: None,
            pause_before_materialize: None,
        }))
    }

    #[cfg(not(test))]
    fn mutation_permit_stopping_before_materialize(
        &self,
        db: &sqlx::SqlitePool,
        rung: &str,
        marker: std::path::PathBuf,
    ) -> DeliveryMutationPermit {
        DeliveryMutationPermit::new(std::sync::Arc::new(DurableDeliveryMutationPermit {
            db: db.clone(),
            run_id: self.id.clone(),
            process: self.process.clone(),
            claim_epoch: self.claim_epoch,
            pause_after_begin: None,
            pause_after_commit: None,
            pause_before_materialize: Some((rung.into(), marker)),
        }))
    }

    fn mutation_permit_stopping_after_commit(
        &self,
        db: &sqlx::SqlitePool,
        rung: &str,
        marker: std::path::PathBuf,
    ) -> DeliveryMutationPermit {
        DeliveryMutationPermit::new(std::sync::Arc::new(DurableDeliveryMutationPermit {
            db: db.clone(),
            run_id: self.id.clone(),
            process: self.process.clone(),
            claim_epoch: self.claim_epoch,
            pause_after_begin: None,
            pause_after_commit: Some((rung.into(), marker)),
            pause_before_materialize: None,
        }))
    }
}

struct DurableDeliveryMutationPermit {
    db: sqlx::SqlitePool,
    run_id: String,
    process: ProcessIdentity,
    claim_epoch: i64,
    pause_after_begin: Option<(String, std::path::PathBuf)>,
    pause_after_commit: Option<(String, std::path::PathBuf)>,
    pause_before_materialize: Option<(String, std::path::PathBuf)>,
}

#[async_trait::async_trait]
impl DeliveryMutationPermitVerifier for DurableDeliveryMutationPermit {
    async fn verify(&self, rung: &str) -> std::result::Result<(), String> {
        match delivery_run::verify_delivery_mutation_permit(
            &self.db,
            &self.run_id,
            &self.process,
            self.claim_epoch,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "run {} owner {} epoch {} is no longer authorized for {rung}",
                self.run_id, self.process.instance_id, self.claim_epoch
            )),
            Err(error) => Err(format!(
                "run {} epoch {} permit could not be verified before {rung}: {error}",
                self.run_id, self.claim_epoch
            )),
        }
    }

    async fn begin_external_mutation(
        &self,
        rung: &str,
        operation_key: &str,
        evidence: &str,
    ) -> std::result::Result<DeliveryMutationBegin, String> {
        let intent_id = uuid::Uuid::new_v4().to_string();
        let evidence = normalize_mutation_evidence(evidence);
        match delivery_run::begin_delivery_mutation_intent(
            &self.db,
            &intent_id,
            &self.run_id,
            &self.process,
            self.claim_epoch,
            rung,
            operation_key,
            Some(&evidence),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => {
                let token = DeliveryMutationIntentToken {
                    id: intent_id,
                    rung: rung.to_string(),
                    operation_key: operation_key.to_string(),
                };
                if let Some((pause_rung, marker)) = self.pause_after_begin.as_ref() {
                    if pause_rung == rung {
                        let parsed = serde_json::from_str::<serde_json::Value>(&evidence)
                            .unwrap_or(serde_json::Value::Null);
                        std::fs::write(
                            marker,
                            serde_json::to_vec_pretty(&serde_json::json!({
                                "worker_pid": std::process::id(),
                                "pre_ref_fault_injected": true,
                                "rung": rung,
                                "operation_key": operation_key,
                                "previous_head_sha": parsed.get("previous_head_sha"),
                                "expected_head_sha": parsed.get("expected_head_sha"),
                            }))
                            .map_err(|error| error.to_string())?,
                        )
                        .map_err(|error| error.to_string())?;
                        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                        return Err(format!(
                            "delivery recovery smoke was not hard-killed after beginning {rung}"
                        ));
                    }
                }
                Ok(DeliveryMutationBegin::Dispatch(Some(token)))
            }
            Ok(false) => {
                let existing = sqlx::query_as::<_, delivery_run::DeliveryMutationIntent>(
                    "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                            process_instance, evidence_json, started_at, updated_at
                     FROM delivery_mutation_intents
                     WHERE run_id=? AND rung=? AND operation_key=?",
                )
                .bind(&self.run_id)
                .bind(rung)
                .bind(operation_key)
                .fetch_optional(&self.db)
                .await
                .map_err(|error| format!("cannot inspect prior mutation receipt: {error}"))?;
                let still_authorized = delivery_run::verify_delivery_mutation_permit(
                    &self.db,
                    &self.run_id,
                    &self.process,
                    self.claim_epoch,
                    chrono::Utc::now().timestamp_millis(),
                )
                .await
                .map_err(|error| format!("cannot recheck prior mutation receipt authority: {error}"))?;
                match existing {
                    Some(intent)
                        if still_authorized
                            && matches!(
                                intent.status.as_str(),
                                "committed" | "reconciled_committed"
                            ) => Ok(DeliveryMutationBegin::AlreadyCommitted(
                        DeliveryMutationCommittedReceipt {
                            intent_id: intent.intent_id,
                            rung: intent.rung,
                            operation_key: intent.operation_key,
                            result_evidence: intent.evidence_json,
                        },
                    )),
                    _ => Err(format!(
                        "run {} owner {} epoch {} could not durably begin external mutation {rung}; the effect was not dispatched",
                        self.run_id, self.process.instance_id, self.claim_epoch
                    )),
                }
            }
            Err(error) => Err(format!(
                "run {} epoch {} failed to persist external mutation intent {rung}: {error}; the effect was not dispatched",
                self.run_id, self.claim_epoch
            )),
        }
    }

    async fn commit_external_mutation(
        &self,
        intent: &DeliveryMutationIntentToken,
        evidence: &str,
    ) -> std::result::Result<(), String> {
        let evidence = normalize_mutation_evidence(evidence);
        match delivery_run::resolve_delivery_mutation_intent_committed(
            &self.db,
            &intent.id,
            &self.process,
            self.claim_epoch,
            Some(&evidence),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => {
                if let Some((pause_rung, marker)) = self.pause_after_commit.as_ref() {
                    if pause_rung == &intent.rung {
                        std::fs::write(
                            marker,
                            serde_json::to_vec_pretty(&serde_json::json!({
                                "worker_pid": std::process::id(),
                                "post_remote_commit_pre_outcome_fault_injected": true,
                                "rung": intent.rung,
                                "operation_key": intent.operation_key,
                                "result_evidence": serde_json::from_str::<serde_json::Value>(&evidence).unwrap_or(serde_json::Value::Null),
                            }))
                            .map_err(|error| error.to_string())?,
                        )
                        .map_err(|error| error.to_string())?;
                        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                        return Err(format!(
                            "delivery recovery smoke was not hard-killed after committing {}",
                            intent.rung
                        ));
                    }
                }
                Ok(())
            }
            Ok(false) => Err(format!(
                "external mutation {} completed but its durable intent {} could not be committed",
                intent.rung, intent.id
            )),
            Err(error) => Err(format!(
                "external mutation {} completed but durable intent settlement failed: {error}",
                intent.rung
            )),
        }
    }

    async fn mark_external_mutation_unknown(
        &self,
        intent: &DeliveryMutationIntentToken,
        detail: &str,
    ) -> std::result::Result<(), String> {
        let evidence = json!({
            "detail_digest": format!("sha256:{:x}", Sha256::digest(detail.as_bytes())),
            "classification": "external_result_uncertain",
        })
        .to_string();
        match delivery_run::mark_delivery_mutation_intent_unknown(
            &self.db,
            &intent.id,
            &self.process,
            self.claim_epoch,
            Some(&evidence),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "durable intent {} could not be marked unknown",
                intent.id
            )),
            Err(error) => Err(format!(
                "durable intent {} unknown settlement failed: {error}",
                intent.id
            )),
        }
    }

    async fn materialize_local_commit(
        &self,
        intent: &DeliveryMutationIntentToken,
        cwd: &std::path::Path,
        default_branch_hint: Option<&str>,
        expected_branch: &str,
        persisted_identity: &DeliveryIdentitySnapshot,
        evidence: &LocalCommitIntentEvidence,
    ) -> std::result::Result<DeliveryIdentitySnapshot, String> {
        let pause_marker = self
            .pause_before_materialize
            .as_ref()
            .and_then(|(pause_rung, marker)| (pause_rung == &intent.rung).then_some(marker));
        delivery_run::with_receipted_local_commit_cas(
            &self.db,
            &self.run_id,
            &self.process,
            self.claim_epoch,
            &intent.id,
            &intent.operation_key,
            chrono::Utc::now().timestamp_millis(),
            || {
                delivery::materialize_receipted_local_commit_with_fault_marker(
                    cwd,
                    default_branch_hint,
                    expected_branch,
                    persisted_identity,
                    evidence,
                    pause_marker.map(std::path::PathBuf::as_path),
                )
            },
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn materialize_branch_update(
        &self,
        request: &delivery::BranchUpdateMaterialization,
    ) -> std::result::Result<DeliveryIdentitySnapshot, String> {
        let operation_key = request.operation_key();
        let intent = sqlx::query_as::<_, delivery_run::DeliveryMutationIntent>(
            "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                    process_instance, evidence_json, started_at, updated_at
             FROM delivery_mutation_intents
             WHERE run_id=? AND rung='provider_pr_branch_update' AND operation_key=?
               AND status IN ('committed','reconciled_committed')",
        )
        .bind(&self.run_id)
        .bind(&operation_key)
        .fetch_optional(&self.db)
        .await
        .map_err(|error| format!("cannot inspect committed branch-update receipt: {error}"))?
        .ok_or_else(|| {
            "exact committed branch-update receipt is absent; local branch was not advanced"
                .to_string()
        })?;
        let envelope: serde_json::Value = intent
            .evidence_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .ok_or_else(|| "committed branch-update receipt has no result evidence".to_string())?;
        let result = envelope
            .get("committed_result")
            .filter(|value| !value.is_null())
            .unwrap_or(&envelope);
        if result.get("head").and_then(serde_json::Value::as_str)
            != Some(request.next_head_sha.as_str())
        {
            return Err(
                "committed branch-update receipt does not match the requested new head".into(),
            );
        }
        delivery_run::with_receipted_branch_update_cas(
            &self.db,
            &self.run_id,
            &self.process,
            self.claim_epoch,
            &intent.intent_id,
            &operation_key,
            chrono::Utc::now().timestamp_millis(),
            || delivery::materialize_fetched_branch_update(request),
        )
        .await
        .map_err(|error| error.to_string())
    }
}

fn normalize_mutation_evidence(evidence: &str) -> String {
    serde_json::from_str::<serde_json::Value>(evidence)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| {
            json!({
                "detail_digest": format!("sha256:{:x}", Sha256::digest(evidence.as_bytes())),
            })
            .to_string()
        })
}

async fn drive_delivery_with_lease_heartbeat<F, T, Tick, TickFuture, Clock>(
    db: &sqlx::SqlitePool,
    run_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    operation: F,
    mut next_tick: Tick,
    mut clock: Clock,
    lease_ttl_ms: i64,
) -> Result<T>
where
    F: Future<Output = T>,
    Tick: FnMut() -> TickFuture,
    TickFuture: Future<Output = ()>,
    Clock: FnMut() -> i64,
{
    let initial_now = clock();
    if !delivery_run::renew_delivery_lease(
        db,
        run_id,
        process,
        claim_epoch,
        initial_now,
        lease_ttl_ms,
    )
    .await?
    {
        return Err(crate::errors::AppError::Other(
            "delivery lease was lost before the in-flight operation started; no new delivery operation was polled"
                .into(),
        ));
    }
    let mut last_confirmed_expiry = initial_now.saturating_add(lease_ttl_ms);

    tokio::pin!(operation);
    loop {
        tokio::select! {
            result = operation.as_mut() => return Ok(result),
            _ = next_tick() => {
                let heartbeat_now = clock();
                match delivery_run::renew_delivery_lease(
                    db,
                    run_id,
                    process,
                    claim_epoch,
                    heartbeat_now,
                    lease_ttl_ms,
                ).await {
                    Ok(true) => {
                        last_confirmed_expiry = heartbeat_now.saturating_add(lease_ttl_ms);
                    }
                    Ok(false) => {
                        tracing::error!(
                            run_id,
                            owner = %process.instance_id,
                            claim_epoch,
                            "delivery lease heartbeat lost ownership; dropping the stale operation before it can cross another mutation rung"
                        );
                        return Err(crate::errors::AppError::Other(
                            "delivery lease ownership changed while an operation was in flight; external state is uncertain and requires observe-only reconciliation"
                                .into(),
                        ));
                    }
                    Err(error) => {
                        // A transient SQLite failure may be retried only while the
                        // last positively-confirmed lease is still live. Once that
                        // boundary is crossed, continuing would let a competitor
                        // claim the run while this future can still mutate.
                        if heartbeat_now >= last_confirmed_expiry {
                            tracing::error!(run_id, claim_epoch, %error, "delivery lease became uncertain at expiry; dropping the stale operation for observe-only reconciliation");
                            return Err(crate::errors::AppError::Other(
                                "delivery lease could not be renewed before its confirmed expiry; external state is uncertain and requires observe-only reconciliation"
                                    .into(),
                            ));
                        }
                        tracing::warn!(run_id, claim_epoch, %error, last_confirmed_expiry, "delivery lease heartbeat failed transiently before confirmed expiry");
                    }
                }
            }
        }
    }
}

async fn await_delivery_with_lease_heartbeat<F, T>(
    db: &sqlx::SqlitePool,
    durable: &PreparedDurableRun,
    operation: F,
) -> Result<T>
where
    F: Future<Output = T>,
{
    drive_delivery_with_lease_heartbeat(
        db,
        &durable.id,
        &durable.process,
        durable.claim_epoch,
        operation,
        || {
            tokio::time::sleep(std::time::Duration::from_millis(
                DELIVERY_LEASE_HEARTBEAT_MS,
            ))
        },
        || chrono::Utc::now().timestamp_millis(),
        DELIVERY_LEASE_TTL_MS,
    )
    .await
}

async fn prepare_durable_run(
    args: &Value,
    ctx: &ExecCtx,
    configured_ceiling: DeliveryCeiling,
    requested_ceiling: Option<DeliveryCeiling>,
) -> Result<Option<PreparedDurableRun>> {
    let Some(db) = ctx.db.as_ref() else {
        return Ok(None);
    };
    let expected_branch = args
        .get("expect_branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let (objective_id, task_segment_id) = durable_objective_identity(db, ctx).await?;
    if ctx.task_id.is_none() && crate::agent::execution_workspace::is_git_repository(&ctx.cwd) {
        crate::agent::execution_workspace::verify_objective_workspace(db, &objective_id, &ctx.cwd)
            .await
            .map_err(|error| {
                crate::errors::AppError::Other(format!(
                "系统身份冲突: deliver_changes cwd is not the Objective managed workspace: {error}"
            ))
            })?;
    }
    let (repo, _) = delivery::resolve_delivery_repo(&ctx.cwd, None, expected_branch)
        .map_err(crate::errors::AppError::Other)?;
    let identity =
        delivery::capture_delivery_identity(&repo).map_err(crate::errors::AppError::Other)?;
    let repo_identity = identity.repo_identity;
    let expected_head_sha = identity.head_sha;
    let change_set_digest = identity.change_set_digest;
    let worktree_identity = identity.worktree_identity;
    let id = durable_run_id(&objective_id, &repo_identity);
    let process = foreground_delivery_process_identity();
    let selected_ceiling = requested_ceiling
        .map(|requested| configured_ceiling.clamp_request(requested))
        .unwrap_or(configured_ceiling);
    let head_branch = repo.branch.clone();
    let run = NewDeliveryRun {
        id: id.clone(),
        objective_id,
        run_kind: "deliver_changes".into(),
        session_id: ctx.session_id.clone(),
        root_turn_id: ctx.root_turn_id.clone(),
        task_segment_id,
        task_id: ctx.task_id.clone(),
        workspace_path: repo.root.to_string_lossy().into_owned(),
        worktree_identity,
        repo_identity,
        base_branch: repo.default_branch,
        head_branch: head_branch.clone(),
        change_set_digest,
        expected_head_sha: expected_head_sha.clone(),
        canonical_pr_number: None,
        canonical_pr_url: None,
        canonical_head_sha: None,
        requested_ceiling: ceiling_label(selected_ceiling).into(),
        reached_ceiling: "local".into(),
        stage: "preflight".into(),
        status: "running".into(),
        wait_class: None,
        next_action: Some("deliver".into()),
        next_action_authorized: true,
        autonomous_completion: autonomous_completion_from_args(args),
    };
    let claim_epoch = delivery_run::create_delivery_run(
        db,
        &run,
        &process,
        chrono::Utc::now().timestamp_millis(),
        90_000,
    )
    .await?;
    Ok(Some(PreparedDurableRun {
        id,
        process,
        claim_epoch,
        objective_id: run.objective_id,
        workspace_path: repo.root,
        worktree_identity: run.worktree_identity,
        repo_identity: run.repo_identity,
        change_set_digest: run.change_set_digest,
        expected_head_sha,
        head_branch,
    }))
}

fn foreground_delivery_process_identity() -> ProcessIdentity {
    ProcessIdentity::new(
        format!(
            "{}:{}:delivery:{}",
            std::process::id(),
            crate::storage::db::current_process_start_token()
                .unwrap_or_else(|| "unknown-start".into()),
            uuid::Uuid::new_v4(),
        ),
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    )
}

fn autonomous_completion_from_args(args: &Value) -> bool {
    // Invoking deliver_changes is already the user's authorization to reach
    // the selected ceiling. Technical waits/restarts must not manufacture a
    // second confirmation gate; only an explicit false opts out.
    args.get("autonomous_completion")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

async fn persist_durable_outcome(
    db: &sqlx::SqlitePool,
    durable: &mut PreparedDurableRun,
    outcome: &delivery::DeliveryOutcome,
) -> Result<()> {
    let core_input = core_input_request_for_outcome(outcome);
    let status = match outcome.final_state.as_str() {
        "delivered" | "noop" => "awaiting_completion_arbitration",
        "waiting" => "waiting",
        _ if core_input.is_some() => "core_input_required",
        _ if outcome.recoverable => "agent_action_required",
        _ if outcome.recovery_class == delivery::RecoveryClass::ExternalStateUncertain => {
            "platform_incident"
        }
        _ => "failed_internal",
    };
    let recovery_class = serde_json::to_value(outcome.recovery_class)
        .ok()
        .and_then(|value| value.as_str().map(String::from));
    let expected_head_sha = outcome
        .commit_sha
        .clone()
        .unwrap_or_else(|| durable.expected_head_sha.clone());
    let identity_revision = if expected_head_sha != durable.expected_head_sha {
        let (observed_repo, _) = delivery::resolve_delivery_repo(
            &durable.workspace_path,
            None,
            Some(&durable.head_branch),
        )
        .map_err(crate::errors::AppError::Other)?;
        let observed_identity = delivery::capture_delivery_identity(&observed_repo)
            .map_err(crate::errors::AppError::Other)?;
        if observed_identity.head_sha != expected_head_sha {
            return Err(crate::errors::AppError::Other(format!(
                "delivery outcome head {} does not match observed workspace head {}",
                expected_head_sha, observed_identity.head_sha
            )));
        }
        build_delivery_identity_revision(
            durable,
            &observed_identity.repo_identity,
            &observed_identity.worktree_identity,
            &observed_identity.head_sha,
            &observed_identity.change_set_digest,
        )?
    } else {
        None
    };
    let observation = DeliveryObservation {
        head_branch: outcome
            .branch
            .clone()
            .unwrap_or_else(|| durable.head_branch.clone()),
        stage: outcome.stage.clone(),
        status: status.into(),
        wait_class: recovery_class,
        next_action: outcome.next_action.clone(),
        reached_ceiling: outcome.reached_state.clone(),
        expected_head_sha: expected_head_sha.clone(),
        canonical_pr_number: outcome.pr_number.map(|value| value as i64),
        canonical_pr_url: outcome.pr_url.clone(),
        canonical_head_sha: outcome.pr_number.map(|_| expected_head_sha),
        failure_signature: (outcome.final_state != "delivered" && outcome.final_state != "noop")
            .then(|| outcome.code.clone()),
        core_input,
        identity_revision: identity_revision.clone(),
    };
    delivery_run::record_delivery_observation(
        db,
        &durable.id,
        &durable.process,
        durable.claim_epoch,
        &observation,
        chrono::Utc::now().timestamp_millis(),
        outcome
            .retry_after_ms
            .unwrap_or(30_000)
            .saturating_add(60_000) as i64,
    )
    .await?;
    if let Some(revision) = identity_revision {
        durable.expected_head_sha = revision.next_expected_head_sha;
        durable.change_set_digest = revision.next_change_set_digest;
    }
    Ok(())
}

fn build_delivery_identity_revision(
    durable: &PreparedDurableRun,
    observed_repo_identity: &str,
    observed_worktree_identity: &str,
    observed_head_sha: &str,
    observed_change_set_digest: &str,
) -> Result<Option<DeliveryIdentityRevision>> {
    if observed_repo_identity != durable.repo_identity
        || observed_worktree_identity != durable.worktree_identity
    {
        return Err(crate::errors::AppError::Other(
            "delivery identity revision observed a different repo/worktree; refused before persistence"
                .into(),
        ));
    }
    if observed_head_sha == durable.expected_head_sha {
        return Ok(None);
    }
    if observed_head_sha.is_empty() || observed_change_set_digest.is_empty() {
        return Err(crate::errors::AppError::Other(
            "delivery identity revision requires a resolved head and change-set digest".into(),
        ));
    }

    let mut revision = DeliveryIdentityRevision {
        receipt_id: String::new(),
        objective_id: durable.objective_id.clone(),
        repo_identity: durable.repo_identity.clone(),
        worktree_identity: durable.worktree_identity.clone(),
        previous_expected_head_sha: durable.expected_head_sha.clone(),
        previous_change_set_digest: durable.change_set_digest.clone(),
        next_expected_head_sha: observed_head_sha.into(),
        next_change_set_digest: observed_change_set_digest.into(),
    };
    revision.receipt_id =
        delivery_run::delivery_identity_revision_receipt_id(&durable.id, &revision);
    Ok(Some(revision))
}

fn core_input_request_for_outcome(outcome: &delivery::DeliveryOutcome) -> Option<CoreInputRequest> {
    if outcome.recovery_class != delivery::RecoveryClass::CoreInputRequired {
        return None;
    }
    let mut missing = Vec::new();
    if let Some(capability) = outcome.capability_gap.as_deref() {
        missing.push(serde_json::json!({
            "kind": "capability",
            "key": capability,
        }));
    }
    if let Some(next_action) = outcome.next_action.as_deref() {
        missing.push(serde_json::json!({
            "kind": "required_input",
            "key": outcome.code,
            "resume_action": next_action,
        }));
    }
    if missing.is_empty() {
        return None;
    }
    Some(CoreInputRequest {
        request_key: outcome.code.clone(),
        inputs_json: serde_json::Value::Array(missing).to_string(),
        attempts_json: serde_json::json!({
            "alternatives_exhausted": true,
            "observed_stages": outcome.steps.iter().map(|step| &step.step).collect::<Vec<_>>(),
        })
        .to_string(),
        resume_stage: outcome.stage.clone(),
    })
}

async fn reconcile_unresolved_delivery_mutation_intents<R: DeliveryRemote>(
    db: &sqlx::SqlitePool,
    claimed: &delivery_run::ClaimedRecovery,
    process: &ProcessIdentity,
    remote: Option<&R>,
    takeover: &mut delivery::DeliveryTakeoverObservation,
) -> Result<bool> {
    let intents =
        delivery_run::list_unresolved_delivery_mutation_intents(db, &claimed.run_id).await?;
    // A settled release intent still proves that durable DB begin happened.
    // In particular, the old owner can commit the provider result and crash
    // before upgrading the local receipt from `intent_release`. Looking only
    // at unresolved intents would then authorize an unsafe second dispatch if
    // the provider's read API is briefly eventually consistent.
    let mut release_intent_seen =
        delivery_run_has_release_mutation_intent(db, &claimed.run_id).await?;
    for intent in intents {
        let confirmation = match intent.rung.as_str() {
            "git_local_commit" => {
                let started =
                    started_delivery_mutation_evidence(db, &claimed.run_id, &intent.intent_id)
                        .await?
                        .ok_or_else(|| {
                            crate::errors::AppError::Other(
                                "unresolved local commit lacks immutable write-ahead evidence"
                                    .into(),
                            )
                        })?;
                let evidence: LocalCommitIntentEvidence =
                    serde_json::from_str(&started).map_err(|error| {
                        crate::errors::AppError::Other(format!(
                            "unresolved local commit evidence is invalid: {error}"
                        ))
                    })?;
                if evidence.operation_key() != intent.operation_key {
                    return Err(crate::errors::AppError::Other(
                        "unresolved local commit operation identity does not match its receipt"
                            .into(),
                    ));
                }
                let previous = DeliveryIdentitySnapshot {
                    repo_identity: evidence.repo_identity.clone(),
                    worktree_identity: evidence.worktree_identity.clone(),
                    head_sha: evidence.previous_head_sha.clone(),
                    change_set_digest: evidence.previous_change_set_digest.clone(),
                };
                let observed = delivery::observe_receipted_local_commit(
                    std::path::Path::new(&claimed.workspace_path),
                    Some(&claimed.base_branch),
                    &claimed.head_branch,
                    &previous,
                    &evidence,
                )
                .map_err(crate::errors::AppError::Other)?;
                if observed != takeover.identity || observed.head_sha != claimed.expected_head_sha {
                    return Err(crate::errors::AppError::Other(
                        "unresolved local commit does not match the revised durable head".into(),
                    ));
                }
                json!({
                    "confirmation": "local_commit_matches",
                    "head_sha": observed.head_sha,
                    "tree_sha": evidence.staged_tree_sha,
                })
            }
            "git_push" => {
                if takeover.remote_head_sha.as_deref() != Some(claimed.expected_head_sha.as_str()) {
                    return Err(crate::errors::AppError::Other(format!(
                        "unresolved git push {} has no positive matching remote-head observation",
                        intent.intent_id
                    )));
                }
                let expected_key = delivery::external_operation_key(
                    "git_push",
                    &[
                        &claimed.repo_identity,
                        &claimed.head_branch,
                        &claimed.expected_head_sha,
                    ],
                );
                if expected_key != intent.operation_key {
                    return Err(crate::errors::AppError::Other(
                        "unresolved git push operation identity does not match the durable run"
                            .into(),
                    ));
                }
                json!({
                    "confirmation": "remote_head_matches",
                    "remote_head_sha": claimed.expected_head_sha,
                })
            }
            "provider_pr_create" | "provider_pr_open_or_get" => {
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "unresolved PR creation has no read-only provider observer".into(),
                    )
                })?;
                let state = match remote
                    .observe_open_pr(&claimed.head_branch, &claimed.base_branch)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    OpenPrObservation::Open(state) => state,
                    OpenPrObservation::Absent | OpenPrObservation::Unsupported => {
                        return Err(crate::errors::AppError::Other(
                            "unresolved PR creation is not positively visible yet; absence is not replay authority"
                                .into(),
                        ))
                    }
                };
                if state.head_sha.as_deref() != Some(claimed.expected_head_sha.as_str()) {
                    return Err(crate::errors::AppError::Other(
                        "observed PR does not prove the persisted delivery head".into(),
                    ));
                }
                let expected_key = delivery::external_operation_key(
                    &intent.rung,
                    &[
                        &state.pr.title,
                        &state.pr.body,
                        &claimed.head_branch,
                        &claimed.base_branch,
                        &claimed.expected_head_sha,
                    ],
                );
                if expected_key != intent.operation_key {
                    return Err(crate::errors::AppError::Other(
                        "observed PR title/body/head/base does not match the unresolved operation"
                            .into(),
                    ));
                }
                if takeover.canonical_pr_number.is_some()
                    && takeover.canonical_pr_number != Some(state.pr.number)
                {
                    return Err(crate::errors::AppError::Other(
                        "observed PR conflicts with the durable canonical PR number".into(),
                    ));
                }
                if takeover.canonical_pr_url.is_some()
                    && takeover.canonical_pr_url.as_deref() != Some(state.pr.url.as_str())
                {
                    return Err(crate::errors::AppError::Other(
                        "observed PR conflicts with the durable canonical PR URL".into(),
                    ));
                }
                if intent.status == "committed" {
                    let committed: serde_json::Value = intent
                        .evidence_json
                        .as_deref()
                        .and_then(|value| serde_json::from_str(value).ok())
                        .ok_or_else(|| {
                            crate::errors::AppError::Other(
                                "committed PR creation lacks its exact provider result".into(),
                            )
                        })?;
                    let committed = committed
                        .get("committed_result")
                        .filter(|value| !value.is_null())
                        .unwrap_or(&committed);
                    if committed
                        .get("pr_number")
                        .and_then(serde_json::Value::as_u64)
                        != Some(state.pr.number)
                        || committed.get("pr_url").and_then(serde_json::Value::as_str)
                            != Some(state.pr.url.as_str())
                    {
                        return Err(crate::errors::AppError::Other(
                            "fresh PR observation conflicts with the committed provider PR identity"
                                .into(),
                        ));
                    }
                }
                takeover.canonical_pr_number = Some(state.pr.number);
                takeover.canonical_pr_url = Some(state.pr.url.clone());
                takeover.canonical_head_sha = Some(claimed.expected_head_sha.clone());
                json!({
                    "confirmation": "open_pr_matches",
                    "pr_number": state.pr.number,
                    "pr_url": state.pr.url,
                    "head_sha": claimed.expected_head_sha,
                })
            }
            "provider_pr_body_update" => {
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "unresolved PR body update has no read-only provider observer".into(),
                    )
                })?;
                let state = match remote
                    .observe_open_pr(&claimed.head_branch, &claimed.base_branch)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    OpenPrObservation::Open(state) => state,
                    OpenPrObservation::Absent | OpenPrObservation::Unsupported => {
                        return Err(crate::errors::AppError::Other(
                            "unresolved PR body update is not positively observable".into(),
                        ))
                    }
                };
                let number_text = state.pr.number.to_string();
                if state.head_sha.as_deref() != Some(claimed.expected_head_sha.as_str()) {
                    return Err(crate::errors::AppError::Other(
                        "observed PR body is attached to a foreign head; no update was replayed"
                            .into(),
                    ));
                }
                let expected_key = delivery::external_operation_key(
                    "provider_pr_body_update",
                    &[
                        &number_text,
                        &state.pr.body,
                        &claimed.head_branch,
                        &claimed.base_branch,
                        &claimed.expected_head_sha,
                    ],
                );
                if expected_key != intent.operation_key
                    || claimed.canonical_pr_number != Some(state.pr.number as i64)
                {
                    return Err(crate::errors::AppError::Other(
                        "observed PR body does not match the unresolved update".into(),
                    ));
                }
                json!({
                    "confirmation": "pr_body_matches",
                    "pr_number": state.pr.number,
                    "body_digest": delivery::external_operation_key("body", &[&state.pr.body]),
                })
            }
            "provider_ci_rerun" => {
                if intent.status != "committed" {
                    return Err(crate::errors::AppError::Other(
                        "an uncertain CI rerun has no exact provider request observer; it remains observe-only"
                            .into(),
                    ));
                }
                let result = intent.evidence_json.as_deref().unwrap_or("null");
                json!({
                    "confirmation": "committed_result_receipt",
                    "result_digest": format!("sha256:{:x}", Sha256::digest(result.as_bytes())),
                })
            }
            "provider_pr_branch_update" => {
                if intent.status != "committed" {
                    return Err(crate::errors::AppError::Other(
                        "an uncertain PR branch update lacks a committed exact result".into(),
                    ));
                }
                let result: serde_json::Value = intent
                    .evidence_json
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok())
                    .ok_or_else(|| {
                        crate::errors::AppError::Other(
                            "committed PR branch update lacks result evidence".into(),
                        )
                    })?;
                let head = result
                    .get("head")
                    .and_then(serde_json::Value::as_str)
                    .filter(|head| !head.is_empty())
                    .ok_or_else(|| {
                        crate::errors::AppError::Other(
                            "committed PR branch update lacks the resulting head".into(),
                        )
                    })?;
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "committed PR branch update has no read-only provider observer".into(),
                    )
                })?;
                let state = match remote
                    .observe_open_pr(&claimed.head_branch, &claimed.base_branch)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    OpenPrObservation::Open(state) => state,
                    _ => {
                        return Err(crate::errors::AppError::Other(
                            "committed PR branch update is not positively visible".into(),
                        ))
                    }
                };
                if state.head_sha.as_deref() != Some(head) {
                    return Err(crate::errors::AppError::Other(
                        "committed PR branch update result differs from the current PR head".into(),
                    ));
                }
                json!({
                    "confirmation": "committed_result_receipt",
                    "result_digest": format!("sha256:{:x}", Sha256::digest(result.to_string().as_bytes())),
                })
            }
            "provider_pr_merge" => {
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "unresolved PR merge has no read-only provider observer".into(),
                    )
                })?;
                let started_evidence =
                    started_delivery_mutation_evidence(db, &claimed.run_id, &intent.intent_id)
                        .await?;
                let evidence: serde_json::Value = started_evidence
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok())
                    .ok_or_else(|| {
                        crate::errors::AppError::Other(
                            "unresolved PR merge lacks its durable operation envelope".into(),
                        )
                    })?;
                let number = evidence
                    .get("pr_number")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| {
                        crate::errors::AppError::Other(
                            "unresolved PR merge lacks its PR number".into(),
                        )
                    })?;
                let method = evidence
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let expected_head = evidence
                    .get("expected_head")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let number_text = number.to_string();
                let expected_key = delivery::external_operation_key(
                    "provider_pr_merge",
                    &[&number_text, method, expected_head],
                );
                if expected_key != intent.operation_key
                    || expected_head != claimed.expected_head_sha
                    || claimed.canonical_pr_number != Some(number as i64)
                {
                    return Err(crate::errors::AppError::Other(
                        "unresolved PR merge identity does not match the durable run".into(),
                    ));
                }
                match remote
                    .observe_merge(number, expected_head)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    MergeObservation::Merged { merge_sha } => json!({
                        "confirmation": "merge_observed",
                        "pr_number": number,
                        "merge_sha": merge_sha,
                    }),
                    MergeObservation::OpenSameHead { auto_merge: true } => json!({
                        "confirmation": "auto_merge_observed",
                        "pr_number": number,
                        "head_sha": expected_head,
                    }),
                    _ => {
                        return Err(crate::errors::AppError::Other(
                            "unresolved PR merge has no positive merged/queued observation".into(),
                        ))
                    }
                }
            }
            "provider_release_trigger" => {
                release_intent_seen = true;
                let remote = remote.ok_or_else(|| {
                    crate::errors::AppError::Other(
                        "unresolved release trigger has no read-only release observer".into(),
                    )
                })?;
                // `unknown` settlement records uncertainty detail and may
                // replace the row's current evidence. The immutable started
                // event retains the original operation envelope, so takeover
                // resolves identity from that ledger rather than losing the
                // workflow/ref/head boundary after an in-flight failure.
                let started_evidence =
                    started_delivery_mutation_evidence(db, &claimed.run_id, &intent.intent_id)
                        .await?;
                let target: delivery::ReleaseDispatchTarget = intent
                    .evidence_json
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok())
                    .or_else(|| {
                        started_evidence
                            .as_deref()
                            .and_then(|value| serde_json::from_str(value).ok())
                    })
                    .ok_or_else(|| {
                        crate::errors::AppError::Other(
                            "unresolved release trigger lacks an exact workflow/ref/head envelope"
                                .into(),
                        )
                    })?;
                if target.git_ref != claimed.base_branch
                    || target.workflow.trim().is_empty()
                    || target.head_sha.trim().is_empty()
                    || target.operation_key() != intent.operation_key
                {
                    return Err(crate::errors::AppError::Other(
                        "unresolved release trigger identity does not match its durable workflow/ref/head operation"
                            .into(),
                    ));
                }
                match remote
                    .observe_release_dispatch(&target)
                    .await
                    .map_err(crate::errors::AppError::Other)?
                {
                    delivery::ReleaseDispatchObservation::Triggered {
                        run_id,
                        status,
                        head_sha,
                        detail,
                    } if head_sha == target.head_sha => json!({
                        "confirmation": "release_observed",
                        "workflow": target.workflow,
                        "git_ref": target.git_ref,
                        "head_sha": head_sha,
                        "run_id": run_id,
                        "status": status,
                        "detail_digest": format!("sha256:{:x}", Sha256::digest(detail.as_bytes())),
                    }),
                    delivery::ReleaseDispatchObservation::Absent => {
                        return Err(crate::errors::AppError::Other(
                            "unresolved release POST may still be in flight; exact absence is observation-only and never replay authority"
                                .into(),
                        ));
                    }
                    delivery::ReleaseDispatchObservation::Triggered { head_sha, .. } => {
                        return Err(crate::errors::AppError::Other(format!(
                            "observed release dispatch head {head_sha} does not match durable {}",
                            target.head_sha
                        )));
                    }
                    delivery::ReleaseDispatchObservation::HeadMismatch { observed_heads } => {
                        return Err(crate::errors::AppError::Other(format!(
                            "release workflow/ref has only nonmatching heads [{}]",
                            observed_heads.join(", ")
                        )));
                    }
                    delivery::ReleaseDispatchObservation::Unsupported(detail) => {
                        return Err(crate::errors::AppError::Other(format!(
                            "unresolved release trigger has no exact read-only observer: {detail}"
                        )));
                    }
                }
            }
            _ => {
                return Err(crate::errors::AppError::Other(format!(
                "unresolved delivery mutation rung {} has no safe positive reconciliation adapter",
                intent.rung
            )))
            }
        };
        let evidence = json!({
            "rung": intent.rung,
            "operation_key": intent.operation_key,
            "observation": confirmation,
            "committed_result": if intent.status == "committed" {
                intent.evidence_json
                    .as_deref()
                    .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
            } else {
                None
            },
        })
        .to_string();
        if !delivery_run::mark_delivery_mutation_intent_reconciled_committed(
            db,
            &intent.intent_id,
            process,
            claimed.claim_epoch,
            Some(&evidence),
            chrono::Utc::now().timestamp_millis(),
        )
        .await?
        {
            return Err(crate::errors::AppError::Other(format!(
                "delivery mutation intent {} lost its takeover epoch before reconciliation",
                intent.intent_id
            )));
        }
    }
    Ok(release_intent_seen)
}

async fn delivery_run_has_release_mutation_intent(
    db: &sqlx::SqlitePool,
    run_id: &str,
) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_mutation_intents
         WHERE run_id=? AND rung='provider_release_trigger'",
    )
    .bind(run_id)
    .fetch_one(db)
    .await?;
    Ok(count > 0)
}

async fn started_delivery_mutation_evidence(
    db: &sqlx::SqlitePool,
    run_id: &str,
    intent_id: &str,
) -> Result<Option<String>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT detail_json FROM delivery_run_events
         WHERE run_id=? AND event_kind='mutation_intent_started'
         ORDER BY created_at, id",
    )
    .bind(run_id)
    .fetch_all(db)
    .await?;
    for detail in rows {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&detail) else {
            continue;
        };
        if value.get("intent_id").and_then(serde_json::Value::as_str) != Some(intent_id) {
            continue;
        }
        return Ok(value.get("evidence").map(serde_json::Value::to_string));
    }
    Ok(None)
}

fn local_takeover_identity_conflict_observation(
    claimed: &delivery_run::ClaimedRecovery,
) -> DeliveryObservation {
    DeliveryObservation {
        head_branch: claimed.head_branch.clone(),
        stage: "takeover_reconciliation".into(),
        status: "platform_incident".into(),
        wait_class: Some("delivery_identity_conflict".into()),
        next_action: Some("await_system_capability_change".into()),
        reached_ceiling: claimed.reached_ceiling.clone(),
        expected_head_sha: claimed.expected_head_sha.clone(),
        canonical_pr_number: claimed.canonical_pr_number,
        canonical_pr_url: claimed.canonical_pr_url.clone(),
        canonical_head_sha: claimed.canonical_head_sha.clone(),
        failure_signature: Some("takeover_reconciliation:delivery_identity_conflict".into()),
        core_input: None,
        identity_revision: None,
    }
}

async fn reconcile_receipted_branch_update_head<R: delivery::DeliveryRemote>(
    db: &sqlx::SqlitePool,
    claimed: &mut delivery_run::ClaimedRecovery,
    process: &ProcessIdentity,
    remote: Option<&R>,
) -> Result<()> {
    let Some(intent) = sqlx::query_as::<_, delivery_run::DeliveryMutationIntent>(
        "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                process_instance, evidence_json, started_at, updated_at
         FROM delivery_mutation_intents
         WHERE run_id=? AND rung='provider_pr_branch_update'
           AND status IN ('committed','reconciled_committed')
         ORDER BY started_at DESC, intent_id DESC LIMIT 1",
    )
    .bind(&claimed.run_id)
    .fetch_optional(db)
    .await?
    else {
        return Ok(());
    };
    if intent.status == "reconciled_committed" {
        return Ok(());
    }
    let started = started_delivery_mutation_evidence(db, &claimed.run_id, &intent.intent_id)
        .await?
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "committed branch update lacks immutable request evidence".into(),
            )
        })?;
    let started: serde_json::Value = serde_json::from_str(&started).map_err(|error| {
        crate::errors::AppError::Other(format!(
            "committed branch update has invalid request evidence: {error}"
        ))
    })?;
    let number = started
        .get("pr_number")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "committed branch update request omitted its PR number".into(),
            )
        })?;
    let previous_head = started
        .get("expected_head")
        .and_then(serde_json::Value::as_str)
        .filter(|head| !head.is_empty())
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "committed branch update request omitted its previous head".into(),
            )
        })?;
    let number_text = number.to_string();
    let expected_key = delivery::external_operation_key(
        "provider_pr_branch_update",
        &[&number_text, previous_head],
    );
    if expected_key != intent.operation_key || claimed.canonical_pr_number != Some(number as i64) {
        return Err(crate::errors::AppError::Other(
            "committed branch update does not extend this durable run identity".into(),
        ));
    }
    let result: serde_json::Value = intent
        .evidence_json
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "committed branch update lacks exact result evidence".into(),
            )
        })?;
    let result_value = result
        .get("committed_result")
        .filter(|value| !value.is_null())
        .unwrap_or(&result);
    let next_head = result_value
        .get("head")
        .and_then(serde_json::Value::as_str)
        .filter(|head| !head.is_empty())
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "committed branch update result omitted its exact new head".into(),
            )
        })?;
    if claimed.expected_head_sha != previous_head && claimed.expected_head_sha != next_head {
        return Err(crate::errors::AppError::Other(
            "committed branch update does not extend this durable run identity".into(),
        ));
    }
    let cwd = std::path::PathBuf::from(&claimed.workspace_path);
    let (repo, _) = delivery::resolve_delivery_repo(
        &cwd,
        Some(&claimed.base_branch),
        Some(&claimed.head_branch),
    )
    .map_err(crate::errors::AppError::Other)?;
    let current =
        delivery::capture_delivery_identity(&repo).map_err(crate::errors::AppError::Other)?;
    if claimed.expected_head_sha == next_head && current.head_sha == next_head {
        let git_repo = git2::Repository::open(&cwd).map_err(|error| {
            crate::errors::AppError::Other(format!(
                "cannot inspect settled branch-update status: {error}"
            ))
        })?;
        if current.repo_identity != claimed.repo_identity
            || current.worktree_identity != claimed.worktree_identity
            || !git_repo
                .statuses(None)
                .map_err(|error| {
                    crate::errors::AppError::Other(format!(
                        "cannot inspect settled branch-update worktree: {error}"
                    ))
                })?
                .is_empty()
        {
            return Err(crate::errors::AppError::Other(
                "settled branch-update identity has unreceipted local changes".into(),
            ));
        }
    }
    let remote = remote.ok_or_else(|| {
        crate::errors::AppError::Other(
            "committed branch update has no read-only provider observer".into(),
        )
    })?;
    let merge_observation = remote
        .observe_merge(number, next_head)
        .await
        .map_err(crate::errors::AppError::Other)?;
    match &merge_observation {
        MergeObservation::OpenSameHead { .. } | MergeObservation::Merged { .. } => {}
        MergeObservation::HeadChanged { actual_head } => {
            return Err(crate::errors::AppError::Other(
                format!(
                    "committed branch update PR moved to foreign head {actual_head}; no local ref was advanced"
                ),
            ))
        }
        MergeObservation::ClosedUnmerged | MergeObservation::Unsupported => {
            return Err(crate::errors::AppError::Other(
                "committed branch update is not currently visible as the same-head PR or exact merge"
                    .into(),
            ))
        }
    }
    let previous_oid = git2::Oid::from_str(previous_head).map_err(|error| {
        crate::errors::AppError::Other(format!("invalid branch-update parent SHA: {error}"))
    })?;
    let next_oid = git2::Oid::from_str(next_head).map_err(|error| {
        crate::errors::AppError::Other(format!("invalid branch-update result SHA: {error}"))
    })?;
    if current.repo_identity != claimed.repo_identity
        || current.worktree_identity != claimed.worktree_identity
        || (current.head_sha != previous_head && current.head_sha != next_head)
        || (current.head_sha == previous_head
            && current.change_set_digest != claimed.change_set_digest)
    {
        return Err(crate::errors::AppError::Other(
            "local workspace no longer matches the receipted branch-update old/new identity".into(),
        ));
    }
    let observed = if current.head_sha == previous_head {
        let temporary_ref = delivery::fetch_updated_pr_head_for_operation(
            &repo,
            next_head,
            &intent.operation_key,
            Some(number),
        )
        .map_err(crate::errors::AppError::Other)?;
        let request = delivery::BranchUpdateMaterialization {
            pr_number: number,
            cwd: cwd.clone(),
            default_branch: claimed.base_branch.clone(),
            head_branch: claimed.head_branch.clone(),
            previous_identity: current.clone(),
            next_head_sha: next_head.to_string(),
            fetched_ref: temporary_ref.clone(),
        };
        let materialized = delivery_run::with_receipted_branch_update_cas(
            db,
            &claimed.run_id,
            process,
            claimed.claim_epoch,
            &intent.intent_id,
            &intent.operation_key,
            chrono::Utc::now().timestamp_millis(),
            || delivery::materialize_fetched_branch_update(&request),
        )
        .await;
        delivery::clear_delivery_operation_ref(&repo, &temporary_ref);
        materialized?
    } else {
        let git_repo = git2::Repository::open(&cwd).map_err(|error| {
            crate::errors::AppError::Other(format!(
                "cannot inspect materialized branch-update graph: {error}"
            ))
        })?;
        if !git_repo
            .statuses(None)
            .map_err(|error| {
                crate::errors::AppError::Other(format!(
                    "cannot inspect materialized branch-update status: {error}"
                ))
            })?
            .is_empty()
        {
            return Err(crate::errors::AppError::Other(
                "materialized branch-update head has unreceipted index/worktree changes".into(),
            ));
        }
        if !git_repo
            .graph_descendant_of(next_oid, previous_oid)
            .map_err(|error| {
                crate::errors::AppError::Other(format!(
                    "cannot compare materialized branch-update ancestry: {error}"
                ))
            })?
        {
            return Err(crate::errors::AppError::Other(
                "committed branch update result is not a descendant of its exact prior head".into(),
            ));
        }
        current
    };
    if claimed.expected_head_sha != next_head {
        let prepared = PreparedDurableRun {
            id: claimed.run_id.clone(),
            process: process.clone(),
            claim_epoch: claimed.claim_epoch,
            objective_id: claimed.objective_id.clone(),
            workspace_path: cwd,
            worktree_identity: claimed.worktree_identity.clone(),
            repo_identity: claimed.repo_identity.clone(),
            change_set_digest: claimed.change_set_digest.clone(),
            expected_head_sha: claimed.expected_head_sha.clone(),
            head_branch: claimed.head_branch.clone(),
        };
        let revision = build_delivery_identity_revision(
            &prepared,
            &observed.repo_identity,
            &observed.worktree_identity,
            &observed.head_sha,
            &observed.change_set_digest,
        )?
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "committed branch update did not advance the durable delivery head".into(),
            )
        })?;
        let observation = DeliveryObservation {
            head_branch: claimed.head_branch.clone(),
            stage: "branch_update_reconciliation".into(),
            status: claimed.status.clone(),
            wait_class: claimed.wait_class.clone(),
            next_action: claimed.next_action.clone(),
            reached_ceiling: "pr_open".into(),
            expected_head_sha: observed.head_sha.clone(),
            canonical_pr_number: claimed.canonical_pr_number,
            canonical_pr_url: claimed.canonical_pr_url.clone(),
            canonical_head_sha: Some(observed.head_sha.clone()),
            failure_signature: None,
            core_input: None,
            identity_revision: Some(revision),
        };
        delivery_run::record_delivery_observation(
            db,
            &claimed.run_id,
            process,
            claimed.claim_epoch,
            &observation,
            chrono::Utc::now().timestamp_millis(),
            DELIVERY_LEASE_TTL_MS,
        )
        .await?;
        claimed.expected_head_sha = observed.head_sha;
        claimed.change_set_digest = observed.change_set_digest;
        claimed.canonical_head_sha = Some(claimed.expected_head_sha.clone());
        claimed.reached_ceiling = "pr_open".into();
        claimed.stage = "branch_update_reconciliation".into();
        claimed.failure_signature = None;
        claimed.stage_attempt = 0;
    }
    let confirmation = match merge_observation {
        MergeObservation::Merged { merge_sha } => json!({
            "confirmation": "merge_observed",
            "pr_number": number,
            "merge_sha": merge_sha,
        }),
        MergeObservation::OpenSameHead { .. } => json!({
            "confirmation": "auto_merge_observed",
            "pr_number": number,
            "head_sha": next_head,
        }),
        _ => unreachable!("non-positive branch-update observation returned above"),
    };
    let evidence = json!({
        "rung": intent.rung,
        "operation_key": intent.operation_key,
        "observation": confirmation,
        "committed_result": result_value,
    })
    .to_string();
    if !delivery_run::mark_delivery_mutation_intent_reconciled_committed(
        db,
        &intent.intent_id,
        process,
        claimed.claim_epoch,
        Some(&evidence),
        chrono::Utc::now().timestamp_millis(),
    )
    .await?
    {
        return Err(crate::errors::AppError::Other(
            "committed branch-update receipt lost its takeover epoch before settlement".into(),
        ));
    }
    Ok(())
}

async fn reconcile_receipted_local_commit_head(
    db: &sqlx::SqlitePool,
    claimed: &mut delivery_run::ClaimedRecovery,
    process: &ProcessIdentity,
) -> Result<Option<String>> {
    let persisted = DeliveryIdentitySnapshot {
        repo_identity: claimed.repo_identity.clone(),
        worktree_identity: claimed.worktree_identity.clone(),
        head_sha: claimed.expected_head_sha.clone(),
        change_set_digest: claimed.change_set_digest.clone(),
    };
    let cwd = std::path::PathBuf::from(&claimed.workspace_path);
    let (repo, _) = delivery::resolve_delivery_repo(
        &cwd,
        Some(&claimed.base_branch),
        Some(&claimed.head_branch),
    )
    .map_err(crate::errors::AppError::Other)?;
    let mut current =
        delivery::capture_delivery_identity(&repo).map_err(crate::errors::AppError::Other)?;
    let Some(intent) = sqlx::query_as::<_, delivery_run::DeliveryMutationIntent>(
        "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                process_instance, evidence_json, started_at, updated_at
         FROM delivery_mutation_intents
         WHERE run_id=? AND rung='git_local_commit'
           AND status IN ('started','unknown','committed','reconciled_committed')
         ORDER BY started_at DESC, intent_id DESC LIMIT 1",
    )
    .bind(&claimed.run_id)
    .fetch_optional(db)
    .await?
    else {
        return Ok(None);
    };
    let started = started_delivery_mutation_evidence(db, &claimed.run_id, &intent.intent_id)
        .await?
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "local commit intent lacks its immutable write-ahead evidence".into(),
            )
        })?;
    let evidence: LocalCommitIntentEvidence = serde_json::from_str(&started).map_err(|error| {
        crate::errors::AppError::Other(format!(
            "local commit intent has invalid write-ahead evidence: {error}"
        ))
    })?;
    if evidence.operation_key() != intent.operation_key {
        return Err(crate::errors::AppError::Other(
            "local commit intent operation key does not match its write-ahead evidence".into(),
        ));
    }
    if current.head_sha == persisted.head_sha && evidence.expected_head_sha != persisted.head_sha {
        current = delivery_run::with_receipted_local_commit_cas(
            db,
            &claimed.run_id,
            process,
            claimed.claim_epoch,
            &intent.intent_id,
            &intent.operation_key,
            chrono::Utc::now().timestamp_millis(),
            || {
                delivery::materialize_receipted_local_commit(
                    &cwd,
                    Some(&claimed.base_branch),
                    &claimed.head_branch,
                    &persisted,
                    &evidence,
                )
            },
        )
        .await?;
    }
    if current == persisted {
        if evidence.expected_head_sha != current.head_sha
            || evidence.repo_identity != current.repo_identity
            || evidence.worktree_identity != current.worktree_identity
        {
            return Ok(None);
        }
        let revision_parent = sqlx::query_scalar::<_, String>(
            "SELECT previous_expected_head_sha
             FROM delivery_identity_revisions
             WHERE run_id=? AND objective_id=?
               AND repo_identity=? AND worktree_identity=?
               AND previous_expected_head_sha=?
               AND next_expected_head_sha=? AND next_change_set_digest=?
             ORDER BY created_at DESC, receipt_id DESC LIMIT 1",
        )
        .bind(&claimed.run_id)
        .bind(&claimed.objective_id)
        .bind(&current.repo_identity)
        .bind(&current.worktree_identity)
        .bind(&evidence.previous_head_sha)
        .bind(&current.head_sha)
        .bind(&current.change_set_digest)
        .fetch_optional(db)
        .await?;
        if revision_parent.as_deref() != Some(evidence.previous_head_sha.as_str()) {
            return Ok(None);
        }
        // Once the durable canonical head has advanced to the child, a later
        // remote regression to the parent is no longer ours to overwrite.
        if claimed.canonical_head_sha.as_deref() == Some(current.head_sha.as_str()) {
            return Ok(None);
        }
        return Ok(revision_parent);
    }
    let observed = delivery::observe_receipted_local_commit(
        &cwd,
        Some(&claimed.base_branch),
        &claimed.head_branch,
        &persisted,
        &evidence,
    )
    .map_err(crate::errors::AppError::Other)?;
    let prepared = PreparedDurableRun {
        id: claimed.run_id.clone(),
        process: process.clone(),
        claim_epoch: claimed.claim_epoch,
        objective_id: claimed.objective_id.clone(),
        workspace_path: cwd,
        worktree_identity: claimed.worktree_identity.clone(),
        repo_identity: claimed.repo_identity.clone(),
        change_set_digest: claimed.change_set_digest.clone(),
        expected_head_sha: claimed.expected_head_sha.clone(),
        head_branch: claimed.head_branch.clone(),
    };
    let revision = build_delivery_identity_revision(
        &prepared,
        &observed.repo_identity,
        &observed.worktree_identity,
        &observed.head_sha,
        &observed.change_set_digest,
    )?
    .ok_or_else(|| {
        crate::errors::AppError::Other(
            "local commit receipt did not advance the durable head".into(),
        )
    })?;
    let observation = DeliveryObservation {
        head_branch: claimed.head_branch.clone(),
        stage: "local_commit_reconciliation".into(),
        status: claimed.status.clone(),
        wait_class: claimed.wait_class.clone(),
        next_action: claimed.next_action.clone(),
        reached_ceiling: "committed".into(),
        expected_head_sha: observed.head_sha.clone(),
        canonical_pr_number: claimed.canonical_pr_number,
        canonical_pr_url: claimed.canonical_pr_url.clone(),
        // A pre-existing PR may still point at the previous head until the
        // normal push rung runs. Preserve that durable observation instead of
        // pretending the local commit is already remote.
        canonical_head_sha: None,
        failure_signature: None,
        core_input: None,
        identity_revision: Some(revision),
    };
    delivery_run::record_delivery_observation(
        db,
        &claimed.run_id,
        process,
        claimed.claim_epoch,
        &observation,
        chrono::Utc::now().timestamp_millis(),
        DELIVERY_LEASE_TTL_MS,
    )
    .await?;
    claimed.expected_head_sha = observed.head_sha;
    claimed.change_set_digest = observed.change_set_digest;
    claimed.reached_ceiling = "committed".into();
    claimed.stage = "local_commit_reconciliation".into();
    claimed.failure_signature = None;
    claimed.stage_attempt = 0;
    Ok(Some(persisted.head_sha))
}

/// Resume an expired recoverable wait after startup. The durable lease was
/// claimed before this function is called; only already-authorized autonomous
/// waits are eligible. `deliver` begins with local/remote reconciliation and is
/// idempotent against the canonical PR/head receipts.
pub(crate) async fn resume_claimed_delivery(
    db: sqlx::SqlitePool,
    settings: crate::config::settings::Settings,
    claimed: delivery_run::ClaimedRecovery,
    process: ProcessIdentity,
) -> Result<()> {
    let cwd = std::path::PathBuf::from(&claimed.workspace_path);
    let remote = delivery::resolve_delivery_remote(&cwd, &settings);
    resume_claimed_delivery_with_remote(db, settings, claimed, process, remote.as_ref(), None, None)
        .await
}

async fn resume_claimed_delivery_with_remote<R: delivery::DeliveryRemote>(
    db: sqlx::SqlitePool,
    settings: crate::config::settings::Settings,
    mut claimed: delivery_run::ClaimedRecovery,
    process: ProcessIdentity,
    remote: Option<&R>,
    pause_after_identity_revision: Option<&std::path::Path>,
    pause_after_committed_mutation: Option<(&str, &std::path::Path)>,
) -> Result<()> {
    if !should_resume_claimed_delivery(&claimed) {
        return Ok(());
    }
    let receipted_parent_head =
        match reconcile_receipted_local_commit_head(&db, &mut claimed, &process).await {
            Ok(parent) => parent,
            Err(error) => {
                tracing::warn!(
                    run_id = %claimed.run_id,
                    claim_epoch = claimed.claim_epoch,
                    %error,
                    "delivery local-commit receipt could not prove the observed identity"
                );
                record_or_park_local_takeover_identity_conflict(&db, &claimed, &process).await?;
                return Ok(());
            }
        };
    if let Err(error) =
        reconcile_receipted_branch_update_head(&db, &mut claimed, &process, remote).await
    {
        tracing::warn!(
            run_id = %claimed.run_id,
            claim_epoch = claimed.claim_epoch,
            %error,
            "delivery branch-update receipt could not prove the observed identity"
        );
        record_or_park_local_takeover_identity_conflict(&db, &claimed, &process).await?;
        return Ok(());
    }
    let Some(requested_ceiling) = parse_ceiling(&claimed.requested_ceiling) else {
        return Err(crate::errors::AppError::Other(format!(
            "durable delivery run {} has an invalid requested ceiling",
            claimed.run_id
        )));
    };
    let cwd = std::path::PathBuf::from(&claimed.workspace_path);
    let persisted_identity = DeliveryIdentitySnapshot {
        repo_identity: claimed.repo_identity.clone(),
        worktree_identity: claimed.worktree_identity.clone(),
        head_sha: claimed.expected_head_sha.clone(),
        change_set_digest: claimed.change_set_digest.clone(),
    };
    let mut opts = DeliverOpts {
        title: None,
        body: None,
        release_urgency: None,
        requested_ceiling: Some(requested_ceiling),
        extra_excludes: settings.delivery_exclude_globs.clone(),
        expect_branch: Some(claimed.head_branch.clone()),
        expected_identity: Some(DeliveryIdentitySnapshot {
            repo_identity: claimed.repo_identity.clone(),
            worktree_identity: claimed.worktree_identity.clone(),
            head_sha: claimed.expected_head_sha.clone(),
            change_set_digest: claimed.change_set_digest.clone(),
        }),
        mutation_permit: None,
    };
    let mut prepared = PreparedDurableRun {
        id: claimed.run_id.clone(),
        process: process.clone(),
        claim_epoch: claimed.claim_epoch,
        objective_id: claimed.objective_id.clone(),
        workspace_path: cwd.clone(),
        worktree_identity: claimed.worktree_identity.clone(),
        repo_identity: claimed.repo_identity.clone(),
        change_set_digest: claimed.change_set_digest.clone(),
        expected_head_sha: claimed.expected_head_sha.clone(),
        head_branch: claimed.head_branch.clone(),
    };

    let mut takeover = match delivery::observe_delivery_takeover_with_receipted_parent(
        &cwd,
        Some(&claimed.base_branch),
        &claimed.head_branch,
        &persisted_identity,
        claimed.canonical_pr_number.map(|number| number as u64),
        claimed.canonical_pr_url.as_deref(),
        receipted_parent_head.as_deref(),
        remote,
    )
    .await
    {
        Ok(observation) => observation,
        Err(error) => {
            tracing::warn!(
                run_id = %claimed.run_id,
                claim_epoch = claimed.claim_epoch,
                %error,
                "delivery takeover found an unreceipted local identity; remaining observe-only"
            );
            record_or_park_local_takeover_identity_conflict(&db, &claimed, &process).await?;
            return Ok(());
        }
    };

    let release_db_intent_seen = match reconcile_unresolved_delivery_mutation_intents(
        &db,
        &claimed,
        &process,
        remote,
        &mut takeover,
    )
    .await
    {
        Ok(seen) => seen,
        Err(error) => {
            let failure = DeliveryObservation {
                head_branch: claimed.head_branch.clone(),
                stage: "takeover_mutation_reconciliation".into(),
                status: "platform_incident".into(),
                wait_class: Some("external_state_uncertain".into()),
                next_action: Some("observe_only_reconcile".into()),
                reached_ceiling: claimed.reached_ceiling.clone(),
                expected_head_sha: claimed.expected_head_sha.clone(),
                canonical_pr_number: claimed.canonical_pr_number,
                canonical_pr_url: claimed.canonical_pr_url.clone(),
                canonical_head_sha: claimed.canonical_head_sha.clone(),
                failure_signature: Some(format!("mutation_intent_reconciliation:{error}")),
                core_input: None,
                identity_revision: None,
            };
            let _ = delivery_run::record_delivery_observation(
                &db,
                &claimed.run_id,
                &process,
                claimed.claim_epoch,
                &failure,
                chrono::Utc::now().timestamp_millis(),
                DELIVERY_LEASE_TTL_MS,
            )
            .await;
            return Err(crate::errors::AppError::Other(format!(
                "delivery takeover remains observe-only: {error}"
            )));
        }
    };

    if let Err(error) = delivery::reconcile_local_release_intent(
        &cwd,
        Some(&claimed.base_branch),
        &claimed.head_branch,
        &claimed.expected_head_sha,
        !release_db_intent_seen,
        remote,
    )
    .await
    {
        let failure = DeliveryObservation {
            head_branch: claimed.head_branch.clone(),
            stage: "takeover_release_reconciliation".into(),
            status: "platform_incident".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            reached_ceiling: claimed.reached_ceiling.clone(),
            expected_head_sha: claimed.expected_head_sha.clone(),
            canonical_pr_number: claimed.canonical_pr_number,
            canonical_pr_url: claimed.canonical_pr_url.clone(),
            canonical_head_sha: claimed.canonical_head_sha.clone(),
            failure_signature: Some(format!("release_intent_reconciliation:{error}")),
            core_input: None,
            identity_revision: None,
        };
        let _ = delivery_run::record_delivery_observation(
            &db,
            &claimed.run_id,
            &process,
            claimed.claim_epoch,
            &failure,
            chrono::Utc::now().timestamp_millis(),
            DELIVERY_LEASE_TTL_MS,
        )
        .await;
        return Err(crate::errors::AppError::Other(format!(
            "delivery takeover remains observe-only: {error}"
        )));
    }

    let identity_revision = build_delivery_identity_revision(
        &prepared,
        &takeover.identity.repo_identity,
        &takeover.identity.worktree_identity,
        &takeover.identity.head_sha,
        &takeover.identity.change_set_digest,
    )?;
    let canonical_head_sha = takeover
        .canonical_head_sha
        .clone()
        .filter(|head| head == &takeover.identity.head_sha);
    let reconciled_observation = DeliveryObservation {
        head_branch: claimed.head_branch.clone(),
        stage: claimed.stage.clone(),
        status: claimed.status.clone(),
        wait_class: claimed.wait_class.clone(),
        next_action: claimed.next_action.clone(),
        reached_ceiling: claimed.reached_ceiling.clone(),
        expected_head_sha: takeover.identity.head_sha.clone(),
        canonical_pr_number: takeover.canonical_pr_number.map(|number| number as i64),
        canonical_pr_url: takeover.canonical_pr_url.clone(),
        canonical_head_sha,
        failure_signature: claimed.failure_signature.clone(),
        core_input: None,
        identity_revision: identity_revision.clone(),
    };
    delivery_run::record_delivery_observation(
        &db,
        &claimed.run_id,
        &process,
        claimed.claim_epoch,
        &reconciled_observation,
        chrono::Utc::now().timestamp_millis(),
        DELIVERY_LEASE_TTL_MS,
    )
    .await?;
    if let Some(revision) = identity_revision {
        prepared.expected_head_sha = revision.next_expected_head_sha;
        prepared.change_set_digest = revision.next_change_set_digest;
    }
    if !delivery_run::mark_delivery_claim_reconciled(
        &db,
        &claimed.run_id,
        &process,
        claimed.claim_epoch,
        chrono::Utc::now().timestamp_millis(),
    )
    .await?
    {
        return Err(crate::errors::AppError::Other(
            "delivery takeover lost its epoch before reconciliation completed".into(),
        ));
    }
    if let Some(marker) = pause_after_identity_revision {
        let revision_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM delivery_identity_revisions WHERE run_id=?")
                .bind(&claimed.run_id)
                .fetch_one(&db)
                .await?;
        let post_commit_mutation_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_mutation_intents
             WHERE run_id=? AND rung<>'git_local_commit'",
        )
        .bind(&claimed.run_id)
        .fetch_one(&db)
        .await?;
        std::fs::write(
            marker,
            serde_json::to_vec_pretty(&serde_json::json!({
                "worker_pid": std::process::id(),
                "run_id": claimed.run_id,
                "claim_epoch": claimed.claim_epoch,
                "identity_revision_count": revision_count,
                "canonical_parent_mutation_count": post_commit_mutation_count,
                "paused_after_production_identity_revision": true,
            }))?,
        )?;
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        return Err(crate::errors::AppError::Other(
            "delivery recovery smoke was not killed after the identity revision fault point".into(),
        ));
    }
    if claimed.status == "awaiting_completion_arbitration" {
        ObjectiveStore::new(db.clone())
            .settle_reconciled_delivery_after_takeover(
                &claimed.run_id,
                claimed.claim_epoch,
                &process.instance_id,
            )
            .await
            .map_err(|error| {
                crate::errors::AppError::Other(format!(
                    "delivery takeover could not settle its durable completion evidence: {error}"
                ))
            })?;
        return Ok(());
    }
    opts.expected_identity = Some(prepared.identity_snapshot());
    opts.mutation_permit = Some(pause_after_committed_mutation.map_or_else(
        || prepared.mutation_permit(&db),
        |(rung, marker)| {
            prepared.mutation_permit_stopping_after_commit(&db, rung, marker.to_path_buf())
        },
    ));
    loop {
        let delivery_future = delivery::deliver(
            &cwd,
            requested_ceiling,
            settings.delivery_merge_method,
            settings.delivery_ci_timeout_secs,
            &opts,
            remote,
            Some(&claimed.base_branch),
        );
        let outcome = await_delivery_with_lease_heartbeat(&db, &prepared, delivery_future).await?;
        persist_durable_outcome(&db, &mut prepared, &outcome).await?;
        opts.expected_identity = Some(prepared.identity_snapshot());
        if outcome.final_state != "waiting" {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            outcome.retry_after_ms.unwrap_or(30_000).clamp(1, 60_000),
        ))
        .await;
    }
}

pub(crate) fn should_resume_claimed_delivery(claimed: &delivery_run::ClaimedRecovery) -> bool {
    // Technical waits never need a second user decision. The explicit
    // authorization attached to the same objective is the only gate here;
    // autonomous_completion governs default business choices, not liveness.
    matches!(
        claimed.status.as_str(),
        "running"
            | "waiting"
            | "platform_incident"
            | "agent_action_required"
            | "failed_internal"
            | "awaiting_completion_arbitration"
    ) && claimed.next_action_authorized
}

async fn record_or_park_local_takeover_identity_conflict(
    db: &sqlx::SqlitePool,
    claimed: &delivery_run::ClaimedRecovery,
    process: &ProcessIdentity,
) -> Result<()> {
    let failure = local_takeover_identity_conflict_observation(claimed);
    let signature = failure.failure_signature.as_deref().ok_or_else(|| {
        crate::errors::AppError::Other("delivery identity conflict has no signature".into())
    })?;
    let now = chrono::Utc::now().timestamp_millis();
    let attempt = delivery_run::next_delivery_failure_attempt(
        db,
        &claimed.run_id,
        process,
        claimed.claim_epoch,
        signature,
        now,
    )
    .await?;
    if attempt >= delivery_run::MAX_IDENTICAL_TAKEOVER_FAILURES {
        ObjectiveStore::new(db.clone())
            .park_delivery_identity_incident_after_takeover(
                &claimed.objective_id,
                &claimed.run_id,
                process,
                claimed.claim_epoch,
                signature,
                attempt,
            )
            .await
            .map_err(|error| {
                crate::errors::AppError::Other(format!(
                    "delivery identity conflict could not atomically park its objective: {error}"
                ))
            })?;
    } else {
        delivery_run::record_delivery_observation(
            db,
            &claimed.run_id,
            process,
            claimed.claim_epoch,
            &failure,
            now,
            DELIVERY_LEASE_TTL_MS,
        )
        .await?;
    }
    Ok(())
}

pub(crate) fn recovery_supervisor_poll_interval() -> std::time::Duration {
    std::time::Duration::from_millis(RECOVERY_SUPERVISOR_POLL_MS)
}

/// Continuously claims expired system-owned delivery work. A tool future can
/// disappear without a process restart (cancelled task, UI teardown, panic),
/// so a startup-only sweep is insufficient. The CAS lease in
/// `plan_startup_recovery` prevents sibling processes from owning the same run.
pub(crate) fn spawn_delivery_recovery_supervisor(
    pool: sqlx::SqlitePool,
    settings: crate::config::settings::Settings,
    process: ProcessIdentity,
) {
    tauri::async_runtime::spawn(async move {
        tracing::info!(
            poll_ms = RECOVERY_SUPERVISOR_POLL_MS,
            "recovery supervisor: started"
        );
        loop {
            let now = chrono::Utc::now().timestamp_millis();
            match delivery_run::plan_startup_recovery(&pool, &process, now, 60_000).await {
                Ok(plan) => {
                    if !plan.fail_closed_identity_missing.is_empty() {
                        tracing::warn!(
                            "recovery supervisor: left {} run(s) unclaimed because stable identity was incomplete",
                            plan.fail_closed_identity_missing.len()
                        );
                    }
                    for claimed in plan.claimed {
                        if !should_resume_claimed_delivery(&claimed) {
                            continue;
                        }
                        let recovery_db = pool.clone();
                        let recovery_settings = settings.clone();
                        let recovery_process = process.clone();
                        tauri::async_runtime::spawn(async move {
                            let run_id = claimed.run_id.clone();
                            if let Err(error) = resume_claimed_delivery(
                                recovery_db,
                                recovery_settings,
                                claimed,
                                recovery_process,
                            )
                            .await
                            {
                                tracing::warn!(
                                    "recovery supervisor: durable delivery run {run_id} could not resume: {error}"
                                );
                            }
                        });
                    }
                }
                Err(error) => tracing::warn!(
                    "recovery supervisor: lease planning failed (will retry): {error}"
                ),
            }
            tokio::time::sleep(recovery_supervisor_poll_interval()).await;
        }
    });
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

async fn durable_objective_identity(
    db: &sqlx::SqlitePool,
    ctx: &ExecCtx,
) -> Result<(String, Option<String>)> {
    if let Some(task_id) = ctx.task_id.as_deref().filter(|value| !value.is_empty()) {
        let objective_id = sqlx::query_scalar::<_, Option<String>>(
            "SELECT objective_id FROM task_runs WHERE id=?",
        )
        .bind(task_id)
        .fetch_optional(db)
        .await?
        .flatten()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::errors::AppError::Other(format!(
                "deliver_changes refused task {task_id} without a unified objective identity; legacy reconciliation is required"
            ))
        })?;
        return Ok((objective_id, None));
    }
    let root_turn_id = ctx
        .root_turn_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "deliver_changes refused external mutation without a durable objective identity"
                    .into(),
            )
        })?;
    let (objective_id, task_segment_id) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT objective_id, task_segment_id FROM chat_turn_state WHERE root_turn_id=?",
    )
    .bind(root_turn_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| {
        crate::errors::AppError::Other(format!(
            "deliver_changes refused unknown root turn {root_turn_id} without a durable objective identity"
        ))
    })?;
    let objective_id = objective_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::errors::AppError::Other(format!(
                "deliver_changes refused root turn {root_turn_id} without a unified objective identity; legacy reconciliation is required"
            ))
        })?;
    Ok((objective_id, task_segment_id))
}

fn durable_run_id(source_identity: &str, repo_identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_identity.as_bytes());
    hasher.update([0]);
    hasher.update(repo_identity.as_bytes());
    format!("delivery-{:x}", hasher.finalize())
}

fn tool_output_for_outcome(outcome: &delivery::DeliveryOutcome) -> ToolOutput {
    if let Err(error) = outcome.validate_contract() {
        return ToolOutput::err(format!(
            "交付状态契约校验失败，未生成成功或阻断结论: {error}"
        ))
        .with_metadata(json!({
            "status": "error",
            "code": "delivery_contract_invalid",
            "recoverable": false,
            "recovery_class": "external_state_uncertain"
        }));
    }
    let system_owned_recovery = outcome.final_state == "blocked"
        && outcome.recoverable
        && outcome.recovery_class == delivery::RecoveryClass::AgentActionRequired;
    let report = render_report(outcome);
    let output = match outcome.final_state.as_str() {
        "waiting" => ToolOutput::waiting(report),
        "blocked" if system_owned_recovery => ToolOutput::waiting(report),
        "blocked" => ToolOutput::blocked(report),
        _ => ToolOutput::ok(report),
    };
    output.with_metadata(json!({
        "status": if system_owned_recovery { "recovering" } else { outcome.final_state.as_str() },
        "delivery_state": outcome.final_state,
        "stage": outcome.stage,
        "code": outcome.code,
        "recoverable": outcome.recoverable,
        "recovery_class": outcome.recovery_class,
        "decision_type": if outcome.recovery_class == crate::agent::delivery::RecoveryClass::CoreInputRequired { "core_input_required" } else { "system_owned" },
        "requires_user_continue": false,
        "retry_after_ms": outcome.retry_after_ms,
        "next_action": outcome.next_action,
        "requested_ceiling": outcome.requested_ceiling,
        "effective_ceiling": outcome.effective_ceiling,
        "reached_state": outcome.reached_state,
        "capability_gap": outcome.capability_gap,
        "branch": outcome.branch,
        "commit_sha": outcome.commit_sha,
        "pr_number": outcome.pr_number,
        "pr_url": outcome.pr_url,
    }))
}

async fn persist_delivery_ref(
    db: &sqlx::SqlitePool,
    session_id: &str,
    outcome: &delivery::DeliveryOutcome,
) -> Result<()> {
    let (Some(branch), Some(pr_number), Some(pr_url)) = (
        outcome.branch.as_deref(),
        outcome.pr_number,
        outcome.pr_url.as_deref(),
    ) else {
        return Ok(());
    };
    let updated_at = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO session_delivery_refs
            (session_id, branch, pr_number, pr_url, commit_sha, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(session_id) DO UPDATE SET
            branch=excluded.branch,
            pr_number=excluded.pr_number,
            pr_url=excluded.pr_url,
            commit_sha=excluded.commit_sha,
            updated_at=excluded.updated_at",
    )
    .bind(session_id)
    .bind(branch)
    .bind(pr_number as i64)
    .bind(pr_url)
    .bind(outcome.commit_sha.as_deref())
    .bind(updated_at)
    .execute(db)
    .await?;
    Ok(())
}

/// Render a compact, model-readable report. Never `is_error` for a "blocked"
/// terminal — blocked is an informative outcome (e.g. CI red / needs a token),
/// not a tool crash; the content explains it so the model reports rather than
/// blindly retries. Split from `execute` so the attribution contract on
/// blocked outcomes stays unit-testable.
fn render_report(outcome: &delivery::DeliveryOutcome) -> String {
    let mut out = String::new();
    let system_owned_recovery = outcome.final_state == "blocked"
        && outcome.recoverable
        && outcome.recovery_class == delivery::RecoveryClass::AgentActionRequired;
    if system_owned_recovery {
        out.push_str("交付状态: recovering\n");
    } else {
        out.push_str(&format!("交付结果: {}\n", outcome.final_state));
    }
    if let Some(branch) = &outcome.branch {
        out.push_str(&format!("分支: {branch}\n"));
    }
    out.push_str(&format!(
        "请求边界: {} · 实际边界: {} · 已到达: {}\n",
        outcome.requested_ceiling, outcome.effective_ceiling, outcome.reached_state
    ));
    for s in &outcome.steps {
        let mark = match s.status.as_str() {
            "ok" => "✅",
            "skipped" => "⏭️",
            "blocked" if system_owned_recovery => "↻",
            "blocked" => "⛔",
            _ => "•",
        };
        out.push_str(&format!("  {mark} {}: {}\n", s.step, s.detail));
    }
    if let Some(url) = &outcome.pr_url {
        out.push_str(&format!("PR: {url}\n"));
    }
    out.push_str(&format!("\n{}", outcome.summary));
    if outcome.final_state == "blocked" || outcome.final_state == "waiting" {
        out.push_str(
            "\n\n注意:本次交付没有达到请求边界；只能报告上面明确列出的已完成步骤。\
即使之后查询发现仓库出现了新的合并或发布,那也是其他执行器(并行 agent 或自动化流水线)\
完成的,不得归因为你本次的交付动作。",
        );
        if outcome.recovery_class == crate::agent::delivery::RecoveryClass::WaitRetryable {
            out.push_str(
                "\n\n这是等待中的交付状态，系统会在退避后自动核对同一 PR；用户无需回复『继续』。",
            );
        } else if outcome.recoverable {
            out.push_str(
                "\n\n这是系统负责的恢复阶段，不是用户阻断或最终总结边界。系统将执行 next_action，\
然后续接同一 PR；用户无需回复『继续』，且不得使用 --admin、force push 或删 required check。",
            );
        } else if outcome.recovery_class == crate::agent::delivery::RecoveryClass::CoreInputRequired
        {
            out.push_str(
                "\n\n系统已把无法推导的外部核心输入合并为一次请求；输入补齐后应自动续接原 run，\
不得要求用户再回复『继续』，也不得降低原交付边界。",
            );
        } else {
            out.push_str("如实报告缺失能力、实际到达层级和恢复动作即可。");
        }
    }
    out
}

fn parse_ceiling(s: &str) -> Option<DeliveryCeiling> {
    match s {
        "off" => Some(DeliveryCeiling::Off),
        "pr_only" => Some(DeliveryCeiling::PrOnly),
        "through_ci_green" => Some(DeliveryCeiling::ThroughCiGreen),
        "through_merge" => Some(DeliveryCeiling::ThroughMerge),
        "through_release" => Some(DeliveryCeiling::ThroughRelease),
        _ => None,
    }
}

fn parse_release_urgency(s: &str) -> Option<ReleaseUrgency> {
    match s {
        "immediate" => Some(ReleaseUrgency::Immediate),
        "hold" => Some(ReleaseUrgency::Hold),
        _ => None,
    }
}

/// `hold` is the pipeline's one real business gate: it stops the WHOLE batch —
/// other people's merges included — and only a human can clear it with
/// `allow_guarded_batch`. So only a human may set it.
///
/// On 2026-08-05 five commits carried `Release-Urgency: hold`, every one
/// authored by the delivery identity rather than the user. The schema presented
/// it as a neutral cautious choice, so the agent picked it, manufactured a
/// block only the user could undo, and then walked into it. `immediate` is not
/// symmetric — it accelerates, its rubric is objective, and a wrong call costs
/// one early release — so it stays agent-decidable.
///
/// An unmandated `hold` is DROPPED, never turned into an error: refusing the
/// call would replace a self-inflicted release block with a self-inflicted
/// delivery block. Delivery proceeds on the ordinary cadence and the report
/// says the signal was ignored.
fn release_urgency_from_args(args: &Value) -> std::result::Result<Option<ReleaseUrgency>, String> {
    let Some(raw) = args.get("release_urgency") else {
        return Ok(None);
    };
    let Some(value) = raw.as_str() else {
        return Err("deliver_changes.release_urgency 必须是 immediate 或 hold".into());
    };
    let urgency = parse_release_urgency(value).ok_or_else(|| {
        format!(
            "无效的 deliver_changes.release_urgency: {value}; 只允许 immediate 或 hold，\
未执行任何交付动作。"
        )
    })?;
    if urgency == ReleaseUrgency::Hold && !user_mandated_hold(args) {
        tracing::info!(
            "dropping agent-initiated Release-Urgency: hold — only the user may gate a batch"
        );
        return Ok(None);
    }
    Ok(Some(urgency))
}

/// Did the USER ask for the hold? The caller must quote their instruction, so
/// the mandate is auditable in the commit rather than asserted by a boolean.
fn user_mandated_hold(args: &Value) -> bool {
    args.get("hold_requested_by_user")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|reason| !reason.is_empty())
}

#[cfg(not(test))]
struct DeliveryRecoverySmokeRemote {
    parent_head: String,
}

#[cfg(not(test))]
impl delivery::DeliveryRemote for DeliveryRecoverySmokeRemote {
    fn capabilities(&self) -> delivery::DeliveryCapabilities {
        delivery::DeliveryCapabilities {
            review: true,
            ..delivery::DeliveryCapabilities::default()
        }
    }

    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        _head: &str,
        _base: &str,
        _expected_head_sha: &str,
        _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
    ) -> std::result::Result<delivery::DeliveryPr, String> {
        Ok(delivery::DeliveryPr {
            number: 1,
            url: "https://example.invalid/delivery-recovery-smoke/pull/1".into(),
            title: title.into(),
            body: body.into(),
        })
    }

    async fn ci_status(&self, _sha: &str) -> std::result::Result<delivery::CiStatus, String> {
        Ok(delivery::CiStatus::None)
    }

    async fn observe_merge(
        &self,
        _number: u64,
        expected_head: &str,
    ) -> std::result::Result<delivery::MergeObservation, String> {
        if expected_head == self.parent_head {
            Ok(delivery::MergeObservation::OpenSameHead { auto_merge: false })
        } else {
            Ok(delivery::MergeObservation::HeadChanged {
                actual_head: self.parent_head.clone(),
            })
        }
    }

    async fn merge_pr(
        &self,
        _number: u64,
        _method: crate::config::settings::MergeMethod,
        _commit_message: Option<&delivery::MergeCommitMessage>,
        _expected_head: &str,
        _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
    ) -> std::result::Result<delivery::MergeRequestResult, String> {
        unreachable!("delivery recovery smoke stops at the PR ceiling")
    }

    async fn trigger_release(
        &self,
        _head_sha: &str,
        _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
    ) -> std::result::Result<String, String> {
        unreachable!("delivery recovery smoke stops at the PR ceiling")
    }
}

#[cfg(not(test))]
fn delivery_recovery_smoke_git(cwd: &std::path::Path, args: &[&str]) -> anyhow::Result<String> {
    let output = std::process::Command::new("git")
        .no_window()
        .current_dir(cwd)
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Seed the exact local-commit persistence gap used by the release executable
/// smoke. The caller hard-kills this process after the returned marker is
/// durable, deliberately skipping `persist_durable_outcome`.
#[cfg(not(test))]
pub(crate) async fn seed_delivery_recovery_smoke(
    state_dir: &std::path::Path,
) -> anyhow::Result<serde_json::Value> {
    let worktree = state_dir.join("worktree");
    let db_url = format!(
        "sqlite:{}",
        state_dir.join("delivery-recovery.db").display()
    );
    let pool = crate::storage::db::connect(&db_url).await?;
    let admitted_at = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO sessions
         (id, title, cwd, model_id, endpoint_id, model_policy,
          permission_mode, created_at, updated_at)
         VALUES ('delivery-recovery-smoke-session', 'Delivery recovery smoke', ?,
                 'smoke-model', 'smoke-endpoint', 'fixed', 'trusted', ?, ?)",
    )
    .bind(worktree.to_string_lossy().as_ref())
    .bind(admitted_at)
    .bind(admitted_at)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at)
         VALUES ('delivery-recovery-smoke-root', 'delivery-recovery-smoke-session',
                 'user', 'Complete this authorized delivery without further input.', ?)",
    )
    .bind(admitted_at)
    .execute(&pool)
    .await?;
    ObjectiveStore::new(pool.clone())
        .create(CreateObjective {
            id: "delivery-recovery-smoke-objective-receipted".into(),
            kind: ObjectiveKind::Delivery,
            session_id: Some("delivery-recovery-smoke-session".into()),
            root_turn_id: Some("delivery-recovery-smoke-root".into()),
            domain: RecoveryDomain::Delivery,
            requested_acceptance: "pr_open".into(),
            created_surface: "delivery_recovery_smoke".into(),
        })
        .await?;
    let binding_id = "delivery-recovery-smoke-binding";
    let action_signature = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    sqlx::query(
        "INSERT INTO objective_bindings
         (id, objective_id, domain, resource_kind, resource_id,
          resource_generation, identity_digest, side_effect_started,
          created_at, updated_at)
         VALUES (?, 'delivery-recovery-smoke-objective-receipted', 'delivery',
                 'chat_root_turn', 'delivery-recovery-smoke-root', 1,
                 'sha256:delivery-recovery-smoke-binding', 1, ?, ?)",
    )
    .bind(binding_id)
    .bind(admitted_at)
    .bind(admitted_at)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at)
         VALUES ('delivery-recovery-smoke-assistant',
                 'delivery-recovery-smoke-session', 'assistant', '', ?)",
    )
    .bind(admitted_at + 1)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO tool_calls
         (id, message_id, tool_name, arguments, status, created_at,
          objective_id, binding_id, action_signature, resource_generation)
         VALUES ('delivery-recovery-smoke-session:deliver-1',
                 'delivery-recovery-smoke-assistant', 'deliver_changes', '{}',
                 'pending', ?, 'delivery-recovery-smoke-objective-receipted',
                 ?, ?, 1)",
    )
    .bind(admitted_at + 2)
    .bind(binding_id)
    .bind(action_signature)
    .execute(&pool)
    .await?;
    sqlx::query(
        "UPDATE objectives SET side_effect_started=1
         WHERE id='delivery-recovery-smoke-objective-receipted'",
    )
    .execute(&pool)
    .await?;
    let (repo, _) = delivery::resolve_delivery_repo(
        &worktree,
        Some("main"),
        Some("fix/delivery-recovery-smoke"),
    )
    .map_err(anyhow::Error::msg)?;
    let before = delivery::capture_delivery_identity(&repo).map_err(anyhow::Error::msg)?;
    let process = ProcessIdentity::new(
        format!("delivery-recovery-seed:{}", std::process::id()),
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    );
    let run = NewDeliveryRun {
        id: "delivery-recovery-smoke-receipted".into(),
        objective_id: "delivery-recovery-smoke-objective-receipted".into(),
        run_kind: "deliver_changes".into(),
        session_id: Some("delivery-recovery-smoke-session".into()),
        root_turn_id: Some("delivery-recovery-smoke-root".into()),
        task_segment_id: Some("delivery-recovery-smoke-segment".into()),
        task_id: None,
        workspace_path: worktree.to_string_lossy().into_owned(),
        worktree_identity: before.worktree_identity.clone(),
        repo_identity: before.repo_identity.clone(),
        base_branch: "main".into(),
        head_branch: "fix/delivery-recovery-smoke".into(),
        change_set_digest: before.change_set_digest.clone(),
        expected_head_sha: before.head_sha.clone(),
        canonical_pr_number: Some(1),
        canonical_pr_url: Some("https://example.invalid/delivery-recovery-smoke/pull/1".into()),
        canonical_head_sha: Some(before.head_sha.clone()),
        requested_ceiling: "pr_only".into(),
        reached_ceiling: "local".into(),
        stage: "preflight".into(),
        status: "running".into(),
        wait_class: None,
        next_action: Some("deliver".into()),
        next_action_authorized: true,
        autonomous_completion: true,
    };
    let now = chrono::Utc::now().timestamp_millis();
    let claim_epoch = delivery_run::create_delivery_run(&pool, &run, &process, now, 90_000).await?;
    let prepared = PreparedDurableRun {
        id: run.id.clone(),
        process,
        claim_epoch,
        objective_id: run.objective_id.clone(),
        workspace_path: worktree.clone(),
        worktree_identity: run.worktree_identity.clone(),
        repo_identity: run.repo_identity.clone(),
        change_set_digest: run.change_set_digest.clone(),
        expected_head_sha: run.expected_head_sha.clone(),
        head_branch: run.head_branch.clone(),
    };
    let outcome = delivery::deliver(
        &worktree,
        DeliveryCeiling::PrOnly,
        crate::config::settings::MergeMethod::Squash,
        1,
        &DeliverOpts {
            title: Some("fix: recover receipted local commit".into()),
            requested_ceiling: Some(DeliveryCeiling::PrOnly),
            expect_branch: Some("fix/delivery-recovery-smoke".into()),
            expected_identity: Some(before.clone()),
            mutation_permit: Some(prepared.mutation_permit_stopping_before_materialize(
                &pool,
                "git_local_commit",
                state_dir.join("seed-ready.json"),
            )),
            ..DeliverOpts::default()
        },
        Some(&DeliveryRecoverySmokeRemote {
            parent_head: before.head_sha.clone(),
        }),
        Some("main"),
    )
    .await;
    let committed_head = outcome
        .commit_sha
        .clone()
        .ok_or_else(|| anyhow::anyhow!("delivery recovery seed did not create a commit"))?;
    if committed_head == before.head_sha {
        anyhow::bail!("delivery recovery seed did not advance the local head");
    }
    let persisted_head: String = sqlx::query_scalar(
        "SELECT expected_head_sha FROM delivery_runs
         WHERE id='delivery-recovery-smoke-receipted'",
    )
    .fetch_one(&pool)
    .await?;
    let local_commit_receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_mutation_intents
         WHERE run_id='delivery-recovery-smoke-receipted'
           AND rung='git_local_commit' AND status='committed'",
    )
    .fetch_one(&pool)
    .await?;
    let push_receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_mutation_intents
         WHERE run_id='delivery-recovery-smoke-receipted'
           AND rung='git_push'
           AND status IN ('committed','reconciled_committed')",
    )
    .fetch_one(&pool)
    .await?;
    if persisted_head != before.head_sha
        || local_commit_receipt_count != 1
        || push_receipt_count != 0
    {
        anyhow::bail!("delivery recovery seed did not preserve the intended persistence gap");
    }
    crate::storage::db::close_and_release_files(pool).await;
    Ok(serde_json::json!({
        "persisted_head": persisted_head,
        "committed_head": committed_head,
        "local_commit_receipt_count": local_commit_receipt_count,
        "push_receipt_count": push_receipt_count,
    }))
}

/// First replacement owner: execute the real startup claim and production
/// resume path through the durable identity revision, then pause so the parent
/// can hard-kill this process before push. This proves the parent allowance is
/// durable across a second lease loss rather than an in-memory exception.
#[cfg(not(test))]
pub(crate) async fn rebind_delivery_recovery_smoke(
    state_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let db_url = format!(
        "sqlite:{}",
        state_dir.join("delivery-recovery.db").display()
    );
    let pool = crate::storage::db::connect(&db_url).await?;
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "UPDATE delivery_runs SET lease_expires_at=?
         WHERE id='delivery-recovery-smoke-receipted'",
    )
    .bind(now - 1)
    .execute(&pool)
    .await?;
    let process = ProcessIdentity::new(
        format!("delivery-recovery-rebind:{}", std::process::id()),
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    );
    let mut plan = delivery_run::plan_startup_recovery(&pool, &process, now, 90_000).await?;
    if plan.claimed.len() != 1 {
        anyhow::bail!(
            "identity rebind expected one production startup claim, observed {}",
            plan.claimed.len()
        );
    }
    let claim = plan.claimed.remove(0);
    let remote = DeliveryRecoverySmokeRemote {
        parent_head: claim
            .canonical_head_sha
            .clone()
            .ok_or_else(|| anyhow::anyhow!("identity rebind omitted canonical parent"))?,
    };
    resume_claimed_delivery_with_remote(
        pool,
        crate::config::settings::Settings::default(),
        claim,
        process,
        Some(&remote),
        Some(&state_dir.join("rebind-ready.json")),
        None,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    anyhow::bail!("identity rebind smoke did not pause at the injected process-loss point")
}

/// Resume the same production run, commit the exact remote push receipt, and
/// pause before the DeliveryRun outcome is persisted. The next process must
/// consume the committed receipt through read-only observation without
/// dispatching the push again.
#[cfg(not(test))]
pub(crate) async fn push_delivery_recovery_smoke(
    state_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let db_url = format!(
        "sqlite:{}",
        state_dir.join("delivery-recovery.db").display()
    );
    let pool = crate::storage::db::connect(&db_url).await?;
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "UPDATE delivery_runs SET lease_expires_at=?
         WHERE id='delivery-recovery-smoke-receipted'",
    )
    .bind(now - 1)
    .execute(&pool)
    .await?;
    let process = ProcessIdentity::new(
        format!("delivery-recovery-push:{}", std::process::id()),
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    );
    let mut plan = delivery_run::plan_startup_recovery(&pool, &process, now, 90_000).await?;
    if plan.claimed.len() != 1 {
        anyhow::bail!(
            "post-rebind push expected one production startup claim, observed {}",
            plan.claimed.len()
        );
    }
    let claim = plan.claimed.remove(0);
    let remote = DeliveryRecoverySmokeRemote {
        parent_head: claim
            .canonical_head_sha
            .clone()
            .ok_or_else(|| anyhow::anyhow!("post-rebind push omitted canonical parent"))?,
    };
    let marker = state_dir.join("push-ready.json");
    resume_claimed_delivery_with_remote(
        pool,
        crate::config::settings::Settings::default(),
        claim,
        process,
        Some(&remote),
        None,
        Some(("git_push", marker.as_path())),
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    anyhow::bail!("post-push recovery smoke did not pause at the injected process-loss point")
}

/// Recover the receipted child commit exactly once, then inject a foreign
/// unreceipted commit and prove the bounded fail-closed path reaches a stable
/// non-claimable incident without touching the remote.
#[cfg(not(test))]
pub(crate) async fn recover_delivery_recovery_smoke(
    state_dir: &std::path::Path,
) -> anyhow::Result<serde_json::Value> {
    let worktree = state_dir.join("worktree");
    let origin = state_dir.join("origin.git");
    let db_url = format!(
        "sqlite:{}",
        state_dir.join("delivery-recovery.db").display()
    );
    let pool = crate::storage::db::connect(&db_url).await?;
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "UPDATE delivery_runs SET lease_expires_at=?
         WHERE id='delivery-recovery-smoke-receipted'",
    )
    .bind(now - 1)
    .execute(&pool)
    .await?;
    let takeover = ProcessIdentity::new(
        format!("delivery-recovery-resume:{}", std::process::id()),
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    );
    let mut plan = delivery_run::plan_startup_recovery(&pool, &takeover, now, 90_000).await?;
    if plan.claimed.len() != 1 {
        anyhow::bail!(
            "receipted recovery expected one claim, observed {}",
            plan.claimed.len()
        );
    }
    let claim = plan.claimed.remove(0);
    let canonical_parent = claim
        .canonical_head_sha
        .clone()
        .ok_or_else(|| anyhow::anyhow!("recovery omitted its canonical parent"))?;
    resume_claimed_delivery_with_remote(
        pool.clone(),
        crate::config::settings::Settings::default(),
        claim,
        takeover,
        Some(&DeliveryRecoverySmokeRemote {
            parent_head: canonical_parent,
        }),
        None,
        None,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    let (
        reconciled_head,
        identity_revision_count,
        canonical_pr_number,
        status,
        stage,
        wait_class,
        failure_code,
    ): (
        String,
        i64,
        Option<i64>,
        String,
        String,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT expected_head_sha,
                (SELECT COUNT(*) FROM delivery_identity_revisions revision
                 WHERE revision.run_id=delivery_runs.id),
                canonical_pr_number, status, stage, wait_class, failure_code
         FROM delivery_runs WHERE id='delivery-recovery-smoke-receipted'",
    )
    .fetch_one(&pool)
    .await?;
    if identity_revision_count != 1
        || canonical_pr_number != Some(1)
        || status != "awaiting_completion_arbitration"
    {
        anyhow::bail!(
            "production recovery did not reach one canonical PR and Completion Arbiter: revisions={identity_revision_count}, pr={canonical_pr_number:?}, status={status}, stage={stage}, wait_class={wait_class:?}, failure_code={failure_code:?}"
        );
    }
    let push_receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_mutation_intents
         WHERE run_id='delivery-recovery-smoke-receipted'
           AND rung='git_push'
           AND status IN ('committed','reconciled_committed')",
    )
    .fetch_one(&pool)
    .await?;
    let pr_receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_mutation_intents
         WHERE run_id='delivery-recovery-smoke-receipted'
           AND rung='provider_pr_create' AND status='committed'",
    )
    .fetch_one(&pool)
    .await?;
    if push_receipt_count != 1 || pr_receipt_count > 1 {
        anyhow::bail!(
            "production recovery duplicated remote work: pushes={push_receipt_count}, pr_receipts={pr_receipt_count}"
        );
    }

    sqlx::query(
        "UPDATE delivery_runs SET lease_expires_at=?
         WHERE id='delivery-recovery-smoke-receipted'",
    )
    .bind(now - 1)
    .execute(&pool)
    .await?;
    let arbiter_process = ProcessIdentity::new(
        format!("delivery-recovery-arbiter:{}", std::process::id()),
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    );
    let mut arbiter_plan =
        delivery_run::plan_startup_recovery(&pool, &arbiter_process, now + 1, 90_000).await?;
    if arbiter_plan.claimed.len() != 1 {
        anyhow::bail!(
            "Completion Arbiter expected one claim, observed {}",
            arbiter_plan.claimed.len()
        );
    }
    resume_claimed_delivery_with_remote(
        pool.clone(),
        crate::config::settings::Settings::default(),
        arbiter_plan.claimed.remove(0),
        arbiter_process,
        Some(&DeliveryRecoverySmokeRemote {
            parent_head: reconciled_head.clone(),
        }),
        None,
        None,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let (delivery_status, objective_status, tool_status): (String, String, String) =
        sqlx::query_as(
            "SELECT run.status, objective.status, tool.status
             FROM delivery_runs run
             JOIN objectives objective ON objective.delivery_run_id=run.id
             JOIN tool_calls tool ON tool.objective_id=objective.id
             WHERE run.id='delivery-recovery-smoke-receipted'
               AND tool.tool_name='deliver_changes'",
        )
        .fetch_one(&pool)
        .await?;
    if delivery_status != "completed" || objective_status != "completed" || tool_status != "done" {
        anyhow::bail!(
            "Completion Arbiter did not converge the production chain: run={delivery_status}, objective={objective_status}, tool={tool_status}"
        );
    }
    let rebind_receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state_dir.join("rebind-ready.json"))?)?;
    let canonical_parent_mutation_count = rebind_receipt
        .get("canonical_parent_mutation_count")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(-1);
    let canonical_parent_reconciled = rebind_receipt
        .get("paused_after_production_identity_revision")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        && canonical_parent_mutation_count == 0;

    let (repo, _) = delivery::resolve_delivery_repo(
        &worktree,
        Some("main"),
        Some("fix/delivery-recovery-smoke"),
    )
    .map_err(anyhow::Error::msg)?;
    let before_foreign = delivery::capture_delivery_identity(&repo).map_err(anyhow::Error::msg)?;
    let foreign_owner = ProcessIdentity::new(
        "delivery-recovery-foreign-owner",
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    );
    sqlx::query(
        "INSERT INTO sessions
         (id, title, cwd, model_id, endpoint_id, model_policy,
          permission_mode, created_at, updated_at)
         VALUES ('delivery-recovery-smoke-foreign-session',
                 'Delivery recovery foreign fixture', ?,
                 'smoke-model', 'smoke-endpoint', 'fixed', 'trusted', ?, ?)",
    )
    .bind(worktree.to_string_lossy().as_ref())
    .bind(now + 1)
    .bind(now + 1)
    .execute(&pool)
    .await?;
    ObjectiveStore::new(pool.clone())
        .create(CreateObjective {
            id: "delivery-recovery-smoke-objective-foreign".into(),
            kind: ObjectiveKind::Delivery,
            session_id: Some("delivery-recovery-smoke-foreign-session".into()),
            root_turn_id: Some("delivery-recovery-smoke-foreign-root".into()),
            domain: RecoveryDomain::Delivery,
            requested_acceptance: "pr_open".into(),
            created_surface: "delivery_recovery_smoke".into(),
        })
        .await?;
    let foreign_run = NewDeliveryRun {
        id: "delivery-recovery-smoke-foreign".into(),
        objective_id: "delivery-recovery-smoke-objective-foreign".into(),
        run_kind: "deliver_changes".into(),
        session_id: Some("delivery-recovery-smoke-foreign-session".into()),
        root_turn_id: Some("delivery-recovery-smoke-foreign-root".into()),
        task_segment_id: Some("delivery-recovery-smoke-foreign-segment".into()),
        task_id: None,
        workspace_path: worktree.to_string_lossy().into_owned(),
        worktree_identity: before_foreign.worktree_identity.clone(),
        repo_identity: before_foreign.repo_identity.clone(),
        base_branch: "main".into(),
        head_branch: "fix/delivery-recovery-smoke".into(),
        change_set_digest: before_foreign.change_set_digest.clone(),
        expected_head_sha: before_foreign.head_sha.clone(),
        canonical_pr_number: None,
        canonical_pr_url: None,
        canonical_head_sha: None,
        requested_ceiling: "pr_only".into(),
        reached_ceiling: "committed".into(),
        stage: "takeover_reconciliation".into(),
        status: "platform_incident".into(),
        wait_class: Some("external_state_uncertain".into()),
        next_action: Some("observe_only_reconcile".into()),
        next_action_authorized: true,
        autonomous_completion: true,
    };
    delivery_run::create_delivery_run(&pool, &foreign_run, &foreign_owner, now + 1, 90_000).await?;
    std::fs::write(worktree.join("foreign.txt"), "unreceipted foreign edit\n")?;
    delivery_recovery_smoke_git(&worktree, &["add", "foreign.txt"])?;
    delivery_recovery_smoke_git(
        &worktree,
        &[
            "-c",
            "user.name=Foreign Fixture",
            "-c",
            "user.email=foreign@example.invalid",
            "commit",
            "-q",
            "-m",
            "chore: unreceipted foreign commit",
        ],
    )?;
    let foreign_head = delivery_recovery_smoke_git(&worktree, &["rev-parse", "HEAD"])?;
    let remote_head_before = delivery_recovery_smoke_git(
        &origin,
        &["rev-parse", "refs/heads/fix/delivery-recovery-smoke"],
    )?;

    let mut last_epoch = 0_i64;
    for attempt in 0..2_i64 {
        sqlx::query(
            "UPDATE delivery_runs SET lease_expires_at=?
             WHERE id='delivery-recovery-smoke-foreign'",
        )
        .bind(now + attempt - 1)
        .execute(&pool)
        .await?;
        let process = ProcessIdentity::new(
            format!("delivery-recovery-foreign-takeover-{attempt}"),
            env!("CARGO_PKG_VERSION"),
            option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
        );
        let mut recovery =
            delivery_run::plan_startup_recovery(&pool, &process, now + attempt + 1, 90_000).await?;
        if recovery.claimed.len() != 1 {
            anyhow::bail!(
                "foreign recovery attempt {attempt} expected one claim, observed {}",
                recovery.claimed.len()
            );
        }
        let claim = recovery.claimed.remove(0);
        last_epoch = claim.claim_epoch;
        resume_claimed_delivery(
            pool.clone(),
            crate::config::settings::Settings::default(),
            claim,
            process,
        )
        .await?;
    }
    let (status, wait_class, stage_attempt, next_action_authorized, claim_epoch): (
        String,
        Option<String>,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT status, wait_class, stage_attempt,
                next_action_authorized, claim_epoch
         FROM delivery_runs WHERE id='delivery-recovery-smoke-foreign'",
    )
    .fetch_one(&pool)
    .await?;
    let third_process = ProcessIdentity::new(
        "delivery-recovery-foreign-takeover-third",
        env!("CARGO_PKG_VERSION"),
        option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(env!("CARGO_PKG_VERSION")),
    );
    let third =
        delivery_run::plan_startup_recovery(&pool, &third_process, now + 10, 90_000).await?;
    let claim_epoch_after_third: i64 = sqlx::query_scalar(
        "SELECT claim_epoch FROM delivery_runs
         WHERE id='delivery-recovery-smoke-foreign'",
    )
    .fetch_one(&pool)
    .await?;
    let foreign_mutation_intent_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_mutation_intents
         WHERE run_id='delivery-recovery-smoke-foreign'",
    )
    .fetch_one(&pool)
    .await?;
    let recovery_parked_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_run_events
         WHERE run_id='delivery-recovery-smoke-foreign'
           AND event_kind='objective_incident_parked'",
    )
    .fetch_one(&pool)
    .await?;
    let user_message_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE role='user' AND completion_state IS NULL",
    )
    .fetch_one(&pool)
    .await?;
    let human_prompt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM objective_decisions WHERE requires_user_action=1")
            .fetch_one(&pool)
            .await?;
    let foreign_objective: (String, String, Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT status, decision_type, failure_code, recovery_owner, requires_user_action
             FROM objectives WHERE id='delivery-recovery-smoke-objective-foreign'",
    )
    .fetch_one(&pool)
    .await?;
    let remote_head_after = delivery_recovery_smoke_git(
        &origin,
        &["rev-parse", "refs/heads/fix/delivery-recovery-smoke"],
    )?;
    let foreign_identity_parked = status == "platform_incident"
        && wait_class.as_deref() == Some("delivery_identity_conflict")
        && stage_attempt == 2
        && next_action_authorized == 0
        && recovery_parked_event_count == 1
        && foreign_objective.0 == "waiting_system"
        && foreign_objective.1 == "failed_internal"
        && foreign_objective.2.as_deref() == Some("technical_recovery_exhausted")
        && foreign_objective.3.as_deref() == Some("objective-incident-controller")
        && foreign_objective.4 == 0;
    let claim_epoch_plateau = third.claimed.is_empty()
        && claim_epoch == last_epoch
        && claim_epoch_after_third == claim_epoch;
    let remote_unchanged = remote_head_before == remote_head_after
        && remote_head_after == reconciled_head
        && foreign_head != remote_head_after;
    if !foreign_identity_parked
        || !claim_epoch_plateau
        || !remote_unchanged
        || foreign_mutation_intent_count != 0
        || user_message_count != 1
        || human_prompt_count != 0
    {
        anyhow::bail!(
            "foreign identity recovery did not converge fail-closed: \
             status={status}, wait_class={wait_class:?}, stage_attempt={stage_attempt}, \
             next_action_authorized={next_action_authorized}, claim_epoch={claim_epoch}, \
             last_epoch={last_epoch}, claim_epoch_after_third={claim_epoch_after_third}, \
             third_claims={}, parked_events={recovery_parked_event_count}, \
             mutation_intents={foreign_mutation_intent_count}, \
             user_messages={user_message_count}, human_prompts={human_prompt_count}, \
             remote_before={remote_head_before}, remote_after={remote_head_after}, \
             reconciled_head={reconciled_head}, foreign_head={foreign_head}",
            third.claimed.len()
        );
    }
    crate::storage::db::close_and_release_files(pool).await;
    let duplicate_remote_write_count = foreign_mutation_intent_count
        + (push_receipt_count - 1).max(0)
        + (pr_receipt_count - 1).max(0);
    Ok(serde_json::json!({
        "exact_receipted_head_reconciled": true,
        "identity_revision_count": identity_revision_count,
        "canonical_parent_reconciled": canonical_parent_reconciled,
        "canonical_parent_mutation_count": canonical_parent_mutation_count,
        "foreign_identity_parked": foreign_identity_parked,
        "claim_epoch_plateau": claim_epoch_plateau,
        "claim_epoch": claim_epoch,
        "recovery_parked_event_count": recovery_parked_event_count,
        "duplicate_remote_write_count": duplicate_remote_write_count,
        "production_resume_path": true,
        "completion_arbiter_converged": true,
        "single_push_receipt_count": push_receipt_count,
        "canonical_pr_number": canonical_pr_number,
        "remote_head_unchanged": remote_unchanged,
        "user_message_count": user_message_count,
        "human_prompt_count": human_prompt_count,
    }))
}

#[cfg(test)]
mod tests {

    // 2026-08-05: five commits carried `Release-Urgency: hold`, all authored by
    // the delivery identity "CodeFactory" — the user never asked for any of
    // them. `hold` is the one genuine business gate in the pipeline: it blocks
    // the WHOLE batch, including other people's merges, and only a human can
    // clear it with allow_guarded_batch. An agent setting it on its own
    // manufactures a block only the user can undo, then walks into it. The tool
    // description read like a neutral safe choice, which is how it happened.
    //
    // `immediate` is not symmetric: it accelerates, its rubric is objective,
    // and a wrong call costs one early release. It stays agent-decidable.

    #[test]
    fn hold_requires_a_user_mandate_and_never_stops_the_delivery_without_one() {
        let unmandated = json!({"release_urgency": "hold"});
        let decided = release_urgency_from_args(&unmandated)
            .expect("an unmandated hold must not fail the delivery");
        assert!(
            decided.is_none(),
            "hold without a user mandate must be dropped, not applied"
        );

        let mandated = json!({
            "release_urgency": "hold",
            "hold_requested_by_user": "用户要求先别发，等文档就绪",
        });
        assert!(
            matches!(
                release_urgency_from_args(&mandated).expect("a mandated hold is valid"),
                Some(ReleaseUrgency::Hold)
            ),
            "an explicit user mandate still allows hold"
        );
    }

    #[test]
    fn immediate_stays_agent_decidable() {
        assert!(matches!(
            release_urgency_from_args(&json!({"release_urgency": "immediate"}))
                .expect("immediate needs no mandate"),
            Some(ReleaseUrgency::Immediate)
        ));
    }

    use super::*;
    use crate::agent::delivery::{DeliveryOutcome, RecoveryClass, StepResult};

    struct LiveOnlyReleaseObserver {
        dispatches: std::sync::atomic::AtomicUsize,
        release_observation: std::sync::Mutex<Option<delivery::ReleaseDispatchObservation>>,
        observed_targets: std::sync::Mutex<Vec<delivery::ReleaseDispatchTarget>>,
    }

    struct MergedBranchUpdateObserver {
        observations: std::sync::atomic::AtomicUsize,
        mutations: std::sync::atomic::AtomicUsize,
        merge_sha: String,
    }

    #[derive(Default)]
    struct LocalCommitRecoveryRemote {
        expected_head: std::sync::Mutex<Option<String>>,
        pr: std::sync::Mutex<Option<delivery::DeliveryPr>>,
    }

    impl delivery::DeliveryRemote for LocalCommitRecoveryRemote {
        fn capabilities(&self) -> delivery::DeliveryCapabilities {
            delivery::DeliveryCapabilities {
                review: true,
                ..delivery::DeliveryCapabilities::default()
            }
        }

        async fn open_or_get_pr(
            &self,
            title: &str,
            body: &str,
            _head: &str,
            _base: &str,
            expected_head_sha: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<delivery::DeliveryPr, String> {
            *self.expected_head.lock().unwrap() = Some(expected_head_sha.to_string());
            let pr = delivery::DeliveryPr {
                number: 1,
                url: "https://example.invalid/pull/1".into(),
                title: title.to_string(),
                body: body.to_string(),
            };
            *self.pr.lock().unwrap() = Some(pr.clone());
            Ok(pr)
        }

        async fn observe_open_pr(
            &self,
            head: &str,
            base: &str,
        ) -> std::result::Result<delivery::OpenPrObservation, String> {
            Ok(delivery::OpenPrObservation::Open(delivery::OpenPrState {
                pr: self
                    .pr
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| "local recovery PR has not been created".to_string())?,
                head_branch: head.to_string(),
                base_branch: base.to_string(),
                head_sha: self.expected_head.lock().unwrap().clone(),
            }))
        }

        async fn ci_status(&self, _sha: &str) -> std::result::Result<delivery::CiStatus, String> {
            Ok(delivery::CiStatus::None)
        }

        async fn merge_pr(
            &self,
            _number: u64,
            _method: crate::config::settings::MergeMethod,
            _commit_message: Option<&delivery::MergeCommitMessage>,
            _expected_head: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<delivery::MergeRequestResult, String> {
            unreachable!("the synthetic ceiling stops at PR")
        }

        async fn trigger_release(
            &self,
            _head_sha: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<String, String> {
            unreachable!("the synthetic ceiling stops at PR")
        }
    }

    fn recovery_git(cwd: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .no_window()
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn local_commit_recovery_repo() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "codefactory-local-commit-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let origin = root.join("origin.git");
        let worktree = root.join("worktree");
        std::fs::create_dir_all(&origin).unwrap();
        recovery_git(&origin, &["init", "--bare", "-q"]);
        std::fs::create_dir_all(&worktree).unwrap();
        recovery_git(&worktree, &["init", "-q"]);
        recovery_git(&worktree, &["config", "user.name", "Fixture"]);
        recovery_git(
            &worktree,
            &["config", "user.email", "fixture@example.invalid"],
        );
        std::fs::write(worktree.join("README.md"), "# fixture\n").unwrap();
        recovery_git(&worktree, &["add", "README.md"]);
        recovery_git(&worktree, &["commit", "-q", "-m", "chore: base"]);
        recovery_git(&worktree, &["branch", "-M", "main"]);
        recovery_git(
            &worktree,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        recovery_git(&worktree, &["push", "-q", "-u", "origin", "main"]);
        recovery_git(&worktree, &["checkout", "-q", "-b", "fix/local-recovery"]);
        std::fs::write(worktree.join("recovery.txt"), "recover me\n").unwrap();
        (root, worktree)
    }

    impl delivery::DeliveryRemote for LiveOnlyReleaseObserver {
        fn capabilities(&self) -> delivery::DeliveryCapabilities {
            delivery::DeliveryCapabilities {
                review: true,
                ci: true,
                merge: true,
                release: true,
                live: true,
            }
        }

        async fn open_or_get_pr(
            &self,
            _title: &str,
            _body: &str,
            _head: &str,
            _base: &str,
            _expected_head_sha: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<delivery::DeliveryPr, String> {
            unreachable!("release reconciliation is read-only")
        }

        async fn ci_status(&self, _sha: &str) -> std::result::Result<delivery::CiStatus, String> {
            unreachable!("release reconciliation does not inspect CI")
        }

        async fn merge_pr(
            &self,
            _number: u64,
            _method: crate::config::settings::MergeMethod,
            _commit_message: Option<&delivery::MergeCommitMessage>,
            _expected_head: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<delivery::MergeRequestResult, String> {
            unreachable!("release reconciliation must never merge")
        }

        async fn trigger_release(
            &self,
            _head_sha: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<String, String> {
            self.dispatches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("unexpected dispatch".into())
        }

        async fn observe_release_dispatch(
            &self,
            target: &delivery::ReleaseDispatchTarget,
        ) -> std::result::Result<delivery::ReleaseDispatchObservation, String> {
            self.observed_targets.lock().unwrap().push(target.clone());
            Ok(self
                .release_observation
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(delivery::ReleaseDispatchObservation::Absent))
        }

        async fn verify_live(
            &self,
            _sha: &str,
            _url: Option<&str>,
        ) -> std::result::Result<delivery::ObservationStatus, String> {
            Ok(delivery::ObservationStatus::Success(
                "generic live endpoint happened to match".into(),
            ))
        }
    }

    impl delivery::DeliveryRemote for MergedBranchUpdateObserver {
        fn capabilities(&self) -> delivery::DeliveryCapabilities {
            delivery::DeliveryCapabilities {
                review: true,
                merge: true,
                ..delivery::DeliveryCapabilities::default()
            }
        }

        async fn open_or_get_pr(
            &self,
            _title: &str,
            _body: &str,
            _head: &str,
            _base: &str,
            _expected_head_sha: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<delivery::DeliveryPr, String> {
            self.mutations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("branch-update recovery must never open a PR".into())
        }

        async fn ci_status(&self, _sha: &str) -> std::result::Result<delivery::CiStatus, String> {
            unreachable!("branch-update receipt reconciliation does not inspect CI")
        }

        async fn merge_pr(
            &self,
            _number: u64,
            _method: crate::config::settings::MergeMethod,
            _commit_message: Option<&delivery::MergeCommitMessage>,
            _expected_head: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<delivery::MergeRequestResult, String> {
            self.mutations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("branch-update recovery must never replay merge".into())
        }

        async fn observe_merge(
            &self,
            _number: u64,
            _expected_head: &str,
        ) -> std::result::Result<delivery::MergeObservation, String> {
            self.observations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(delivery::MergeObservation::Merged {
                merge_sha: self.merge_sha.clone(),
            })
        }

        async fn trigger_release(
            &self,
            _head_sha: &str,
            _mutation_permit: Option<&delivery::DeliveryMutationPermit>,
        ) -> std::result::Result<String, String> {
            self.mutations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("branch-update recovery must never dispatch release".into())
        }
    }

    #[test]
    fn foreground_delivery_lease_owner_is_unique_per_invocation() {
        let first = foreground_delivery_process_identity();
        let second = foreground_delivery_process_identity();
        assert_ne!(first.instance_id, second.instance_id);
        assert!(first.instance_id.contains(":delivery:"));
        assert!(second.instance_id.contains(":delivery:"));
    }

    fn outcome(final_state: &str) -> DeliveryOutcome {
        DeliveryOutcome {
            steps: vec![StepResult {
                step: "ci".into(),
                status: if final_state == "blocked" {
                    "blocked"
                } else if final_state == "waiting" {
                    "waiting"
                } else {
                    "ok"
                }
                .into(),
                detail: "detail".into(),
            }],
            branch: Some("feature/x".into()),
            commit_sha: None,
            pr_url: None,
            pr_number: None,
            final_state: final_state.into(),
            stage: "ci".into(),
            code: if final_state == "blocked" {
                "delivery_ci_blocked"
            } else if final_state == "waiting" {
                "delivery_ci_waiting"
            } else {
                "delivery_ceiling_reached"
            }
            .into(),
            recoverable: matches!(final_state, "blocked" | "waiting"),
            recovery_class: if final_state == "blocked" {
                RecoveryClass::AgentActionRequired
            } else if final_state == "waiting" {
                RecoveryClass::WaitRetryable
            } else {
                RecoveryClass::None
            },
            retry_after_ms: (final_state == "waiting").then_some(30_000),
            next_action: matches!(final_state, "blocked" | "waiting")
                .then(|| "retry same PR".into()),
            reached_state: "local".into(),
            requested_ceiling: "through_release".into(),
            effective_ceiling: "through_release".into(),
            capability_gap: None,
            release_receipt: None,
            summary: "summary".into(),
        }
    }

    #[test]
    fn blocked_report_bans_claiming_foreign_delivery() {
        // A blocked delivery must tell the model that any PR/merge/release it
        // later observes in the repo was produced by other executors — the
        // 2026-07-16 session claimed Codex's release as its own.
        let report = render_report(&outcome("blocked"));
        assert!(report.contains("不得归因"));
        assert!(report.contains("其他执行器"));
    }

    #[test]
    fn delivered_report_carries_no_attribution_warning() {
        let report = render_report(&outcome("delivered"));
        assert!(!report.contains("不得归因"));
    }

    #[test]
    fn release_urgency_parser_accepts_only_the_governed_values() {
        assert_eq!(
            parse_release_urgency("immediate"),
            Some(ReleaseUrgency::Immediate)
        );
        assert_eq!(parse_release_urgency("hold"), Some(ReleaseUrgency::Hold));
        assert_eq!(parse_release_urgency("soon"), None);
        assert!(
            release_urgency_from_args(&json!({"release_urgency": "soon"}))
                .unwrap_err()
                .contains("未执行任何交付动作")
        );
        assert!(release_urgency_from_args(&json!({"release_urgency": 1})).is_err());
        assert_eq!(release_urgency_from_args(&json!({})).unwrap(), None);
    }

    #[test]
    fn delivery_defaults_to_autonomous_completion_unless_explicitly_disabled() {
        assert!(autonomous_completion_from_args(&json!({})));
        assert!(autonomous_completion_from_args(&json!({
            "autonomous_completion": true
        })));
        assert!(!autonomous_completion_from_args(&json!({
            "autonomous_completion": false
        })));
    }

    #[test]
    fn system_owned_recovery_never_projects_as_a_blocked_tool() {
        let output = tool_output_for_outcome(&outcome("blocked"));
        assert_eq!(output.status, ToolExecutionStatus::Waiting);
        assert!(!output.is_error, "system-owned recovery is active work");
        assert!(output.content.contains("交付状态: recovering"));
        assert!(!output.content.contains("交付结果: blocked"));
        let metadata = output.metadata.expect("delivery metadata");
        assert_eq!(metadata["status"], "recovering");
        assert_eq!(metadata["delivery_state"], "blocked");
        assert_eq!(metadata["recoverable"], true);
        assert_eq!(metadata["decision_type"], "system_owned");
        assert_eq!(metadata["requires_user_continue"], false);
        assert_eq!(metadata["requested_ceiling"], "through_release");
        assert_eq!(metadata["effective_ceiling"], "through_release");
        assert_eq!(metadata["reached_state"], "local");
    }

    #[test]
    fn genuine_core_input_still_projects_as_blocked() {
        let mut core_input = outcome("blocked");
        core_input.recoverable = false;
        core_input.recovery_class = RecoveryClass::CoreInputRequired;
        let output = tool_output_for_outcome(&core_input);
        assert_eq!(output.status, ToolExecutionStatus::Blocked);
        assert_eq!(
            output.metadata.expect("delivery metadata")["decision_type"],
            "core_input_required"
        );
    }

    #[test]
    fn retryable_wait_has_a_distinct_tool_status_and_matching_report() {
        let output = tool_output_for_outcome(&outcome("waiting"));
        assert_eq!(output.status, ToolExecutionStatus::Waiting);
        assert!(!output.is_error);
        assert!(output.content.contains("交付结果: waiting"));
        assert!(output.content.contains("用户无需回复『继续』"));
        let metadata = output.metadata.expect("delivery metadata");
        assert_eq!(metadata["status"], "waiting");
        assert_eq!(metadata["recovery_class"], "wait_retryable");
        assert_eq!(metadata["retry_after_ms"], 30_000);
    }

    #[test]
    fn contradictory_delivery_contract_fails_closed() {
        let mut invalid = outcome("delivered");
        invalid.recoverable = true;
        invalid.recovery_class = RecoveryClass::WaitRetryable;
        invalid.retry_after_ms = Some(30_000);

        let output = tool_output_for_outcome(&invalid);

        assert_eq!(output.status, ToolExecutionStatus::Error);
        assert!(output.is_error);
        assert_eq!(
            output.metadata.expect("contract error metadata")["code"],
            "delivery_contract_invalid"
        );

        let mut zero_delay_wait = outcome("waiting");
        zero_delay_wait.retry_after_ms = Some(0);
        let output = tool_output_for_outcome(&zero_delay_wait);
        assert_eq!(output.status, ToolExecutionStatus::Error);
        assert!(output.is_error);
    }

    #[tokio::test]
    async fn session_delivery_reference_is_durable_and_replaced_by_latest_pr() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE session_delivery_refs (
                session_id TEXT PRIMARY KEY, branch TEXT NOT NULL,
                pr_number INTEGER NOT NULL, pr_url TEXT NOT NULL,
                commit_sha TEXT, updated_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut first = outcome("delivered");
        first.pr_number = Some(7);
        first.pr_url = Some("https://github.com/acme/repo/pull/7".into());
        first.commit_sha = Some("head-7".into());
        persist_delivery_ref(&pool, "session-1", &first)
            .await
            .unwrap();

        let mut latest = first.clone();
        latest.branch = Some("feature/latest".into());
        latest.pr_number = Some(8);
        latest.pr_url = Some("https://github.com/acme/repo/pull/8".into());
        latest.commit_sha = Some("head-8".into());
        persist_delivery_ref(&pool, "session-1", &latest)
            .await
            .unwrap();

        let row: (String, i64, String) = sqlx::query_as(
            "SELECT branch, pr_number, commit_sha FROM session_delivery_refs WHERE session_id = ?",
        )
        .bind("session-1")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, ("feature/latest".into(), 8, "head-8".into()));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_delivery_refs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn incomplete_or_noop_delivery_never_projects_business_completed() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let process = ProcessIdentity::new("process", "1.79.1", "17901");
        let run = NewDeliveryRun {
            id: "arbiter-run".into(),
            objective_id: "objective-opaque-arbiter".into(),
            run_kind: "chat_delivery".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("objective".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:arbiter".into(),
            repo_identity: "repo".into(),
            base_branch: "main".into(),
            head_branch: "feature/x".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "preflight".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("deliver".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let claim_epoch = delivery_run::create_delivery_run(
            &pool,
            &run,
            &process,
            chrono::Utc::now().timestamp_millis(),
            DELIVERY_LEASE_TTL_MS,
        )
        .await
        .unwrap();
        let mut prepared = PreparedDurableRun {
            id: run.id,
            process,
            claim_epoch,
            objective_id: run.objective_id,
            workspace_path: run.workspace_path.into(),
            worktree_identity: run.worktree_identity,
            repo_identity: run.repo_identity,
            change_set_digest: run.change_set_digest,
            expected_head_sha: "abc".into(),
            head_branch: "feature/x".into(),
        };

        let mut incomplete = outcome("delivered");
        incomplete.requested_ceiling = "through_release".into();
        incomplete.reached_state = "local".into();
        persist_durable_outcome(&pool, &mut prepared, &incomplete)
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM delivery_runs WHERE id='arbiter-run'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "awaiting_completion_arbitration");

        let mut evidence_candidate = outcome("delivered");
        evidence_candidate.requested_ceiling = "through_release".into();
        evidence_candidate.reached_state = "live_verified".into();
        evidence_candidate.commit_sha = Some("abc".into());
        evidence_candidate.pr_number = Some(42);
        evidence_candidate.pr_url = Some("https://example.invalid/pr/42".into());
        persist_durable_outcome(&pool, &mut prepared, &evidence_candidate)
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM delivery_runs WHERE id='arbiter-run'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            status, "awaiting_completion_arbitration",
            "DeliveryRun may project evidence but only CompletionArbiter may complete the Objective"
        );

        let mut noop = outcome("noop");
        noop.requested_ceiling = "through_release".into();
        noop.reached_state = "local".into();
        persist_durable_outcome(&pool, &mut prepared, &noop)
            .await
            .unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM delivery_runs WHERE id='arbiter-run'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "awaiting_completion_arbitration");
    }

    #[tokio::test]
    async fn external_state_uncertainty_remains_system_owned_in_durable_projection() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let process = ProcessIdentity::new("process", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "external-uncertainty-run".into(),
            objective_id: "objective-opaque-external-uncertainty".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:external-uncertainty".into(),
            repo_identity: "repo:external-uncertainty".into(),
            base_branch: "main".into(),
            head_branch: "feature/x".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "delivery".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let now = chrono::Utc::now().timestamp_millis();
        let claim_epoch = delivery_run::create_delivery_run(&pool, &run, &process, now, 60_000)
            .await
            .unwrap();
        let mut prepared = PreparedDurableRun {
            id: run.id,
            process,
            claim_epoch,
            objective_id: run.objective_id,
            workspace_path: run.workspace_path.into(),
            worktree_identity: run.worktree_identity,
            repo_identity: run.repo_identity,
            change_set_digest: run.change_set_digest,
            expected_head_sha: run.expected_head_sha,
            head_branch: run.head_branch,
        };
        let mut uncertain = outcome("waiting");
        uncertain.recovery_class = RecoveryClass::ExternalStateUncertain;
        uncertain.code = "delivery_mutation_uncertain".into();
        uncertain.next_action = Some("observe_only_reconcile".into());

        persist_durable_outcome(&pool, &mut prepared, &uncertain)
            .await
            .unwrap();
        let (status, wait_class, next_action): (String, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT status, wait_class, next_action FROM delivery_runs
                 WHERE id='external-uncertainty-run'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "waiting");
        assert_eq!(wait_class.as_deref(), Some("external_state_uncertain"));
        assert_eq!(next_action.as_deref(), Some("observe_only_reconcile"));
    }

    #[test]
    fn identity_revision_receipt_binds_stable_repo_and_worktree() {
        let prepared = PreparedDurableRun {
            id: "run".into(),
            process: ProcessIdentity::new("process", "1.79.2", "17902"),
            claim_epoch: 1,
            objective_id: "objective-opaque-revision".into(),
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:a".into(),
            repo_identity: "repo:a".into(),
            change_set_digest: "digest-before".into(),
            expected_head_sha: "aaa".into(),
            head_branch: "feature".into(),
        };

        assert!(build_delivery_identity_revision(
            &prepared,
            "repo:a",
            "worktree:a",
            "aaa",
            "digest-before",
        )
        .unwrap()
        .is_none());
        assert!(build_delivery_identity_revision(
            &prepared,
            "repo:b",
            "worktree:a",
            "bbb",
            "digest-after",
        )
        .is_err());

        let receipt = build_delivery_identity_revision(
            &prepared,
            "repo:a",
            "worktree:a",
            "bbb",
            "digest-after",
        )
        .unwrap()
        .expect("a new observed head requires a receipt");
        assert_eq!(receipt.objective_id, "objective-opaque-revision");
        assert_eq!(receipt.previous_expected_head_sha, "aaa");
        assert_eq!(receipt.next_expected_head_sha, "bbb");
        assert_eq!(receipt.next_change_set_digest, "digest-after");
        assert!(receipt.receipt_id.starts_with("sha256:"));
    }

    /// Gating `gh --watch` only works if the sanctioned alternative is
    /// discoverable. `through_ci_green` already waits for CI with backoff, but
    /// nothing said so — so the model reached for `--watch` and burned the
    /// quota (2026-08-03/04). Deny without an alternative just moves the bypass.
    #[test]
    fn the_schema_points_at_the_managed_way_to_wait_for_ci() {
        let schema = definition().function.parameters.to_string();
        assert!(schema.contains("through_ci_green"));
        assert!(
            schema.contains("WAIT for CI"),
            "the ceiling doc must name waiting as its purpose"
        );
        assert!(
            schema.contains("--watch"),
            "and must name the bypass it replaces: {schema}"
        );
    }

    #[test]
    fn startup_resumes_authorized_system_owned_states_without_requiring_autonomous_flag() {
        let claimed = |status: &str, authorized: bool| delivery_run::ClaimedRecovery {
            run_id: "run".into(),
            claim_epoch: 1,
            objective_id: "objective-opaque-supervisor".into(),
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:run".into(),
            repo_identity: "repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            requested_ceiling: "through_release".into(),
            autonomous_completion: false,
            canonical_pr_number: Some(1),
            canonical_pr_url: Some("https://example.invalid/pr/1".into()),
            canonical_head_sha: Some("abc".into()),
            reached_ceiling: "pr_open".into(),
            stage: "ci".into(),
            status: status.into(),
            wait_class: Some("wait_retryable".into()),
            next_action: Some("observe_ci".into()),
            next_action_authorized: authorized,
            failure_signature: None,
            stage_attempt: 0,
            progress_revision: 1,
            action: delivery_run::RecoveryAction::ObserveOnly,
        };

        assert!(should_resume_claimed_delivery(&claimed("running", true)));
        assert!(should_resume_claimed_delivery(&claimed("waiting", true)));
        assert!(should_resume_claimed_delivery(&claimed(
            "platform_incident",
            true
        )));
        assert!(should_resume_claimed_delivery(&claimed(
            "agent_action_required",
            true
        )));
        assert!(should_resume_claimed_delivery(&claimed(
            "failed_internal",
            true
        )));
        assert!(should_resume_claimed_delivery(&claimed(
            "awaiting_completion_arbitration",
            true
        )));
        assert!(!should_resume_claimed_delivery(&claimed(
            "platform_incident",
            false
        )));
        assert!(!should_resume_claimed_delivery(&claimed("waiting", false)));
        assert!(!should_resume_claimed_delivery(&claimed(
            "core_input_required",
            true
        )));
        assert!(!should_resume_claimed_delivery(&claimed(
            "needs_business_decision",
            true
        )));
    }

    #[test]
    fn local_takeover_identity_conflict_has_a_stable_bounded_recovery_signature() {
        let claimed = delivery_run::ClaimedRecovery {
            run_id: "identity-conflict-run".into(),
            claim_epoch: 7,
            objective_id: "objective-opaque-identity-conflict".into(),
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:identity-conflict".into(),
            repo_identity: "repo:identity-conflict".into(),
            base_branch: "main".into(),
            head_branch: "feature/identity-conflict".into(),
            change_set_digest: "sha256:persisted".into(),
            expected_head_sha: "abc".into(),
            requested_ceiling: "through_release".into(),
            autonomous_completion: true,
            canonical_pr_number: Some(411),
            canonical_pr_url: Some("https://example.invalid/pull/411".into()),
            canonical_head_sha: Some("abc".into()),
            reached_ceiling: "pr_open".into(),
            stage: "takeover_reconciliation".into(),
            status: "platform_incident".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            failure_signature: None,
            stage_attempt: 0,
            progress_revision: 1,
            action: delivery_run::RecoveryAction::ObserveOnly,
        };

        let observation = local_takeover_identity_conflict_observation(&claimed);
        assert_eq!(
            observation.wait_class.as_deref(),
            Some("delivery_identity_conflict")
        );
        assert_eq!(
            observation.next_action.as_deref(),
            Some("await_system_capability_change")
        );
        assert_eq!(
            observation.failure_signature.as_deref(),
            Some("takeover_reconciliation:delivery_identity_conflict")
        );
        assert_eq!(observation.expected_head_sha, "abc");
        assert_eq!(observation.canonical_pr_number, Some(411));
    }

    #[tokio::test]
    async fn post_commit_pre_persist_crash_reconciles_the_exact_receipted_head_once() {
        let (fixture_root, worktree) = local_commit_recovery_repo();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE objectives (
                id TEXT PRIMARY KEY,
                delivery_run_id TEXT,
                status TEXT NOT NULL,
                failure_code TEXT,
                recovery_owner TEXT,
                remediation_id TEXT,
                next_observation_at INTEGER,
                requires_user_action INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objectives (id, delivery_run_id, status, updated_at)
             VALUES ('objective-opaque-post-commit-crash',
                     'post-commit-crash-run', 'active', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (repo, _) =
            delivery::resolve_delivery_repo(&worktree, Some("main"), Some("fix/local-recovery"))
                .unwrap();
        let before = delivery::capture_delivery_identity(&repo).unwrap();
        let owner = ProcessIdentity::new("owner-before-crash", "test", "test");
        let run = NewDeliveryRun {
            id: "post-commit-crash-run".into(),
            objective_id: "objective-opaque-post-commit-crash".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: worktree.to_string_lossy().into_owned(),
            worktree_identity: before.worktree_identity.clone(),
            repo_identity: before.repo_identity.clone(),
            base_branch: "main".into(),
            head_branch: "fix/local-recovery".into(),
            change_set_digest: before.change_set_digest.clone(),
            expected_head_sha: before.head_sha.clone(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "pr_only".into(),
            reached_ceiling: "local".into(),
            stage: "preflight".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("deliver".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let now = chrono::Utc::now().timestamp_millis();
        let owner_epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 90_000)
            .await
            .unwrap();
        let prepared = PreparedDurableRun {
            id: run.id.clone(),
            process: owner.clone(),
            claim_epoch: owner_epoch,
            objective_id: run.objective_id.clone(),
            workspace_path: worktree.clone(),
            worktree_identity: run.worktree_identity.clone(),
            repo_identity: run.repo_identity.clone(),
            change_set_digest: run.change_set_digest.clone(),
            expected_head_sha: run.expected_head_sha.clone(),
            head_branch: run.head_branch.clone(),
        };
        let outcome = delivery::deliver(
            &worktree,
            DeliveryCeiling::PrOnly,
            crate::config::settings::MergeMethod::Squash,
            1,
            &DeliverOpts {
                title: Some("fix: local commit recovery".into()),
                requested_ceiling: Some(DeliveryCeiling::PrOnly),
                expect_branch: Some("fix/local-recovery".into()),
                expected_identity: Some(before.clone()),
                mutation_permit: Some(prepared.mutation_permit(&pool)),
                ..DeliverOpts::default()
            },
            Some(&LocalCommitRecoveryRemote::default()),
            Some("main"),
        )
        .await;
        let committed_head = outcome.commit_sha.clone().unwrap_or_else(|| {
            panic!(
                "delivery created the local commit before the injected persistence gap: {:?}",
                outcome.steps
            )
        });
        assert_ne!(committed_head, before.head_sha);
        let persisted_before_recovery: String = sqlx::query_scalar(
            "SELECT expected_head_sha FROM delivery_runs WHERE id='post-commit-crash-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(persisted_before_recovery, before.head_sha);
        let local_intent_status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_mutation_intents
             WHERE run_id='post-commit-crash-run' AND rung='git_local_commit'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(local_intent_status, "committed");

        sqlx::query(
            "UPDATE delivery_runs SET lease_expires_at=?
             WHERE id='post-commit-crash-run'",
        )
        .bind(now - 1)
        .execute(&pool)
        .await
        .unwrap();
        let takeover_process = ProcessIdentity::new("owner-after-crash", "test", "test");
        let mut claimed =
            delivery_run::plan_startup_recovery(&pool, &takeover_process, now + 1, 90_000)
                .await
                .unwrap()
                .claimed
                .into_iter()
                .next()
                .expect("the expired authorized run is claimed once");
        let previous_head =
            reconcile_receipted_local_commit_head(&pool, &mut claimed, &takeover_process)
                .await
                .unwrap()
                .expect("the exact receipted commit advances the durable head");
        assert_eq!(previous_head, before.head_sha);
        assert_eq!(claimed.expected_head_sha, committed_head);
        assert_eq!(
            reconcile_receipted_local_commit_head(&pool, &mut claimed, &takeover_process)
                .await
                .unwrap(),
            Some(before.head_sha.clone()),
            "the exact identity revision is idempotent while its parent allowance remains durable"
        );
        sqlx::query(
            "UPDATE delivery_runs SET lease_expires_at=?
             WHERE id='post-commit-crash-run'",
        )
        .bind(now - 1)
        .execute(&pool)
        .await
        .unwrap();
        let third_process = ProcessIdentity::new("owner-after-second-loss", "test", "test");
        let mut claimed_after_second_loss =
            delivery_run::plan_startup_recovery(&pool, &third_process, now + 2, 90_000)
                .await
                .unwrap()
                .claimed
                .into_iter()
                .next()
                .expect("the same run is reclaimed after a second lease loss");
        assert_eq!(
            reconcile_receipted_local_commit_head(
                &pool,
                &mut claimed_after_second_loss,
                &third_process,
            )
            .await
            .unwrap(),
            Some(before.head_sha.clone()),
            "the receipted parent allowance must survive a second process loss"
        );
        let persisted_after_recovery: (String, i64) = sqlx::query_as(
            "SELECT expected_head_sha,
                    (SELECT COUNT(*) FROM delivery_identity_revisions
                     WHERE run_id=delivery_runs.id)
             FROM delivery_runs WHERE id='post-commit-crash-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(persisted_after_recovery, (committed_head, 1));
        std::fs::remove_dir_all(fixture_root).ok();
    }

    #[tokio::test]
    async fn committed_branch_update_recovers_from_merged_pr_ref_after_head_branch_deletion_once() {
        let (fixture_root, worktree) = local_commit_recovery_repo();
        recovery_git(&worktree, &["add", "recovery.txt"]);
        recovery_git(&worktree, &["commit", "-q", "-m", "fix: old PR head"]);
        recovery_git(
            &worktree,
            &["push", "-q", "-u", "origin", "fix/local-recovery"],
        );
        let previous_head = recovery_git(&worktree, &["rev-parse", "HEAD"]);
        let tree = recovery_git(&worktree, &["rev-parse", "HEAD^{tree}"]);
        let next_head = recovery_git(
            &worktree,
            &[
                "commit-tree",
                &tree,
                "-p",
                &previous_head,
                "-m",
                "synthetic provider branch update",
            ],
        );
        let pr_refspec = format!("+{next_head}:refs/pull/7/head");
        recovery_git(&worktree, &["push", "-q", "origin", &pr_refspec]);
        let main_refspec = format!("+{next_head}:refs/heads/main");
        recovery_git(&worktree, &["push", "-q", "origin", &main_refspec]);
        recovery_git(
            &worktree,
            &["push", "-q", "origin", "--delete", "fix/local-recovery"],
        );

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE objectives (
                id TEXT PRIMARY KEY,
                delivery_run_id TEXT,
                status TEXT NOT NULL,
                failure_code TEXT,
                recovery_owner TEXT,
                remediation_id TEXT,
                next_observation_at INTEGER,
                requires_user_action INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objectives (id, delivery_run_id, status, updated_at)
             VALUES ('objective-branch-update-deleted-head',
                     'branch-update-deleted-head-run', 'active', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (repo, _) =
            delivery::resolve_delivery_repo(&worktree, Some("main"), Some("fix/local-recovery"))
                .unwrap();
        let before = delivery::capture_delivery_identity(&repo).unwrap();
        assert_eq!(before.head_sha, previous_head);
        let owner = ProcessIdentity::new("branch-update-old-owner", "test", "test");
        let run = NewDeliveryRun {
            id: "branch-update-deleted-head-run".into(),
            objective_id: "objective-branch-update-deleted-head".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: worktree.to_string_lossy().into_owned(),
            worktree_identity: before.worktree_identity.clone(),
            repo_identity: before.repo_identity.clone(),
            base_branch: "main".into(),
            head_branch: "fix/local-recovery".into(),
            change_set_digest: before.change_set_digest.clone(),
            expected_head_sha: previous_head.clone(),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pull/7".into()),
            canonical_head_sha: Some(previous_head.clone()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "merge_queued".into(),
            stage: "branch_update".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let now = chrono::Utc::now().timestamp_millis();
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 90_000)
            .await
            .unwrap();
        let number = "7";
        let operation_key = delivery::external_operation_key(
            "provider_pr_branch_update",
            &[number, &previous_head],
        );
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "branch-update-deleted-head-intent",
            &run.id,
            &owner,
            epoch,
            "provider_pr_branch_update",
            &operation_key,
            Some(&json!({"pr_number": 7, "expected_head": previous_head}).to_string(),),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::resolve_delivery_mutation_intent_committed(
            &pool,
            "branch-update-deleted-head-intent",
            &owner,
            epoch,
            Some(&json!({"head": next_head}).to_string()),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();
        let replacement = ProcessIdentity::new("branch-update-new-owner", "test", "test");
        let mut claimed = delivery_run::plan_startup_recovery(&pool, &replacement, now + 3, 90_000)
            .await
            .unwrap()
            .claimed
            .into_iter()
            .next()
            .expect("the expired branch-update run is claimed");
        let observer = MergedBranchUpdateObserver {
            observations: std::sync::atomic::AtomicUsize::new(0),
            mutations: std::sync::atomic::AtomicUsize::new(0),
            merge_sha: next_head.clone(),
        };

        reconcile_receipted_branch_update_head(&pool, &mut claimed, &replacement, Some(&observer))
            .await
            .unwrap();
        assert_eq!(claimed.expected_head_sha, next_head);
        assert_eq!(recovery_git(&worktree, &["rev-parse", "HEAD"]), next_head);
        let status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_mutation_intents
             WHERE intent_id='branch-update-deleted-head-intent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "reconciled_committed");
        assert_eq!(
            observer
                .observations
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            observer.mutations.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        reconcile_receipted_branch_update_head(&pool, &mut claimed, &replacement, Some(&observer))
            .await
            .unwrap();
        assert_eq!(
            observer
                .observations
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a reconciled committed branch update must not be observed twice"
        );
        assert!(delivery_run::mark_delivery_claim_reconciled(
            &pool,
            &run.id,
            &replacement,
            claimed.claim_epoch,
            now + 5,
        )
        .await
        .unwrap());
        assert!(delivery_run::verify_delivery_mutation_permit(
            &pool,
            &run.id,
            &replacement,
            claimed.claim_epoch,
            now + 5,
        )
        .await
        .unwrap());
        std::fs::remove_dir_all(fixture_root).ok();
    }

    #[test]
    fn recovery_supervisor_reclaims_within_product_slo() {
        assert!(recovery_supervisor_poll_interval() <= std::time::Duration::from_secs(30));
    }

    #[tokio::test]
    async fn in_flight_heartbeat_prevents_competing_recovery_across_original_lease() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-competitor", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "heartbeat-wrapper-run".into(),
            objective_id: "objective-opaque-heartbeat".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:heartbeat".into(),
            repo_identity: "repo:heartbeat".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "delivery".into(),
            status: "waiting".into(),
            wait_class: Some("wait_retryable".into()),
            next_action: Some("observe_ci".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let claim_epoch = delivery_run::create_delivery_run(&pool, &run, &owner, 100, 10)
            .await
            .unwrap();

        let ticks = std::sync::Arc::new(tokio::sync::Notify::new());
        let clock = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from([
            100_i64, 120, 140,
        ])));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
        let heartbeat_task = {
            let pool = pool.clone();
            let owner = owner.clone();
            let ticks_for_driver = ticks.clone();
            let clock_for_driver = clock.clone();
            let calls = calls.clone();
            tokio::spawn(async move {
                drive_delivery_with_lease_heartbeat(
                    &pool,
                    "heartbeat-wrapper-run",
                    &owner,
                    claim_epoch,
                    async move {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        finish_rx.await.unwrap();
                        "finished"
                    },
                    move || {
                        let ticks = ticks_for_driver.clone();
                        async move { ticks.notified().await }
                    },
                    move || {
                        clock_for_driver
                            .lock()
                            .unwrap()
                            .pop_front()
                            .expect("one timestamp per heartbeat")
                    },
                    30,
                )
                .await
            })
        };

        async fn wait_for_lease(pool: &sqlx::SqlitePool, expected: i64) {
            for _ in 0..100 {
                let observed: i64 = sqlx::query_scalar(
                    "SELECT lease_expires_at FROM delivery_runs WHERE id='heartbeat-wrapper-run'",
                )
                .fetch_one(pool)
                .await
                .unwrap();
                if observed == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("heartbeat did not advance lease to {expected}");
        }

        wait_for_lease(&pool, 130).await;
        ticks.notify_one();
        wait_for_lease(&pool, 150).await;
        let first_competing_claim =
            delivery_run::plan_startup_recovery(&pool, &competitor, 140, 30)
                .await
                .unwrap();
        assert!(first_competing_claim.claimed.is_empty());

        ticks.notify_one();
        wait_for_lease(&pool, 170).await;
        let second_competing_claim =
            delivery_run::plan_startup_recovery(&pool, &competitor, 160, 30)
                .await
                .unwrap();
        assert!(second_competing_claim.claimed.is_empty());

        finish_tx.send(()).unwrap();
        assert_eq!(heartbeat_task.await.unwrap().unwrap(), "finished");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let mut revision = DeliveryIdentityRevision {
            receipt_id: String::new(),
            objective_id: run.objective_id.clone(),
            repo_identity: "repo:heartbeat".into(),
            worktree_identity: "worktree:heartbeat".into(),
            previous_expected_head_sha: "abc".into(),
            previous_change_set_digest: "digest".into(),
            next_expected_head_sha: "def".into(),
            next_change_set_digest: "digest-after".into(),
        };
        revision.receipt_id =
            delivery_run::delivery_identity_revision_receipt_id("heartbeat-wrapper-run", &revision);
        let observation = DeliveryObservation {
            head_branch: "feature".into(),
            stage: "commit".into(),
            status: "waiting".into(),
            wait_class: Some("wait_retryable".into()),
            next_action: Some("push".into()),
            reached_ceiling: "committed".into(),
            expected_head_sha: "def".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            failure_signature: None,
            core_input: None,
            identity_revision: Some(revision),
        };
        assert!(delivery_run::record_delivery_observation(
            &pool,
            "heartbeat-wrapper-run",
            &owner,
            claim_epoch,
            &observation,
            165,
            30,
        )
        .await
        .unwrap());
        let persisted_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_identity_revisions
             WHERE run_id='heartbeat-wrapper-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(persisted_receipts, 1);
    }

    #[tokio::test]
    async fn lost_heartbeat_drops_old_future_and_takeover_stays_observe_only() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-competitor", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "heartbeat-loss-run".into(),
            objective_id: "objective-opaque-heartbeat-loss".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:heartbeat-loss".into(),
            repo_identity: "repo:heartbeat-loss".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "delivery".into(),
            status: "waiting".into(),
            wait_class: Some("wait_retryable".into()),
            next_action: Some("observe_ci".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let owner_epoch = delivery_run::create_delivery_run(&pool, &run, &owner, 100, 10)
            .await
            .unwrap();
        let ticks = std::sync::Arc::new(tokio::sync::Notify::new());
        let clock = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from([
            100_i64, 132,
        ])));
        let mutations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (allow_mutation_tx, allow_mutation_rx) = tokio::sync::oneshot::channel::<()>();
        let task = {
            let pool = pool.clone();
            let owner = owner.clone();
            let ticks_for_driver = ticks.clone();
            let clock_for_driver = clock.clone();
            let mutations = mutations.clone();
            tokio::spawn(async move {
                drive_delivery_with_lease_heartbeat(
                    &pool,
                    "heartbeat-loss-run",
                    &owner,
                    owner_epoch,
                    async move {
                        let _ = allow_mutation_rx.await;
                        mutations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    },
                    move || {
                        let ticks = ticks_for_driver.clone();
                        async move { ticks.notified().await }
                    },
                    move || {
                        clock_for_driver
                            .lock()
                            .unwrap()
                            .pop_front()
                            .expect("one timestamp per heartbeat")
                    },
                    30,
                )
                .await
            })
        };

        for _ in 0..100 {
            let expires: i64 = sqlx::query_scalar(
                "SELECT lease_expires_at FROM delivery_runs WHERE id='heartbeat-loss-run'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if expires == 130 {
                break;
            }
            tokio::task::yield_now().await;
        }
        let takeover = delivery_run::plan_startup_recovery(&pool, &competitor, 131, 30)
            .await
            .unwrap()
            .claimed
            .pop()
            .expect("competitor claims the expired epoch");
        ticks.notify_one();
        let error = task.await.unwrap().expect_err("old owner must be dropped");
        assert!(error.to_string().contains("external state is uncertain"));
        assert!(
            allow_mutation_tx.send(()).is_err(),
            "old future was dropped"
        );
        assert_eq!(mutations.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!delivery_run::verify_delivery_mutation_permit(
            &pool,
            "heartbeat-loss-run",
            &competitor,
            takeover.claim_epoch,
            132,
        )
        .await
        .unwrap());
        assert!(delivery_run::mark_delivery_claim_reconciled(
            &pool,
            "heartbeat-loss-run",
            &competitor,
            takeover.claim_epoch,
            132,
        )
        .await
        .unwrap());
        assert!(delivery_run::verify_delivery_mutation_permit(
            &pool,
            "heartbeat-loss-run",
            &competitor,
            takeover.claim_epoch,
            132,
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn takeover_requires_and_consumes_positive_push_observation_before_new_permit() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-competitor", "1.79.2", "17902");
        let now = chrono::Utc::now().timestamp_millis();
        let run = NewDeliveryRun {
            id: "push-takeover-run".into(),
            objective_id: "objective-opaque-push-takeover".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:push-takeover".into(),
            repo_identity: "repo:push-takeover".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "push".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 60_000)
            .await
            .unwrap();
        let operation_key = delivery::external_operation_key(
            "git_push",
            &[&run.repo_identity, &run.head_branch, &run.expected_head_sha],
        );
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "push-intent",
            &run.id,
            &owner,
            epoch,
            "git_push",
            &operation_key,
            Some(r#"{"commit_sha":"abc"}"#),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::mark_delivery_mutation_intent_unknown(
            &pool,
            "push-intent",
            &owner,
            epoch,
            Some(r#"{"classification":"timeout"}"#),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();

        let plan = delivery_run::plan_startup_recovery(&pool, &competitor, now + 3, 60_000)
            .await
            .unwrap();
        let claimed = plan.claimed.into_iter().next().unwrap();
        let mut absent = delivery::DeliveryTakeoverObservation {
            identity: DeliveryIdentitySnapshot {
                repo_identity: run.repo_identity.clone(),
                worktree_identity: run.worktree_identity.clone(),
                head_sha: run.expected_head_sha.clone(),
                change_set_digest: run.change_set_digest.clone(),
            },
            remote_head_sha: None,
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
        };
        assert!(reconcile_unresolved_delivery_mutation_intents(
            &pool,
            &claimed,
            &competitor,
            None::<&delivery::EitherRemote>,
            &mut absent,
        )
        .await
        .is_err());
        assert!(!delivery_run::mark_delivery_claim_reconciled(
            &pool,
            &run.id,
            &competitor,
            claimed.claim_epoch,
            now + 4,
        )
        .await
        .unwrap());

        absent.remote_head_sha = Some("abc".into());
        reconcile_unresolved_delivery_mutation_intents(
            &pool,
            &claimed,
            &competitor,
            None::<&delivery::EitherRemote>,
            &mut absent,
        )
        .await
        .unwrap();
        assert!(delivery_run::mark_delivery_claim_reconciled(
            &pool,
            &run.id,
            &competitor,
            claimed.claim_epoch,
            now + 5,
        )
        .await
        .unwrap());
        assert!(delivery_run::verify_delivery_mutation_permit(
            &pool,
            &run.id,
            &competitor,
            claimed.claim_epoch,
            now + 6,
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn release_mutation_intent_rejects_nonmatching_workflow_ref_head_even_if_live_is_green() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("release-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("release-competitor", "1.79.2", "17902");
        let now = chrono::Utc::now().timestamp_millis();
        let run = NewDeliveryRun {
            id: "release-takeover-run".into(),
            objective_id: "objective-opaque-release-takeover".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:release-takeover".into(),
            repo_identity: "repo:release-takeover".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "expected-head".into(),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some("expected-head".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "merged".into(),
            stage: "release".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 60_000)
            .await
            .unwrap();
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "release-intent",
            &run.id,
            &owner,
            epoch,
            "provider_release_trigger",
            "sha256:not-the-exact-operation",
            Some(
                r#"{"workflow":"other-release.yml","git_ref":"other","expected_head":"other-head"}"#,
            ),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::mark_delivery_mutation_intent_unknown(
            &pool,
            "release-intent",
            &owner,
            epoch,
            Some(r#"{"classification":"post_in_flight"}"#),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();
        let claimed = delivery_run::plan_startup_recovery(&pool, &competitor, now + 3, 60_000)
            .await
            .unwrap()
            .claimed
            .into_iter()
            .next()
            .unwrap();
        let mut takeover = delivery::DeliveryTakeoverObservation {
            identity: DeliveryIdentitySnapshot {
                repo_identity: run.repo_identity.clone(),
                worktree_identity: run.worktree_identity.clone(),
                head_sha: run.expected_head_sha.clone(),
                change_set_digest: run.change_set_digest.clone(),
            },
            remote_head_sha: Some(run.expected_head_sha.clone()),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some(run.expected_head_sha.clone()),
        };
        let remote = LiveOnlyReleaseObserver {
            dispatches: std::sync::atomic::AtomicUsize::new(0),
            release_observation: std::sync::Mutex::new(None),
            observed_targets: std::sync::Mutex::new(Vec::new()),
        };

        reconcile_unresolved_delivery_mutation_intents(
            &pool,
            &claimed,
            &competitor,
            Some(&remote),
            &mut takeover,
        )
        .await
        .expect_err("generic live evidence must not reconcile a different workflow/ref/head");

        assert_eq!(
            remote.dispatches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_mutation_intents WHERE intent_id='release-intent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "unknown");
    }

    #[tokio::test]
    async fn in_flight_release_post_reconciles_only_the_exact_workflow_ref_head_without_redispatch()
    {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("release-owner-exact", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("release-competitor-exact", "1.79.2", "17902");
        let now = chrono::Utc::now().timestamp_millis();
        let run = NewDeliveryRun {
            id: "release-takeover-exact-run".into(),
            objective_id: "objective-opaque-release-exact".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:release-exact".into(),
            repo_identity: "repo:release-exact".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "delivery-feature-head".into(),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some("delivery-feature-head".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "merged".into(),
            stage: "release".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 60_000)
            .await
            .unwrap();
        let target = delivery::ReleaseDispatchTarget {
            workflow: "auto-release.yml".into(),
            git_ref: run.base_branch.clone(),
            head_sha: "release-main-head".into(),
        };
        let envelope = serde_json::to_string(&target).unwrap();
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "release-exact-intent",
            &run.id,
            &owner,
            epoch,
            "provider_release_trigger",
            &target.operation_key(),
            Some(&envelope),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::mark_delivery_mutation_intent_unknown(
            &pool,
            "release-exact-intent",
            &owner,
            epoch,
            Some(r#"{"classification":"post_in_flight"}"#),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();
        let claimed = delivery_run::plan_startup_recovery(&pool, &competitor, now + 3, 60_000)
            .await
            .unwrap()
            .claimed
            .into_iter()
            .next()
            .unwrap();
        let remote = LiveOnlyReleaseObserver {
            dispatches: std::sync::atomic::AtomicUsize::new(0),
            release_observation: std::sync::Mutex::new(Some(
                delivery::ReleaseDispatchObservation::Triggered {
                    run_id: "workflow-run-42".into(),
                    status: "in_progress".into(),
                    head_sha: target.head_sha.clone(),
                    detail: "https://example.invalid/actions/runs/42".into(),
                },
            )),
            observed_targets: std::sync::Mutex::new(Vec::new()),
        };
        let mut takeover = delivery::DeliveryTakeoverObservation {
            identity: DeliveryIdentitySnapshot {
                repo_identity: run.repo_identity.clone(),
                worktree_identity: run.worktree_identity.clone(),
                head_sha: run.expected_head_sha.clone(),
                change_set_digest: run.change_set_digest.clone(),
            },
            remote_head_sha: Some(run.expected_head_sha.clone()),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some(run.expected_head_sha.clone()),
        };

        reconcile_unresolved_delivery_mutation_intents(
            &pool,
            &claimed,
            &competitor,
            Some(&remote),
            &mut takeover,
        )
        .await
        .unwrap();

        assert_eq!(*remote.observed_targets.lock().unwrap(), vec![target]);
        assert_eq!(
            remote.dispatches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_mutation_intents WHERE intent_id='release-exact-intent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "reconciled_committed");
    }

    #[tokio::test]
    async fn committed_release_intent_still_fences_local_absence_replay() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        delivery_run::ensure_schema(&pool).await.unwrap();
        let owner = ProcessIdentity::new("release-owner-committed", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("release-competitor-committed", "1.79.2", "17902");
        let now = chrono::Utc::now().timestamp_millis();
        let run = NewDeliveryRun {
            id: "release-takeover-committed-run".into(),
            objective_id: "objective-opaque-release-committed".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:release-committed".into(),
            repo_identity: "repo:release-committed".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "delivery-feature-head".into(),
            canonical_pr_number: Some(7),
            canonical_pr_url: Some("https://example.invalid/pr/7".into()),
            canonical_head_sha: Some("delivery-feature-head".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "merged".into(),
            stage: "release".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let epoch = delivery_run::create_delivery_run(&pool, &run, &owner, now, 60_000)
            .await
            .unwrap();
        let target = delivery::ReleaseDispatchTarget {
            workflow: "auto-release.yml".into(),
            git_ref: run.base_branch.clone(),
            head_sha: "release-main-head".into(),
        };
        assert!(delivery_run::begin_delivery_mutation_intent(
            &pool,
            "release-committed-intent",
            &run.id,
            &owner,
            epoch,
            "provider_release_trigger",
            &target.operation_key(),
            Some(&serde_json::to_string(&target).unwrap()),
            now + 1,
        )
        .await
        .unwrap());
        assert!(delivery_run::resolve_delivery_mutation_intent_committed(
            &pool,
            "release-committed-intent",
            &owner,
            epoch,
            Some(r#"{"dispatch":"accepted"}"#),
            now + 2,
        )
        .await
        .unwrap());
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(now - 1)
            .bind(&run.id)
            .execute(&pool)
            .await
            .unwrap();
        let claimed = delivery_run::plan_startup_recovery(&pool, &competitor, now + 3, 60_000)
            .await
            .unwrap()
            .claimed
            .into_iter()
            .next()
            .unwrap();
        let remote = LiveOnlyReleaseObserver {
            dispatches: std::sync::atomic::AtomicUsize::new(0),
            release_observation: std::sync::Mutex::new(Some(
                delivery::ReleaseDispatchObservation::Triggered {
                    run_id: "workflow-run-committed".into(),
                    status: "completed".into(),
                    head_sha: target.head_sha.clone(),
                    detail: "https://example.invalid/actions/runs/committed".into(),
                },
            )),
            observed_targets: std::sync::Mutex::new(Vec::new()),
        };
        let mut takeover = delivery::DeliveryTakeoverObservation {
            identity: DeliveryIdentitySnapshot {
                repo_identity: run.repo_identity.clone(),
                worktree_identity: run.worktree_identity.clone(),
                head_sha: run.expected_head_sha.clone(),
                change_set_digest: run.change_set_digest.clone(),
            },
            remote_head_sha: Some(run.expected_head_sha.clone()),
            canonical_pr_number: Some(7),
            canonical_pr_url: run.canonical_pr_url.clone(),
            canonical_head_sha: run.canonical_head_sha.clone(),
        };

        assert!(
            reconcile_unresolved_delivery_mutation_intents(
                &pool,
                &claimed,
                &competitor,
                Some(&remote),
                &mut takeover,
            )
            .await
            .unwrap(),
            "a committed release dispatch still proves DB begin occurred and must fence local absence replay"
        );
        assert_eq!(*remote.observed_targets.lock().unwrap(), vec![target]);
        assert_eq!(
            remote.dispatches.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_mutation_intents WHERE intent_id='release-committed-intent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "reconciled_committed");
    }

    #[tokio::test]
    async fn contextual_root_turns_share_one_durable_objective() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_task_segments (
                id TEXT PRIMARY KEY,
                previous_segment_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
                root_turn_id TEXT PRIMARY KEY,
                task_segment_id TEXT,
                objective_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_task_segments(id, previous_segment_id) VALUES
             ('segment-a', NULL), ('segment-b', 'segment-a')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, task_segment_id, objective_id) VALUES
             ('turn-a', 'segment-a', 'objective-opaque-uuid'),
             ('turn-b', 'segment-b', 'objective-opaque-uuid')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut first = ExecCtx::new(std::path::PathBuf::from("/tmp"), Some(pool.clone()));
        first.root_turn_id = Some("turn-a".into());
        let mut continuation = ExecCtx::new(std::path::PathBuf::from("/tmp"), Some(pool.clone()));
        continuation.root_turn_id = Some("turn-b".into());

        let first_identity = durable_objective_identity(&pool, &first).await.unwrap();
        let continuation_identity = durable_objective_identity(&pool, &continuation)
            .await
            .unwrap();
        assert_eq!(first_identity.0, "objective-opaque-uuid");
        assert_eq!(continuation_identity.0, first_identity.0);
        assert_eq!(continuation_identity.1.as_deref(), Some("segment-b"));
    }

    #[tokio::test]
    async fn delivery_refuses_legacy_string_fallback_without_unified_objective_binding() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
                root_turn_id TEXT PRIMARY KEY,
                task_segment_id TEXT,
                objective_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, task_segment_id, objective_id)
             VALUES ('legacy-turn', 'legacy-segment', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut ctx = ExecCtx::new(std::path::PathBuf::from("/tmp"), Some(pool.clone()));
        ctx.root_turn_id = Some("legacy-turn".into());

        let error = durable_objective_identity(&pool, &ctx).await.unwrap_err();
        assert!(error.to_string().contains("unified objective identity"));
    }

    #[tokio::test]
    async fn task_delivery_uses_the_unified_objective_binding() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY,
                objective_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_runs(id, objective_id)
             VALUES ('task-run', 'task-objective-opaque-uuid')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut ctx = ExecCtx::new(std::path::PathBuf::from("/tmp"), Some(pool.clone()));
        ctx.task_id = Some("task-run".into());

        let identity = durable_objective_identity(&pool, &ctx).await.unwrap();
        assert_eq!(identity, ("task-objective-opaque-uuid".into(), None));
    }
}
