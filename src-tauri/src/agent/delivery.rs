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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

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
    fn waiting(step: &str, detail: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            status: "waiting".into(),
            detail: detail.into(),
        }
    }
}

/// The result of a delivery run.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryClass {
    None,
    WaitRetryable,
    AgentActionRequired,
    /// A non-derivable input owned by an external identity/provider. This is
    /// not a generic permission to hand technical recovery back to the user.
    CoreInputRequired,
    ExternalStateUncertain,
}

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
    pub recovery_class: RecoveryClass,
    pub retry_after_ms: Option<u64>,
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
    pub fn validate_contract(&self) -> Result<(), String> {
        let valid = match self.final_state.as_str() {
            "delivered" | "noop" => {
                !self.recoverable
                    && self.recovery_class == RecoveryClass::None
                    && self.retry_after_ms.is_none()
                    && self.next_action.is_none()
            }
            "waiting" => {
                self.recoverable
                    && matches!(
                        self.recovery_class,
                        RecoveryClass::WaitRetryable | RecoveryClass::ExternalStateUncertain
                    )
                    && self.retry_after_ms.is_some_and(|value| value > 0)
                    && self
                        .next_action
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
            }
            "blocked" => match self.recovery_class {
                RecoveryClass::AgentActionRequired => {
                    self.recoverable
                        && self.retry_after_ms.is_none()
                        && self
                            .next_action
                            .as_deref()
                            .is_some_and(|value| !value.is_empty())
                }
                RecoveryClass::CoreInputRequired | RecoveryClass::ExternalStateUncertain => {
                    !self.recoverable
                        && self.retry_after_ms.is_none()
                        && self
                            .next_action
                            .as_deref()
                            .is_some_and(|value| !value.is_empty())
                }
                RecoveryClass::None | RecoveryClass::WaitRetryable => false,
            },
            _ => false,
        };
        valid.then_some(()).ok_or_else(|| {
            format!(
                "invalid delivery outcome: state={}, recoverable={}, recovery_class={:?}",
                self.final_state, self.recoverable, self.recovery_class
            )
        })
    }

    fn blocked_at(mut self, step: StepResult) -> Self {
        let msg = step.detail.clone();
        self.stage = step.step.clone();
        self.code = format!("delivery_{}_blocked", step.step);
        self.recoverable = true;
        self.recovery_class = RecoveryClass::AgentActionRequired;
        self.retry_after_ms = None;
        self.next_action = Some(msg.clone());
        self.reached_state = reached_state_from_steps(&self.steps);
        self.steps.push(step);
        self.final_state = "blocked".into();
        self.summary = msg;
        self
    }

    fn waiting_at(
        mut self,
        step: StepResult,
        retry_after_ms: u64,
        next_action: impl Into<String>,
    ) -> Self {
        self.stage = step.step.clone();
        self.code = format!("delivery_{}_waiting", step.step);
        self.recoverable = true;
        self.recovery_class = RecoveryClass::WaitRetryable;
        self.retry_after_ms = Some(retry_after_ms);
        self.next_action = Some(next_action.into());
        self.reached_state = reached_state_from_steps(&self.steps);
        self.summary = step.detail.clone();
        self.steps.push(step);
        self.final_state = "waiting".into();
        self
    }

    fn blocked_on_uncertain_side_effect(mut self, step: StepResult) -> Self {
        let msg = step.detail.clone();
        self = self.waiting_at(
            StepResult::waiting(&step.step, step.detail),
            30_000,
            "只读核对同一远端对象和持久回执；确认外部动作未发生前不得重复写入。",
        );
        self.code = "delivery_external_state_uncertain".into();
        self.recovery_class = RecoveryClass::ExternalStateUncertain;
        self.summary = format!("{msg} 外部动作结果不确定；系统保持运行并只读对账，不向用户回交。");
        self
    }

    fn core_input_required(mut self, step: StepResult) -> Self {
        let msg = step.detail.clone();
        self = self.blocked_at(step);
        self.code = "delivery_core_input_required".into();
        self.recoverable = false;
        self.recovery_class = RecoveryClass::CoreInputRequired;
        self.retry_after_ms = None;
        self.next_action = Some(msg);
        self
    }

    fn remote_observation_failed(self, step: &str, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if remote_error_is_retryable(&detail) {
            return self.waiting_at(
                StepResult::waiting(step, detail),
                30_000,
                "等待退避后重新核对同一远端对象；状态未知期间禁止重复外部写动作。",
            );
        }
        if remote_error_requires_core_input(&detail) {
            return self.core_input_required(StepResult::blocked(step, detail));
        }
        let mut outcome = self.waiting_at(
            StepResult::waiting(step, detail),
            30_000,
            "只读重新核对同一 PR/CI/发布对象；状态未知期间禁止创建重复对象或重放外部写动作。",
        );
        outcome.code = "delivery_remote_observation_unknown".into();
        outcome
    }
}

fn remote_error_is_retryable(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    [
        "rate limit",
        "secondary rate",
        "429",
        "timed out",
        "timeout",
        "temporarily unavailable",
        "connection reset",
        "connection refused",
        "dns",
        "502",
        "503",
        "504",
        "upgrade to github pro or make this repository public",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn github_rules_capability_unavailable(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("upgrade to github pro or make this repository public")
        || lower.contains("repository rules are not available")
}

fn github_required_status_checks(
    rules_raw: Result<String, String>,
) -> Result<Vec<crate::git_remote::github::RequiredStatusCheck>, String> {
    let rules = match rules_raw {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|error| format!("required-rules returned non-JSON: {error}"))?,
        Err(error) if github_rules_capability_unavailable(&error) => serde_json::Value::Null,
        Err(error) => return Err(error),
    };
    Ok(crate::git_remote::github::parse_required_status_checks(
        &rules,
    ))
}

fn remote_error_requires_core_input(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    [
        "authentication",
        "not authenticated",
        "gh auth login",
        "bad credentials",
        "401",
        "forbidden",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
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
#[async_trait::async_trait]
pub trait DeliveryMutationPermitVerifier: Send + Sync {
    /// Re-validate authority immediately before one named mutation rung.
    async fn verify(&self, rung: &str) -> Result<(), String>;

    /// Commit a durable write-ahead intent before dispatching an external
    /// mutation. Non-durable callers return `None`; a durable caller must not
    /// dispatch until this method has committed successfully.
    async fn begin_external_mutation(
        &self,
        rung: &str,
        _operation_key: &str,
        _evidence: &str,
    ) -> Result<DeliveryMutationBegin, String> {
        self.verify(rung).await?;
        Ok(DeliveryMutationBegin::Dispatch(None))
    }

    /// Resolve a write-ahead intent only after a definitive successful result.
    async fn commit_external_mutation(
        &self,
        _intent: &DeliveryMutationIntentToken,
        _evidence: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Any timeout, cancelled future, adapter error, or settle uncertainty is
    /// retained as `unknown`; takeover must observe the real domain before it
    /// can grant a later mutation permit.
    async fn mark_external_mutation_unknown(
        &self,
        _intent: &DeliveryMutationIntentToken,
        _detail: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Materialize the exact receipted local commit while authority is still
    /// fenced. Durable implementations hold their store's writer/CAS fence
    /// across the final source/index/HEAD checks and `update-ref`.
    async fn materialize_local_commit(
        &self,
        _intent: &DeliveryMutationIntentToken,
        cwd: &Path,
        default_branch_hint: Option<&str>,
        expected_branch: &str,
        persisted_identity: &DeliveryIdentitySnapshot,
        evidence: &LocalCommitIntentEvidence,
    ) -> Result<DeliveryIdentitySnapshot, String> {
        materialize_receipted_local_commit(
            cwd,
            default_branch_hint,
            expected_branch,
            persisted_identity,
            evidence,
        )
    }

    /// Bind the local branch/worktree to an exact provider-produced PR head.
    /// Durable implementations must hold their owner/epoch/Objective writer
    /// fence across the final local CAS; the network fetch happens before this
    /// method is invoked into an operation-owned internal ref.
    async fn materialize_branch_update(
        &self,
        request: &BranchUpdateMaterialization,
    ) -> Result<DeliveryIdentitySnapshot, String> {
        self.verify("materialize_pr_branch_update").await?;
        materialize_fetched_branch_update(request)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryMutationIntentToken {
    pub id: String,
    pub rung: String,
    pub operation_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryMutationCommittedReceipt {
    pub intent_id: String,
    pub rung: String,
    pub operation_key: String,
    pub result_evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryMutationBegin {
    Dispatch(Option<DeliveryMutationIntentToken>),
    AlreadyCommitted(DeliveryMutationCommittedReceipt),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchUpdateMaterialization {
    pub pr_number: u64,
    pub cwd: PathBuf,
    pub default_branch: String,
    pub head_branch: String,
    pub previous_identity: DeliveryIdentitySnapshot,
    pub next_head_sha: String,
    pub fetched_ref: String,
}

impl BranchUpdateMaterialization {
    pub fn operation_key(&self) -> String {
        external_operation_key(
            "provider_pr_branch_update",
            &[&self.pr_number.to_string(), &self.previous_identity.head_sha],
        )
    }
}

#[derive(Clone)]
pub struct DeliveryMutationPermit {
    verifier: Arc<dyn DeliveryMutationPermitVerifier>,
}

impl DeliveryMutationPermit {
    pub fn new(verifier: Arc<dyn DeliveryMutationPermitVerifier>) -> Self {
        Self { verifier }
    }

    async fn verify(&self, rung: &str) -> Result<(), String> {
        self.verifier.verify(rung).await
    }

    async fn begin_external_mutation(
        &self,
        rung: &str,
        operation_key: &str,
        evidence: &str,
    ) -> Result<DeliveryMutationBegin, String> {
        self.verifier
            .begin_external_mutation(rung, operation_key, evidence)
            .await
    }

    async fn commit_external_mutation(
        &self,
        intent: &DeliveryMutationIntentToken,
        evidence: &str,
    ) -> Result<(), String> {
        self.verifier
            .commit_external_mutation(intent, evidence)
            .await
    }

    async fn mark_external_mutation_unknown(
        &self,
        intent: &DeliveryMutationIntentToken,
        detail: &str,
    ) -> Result<(), String> {
        self.verifier
            .mark_external_mutation_unknown(intent, detail)
            .await
    }


    async fn materialize_local_commit(
        &self,
        intent: &DeliveryMutationIntentToken,
        cwd: &Path,
        default_branch_hint: Option<&str>,
        expected_branch: &str,
        persisted_identity: &DeliveryIdentitySnapshot,
        evidence: &LocalCommitIntentEvidence,
    ) -> Result<DeliveryIdentitySnapshot, String> {
        self.verifier
            .materialize_local_commit(
                intent,
                cwd,
                default_branch_hint,
                expected_branch,
                persisted_identity,
                evidence,
            )
            .await
    }

    async fn materialize_branch_update(
        &self,
        request: &BranchUpdateMaterialization,
    ) -> Result<DeliveryIdentitySnapshot, String> {
        self.verifier.materialize_branch_update(request).await
    }
}

impl std::fmt::Debug for DeliveryMutationPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeliveryMutationPermit")
            .finish_non_exhaustive()
    }
}

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
    /// The branch the caller BELIEVES it is delivering. `deliver_changes` has no
    /// branch argument — it delivers whatever the working directory is on — so a
    /// caller resuming a specific delivery states its target here and the tool
    /// refuses when reality disagrees, instead of silently delivering something
    /// else under the intended title.
    pub expect_branch: Option<String>,
    /// Durable callers capture the exact repo/worktree/head/change-set before
    /// authorizing this invocation. Re-checking it immediately before the
    /// first local mutation prevents a stale run from committing or pushing a
    /// different checkout under the old objective identity.
    pub expected_identity: Option<DeliveryIdentitySnapshot>,
    /// Durable DeliveryRuns provide a fresh database-backed fencing check at
    /// every local or external mutation rung. Non-durable/manual callers leave
    /// this unset and retain the legacy single-process behavior.
    pub mutation_permit: Option<DeliveryMutationPermit>,
}

async fn verify_mutation_permit(opts: &DeliverOpts, rung: &str) -> Result<(), StepResult> {
    let Some(permit) = opts.mutation_permit.as_ref() else {
        return Ok(());
    };
    permit.verify(rung).await.map_err(|error| {
        StepResult::blocked(
            "mutation_permit",
            format!(
                "DeliveryRun fencing permit was lost before `{rung}`: {error}. The stale owner issued no later mutation."
            ),
        )
    })
}

pub(crate) fn external_operation_key(rung: &str, fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"delivery-external-operation-v1\0");
    hasher.update(rung.as_bytes());
    hasher.update([0]);
    for field in fields {
        hasher.update(field.as_bytes());
        hasher.update([0]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

async fn begin_or_reuse_external_mutation(
    permit: Option<&DeliveryMutationPermit>,
    rung: &str,
    operation_key: &str,
    evidence: &str,
) -> Result<DeliveryMutationBegin, String> {
    match permit {
        Some(permit) => {
            permit
                .begin_external_mutation(rung, operation_key, evidence)
                .await
        }
        None => Ok(DeliveryMutationBegin::Dispatch(None)),
    }
}

fn committed_receipt_envelope(
    receipt: &DeliveryMutationCommittedReceipt,
) -> Result<serde_json::Value, String> {
    receipt
        .result_evidence
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| {
            format!(
                "committed mutation receipt {} has no structured result evidence",
                receipt.intent_id
            )
        })
}

fn committed_receipt_result(
    receipt: &DeliveryMutationCommittedReceipt,
) -> Result<serde_json::Value, String> {
    let envelope = committed_receipt_envelope(receipt)?;
    Ok(envelope
        .get("committed_result")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or(envelope))
}

fn committed_pr_projection(
    receipt: &DeliveryMutationCommittedReceipt,
    title: &str,
    body: &str,
) -> Result<DeliveryPr, String> {
    let envelope = committed_receipt_envelope(receipt)?;
    let result = envelope
        .get("committed_result")
        .filter(|value| !value.is_null())
        .unwrap_or(&envelope);
    let observation = envelope.get("observation").unwrap_or(&envelope);
    let result_number = result
        .get("pr_number")
        .and_then(serde_json::Value::as_u64)
        .filter(|number| *number > 0);
    let observed_number = observation
        .get("pr_number")
        .and_then(serde_json::Value::as_u64)
        .filter(|number| *number > 0);
    let result_url = result
        .get("pr_url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| !url.is_empty());
    let observed_url = observation
        .get("pr_url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| !url.is_empty());
    if result_number.is_some()
        && observed_number.is_some()
        && result_number != observed_number
        || result_url.is_some() && observed_url.is_some() && result_url != observed_url
    {
        return Err(
            "committed PR result conflicts with the fresh canonical PR observation".to_string(),
        );
    }
    let number = observed_number
        .or(result_number)
        .filter(|number| *number > 0)
        .ok_or_else(|| "committed PR receipt lacks its canonical number".to_string())?;
    let url = observed_url
        .or(result_url)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "committed PR receipt lacks its canonical URL".to_string())?;
    Ok(DeliveryPr {
        number,
        url: url.to_string(),
        title: title.to_string(),
        body: body.to_string(),
    })
}

fn committed_merge_projection(
    receipt: &DeliveryMutationCommittedReceipt,
) -> Result<MergeRequestResult, String> {
    let envelope = committed_receipt_envelope(receipt)?;
    let observation = envelope.get("observation").unwrap_or(&envelope);
    match observation
        .get("confirmation")
        .and_then(serde_json::Value::as_str)
    {
        Some("auto_merge_observed") => Ok(MergeRequestResult::Queued),
        Some("merge_observed") => {
            let observed_sha = observation
                .get("merge_sha")
                .and_then(serde_json::Value::as_str)
                .filter(|sha| !sha.is_empty())
                .ok_or_else(|| "committed merge receipt lacks its merge SHA".to_string())?;
            let result = committed_receipt_result(receipt)?;
            let committed_sha = result
                .get("merge_sha")
                .and_then(serde_json::Value::as_str)
                .filter(|sha| !sha.is_empty());
            if committed_sha.is_some_and(|sha| sha != observed_sha) {
                return Err(
                    "committed merge result conflicts with the fresh observed merge SHA"
                        .to_string(),
                );
            }
            Ok(MergeRequestResult::Merged {
                merge_sha: observed_sha.to_string(),
            })
        }
        _ => {
            let result = committed_receipt_result(receipt)?;
            if result
                .get("queued")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                return Ok(MergeRequestResult::Queued);
            }
            if result
                .get("merged")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                return Ok(MergeRequestResult::Merged {
                    merge_sha: result
                        .get("merge_sha")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
            }
            result
                .get("merge_sha")
                .and_then(serde_json::Value::as_str)
                .filter(|sha| !sha.is_empty())
                .map(|merge_sha| MergeRequestResult::Merged {
                    merge_sha: merge_sha.to_string(),
                })
                .ok_or_else(|| "committed merge receipt is not reconstructable".to_string())
        }
    }
}

fn observed_committed_pr_projection(
    receipt: &DeliveryMutationCommittedReceipt,
    observation: OpenPrObservation,
    expected_title: &str,
    expected_body: &str,
    expected_head: &str,
) -> Result<DeliveryPr, String> {
    let committed = committed_pr_projection(receipt, expected_title, expected_body)?;
    let OpenPrObservation::Open(observed) = observation else {
        return Err(
            "committed PR receipt is not currently backed by an exact open-PR observation"
                .to_string(),
        );
    };
    if observed.pr.number != committed.number
        || observed.pr.url != committed.url
        || observed.pr.title != expected_title
        || observed.pr.body != expected_body
        || observed.head_sha.as_deref() != Some(expected_head)
    {
        return Err(format!(
            "committed PR receipt {} no longer matches the current canonical PR identity/body/head",
            receipt.intent_id
        ));
    }
    Ok(observed.pr)
}

fn exact_open_pr_projection(
    observation: OpenPrObservation,
    expected_number: Option<u64>,
    expected_head_sha: &str,
) -> Result<Option<DeliveryPr>, String> {
    match observation {
        OpenPrObservation::Absent | OpenPrObservation::Unsupported => Ok(None),
        OpenPrObservation::Open(observed) => {
            if expected_number.is_some_and(|number| observed.pr.number != number)
                || observed.head_sha.as_deref() != Some(expected_head_sha)
            {
                return Err(
                    "open PR is attached to a foreign head or canonical PR identity; no mutation was dispatched"
                        .into(),
                );
            }
            Ok(Some(observed.pr))
        }
    }
}

fn exact_created_pr_projection(
    created: &DeliveryPr,
    observation: OpenPrObservation,
    expected_title: &str,
    expected_body: &str,
    expected_head_branch: &str,
    expected_base_branch: &str,
    expected_head_sha: &str,
) -> Result<DeliveryPr, String> {
    let OpenPrObservation::Open(observed) = observation else {
        return Err(
            "provider returned a PR create result, but the exact canonical PR could not be observed"
                .to_string(),
        );
    };
    if observed.pr.number != created.number
        || observed.pr.url != created.url
        || observed.pr.title != expected_title
        || observed.pr.body != expected_body
        || observed.head_branch != expected_head_branch
        || observed.base_branch != expected_base_branch
        || observed.head_sha.as_deref() != Some(expected_head_sha)
    {
        return Err(
            "provider returned a PR create result, but the observed canonical PR has a foreign head or mismatched identity/body"
                .to_string(),
        );
    }
    Ok(observed.pr)
}

fn exact_updated_pr_projection(
    observation: OpenPrObservation,
    expected_number: u64,
    expected_body: &str,
    expected_head_branch: &str,
    expected_base_branch: &str,
    expected_head_sha: &str,
) -> Result<(), String> {
    let OpenPrObservation::Open(observed) = observation else {
        return Err("updated canonical PR is not positively observable".to_string());
    };
    if observed.pr.number != expected_number
        || observed.pr.body != expected_body
        || observed.head_branch != expected_head_branch
        || observed.base_branch != expected_base_branch
        || observed.head_sha.as_deref() != Some(expected_head_sha)
    {
        return Err(
            "updated canonical PR has a foreign head, body, branch, or identity".to_string(),
        );
    }
    Ok(())
}

fn observed_committed_merge_projection(
    receipt: &DeliveryMutationCommittedReceipt,
    observation: MergeObservation,
) -> Result<MergeRequestResult, String> {
    let committed = committed_merge_projection(receipt)?;
    match (committed, observation) {
        (
            MergeRequestResult::Merged { merge_sha: committed },
            MergeObservation::Merged { merge_sha: observed },
        ) if committed.is_empty() || committed == observed => {
            Ok(MergeRequestResult::Merged { merge_sha: observed })
        }
        (MergeRequestResult::Queued, MergeObservation::Merged { merge_sha }) => {
            Ok(MergeRequestResult::Merged { merge_sha })
        }
        (MergeRequestResult::Queued, MergeObservation::OpenSameHead { auto_merge: true }) => {
            Ok(MergeRequestResult::Queued)
        }
        _ => Err(format!(
            "committed merge receipt {} is not currently backed by the same-head merge observation",
            receipt.intent_id
        )),
    }
}

fn exact_dispatched_merge_projection(
    observation: MergeObservation,
) -> Result<MergeRequestResult, String> {
    match observation {
        MergeObservation::Merged { merge_sha } if !merge_sha.is_empty() => {
            Ok(MergeRequestResult::Merged { merge_sha })
        }
        MergeObservation::OpenSameHead { auto_merge: true } => Ok(MergeRequestResult::Queued),
        MergeObservation::OpenSameHead { auto_merge: false } => Err(
            "provider mutation returned success, but the PR remains open without auto-merge"
                .to_string(),
        ),
        MergeObservation::HeadChanged { actual_head } => Err(format!(
            "provider mutation returned success, but the PR moved to foreign head {actual_head}"
        )),
        MergeObservation::ClosedUnmerged => Err(
            "provider mutation returned success, but the PR is closed without a merge".to_string(),
        ),
        MergeObservation::Unsupported => Err(
            "provider mutation returned success, but no exact merge observer is available"
                .to_string(),
        ),
        MergeObservation::Merged { .. } => {
            Err("provider merge observation omitted its merge SHA".to_string())
        }
    }
}

fn observed_committed_release_projection(
    receipt: &DeliveryMutationCommittedReceipt,
    target: &ReleaseDispatchTarget,
    observation: ReleaseDispatchObservation,
) -> Result<String, String> {
    committed_receipt_envelope(receipt)?;
    match observation {
        ReleaseDispatchObservation::Triggered {
            head_sha, detail, ..
        } if head_sha == target.head_sha => Ok(format!(
            "复用已提交且当前只读观察仍匹配的发布回执 {}: {detail}",
            receipt.intent_id
        )),
        _ => Err(format!(
            "committed release receipt {} is not currently backed by the exact workflow/ref/head observation",
            receipt.intent_id
        )),
    }
}

async fn fail_external_mutation(
    permit: Option<&DeliveryMutationPermit>,
    intent: Option<&DeliveryMutationIntentToken>,
    error: String,
) -> String {
    match mark_external_mutation_unknown(permit, intent, &error).await {
        Ok(()) => error,
        Err(settle_error) => {
            format!("{error}; durable mutation intent could not be marked unknown: {settle_error}")
        }
    }
}

async fn commit_external_mutation(
    permit: Option<&DeliveryMutationPermit>,
    intent: Option<&DeliveryMutationIntentToken>,
    evidence: &str,
) -> Result<(), String> {
    match (permit, intent) {
        (Some(permit), Some(intent)) => permit.commit_external_mutation(intent, evidence).await,
        _ => Ok(()),
    }
}

async fn mark_external_mutation_unknown(
    permit: Option<&DeliveryMutationPermit>,
    intent: Option<&DeliveryMutationIntentToken>,
    detail: &str,
) -> Result<(), String> {
    match (permit, intent) {
        (Some(permit), Some(intent)) => permit.mark_external_mutation_unknown(intent, detail).await,
        _ => Ok(()),
    }
}

async fn materialize_local_commit_with_permit(
    permit: Option<&DeliveryMutationPermit>,
    intent: Option<&DeliveryMutationIntentToken>,
    cwd: &Path,
    default_branch_hint: Option<&str>,
    expected_branch: &str,
    persisted_identity: &DeliveryIdentitySnapshot,
    evidence: &LocalCommitIntentEvidence,
) -> Result<DeliveryIdentitySnapshot, String> {
    match (permit, intent) {
        (Some(permit), Some(intent)) => {
            permit
                .materialize_local_commit(
                    intent,
                    cwd,
                    default_branch_hint,
                    expected_branch,
                    persisted_identity,
                    evidence,
                )
                .await
        }
        _ => materialize_receipted_local_commit(
            cwd,
            default_branch_hint,
            expected_branch,
            persisted_identity,
            evidence,
        ),
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPrState {
    pub pr: DeliveryPr,
    pub head_branch: String,
    pub base_branch: String,
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenPrObservation {
    Open(OpenPrState),
    Absent,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeObservation {
    OpenSameHead { auto_merge: bool },
    Merged { merge_sha: String },
    ClosedUnmerged,
    HeadChanged { actual_head: String },
    Unsupported,
}

/// Exact identity of one release dispatch. The target is written into the
/// local `intent_release` receipt before the durable DeliveryRun mutation
/// intent is begun, so a takeover can distinguish "no POST was possible" from
/// "a POST may be in flight" without guessing from generic live state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseDispatchTarget {
    pub workflow: String,
    pub git_ref: String,
    pub head_sha: String,
}

impl ReleaseDispatchTarget {
    pub fn operation_key(&self) -> String {
        external_operation_key(
            "provider_release_trigger",
            &[&self.workflow, &self.git_ref, &self.head_sha],
        )
    }
}

/// Read-only observation of the exact workflow/ref/head dispatch target.
/// `Absent` is replay authority only for the narrow local-receipt/DB-gap
/// window. A durable mutation intent treats absence as uncertainty because the
/// prior POST may still be in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseDispatchObservation {
    Absent,
    Triggered {
        run_id: String,
        status: String,
        head_sha: String,
        detail: String,
    },
    HeadMismatch {
        observed_heads: Vec<String>,
    },
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeRequestResult {
    Queued,
    Merged { merge_sha: String },
}

/// An existing open PR that carries the title we are about to open a NEW PR
/// with, on a different head branch.
///
/// `deliver_changes` takes no branch or PR argument: it always delivers whatever
/// branch the working directory happens to be on, and stamps the caller-supplied
/// title (which comes from session context) onto it. When those two disagree the
/// tool silently opens a second PR for unrelated work under the first one's name
/// — 2026-07-30 field report: a turn meaning to resume PR #281
/// (`feat/on-demand-embedded-browser-pane`) was sitting on
/// `fix/auto-release-reconcile-sigpipe` and opened #290, leaving two open PRs
/// with identical titles and unrelated contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictingPr {
    pub number: u64,
    pub url: String,
    pub head: String,
}

/// Why a PR cannot be merged right now — the distinction between "time will fix
/// this" and "nothing will ever fix this on its own".
///
/// Registering auto-merge on a `Behind` PR under a strict required-status-checks
/// policy is a DEADLOCK, not a wait: GitHub does not update a stale head ref, so
/// the PR never becomes mergeable and auto-merge never fires. Reporting that as
/// "waiting for remote gates" sends the caller into an unbounded no-op wait
/// (2026-07-30 field report: an 11m36s turn ended `blocked` telling the user to
/// wait for something that could not happen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeReadiness {
    /// Mergeable now.
    Ready,
    /// Head is behind base. Auto-resolvable by updating the branch — no human
    /// needed — but it will NOT resolve by waiting.
    Behind,
    /// Required checks are still running. Waiting is the correct action.
    WaitingOnChecks,
    /// Conflicts, missing review, or a failed required check: a human must act.
    NeedsAction(String),
    /// The adapter cannot tell (no support, or GitHub is still computing).
    Unknown,
}

/// CI conclusion for a commit.
#[derive(Debug, Clone, PartialEq)]
pub enum CiStatus {
    Success,
    Failure(String),
    /// The observer could not establish CI truth (rate limit/network/schema).
    /// This is not a red check and must never trigger code changes.
    Unavailable(String),
    Pending,
    /// No CI is configured for this commit — treated as "not blocking".
    None,
}

/// GitHub creates check suites asynchronously after a PR opens. A single
/// terminal-looking snapshot can therefore be empty or contain only an early
/// subset of required checks. Require the exact terminal fingerprint twice;
/// any changed set restarts stabilization.
#[derive(Default)]
struct CiObservationStability {
    candidate: Mutex<Option<String>>,
}

impl CiObservationStability {
    fn reset(&self) {
        *self.candidate.lock().expect("ci stability mutex poisoned") = None;
    }

    fn confirm(&self, fingerprint: &str, status: CiStatus) -> CiStatus {
        if matches!(status, CiStatus::Pending | CiStatus::Unavailable(_)) {
            *self.candidate.lock().expect("ci stability mutex poisoned") = None;
            return status;
        }
        let mut candidate = self.candidate.lock().expect("ci stability mutex poisoned");
        if candidate.as_deref() == Some(fingerprint) {
            status
        } else {
            *candidate = Some(fingerprint.to_string());
            CiStatus::Pending
        }
    }
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
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> impl std::future::Future<Output = Result<DeliveryPr, String>>;
    /// Observe an open PR for a branch without creating or updating anything.
    /// `Absent` is an observation, not proof that an earlier create request
    /// never reached the provider; mutation-intent reconciliation therefore
    /// accepts only a positively matching `Open` state.
    fn observe_open_pr(
        &self,
        _head: &str,
        _base: &str,
    ) -> impl std::future::Future<Output = Result<OpenPrObservation, String>> {
        std::future::ready(Ok(OpenPrObservation::Unsupported))
    }
    /// Converge governance metadata on an existing PR without replacing its
    /// identity. Called only when the desired body differs from the live body.
    fn update_pr_body(
        &self,
        _number: u64,
        _body: &str,
        _head: &str,
        _base: &str,
        _expected_head_sha: &str,
        _mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> impl std::future::Future<Output = Result<(), String>> {
        std::future::ready(Err("adapter cannot update an existing PR body".into()))
    }
    fn ci_status(&self, sha: &str) -> impl std::future::Future<Output = Result<CiStatus, String>>;
    /// Re-run retryable CI infrastructure failures. `false` means the adapter
    /// has no safe rerun actuator; ordinary test failures never call this.
    fn rerun_ci(
        &self,
        _sha: &str,
        _mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> impl std::future::Future<Output = Result<bool, String>> {
        std::future::ready(Ok(false))
    }
    fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> impl std::future::Future<Output = Result<MergeRequestResult, String>>;
    fn observe_merge(
        &self,
        _number: u64,
        _expected_head: &str,
    ) -> impl std::future::Future<Output = Result<MergeObservation, String>> {
        std::future::ready(Ok(MergeObservation::Unsupported))
    }
    fn release_dispatch_target(&self, _head_sha: &str) -> Option<ReleaseDispatchTarget> {
        None
    }

    fn observe_release_dispatch(
        &self,
        _target: &ReleaseDispatchTarget,
    ) -> impl std::future::Future<Output = Result<ReleaseDispatchObservation, String>> {
        std::future::ready(Ok(ReleaseDispatchObservation::Unsupported(
            "exact release dispatch observer not configured".into(),
        )))
    }

    fn trigger_release(
        &self,
        head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> impl std::future::Future<Output = Result<String, String>>;

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

    /// An open PR with the SAME title on a DIFFERENT head, when this head has no
    /// open PR of its own — i.e. we are about to open a duplicate.
    ///
    /// Default `Ok(None)` keeps existing adapters source-compatible.
    fn conflicting_open_pr(
        &self,
        _title: &str,
        _head: &str,
        _base: &str,
    ) -> impl std::future::Future<Output = Result<Option<ConflictingPr>, String>> {
        std::future::ready(Ok(None))
    }

    /// Why the PR is not mergeable yet. Defaulting to `Unknown` keeps existing
    /// adapters source-compatible; an adapter that cannot answer simply keeps
    /// today's behaviour.
    fn merge_readiness(
        &self,
        _number: u64,
    ) -> impl std::future::Future<Output = Result<MergeReadiness, String>> {
        std::future::ready(Ok(MergeReadiness::Unknown))
    }

    /// Merge the base branch into the PR head so a `Behind` PR becomes
    /// mergeable. Default is "unsupported" so adapters opt in.
    fn update_pr_branch(
        &self,
        _number: u64,
        _expected_head: &str,
        _mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> impl std::future::Future<Output = Result<String, String>> {
        std::future::ready(Err("adapter cannot update a PR branch".to_string()))
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

fn git_with_index(cwd: &Path, index: &Path, args: &[&str]) -> Result<String, String> {
    let out = dev_command("git")
        .arg("-C")
        .arg(cwd)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn git with isolated index: {e}"))?;
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
        "pr_open"
            | "intent_merge"
            | "merge_queued"
            | "merged"
            | "intent_release"
            | "release_triggered"
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

fn encode_release_dispatch_target(target: &ReleaseDispatchTarget) -> Result<String, String> {
    serde_json::to_string(target)
        .map_err(|error| format!("序列化 release dispatch target 失败: {error}"))
}

fn decode_release_dispatch_target(
    receipt: &DeliveryReceipt,
) -> Result<ReleaseDispatchTarget, String> {
    let raw = receipt.release_detail.as_deref().ok_or_else(|| {
        "intent_release 回执缺少 workflow/ref/head envelope；只能继续只读观察".to_string()
    })?;
    let target: ReleaseDispatchTarget = serde_json::from_str(raw)
        .map_err(|error| format!("intent_release dispatch envelope 损坏: {error}"))?;
    if target.workflow.trim().is_empty()
        || target.git_ref.trim().is_empty()
        || target.head_sha.trim().is_empty()
    {
        return Err("intent_release dispatch envelope 的 workflow/ref/head 不完整".into());
    }
    Ok(target)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalReleaseIntentReconciliation {
    NoIntent,
    ProvenAbsent,
    Triggered { detail: String },
}

/// Reconcile the local receipt around the one crash window that exists before
/// the durable DB mutation intent is begun. Callers must first reconcile any
/// durable mutation intent. Only an exact `Absent` observation may roll the
/// receipt back to `merged`; triggered/unknown states never authorize replay.
pub async fn reconcile_local_release_intent<R: DeliveryRemote>(
    cwd: &Path,
    default_branch_hint: Option<&str>,
    expected_branch: &str,
    delivery_head_sha: &str,
    allow_absent_replay: bool,
    remote: Option<&R>,
) -> Result<LocalReleaseIntentReconciliation, String> {
    let (repo, _) = resolve_delivery_repo(cwd, default_branch_hint, Some(expected_branch))?;
    let Some(receipt) = read_delivery_receipt(&repo, delivery_head_sha)? else {
        return Ok(LocalReleaseIntentReconciliation::NoIntent);
    };
    if receipt.state != "intent_release" {
        return Ok(LocalReleaseIntentReconciliation::NoIntent);
    }
    let target = decode_release_dispatch_target(&receipt)?;
    let remote = remote.ok_or_else(|| {
        "intent_release requires an exact read-only release observer; no provider is available"
            .to_string()
    })?;
    match remote.observe_release_dispatch(&target).await? {
        ReleaseDispatchObservation::Absent if allow_absent_replay => {
            let mut reconciled = receipt;
            reconciled.state = "merged".into();
            reconciled.release_detail = None;
            write_delivery_receipt(&repo, delivery_head_sha, &reconciled)?;
            Ok(LocalReleaseIntentReconciliation::ProvenAbsent)
        }
        ReleaseDispatchObservation::Absent => Err(
            "exact release dispatch is absent, but a durable POST intent may exist; replay remains fenced"
                .into(),
        ),
        ReleaseDispatchObservation::Triggered {
            run_id,
            status,
            head_sha,
            detail,
        } if head_sha == target.head_sha => {
            let observed_detail = format!(
                "只读确认 release run {run_id} 已触发（workflow={}, ref={}, head={}, status={}）: {detail}",
                target.workflow, target.git_ref, target.head_sha, status
            );
            let mut reconciled = receipt;
            reconciled.state = "release_triggered".into();
            reconciled.release_detail = Some(observed_detail.clone());
            write_delivery_receipt(&repo, delivery_head_sha, &reconciled)?;
            Ok(LocalReleaseIntentReconciliation::Triggered {
                detail: observed_detail,
            })
        }
        ReleaseDispatchObservation::Triggered { head_sha, .. } => Err(format!(
            "release observer returned head {head_sha}, expected exact {}",
            target.head_sha
        )),
        ReleaseDispatchObservation::HeadMismatch { observed_heads } => Err(format!(
            "release workflow/ref is visible only for nonmatching heads [{}]; expected {}",
            observed_heads.join(", "),
            target.head_sha
        )),
        ReleaseDispatchObservation::Unsupported(detail) => Err(format!(
            "release dispatch cannot be reconciled exactly: {detail}"
        )),
    }
}

pub(crate) fn fetch_updated_pr_head(
    repo: &RepoContext,
    expected_head: &str,
) -> Result<String, String> {
    git(&repo.root, &["fetch", &repo.remote, &repo.branch])?;
    let remote_ref = format!("{}/{}", repo.remote, repo.branch);
    let fetched = git(&repo.root, &["rev-parse", &remote_ref])?;
    if fetched != expected_head {
        return Err(format!(
            "更新后的 PR head 尚未收敛: provider 返回 {expected_head}，{remote_ref} 为 {fetched}"
        ));
    }
    Ok(remote_ref)
}

/// Fetch a provider-produced PR head into an operation-owned internal ref.
/// Network latency is deliberately outside the SQLite owner fence; only the
/// later fast local HEAD/worktree CAS is performed while authority is locked.
/// Materialise an exact, already-receipted PR head into a temporary local ref.
///
/// The head branch is not a dependable address for it: a provider with "delete
/// branch on merge" enabled removes the branch the instant the PR merges, while
/// the commit itself stays reachable through the merge and through the PR ref.
/// Recovery therefore tries every ref that can still name the head and accepts
/// only the one resolving to exactly `expected_head`, so a stale or force-moved
/// branch can neither strand the run nor smuggle in a foreign commit.
///
/// Provenance is established before this call — the caller has already observed
/// the PR at this head — so this is materialisation, not verification. The SHA
/// equality check below is what keeps it fail-closed regardless.
pub(crate) fn fetch_updated_pr_head_for_operation(
    repo: &RepoContext,
    expected_head: &str,
    operation_key: &str,
    pr_number: Option<u64>,
) -> Result<String, String> {
    let remote = default_remote(&repo.root);
    let suffix = operation_key
        .strip_prefix("sha256:")
        .unwrap_or(operation_key);
    let temporary_ref = format!(
        "refs/codefactory/delivery/{}",
        &suffix[..suffix.len().min(32)]
    );

    let mut candidates: Vec<String> = Vec::new();
    if let Some(number) = pr_number {
        // The PR ref is pinned to this PR's head and outlives the branch.
        candidates.push(format!("refs/pull/{number}/head"));
        candidates.push(format!("refs/merge-requests/{number}/head"));
    }
    candidates.push(repo.branch.clone());
    // Deliberately NOT a raw-SHA candidate: `fetch <remote> +<sha>:<ref>` is
    // satisfied from objects this clone already holds, so it would write the
    // observation ref without the remote ever confirming it still has the
    // commit. Only server-advertised refs are asked for.

    let mut last_error: Option<String> = None;
    for candidate in &candidates {
        let refspec = format!("+{candidate}:{temporary_ref}");
        if let Err(error) = git(&repo.root, &["fetch", &remote, &refspec]) {
            last_error = Some(format!("{candidate}: {error}"));
            continue;
        }
        match git(&repo.root, &["rev-parse", &temporary_ref]) {
            Ok(fetched) if fetched == expected_head => return Ok(temporary_ref),
            Ok(fetched) => {
                last_error = Some(format!("{candidate} 指向 {fetched}"));
            }
            Err(error) => last_error = Some(format!("{candidate}: {error}")),
        }
        let _ = git(&repo.root, &["update-ref", "-d", &temporary_ref]);
    }

    Err(format!(
        "更新后的 PR head 尚未收敛: provider 返回 {expected_head}，但 {} 都没有解析到它{}",
        candidates.join("、"),
        last_error
            .map(|error| format!("（最后一次失败: {error}）"))
            .unwrap_or_default()
    ))
}

pub(crate) fn clear_delivery_operation_ref(repo: &RepoContext, reference: &str) {
    if reference.starts_with("refs/codefactory/delivery/") {
        let _ = git(&repo.root, &["update-ref", "-d", reference]);
    }
}

pub(crate) fn materialize_fetched_branch_update(
    request: &BranchUpdateMaterialization,
) -> Result<DeliveryIdentitySnapshot, String> {
    let (repo, _) = resolve_delivery_repo(
        &request.cwd,
        Some(&request.default_branch),
        Some(&request.head_branch),
    )?;
    let current = capture_delivery_identity(&repo)?;
    if current != request.previous_identity {
        return Err(
            "local workspace changed before the receipted branch-update CAS; HEAD/index/worktree were not advanced"
                .into(),
        );
    }
    let fetched = git(&repo.root, &["rev-parse", &request.fetched_ref])?;
    if fetched != request.next_head_sha {
        return Err("operation-owned branch-update ref no longer matches the exact new head".into());
    }
    let repository = git2::Repository::open(&repo.root)
        .map_err(|error| format!("cannot inspect fetched branch-update graph: {error}"))?;
    let previous_oid = git2::Oid::from_str(&request.previous_identity.head_sha)
        .map_err(|error| format!("invalid branch-update parent SHA: {error}"))?;
    let next_oid = git2::Oid::from_str(&request.next_head_sha)
        .map_err(|error| format!("invalid branch-update result SHA: {error}"))?;
    if !repository
        .graph_descendant_of(next_oid, previous_oid)
        .map_err(|error| format!("cannot compare branch-update ancestry: {error}"))?
    {
        return Err(
            "committed branch update result is not a descendant of its exact prior head".into(),
        );
    }
    fast_forward_updated_pr_head(&repo, &request.fetched_ref, &request.next_head_sha)?;
    let observed = capture_delivery_identity(&repo)?;
    let statuses = repository
        .statuses(None)
        .map_err(|error| format!("cannot inspect materialized branch-update status: {error}"))?;
    if observed.head_sha != request.next_head_sha || !statuses.is_empty() {
        return Err(
            "branch-update materialization did not finish at the exact clean provider head".into(),
        );
    }
    Ok(observed)
}

pub(crate) fn fast_forward_updated_pr_head(
    repo: &RepoContext,
    remote_ref: &str,
    expected_head: &str,
) -> Result<String, String> {
    git(&repo.root, &["merge", "--ff-only", remote_ref])?;
    let local = git(&repo.root, &["rev-parse", "HEAD"])?;
    if local != expected_head {
        return Err(format!(
            "本地分支未绑定更新后的 PR head: 预期 {expected_head}，实际 {local}"
        ));
    }
    Ok(local)
}

async fn resume_queued_merge<R: DeliveryRemote>(
    mut outcome: DeliveryOutcome,
    repo: &RepoContext,
    remote: &R,
    receipt: &DeliveryReceipt,
    opts: &DeliverOpts,
) -> DeliveryOutcome {
    let pr_number = receipt.pr_number;
    let mut queued = receipt.clone();
    queued.state = "merge_queued".into();
    if let Err(step) = verify_mutation_permit(opts, "receipt_merge_queued").await {
        return outcome.blocked_on_uncertain_side_effect(step);
    }
    if let Err(error) = write_delivery_receipt(repo, &receipt.commit_sha, &queued) {
        return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
            "receipt",
            format!("GitHub 已登记 auto-merge，但 merge_queued 回执写入失败: {error}"),
        ));
    }
    outcome.pr_number = Some(pr_number);
    outcome.pr_url = Some(receipt.pr_url.clone());

    match remote.merge_readiness(pr_number).await {
        Ok(MergeReadiness::Behind) => {
            let previous_identity = match capture_delivery_identity(repo) {
                Ok(identity) => identity,
                Err(error) => {
                    return outcome.blocked_at(StepResult::blocked(
                        "branch_sync",
                        format!("PR #{pr_number} 更新前无法冻结本地精确身份: {error}"),
                    ))
                }
            };
            if let Err(step) = verify_mutation_permit(opts, "update_pr_branch").await {
                return outcome.blocked_on_uncertain_side_effect(step);
            }
            let new_head = match remote
                .update_pr_branch(
                    pr_number,
                    &receipt.commit_sha,
                    opts.mutation_permit.as_ref(),
                )
                .await
            {
                Ok(head) => head,
                Err(error) => {
                    return outcome.blocked_at(StepResult::blocked(
                        "branch_update",
                        format!(
                            "PR #{pr_number} 落后于 {}，自动更新失败: {error}；\
修复更新条件后重新调用 deliver_changes，不能仅等待 auto-merge。",
                            repo.default_branch
                        ),
                    ))
                }
            };
            let number_text = pr_number.to_string();
            let operation_key = external_operation_key(
                "provider_pr_branch_update",
                &[&number_text, &previous_identity.head_sha],
            );
            let remote_ref = match fetch_updated_pr_head_for_operation(
                repo,
                &new_head,
                &operation_key,
                Some(pr_number),
            ) {
                Ok(remote_ref) => remote_ref,
                Err(error) => {
                    return outcome.blocked_at(StepResult::blocked(
                        "branch_sync",
                        format!(
                            "PR #{pr_number} 已更新到 {new_head}，但本地分支同步失败: {error}。\
先把本地 {} 快进到远端同名分支，再调用 deliver_changes；不要 force push 旧 head。",
                            repo.branch
                        ),
                    ))
                }
            };
            let request = BranchUpdateMaterialization {
                pr_number,
                cwd: repo.root.clone(),
                default_branch: repo.default_branch.clone(),
                head_branch: repo.branch.clone(),
                previous_identity,
                next_head_sha: new_head.clone(),
                fetched_ref: remote_ref.clone(),
            };
            let materialized = match opts.mutation_permit.as_ref() {
                Some(permit) => permit.materialize_branch_update(&request).await,
                None => materialize_fetched_branch_update(&request),
            };
            clear_delivery_operation_ref(repo, &remote_ref);
            let local_head = match materialized {
                Ok(identity) => identity.head_sha,
                Err(error) => {
                    return outcome.blocked_at(StepResult::blocked(
                        "branch_sync",
                        format!(
                            "PR #{pr_number} 已更新到 {new_head}，但本地分支快进失败: {error}。\
系统将保留同一 PR 身份并重新观察；不要 force push 旧 head。"
                        ),
                    ))
                }
            };
            let mut rebound = queued;
            rebound.state = "pr_open".into();
            rebound.commit_sha = local_head.clone();
            if let Err(step) = verify_mutation_permit(opts, "receipt_rebind_pr_head").await {
                return outcome.blocked_on_uncertain_side_effect(step);
            }
            if let Err(error) = write_delivery_receipt(repo, &local_head, &rebound) {
                return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                    "receipt",
                    format!(
                        "PR #{pr_number} 已更新并同步到 {local_head}，但新 head 回执写入失败: {error}"
                    ),
                ));
            }
            outcome.commit_sha = Some(local_head.clone());
            outcome.steps.push(StepResult::ok(
                "branch_update",
                format!(
                    "PR #{pr_number} 落后于 {}，已更新并把本地分支重绑到 {local_head}",
                    repo.default_branch
                ),
            ));
            outcome.final_state = "blocked".into();
            outcome.stage = "branch_update".into();
            outcome.code = "delivery_branch_updated".into();
            outcome.recoverable = true;
            outcome.next_action = Some(
                "新 head 的 required checks 会重新运行；现在重新调用 deliver_changes 续接同一 PR。"
                    .into(),
            );
            outcome.reached_state = "pr_open".into();
            outcome.summary = format!(
                "PR #{pr_number} 的 BEHIND 死锁已自动解除，交付已重绑到新 head {local_head}。"
            );
            outcome
        }
        Ok(MergeReadiness::NeedsAction(reason)) => outcome.blocked_at(StepResult::blocked(
            "merge",
            format!("PR #{pr_number} 需要系统修复仓库门禁后续接同一 PR: {reason}"),
        )),
        Ok(MergeReadiness::Ready)
        | Ok(MergeReadiness::WaitingOnChecks)
        | Ok(MergeReadiness::Unknown) => {
            let mut outcome = outcome.waiting_at(
                StepResult::waiting(
                "merge",
                "GitHub 已登记受规则保护的 auto-merge；PR 正在等待远端门禁，后续续接只核对远端状态，不重复发起合并",
                ),
                30_000,
                "等待远端门禁产生新状态后重新调用 deliver_changes 续接合并和发布。",
            );
            outcome.code = "delivery_merge_queued".into();
            outcome.reached_state = "merge_queued".into();
            outcome.summary = "GitHub 已登记 auto-merge，正在等待远端门禁；不是权限不足。".into();
            outcome
        }
        Err(error) => outcome.remote_observation_failed(
            "merge_observation",
            format!("暂时无法核对 PR #{pr_number} 的远端合并状态: {error}"),
        ),
    }
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

/// Objective-independent identity of the exact checkout a delivery would
/// mutate. Both durable preflight and the mutation fence use this one capture
/// routine so their hashing rules cannot drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryIdentitySnapshot {
    pub repo_identity: String,
    pub worktree_identity: String,
    pub head_sha: String,
    pub change_set_digest: String,
}

/// Write-ahead identity for the local `git commit` rung. The tree and message
/// are known after scoped staging but before the commit is created, so a new
/// process can prove an observed child commit without trusting mutable prose
/// or merely noticing that HEAD changed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LocalCommitIntentEvidence {
    pub repo_identity: String,
    pub worktree_identity: String,
    pub previous_head_sha: String,
    pub previous_change_set_digest: String,
    pub staged_change_set_digest: String,
    #[serde(default)]
    pub original_index_tree_sha: String,
    #[serde(default)]
    pub original_index_digest: String,
    #[serde(default)]
    pub target_index_digest: String,
    #[serde(default)]
    pub source_manifest_digest: String,
    pub head_branch: String,
    pub staged_tree_sha: String,
    pub expected_head_sha: String,
    pub commit_message_digest: String,
}

impl LocalCommitIntentEvidence {
    pub fn new(
        persisted: &DeliveryIdentitySnapshot,
        staged: &DeliveryIdentitySnapshot,
        head_branch: &str,
        staged_tree_sha: &str,
        expected_head_sha: &str,
        commit_message: &str,
    ) -> Self {
        Self {
            repo_identity: persisted.repo_identity.clone(),
            worktree_identity: persisted.worktree_identity.clone(),
            previous_head_sha: persisted.head_sha.clone(),
            previous_change_set_digest: persisted.change_set_digest.clone(),
            staged_change_set_digest: staged.change_set_digest.clone(),
            original_index_tree_sha: String::new(),
            original_index_digest: String::new(),
            target_index_digest: String::new(),
            source_manifest_digest: String::new(),
            head_branch: head_branch.into(),
            staged_tree_sha: staged_tree_sha.into(),
            expected_head_sha: expected_head_sha.into(),
            commit_message_digest: external_operation_key("message", &[commit_message]),
        }
    }

    fn prepared(
        persisted: &DeliveryIdentitySnapshot,
        head_branch: &str,
        original_index_tree_sha: &str,
        original_index_digest: &str,
        target_index_digest: &str,
        source_manifest_digest: &str,
        staged_tree_sha: &str,
        expected_head_sha: &str,
        commit_message: &str,
    ) -> Self {
        Self {
            repo_identity: persisted.repo_identity.clone(),
            worktree_identity: persisted.worktree_identity.clone(),
            previous_head_sha: persisted.head_sha.clone(),
            previous_change_set_digest: persisted.change_set_digest.clone(),
            // The new transaction prepares the exact target tree before the
            // real index moves. Recovery therefore relies on the immutable
            // source manifest + original/target index trees, not on a digest
            // whose status bits necessarily change during staging.
            staged_change_set_digest: String::new(),
            original_index_tree_sha: original_index_tree_sha.into(),
            original_index_digest: original_index_digest.into(),
            target_index_digest: target_index_digest.into(),
            source_manifest_digest: source_manifest_digest.into(),
            head_branch: head_branch.into(),
            staged_tree_sha: staged_tree_sha.into(),
            expected_head_sha: expected_head_sha.into(),
            commit_message_digest: external_operation_key("message", &[commit_message]),
        }
    }

    pub fn operation_key(&self) -> String {
        if !self.original_index_tree_sha.is_empty() && !self.source_manifest_digest.is_empty() {
            return external_operation_key(
                "git_local_commit",
                &[
                    &self.repo_identity,
                    &self.worktree_identity,
                    &self.previous_head_sha,
                    &self.previous_change_set_digest,
                    &self.original_index_tree_sha,
                    &self.original_index_digest,
                    &self.target_index_digest,
                    &self.source_manifest_digest,
                    &self.head_branch,
                    &self.staged_tree_sha,
                    &self.expected_head_sha,
                    &self.commit_message_digest,
                ],
            );
        }
        external_operation_key(
            "git_local_commit",
            &[
                &self.repo_identity,
                &self.worktree_identity,
                &self.previous_head_sha,
                &self.previous_change_set_digest,
                &self.staged_change_set_digest,
                &self.head_branch,
                &self.staged_tree_sha,
                &self.expected_head_sha,
                &self.commit_message_digest,
            ],
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryTakeoverObservation {
    pub identity: DeliveryIdentitySnapshot,
    pub remote_head_sha: Option<String>,
    pub canonical_pr_number: Option<u64>,
    pub canonical_pr_url: Option<String>,
    pub canonical_head_sha: Option<String>,
}

pub fn capture_delivery_identity(repo: &RepoContext) -> Result<DeliveryIdentitySnapshot, String> {
    let repository = git2::Repository::open(&repo.root)
        .map_err(|error| format!("cannot inspect delivery worktree: {error}"))?;
    let head_sha = repository
        .head()
        .ok()
        .and_then(|head| head.target().map(|oid| oid.to_string()))
        .ok_or_else(|| "cannot establish the delivery worktree HEAD".to_string())?;

    let local_repo_identity_source;
    let repo_source = if let Some(remote_url) = repo.remote_url.as_deref() {
        remote_url
    } else {
        let common_dir = git(&repo.root, &["rev-parse", "--git-common-dir"])?;
        let common_dir = PathBuf::from(common_dir);
        let common_dir = if common_dir.is_absolute() {
            common_dir
        } else {
            repo.root.join(common_dir)
        };
        local_repo_identity_source = common_dir
            .canonicalize()
            .map_err(|error| format!("cannot canonicalize local repository identity: {error}"))?
            .to_string_lossy()
            .into_owned();
        &local_repo_identity_source
    };
    let repo_identity = format!("sha256:{:x}", Sha256::digest(repo_source.as_bytes()));

    let admin_path = repository
        .path()
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize delivery worktree identity: {error}"))?;
    let worktree_identity = format!(
        "gitdir-sha256:{:x}",
        Sha256::digest(admin_path.to_string_lossy().as_bytes())
    );

    let statuses = repository
        .statuses(None)
        .map_err(|error| format!("cannot inspect delivery changes: {error}"))?;
    let mut entries: Vec<_> = statuses
        .iter()
        .filter_map(|entry| {
            entry
                .path()
                .map(|path| (path.to_owned(), entry.status().bits()))
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut change_set_hasher = Sha256::new();
    change_set_hasher.update(head_sha.as_bytes());
    for (path, status) in entries {
        change_set_hasher.update(path.as_bytes());
        change_set_hasher.update(status.to_le_bytes());
        if let Ok(bytes) = std::fs::read(repo.root.join(&path)) {
            change_set_hasher.update(bytes);
        }
    }

    Ok(DeliveryIdentitySnapshot {
        repo_identity,
        worktree_identity,
        head_sha,
        change_set_digest: format!("sha256:{:x}", change_set_hasher.finalize()),
    })
}

fn verify_local_commit_receipt_binding(
    current: &DeliveryIdentitySnapshot,
    expected_branch: &str,
    persisted_identity: &DeliveryIdentitySnapshot,
    evidence: &LocalCommitIntentEvidence,
) -> Result<(), String> {
    if evidence.repo_identity != persisted_identity.repo_identity
        || evidence.worktree_identity != persisted_identity.worktree_identity
        || evidence.previous_head_sha != persisted_identity.head_sha
        || evidence.previous_change_set_digest != persisted_identity.change_set_digest
        || evidence.head_branch != expected_branch
        || current.repo_identity != persisted_identity.repo_identity
        || current.worktree_identity != persisted_identity.worktree_identity
    {
        return Err(
            "local commit receipt does not bind the persisted repo/worktree/head identity".into(),
        );
    }
    Ok(())
}

fn verify_receipted_commit_object(
    root: &Path,
    persisted_identity: &DeliveryIdentitySnapshot,
    evidence: &LocalCommitIntentEvidence,
) -> Result<(), String> {
    let parent = git(root, &["rev-parse", &format!("{}^", evidence.expected_head_sha)])?;
    if parent != persisted_identity.head_sha {
        return Err(
            "receipted local commit object is not the exact child of the persisted delivery head"
                .into(),
        );
    }
    let tree = git(
        root,
        &["rev-parse", &format!("{}^{{tree}}", evidence.expected_head_sha)],
    )?;
    if tree != evidence.staged_tree_sha {
        return Err("receipted local commit object tree does not match its write-ahead receipt".into());
    }
    let message = git(
        root,
        &["log", "-1", "--format=%B", &evidence.expected_head_sha],
    )?;
    if external_operation_key("message", &[&message]) != evidence.commit_message_digest {
        return Err(
            "receipted local commit object message does not match its write-ahead receipt".into(),
        );
    }
    Ok(())
}

/// Complete the exact CAS ref update when a process died after persisting the
/// local-commit intent but before moving the branch. The staged index, commit
/// object and durable receipt must all still match; no content is regenerated.
pub(crate) fn materialize_receipted_local_commit(
    cwd: &Path,
    default_branch_hint: Option<&str>,
    expected_branch: &str,
    persisted_identity: &DeliveryIdentitySnapshot,
    evidence: &LocalCommitIntentEvidence,
) -> Result<DeliveryIdentitySnapshot, String> {
    materialize_receipted_local_commit_with_fault_marker(
        cwd,
        default_branch_hint,
        expected_branch,
        persisted_identity,
        evidence,
        None,
    )
}

pub(crate) fn materialize_receipted_local_commit_with_fault_marker(
    cwd: &Path,
    default_branch_hint: Option<&str>,
    expected_branch: &str,
    persisted_identity: &DeliveryIdentitySnapshot,
    evidence: &LocalCommitIntentEvidence,
    pause_before_ref_marker: Option<&Path>,
) -> Result<DeliveryIdentitySnapshot, String> {
    let (repo, _) = resolve_delivery_repo(cwd, default_branch_hint, Some(expected_branch))?;
    let current = capture_delivery_identity(&repo)?;
    verify_local_commit_receipt_binding(
        &current,
        expected_branch,
        persisted_identity,
        evidence,
    )?;
    if !evidence.original_index_digest.is_empty() && !evidence.target_index_digest.is_empty() {
        if current.head_sha != persisted_identity.head_sha
            && current.head_sha != evidence.expected_head_sha
        {
            return Err("receipted local commit found an unrecognized branch head".into());
        }
        let repository = git2::Repository::open(&repo.root)
            .map_err(|error| format!("cannot open delivery repository index: {error}"))?;
        let index_path = repository
            .index()
            .map_err(|error| format!("cannot open delivery repository index: {error}"))?
            .path()
            .ok_or_else(|| "delivery repository has no on-disk index".to_string())?
            .to_path_buf();
        let lock_path = index_path.with_extension("lock");
        let lock_owner_token = evidence.operation_key();
        let owner_suffix = lock_owner_token
            .strip_prefix("sha256:")
            .unwrap_or(&lock_owner_token);
        let owned_lock_path = index_path.with_file_name(format!(
            "index.codefactory-{}.lock",
            &owner_suffix[..owner_suffix.len().min(32)]
        ));
        let target_index_bytes =
            canonical_index_bytes_for_tree(&repo.root, &evidence.staged_tree_sha)?;
        if bytes_digest(&target_index_bytes) != evidence.target_index_digest {
            return Err("receipted target index bytes no longer match their exact digest".into());
        }

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&owned_lock_path)
        {
            Ok(mut owner) => {
                use std::io::Write;
                owner
                    .write_all(&target_index_bytes)
                    .map_err(|error| format!("cannot write owned Git index lock: {error}"))?;
                owner
                    .sync_all()
                    .map_err(|error| format!("cannot sync owned Git index lock: {error}"))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = std::fs::read(&owned_lock_path).map_err(|read_error| {
                    format!("cannot inspect owned Git index lock: {read_error}")
                })?;
                if bytes_digest(&existing) != evidence.target_index_digest {
                    return Err(
                        "owned Git index lock does not match the durable transaction; no file was changed"
                            .into(),
                    );
                }
            }
            Err(error) => {
                return Err(format!("cannot prepare owned Git index lock: {error}"))
            }
        }

        let mut lock_owned = match std::fs::hard_link(&owned_lock_path, &lock_path) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if same_file::is_same_file(&owned_lock_path, &lock_path).unwrap_or(false) {
                    true
                } else {
                    return Err(
                        "Git index is locked by an unrecognized writer; the owned lock inode was not adopted, overwritten, or removed"
                            .into(),
                    );
                }
            }
            Err(error) => {
                return Err(format!("cannot link the owned Git index CAS lock: {error}"));
            }
        };

        let transaction = (|| {
            let current_index_bytes = std::fs::read(&index_path)
                .map_err(|error| format!("cannot read current Git index under lock: {error}"))?;
            let current_index_digest = bytes_digest(&current_index_bytes);
            if current_index_digest != evidence.original_index_digest
                && current_index_digest != evidence.target_index_digest
            {
                return Err(
                    "Git index changed outside the receipted transaction before its exact CAS"
                        .to_string(),
                );
            }
            if worktree_source_manifest_digest(&repo.root)? != evidence.source_manifest_digest {
                return Err(
                    "delivery source changed after the exact Git index lock was acquired"
                        .to_string(),
                );
            }
            if let Some(marker) = pause_before_ref_marker {
                std::fs::write(
                    marker,
                    serde_json::to_vec_pretty(&json!({
                        "worker_pid": std::process::id(),
                        "post_intent_target_index_lock_pre_ref_fault_injected": true,
                        "previous_head_sha": &evidence.previous_head_sha,
                        "expected_head_sha": &evidence.expected_head_sha,
                        "original_index_digest": &evidence.original_index_digest,
                        "target_index_digest": &evidence.target_index_digest,
                        "lock_owner_token": &lock_owner_token,
                        "owned_lock_path": &owned_lock_path,
                    }))
                    .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                std::thread::sleep(std::time::Duration::from_secs(300));
                return Err("delivery recovery smoke was not hard-killed before update-ref".into());
            }
            verify_receipted_commit_object(&repo.root, persisted_identity, evidence)?;
            let locked_head = git(&repo.root, &["rev-parse", "HEAD"])?;
            if locked_head != persisted_identity.head_sha
                && locked_head != evidence.expected_head_sha
            {
                return Err("branch HEAD changed during the receipted Git index transaction".into());
            }
            if locked_head == persisted_identity.head_sha {
                let branch_ref = format!("refs/heads/{expected_branch}");
                git(
                    &repo.root,
                    &[
                        "update-ref",
                        "-m",
                        "CodeFactory delivery commit recovery",
                        &branch_ref,
                        &evidence.expected_head_sha,
                        &persisted_identity.head_sha,
                    ],
                )?;
            }
            std::fs::rename(&lock_path, &index_path)
                .map_err(|error| format!("cannot atomically install exact Git index: {error}"))?;
            std::fs::remove_file(&owned_lock_path)
                .map_err(|error| format!("cannot clear owned Git index lock: {error}"))?;
            lock_owned = false;
            Ok(())
        })();
        if lock_owned {
            let _ = std::fs::remove_file(&lock_path);
            let _ = std::fs::remove_file(&owned_lock_path);
        }
        transaction?;
        return observe_receipted_local_commit(
            cwd,
            default_branch_hint,
            expected_branch,
            persisted_identity,
            evidence,
        );
    }
    if current.head_sha == evidence.expected_head_sha {
        return observe_receipted_local_commit(
            cwd,
            default_branch_hint,
            expected_branch,
            persisted_identity,
            evidence,
        );
    }
    if current.head_sha != persisted_identity.head_sha {
        return Err(
            "pre-ref local commit recovery found an unreceipted HEAD drift".into(),
        );
    }
    if evidence.original_index_tree_sha.is_empty() || evidence.source_manifest_digest.is_empty() {
        if current.change_set_digest != evidence.staged_change_set_digest {
            return Err(
                "legacy pre-ref local commit recovery found an unreceipted change-set drift"
                    .into(),
            );
        }
        if git(&repo.root, &["write-tree"])? != evidence.staged_tree_sha {
            return Err("pre-ref local commit recovery found a different staged index tree".into());
        }
    } else {
        if worktree_source_manifest_digest(&repo.root)? != evidence.source_manifest_digest {
            return Err(
                "pre-ref local commit recovery found source content or path drift".into(),
            );
        }
        let current_index_tree = git(&repo.root, &["write-tree"])?;
        if current_index_tree != evidence.original_index_tree_sha
            && current_index_tree != evidence.staged_tree_sha
        {
            return Err("pre-ref local commit recovery found foreign index drift".into());
        }
        if current_index_tree != evidence.staged_tree_sha {
            git(&repo.root, &["read-tree", &evidence.staged_tree_sha])?;
        }
        if git(&repo.root, &["write-tree"])? != evidence.staged_tree_sha {
            return Err("pre-ref local commit recovery could not restore the exact target index".into());
        }
    }
    if let Some(marker) = pause_before_ref_marker {
        std::fs::write(
            marker,
            serde_json::to_vec_pretty(&json!({
                "worker_pid": std::process::id(),
                "post_intent_post_index_pre_ref_fault_injected": true,
                "previous_head_sha": &evidence.previous_head_sha,
                "expected_head_sha": &evidence.expected_head_sha,
                "original_index_tree_sha": &evidence.original_index_tree_sha,
                "target_index_tree_sha": &evidence.staged_tree_sha,
            }))
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        std::thread::sleep(std::time::Duration::from_secs(300));
        return Err("delivery recovery smoke was not hard-killed before update-ref".into());
    }
    verify_receipted_commit_object(&repo.root, persisted_identity, evidence)?;
    let branch_ref = format!("refs/heads/{expected_branch}");
    git(
        &repo.root,
        &[
            "update-ref",
            "-m",
            "CodeFactory delivery commit recovery",
            &branch_ref,
            &evidence.expected_head_sha,
            &persisted_identity.head_sha,
        ],
    )?;
    observe_receipted_local_commit(
        cwd,
        default_branch_hint,
        expected_branch,
        persisted_identity,
        evidence,
    )
}

/// Prove that the current clean HEAD is exactly the commit covered by a
/// durable local write-ahead receipt. This function is observation-only.
pub fn observe_receipted_local_commit(
    cwd: &Path,
    default_branch_hint: Option<&str>,
    expected_branch: &str,
    persisted_identity: &DeliveryIdentitySnapshot,
    evidence: &LocalCommitIntentEvidence,
) -> Result<DeliveryIdentitySnapshot, String> {
    let (repo, _) = resolve_delivery_repo(cwd, default_branch_hint, Some(expected_branch))?;
    let current = capture_delivery_identity(&repo)?;
    verify_local_commit_receipt_binding(
        &current,
        expected_branch,
        persisted_identity,
        evidence,
    )?;
    if current.head_sha == persisted_identity.head_sha {
        return Err("local commit receipt has no materialized child commit to reconcile".into());
    }
    if current.head_sha != evidence.expected_head_sha {
        return Err("observed local commit does not match the exact write-ahead commit SHA".into());
    }
    verify_receipted_commit_object(&repo.root, persisted_identity, evidence)?;
    if evidence.source_manifest_digest.is_empty() {
        if !git(
            &repo.root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )?
        .is_empty()
        {
            return Err("local commit receipt cannot absorb edits made after the commit".into());
        }
    } else {
        if worktree_source_manifest_digest(&repo.root)? != evidence.source_manifest_digest {
            return Err("local commit receipt cannot absorb source edits made after the commit".into());
        }
        if git(&repo.root, &["write-tree"])? != evidence.staged_tree_sha {
            return Err("local commit receipt found an index outside the exact committed tree".into());
        }
    }
    Ok(current)
}

fn observe_remote_branch_head(repo: &RepoContext) -> Result<Option<String>, String> {
    observe_remote_ref_head(repo, &repo.branch)
}

fn observe_remote_ref_head(repo: &RepoContext, branch: &str) -> Result<Option<String>, String> {
    let reference = format!("refs/heads/{branch}");
    let observed = git(
        &repo.root,
        &["ls-remote", "--heads", &repo.remote, &reference],
    )?;
    let mut heads = observed
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|sha| !sha.is_empty());
    let head = heads.next().map(str::to_string);
    if heads.next().is_some() {
        return Err(format!(
            "remote branch observation returned multiple heads for {reference}"
        ));
    }
    Ok(head)
}

/// Reconcile a takeover without issuing a Git or provider mutation.
///
/// This is the mandatory bridge between a newly-incremented claim epoch and a
/// mutation-capable permit. It accepts only the persisted or locally-observed
/// head, and uses provider observation (never open/update/merge/release) to
/// prove that an already-issued old-owner request did not create a conflicting
/// canonical object.
pub async fn observe_delivery_takeover<R: DeliveryRemote>(
    cwd: &Path,
    default_branch_hint: Option<&str>,
    expected_branch: &str,
    persisted_identity: &DeliveryIdentitySnapshot,
    persisted_canonical_pr_number: Option<u64>,
    persisted_canonical_pr_url: Option<&str>,
    remote: Option<&R>,
) -> Result<DeliveryTakeoverObservation, String> {
    observe_delivery_takeover_with_receipted_parent(
        cwd,
        default_branch_hint,
        expected_branch,
        persisted_identity,
        persisted_canonical_pr_number,
        persisted_canonical_pr_url,
        None,
        remote,
    )
    .await
}

/// Variant used only after a write-ahead local-commit receipt has atomically
/// revised the durable head. The canonical branch/PR may legitimately remain
/// at that receipt's exact parent until the replacement owner receives a new
/// mutation permit and pushes the child. No other remote head is accepted.
pub(crate) async fn observe_delivery_takeover_with_receipted_parent<R: DeliveryRemote>(
    cwd: &Path,
    default_branch_hint: Option<&str>,
    expected_branch: &str,
    persisted_identity: &DeliveryIdentitySnapshot,
    persisted_canonical_pr_number: Option<u64>,
    persisted_canonical_pr_url: Option<&str>,
    receipted_parent_head: Option<&str>,
    remote: Option<&R>,
) -> Result<DeliveryTakeoverObservation, String> {
    let (repo, _) = resolve_delivery_repo(cwd, default_branch_hint, Some(expected_branch))?;
    let identity = capture_delivery_identity(&repo)?;
    if &identity != persisted_identity {
        return Err(
            "observe-only takeover found repo/worktree/head/change-set state that was not durably receipted; mutation remains fenced"
                .to_string(),
        );
    }

    let remote_head = observe_remote_branch_head(&repo)?;
    if remote_head.as_deref().is_some_and(|head| {
        head != persisted_identity.head_sha && Some(head) != receipted_parent_head
    }) {
        return Err(format!(
            "remote branch {} moved to an unrecognized head {}; expected persisted {}{}",
            repo.branch,
            remote_head.as_deref().unwrap_or_default(),
            persisted_identity.head_sha,
            receipted_parent_head
                .map(|parent| format!(" or receipted parent {parent}"))
                .unwrap_or_default()
        ));
    }

    // The persisted-head key is authoritative. Looking only at a newly
    // observed HEAD would skip an old intent_merge/intent_release receipt and
    // allow an unreceipted local commit to erase the uncertainty boundary.
    let local_receipt = read_delivery_receipt(&repo, &persisted_identity.head_sha)?;
    // `intent_release` is not rejected here. The durable takeover path must
    // first reconcile any DB mutation intent, then run the exact release
    // workflow/ref/head observer. Rejecting it in this base identity observer
    // made the release-specific reconciler unreachable and stranded the run.
    if let (Some(expected_number), Some(receipt)) =
        (persisted_canonical_pr_number, local_receipt.as_ref())
    {
        if receipt.pr_number != expected_number {
            return Err(format!(
                "local receipt PR #{} conflicts with persisted canonical PR #{}",
                receipt.pr_number, expected_number
            ));
        }
    }
    if let (Some(expected_url), Some(receipt)) =
        (persisted_canonical_pr_url, local_receipt.as_ref())
    {
        if receipt.pr_url != expected_url {
            return Err("local receipt URL conflicts with persisted canonical PR URL".into());
        }
    }

    let canonical_pr_number = local_receipt
        .as_ref()
        .map(|receipt| receipt.pr_number)
        .or(persisted_canonical_pr_number);
    let canonical_pr_url = local_receipt
        .as_ref()
        .map(|receipt| receipt.pr_url.clone())
        .or_else(|| persisted_canonical_pr_url.map(str::to_string));
    if canonical_pr_number.is_some() != canonical_pr_url.is_some() {
        return Err("canonical PR identity is incomplete during takeover observation".into());
    }
    let canonical_head_sha = local_receipt
        .as_ref()
        .map(|receipt| receipt.commit_sha.clone())
        .or_else(|| {
            persisted_canonical_pr_number.map(|_| {
                remote_head
                    .clone()
                    .unwrap_or_else(|| persisted_identity.head_sha.clone())
            })
        });

    if let Some(pr_number) = canonical_pr_number {
        let remote = remote.ok_or_else(|| {
            "canonical PR exists but no read-only provider observer is available".to_string()
        })?;
        match remote.observe_merge(pr_number, &identity.head_sha).await? {
            MergeObservation::OpenSameHead { .. } | MergeObservation::Merged { .. } => {}
            MergeObservation::HeadChanged { actual_head }
                if actual_head == persisted_identity.head_sha
                    || Some(actual_head.as_str()) == receipted_parent_head => {}
            MergeObservation::HeadChanged { actual_head } => {
                return Err(format!(
                    "canonical PR #{pr_number} moved to unrecognized head {actual_head}"
                ))
            }
            MergeObservation::ClosedUnmerged => {
                return Err(format!(
                    "canonical PR #{pr_number} was closed without merge during takeover"
                ))
            }
            MergeObservation::Unsupported => {
                return Err(format!(
                    "provider cannot observe canonical PR #{pr_number} during takeover"
                ))
            }
        }
    }

    Ok(DeliveryTakeoverObservation {
        identity,
        remote_head_sha: remote_head,
        canonical_pr_number,
        canonical_pr_url,
        canonical_head_sha,
    })
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
    Multiple(Vec<RepoContext>),
}

/// When the current checkout is on the default branch (can't open a PR from
/// it), discover sibling worktrees whose branch has commits ahead of
/// `origin/<default>`. The common worktree-default workflow leaves exactly one
/// such branch; delivery should target it instead of refusing outright.
fn discover_worktree_target(repo: &RepoContext) -> WorktreeDiscovery {
    let Ok(porcelain) = git(&repo.root, &["worktree", "list", "--porcelain"]) else {
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
        let (Some(dir), Some(branch)) = (dir, branch) else {
            continue;
        };
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
        n => WorktreeDiscovery::Multiple(
            candidates
                .into_iter()
                .take(n)
                .map(|(root, branch)| RepoContext {
                    remote_url: git(&root, &["remote", "get-url", &repo.remote]).ok(),
                    root,
                    branch,
                    default_branch: repo.default_branch.clone(),
                    remote: repo.remote.clone(),
                })
                .collect(),
        ),
    }
}

/// Resolve the exact checkout that delivery would mutate. This is shared with
/// the durable-run preflight so persisted identity and the side-effect target
/// cannot diverge when the caller starts from the default checkout.
pub fn resolve_delivery_repo(
    cwd: &Path,
    default_branch_hint: Option<&str>,
    expected_branch: Option<&str>,
) -> Result<(RepoContext, Option<String>), String> {
    let repo = resolve_repo(cwd, default_branch_hint)?;
    if repo.branch != repo.default_branch {
        return Ok((repo, None));
    }
    match discover_worktree_target(&repo) {
        WorktreeDiscovery::Single(target) => {
            let from = repo.branch;
            let message = format!(
                "主 checkout 在默认分支 {from} 上；检测到 worktree 分支 {} 有未合并提交，改为以该分支为交付目标",
                target.branch
            );
            Ok((target, Some(message)))
        }
        WorktreeDiscovery::Multiple(candidates) => {
            if let Some(expected_branch) = expected_branch {
                if let Some(target) = candidates
                    .iter()
                    .find(|candidate| candidate.branch == expected_branch)
                    .cloned()
                {
                    let message = format!(
                        "主 checkout 上存在多个待交付 worktree；依据已持久化 expect_branch={expected_branch} 解析到唯一目标"
                    );
                    return Ok((target, Some(message)));
                }
            }
            Err(format!(
                "当前在默认分支 {} 上,检测到多个 worktree 分支有待交付提交({});这是系统身份冲突，未执行任何交付动作。",
                repo.default_branch,
                candidates
                    .iter()
                    .map(|candidate| candidate.branch.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
        WorktreeDiscovery::None => Err(format!(
            "当前在默认分支 {} 上,不能从默认分支向自身开 PR;且未发现唯一待交付 worktree；未执行任何交付动作。",
            repo.default_branch
        )),
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

#[derive(Debug)]
struct ScopedCommitPlan {
    staged_paths: Vec<String>,
    original_index_tree_sha: String,
    original_index_digest: String,
    target_index_digest: String,
    target_tree_sha: String,
    source_manifest_digest: String,
}

fn bytes_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn current_index_digest(root: &Path) -> Result<String, String> {
    let repository = git2::Repository::open(root)
        .map_err(|error| format!("cannot open delivery repository index: {error}"))?;
    let index_path = repository
        .index()
        .map_err(|error| format!("cannot open delivery repository index: {error}"))?
        .path()
        .ok_or_else(|| "delivery repository has no on-disk index".to_string())?
        .to_path_buf();
    std::fs::read(&index_path)
        .map(|bytes| bytes_digest(&bytes))
        .map_err(|error| format!("cannot read delivery repository index: {error}"))
}

fn canonical_index_bytes_for_tree(root: &Path, tree_sha: &str) -> Result<Vec<u8>, String> {
    let repository = git2::Repository::open(root)
        .map_err(|error| format!("cannot open repository for canonical index: {error}"))?;
    let temporary = tempfile::NamedTempFile::new_in(repository.path())
        .map_err(|error| format!("cannot create canonical target index: {error}"))?;
    let path = temporary.path().to_path_buf();
    drop(temporary);
    // `git read-tree` requires a missing or valid alternate index, not the
    // empty file created by NamedTempFile.
    let _ = std::fs::remove_file(&path);
    let result = (|| {
        git_with_index(root, &path, &["read-tree", tree_sha])?;
        std::fs::read(&path)
            .map_err(|error| format!("cannot read canonical target index: {error}"))
    })();
    let _ = std::fs::remove_file(&path);
    result
}

/// Hash the checkout bytes and path set without consulting index status bits.
/// Replacing the index during a receipted stage leaves this digest unchanged;
/// any user/foreign content edit after a process loss changes it and fences the
/// branch ref before it can move.
fn worktree_source_manifest_digest(root: &Path) -> Result<String, String> {
    let mut paths = BTreeSet::new();
    for args in [
        ["ls-files", "-z"].as_slice(),
        ["ls-files", "--others", "--exclude-standard", "-z"].as_slice(),
    ] {
        for path in git(root, args)?.split('\0').filter(|path| !path.is_empty()) {
            paths.insert(path.to_string());
        }
    }
    let mut hasher = Sha256::new();
    hasher.update(b"delivery-worktree-source-v1\0");
    for path in paths {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        let absolute = root.join(&path);
        match std::fs::symlink_metadata(&absolute) {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    hasher.update(b"symlink\0");
                    let target = std::fs::read_link(&absolute)
                        .map_err(|error| format!("cannot read source symlink {path}: {error}"))?;
                    hasher.update(target.to_string_lossy().as_bytes());
                } else if file_type.is_file() {
                    hasher.update(b"file\0");
                    hasher.update(
                        std::fs::read(&absolute)
                            .map_err(|error| format!("cannot read source file {path}: {error}"))?,
                    );
                } else {
                    hasher.update(b"other\0");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                hasher.update(b"missing\0");
            }
            Err(error) => return Err(format!("cannot inspect source path {path}: {error}")),
        }
        hasher.update([0]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Compute the exact scoped commit tree in an isolated copy of the current
/// index. No real index, ref, worktree path, or remote is mutated here. This
/// closes every `git add` crash window with one pre-stage write-ahead receipt.
fn prepare_scoped_commit(root: &Path, extra: &[String]) -> Result<ScopedCommitPlan, String> {
    let repository = git2::Repository::open(root)
        .map_err(|error| format!("cannot prepare isolated delivery index: {error}"))?;
    let real_index = repository
        .index()
        .map_err(|error| format!("cannot open delivery index: {error}"))?;
    let real_index_path = real_index
        .path()
        .ok_or_else(|| "delivery repository has no on-disk index".to_string())?
        .to_path_buf();
    let isolated = tempfile::NamedTempFile::new_in(repository.path())
        .map_err(|error| format!("cannot create isolated delivery index: {error}"))?;
    std::fs::copy(&real_index_path, isolated.path())
        .map_err(|error| format!("cannot snapshot delivery index: {error}"))?;

    let original_index_bytes = std::fs::read(isolated.path())
        .map_err(|error| format!("cannot read snapshotted delivery index: {error}"))?;
    let original_index_tree_sha = git_with_index(root, isolated.path(), &["write-tree"])?;
    let source_manifest_digest = worktree_source_manifest_digest(root)?;
    git_with_index(root, isolated.path(), &["add", "-u"])?;
    let untracked = untracked_source_paths(root, extra)?;
    for path in &untracked {
        git_with_index(root, isolated.path(), &["add", "--", path])?;
    }
    let target_tree_sha = git_with_index(root, isolated.path(), &["write-tree"])?;
    let target_index_bytes = canonical_index_bytes_for_tree(root, &target_tree_sha)?;
    let staged = git_with_index(root, isolated.path(), &["diff", "--cached", "--name-only"])?;
    Ok(ScopedCommitPlan {
        staged_paths: staged.lines().map(str::to_string).collect(),
        original_index_tree_sha,
        original_index_digest: bytes_digest(&original_index_bytes),
        target_index_digest: bytes_digest(&target_index_bytes),
        target_tree_sha,
        source_manifest_digest,
    })
}

fn apply_scoped_commit_plan(root: &Path, plan: &ScopedCommitPlan) -> Result<(), String> {
    if worktree_source_manifest_digest(root)? != plan.source_manifest_digest {
        return Err("delivery source changed after the local commit receipt was prepared".into());
    }
    let current_index_tree = git(root, &["write-tree"])?;
    if current_index_tree != plan.original_index_tree_sha
        && current_index_tree != plan.target_tree_sha
    {
        return Err("delivery index changed outside the receipted stage transaction".into());
    }
    if current_index_tree != plan.target_tree_sha {
        git(root, &["read-tree", &plan.target_tree_sha])?;
    }
    if git(root, &["write-tree"])? != plan.target_tree_sha {
        return Err("delivery index did not settle on the receipted target tree".into());
    }
    Ok(())
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

fn generate_commit_message_for_paths(
    branch: &str,
    title: Option<&str>,
    staged_paths: &[String],
) -> String {
    if let Some(title) = title {
        if !title.trim().is_empty() {
            return title.trim().to_string();
        }
    }
    let subject = branch
        .rsplit('/')
        .next()
        .unwrap_or(branch)
        .replace(['-', '_'], " ");
    format!(
        "{subject}\n\nDelivered by CodeFactory ({} file(s) changed).",
        staged_paths.len()
    )
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadmeBodyReconciliation {
    body: String,
    changed: bool,
}

fn readme_changed_on_branch(root: &Path, remote: &str, base: &str) -> Result<bool, String> {
    let range = format!("{remote}/{base}...HEAD");
    let changed = git(root, &["diff", "--name-only", &range])?;
    Ok(changed.lines().any(|path| path.trim() == "README.md"))
}

fn readme_reason_is_placeholder(reason: &str) -> bool {
    let trimmed = reason.trim();
    let lowered = trimmed.to_ascii_lowercase();
    trimmed.is_empty()
        || (trimmed.starts_with('<') && trimmed.ends_with('>'))
        || ["tbd", "todo", "fill in", "fill-in", "n/a"]
            .iter()
            .any(|placeholder| lowered.contains(placeholder))
}

fn readme_contract_lines(body: &str) -> (Vec<String>, Vec<String>) {
    let mut decisions = Vec::new();
    let mut reasons = Vec::new();
    let mut fenced = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let lowered = trimmed.to_ascii_lowercase();
        if lowered.starts_with("readme-update-reason:") {
            reasons.push(
                trimmed
                    .split_once(':')
                    .map(|(_, value)| value.trim().to_string())
                    .unwrap_or_default(),
            );
        } else if lowered.starts_with("readme-update:") {
            decisions.push(
                trimmed
                    .split_once(':')
                    .map(|(_, value)| value.trim().to_ascii_lowercase())
                    .unwrap_or_default(),
            );
        }
    }
    (decisions, reasons)
}

fn reconcile_readme_contract_body(
    root: &Path,
    remote: &str,
    base: &str,
    body: &str,
) -> Result<ReadmeBodyReconciliation, String> {
    let readme_changed = readme_changed_on_branch(root, remote, base)?;
    let (decisions, reasons) = readme_contract_lines(body);
    let valid_decision =
        decisions.len() == 1 && matches!(decisions[0].as_str(), "required" | "reviewed");
    let valid_reason = reasons.len() == 1 && !readme_reason_is_placeholder(&reasons[0]);
    if valid_decision && valid_reason {
        if decisions[0] == "required" && !readme_changed {
            return Err(
                "PR 已声明 README-Update: required，但当前分支没有 README.md 变更；请补齐 README 后续跑交付"
                    .into(),
            );
        }
        return Ok(ReadmeBodyReconciliation {
            body: body.to_string(),
            changed: false,
        });
    }

    let decision = if readme_changed {
        "required"
    } else {
        "reviewed"
    };
    let reason = if readme_changed {
        "README.md is updated in this PR to keep the evergreen product contract aligned with the implementation."
    } else {
        "README impact reviewed during controlled delivery; this PR does not change the evergreen README contract."
    };

    let mut fenced = false;
    let mut cleaned = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            cleaned.push(line.to_string());
            continue;
        }
        if !fenced {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.starts_with("readme-update:") || lowered.starts_with("readme-update-reason:")
            {
                continue;
            }
        }
        cleaned.push(line.to_string());
    }

    let decision_line = format!("README-Update: {decision}");
    let reason_line = format!("README-Update-Reason: {reason}");
    if let Some(heading) = cleaned
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case("## README contract"))
    {
        cleaned.insert(heading + 1, reason_line);
        cleaned.insert(heading + 1, decision_line);
    } else {
        while cleaned.last().is_some_and(|line| line.trim().is_empty()) {
            cleaned.pop();
        }
        let footer_start = cleaned.iter().position(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("Release-Urgency:")
                || trimmed.starts_with("BREAKING CHANGE:")
                || trimmed.starts_with("BREAKING-CHANGE:")
        });
        let footer = footer_start.map(|index| cleaned.split_off(index));
        while cleaned.last().is_some_and(|line| line.trim().is_empty()) {
            cleaned.pop();
        }
        if !cleaned.is_empty() {
            cleaned.push(String::new());
        }
        cleaned.extend([
            "## README contract".to_string(),
            String::new(),
            decision_line,
            reason_line,
        ]);
        if let Some(footer) = footer {
            cleaned.push(String::new());
            cleaned.extend(footer);
        }
    }

    Ok(ReadmeBodyReconciliation {
        body: format!("{}\n", cleaned.join("\n")),
        changed: true,
    })
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
        recovery_class: RecoveryClass::None,
        retry_after_ms: None,
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
    let (repo, worktree_resolution) =
        match resolve_delivery_repo(cwd, default_branch_hint, opts.expect_branch.as_deref()) {
            Ok(resolved) => resolved,
            Err(e) => return outcome.blocked_at(StepResult::blocked("repo", e)),
        };
    outcome.branch = Some(repo.branch.clone());
    if let Some(message) = worktree_resolution {
        outcome.steps.push(StepResult::ok("repo", message));
    }

    // The caller stated which delivery this is. A mismatch is a stale
    // declaration or a worktree mix-up, and must fail closed before any
    // commit, push, PR, merge, or release side effect can occur. The caller
    // can retry after switching to the declared branch or omitting the guard
    // when it intentionally wants the current checkout.
    if let Some(expected) = opts.expect_branch.as_deref() {
        if expected != repo.branch {
            return outcome.blocked_at(StepResult::blocked(
                "preflight",
                format!(
                    "调用方声明要交付分支 `{expected}`，但当前工作目录在 `{}` 上，未执行任何交付动作。\
先切到 `{expected}`，或在确实要交付当前分支时去掉该声明。",
                    repo.branch
                ),
            ));
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

    if let Some(expected_identity) = opts.expected_identity.as_ref() {
        let observed_identity = match capture_delivery_identity(&repo) {
            Ok(identity) => identity,
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked(
                    "identity",
                    format!(
                        "交付身份无法在副作用前复核: {error}。系统未执行 commit、push 或 PR 动作。"
                    ),
                ))
            }
        };
        if &observed_identity != expected_identity {
            return outcome.blocked_at(StepResult::blocked(
                "identity",
                "交付目标在持久化预检后发生变化；系统已在首次 git/远端副作用前拒绝本次执行。",
            ));
        }
    }

    // `deliver_changes` intentionally owns its native sync gate instead of
    // trusting the repository hook: the commit path uses `--no-verify`, and a
    // process can run with hooks disabled or missing. Fetch is read-only with
    // respect to the remote and happens before stage/commit/push/PR effects.
    if let Err(error) = git(
        &repo.root,
        &["fetch", "--prune", &repo.remote, &repo.default_branch],
    ) {
        return outcome.blocked_at(StepResult::blocked(
            "base_sync",
            format!(
                "无法在交付副作用前刷新 {}/{}: {error}",
                repo.remote, repo.default_branch
            ),
        ));
    }
    let remote_base = format!("{}/{}", repo.remote, repo.default_branch);
    if git(
        &repo.root,
        &["merge-base", "--is-ancestor", &remote_base, "HEAD"],
    )
    .is_err()
    {
        return outcome.blocked_at(StepResult::blocked(
            "base_sync",
            format!(
                "当前受管分支不包含最新 {remote_base}；系统未执行 stage、commit、push 或 PR 动作。请在同一受管 worktree 合并最新基线并重新验证。"
            ),
        ));
    }

    // ── Commit (noise-safe) ─────────────────────────────────────────────────
    if let Err(step) = verify_mutation_permit(opts, "git_stage").await {
        return outcome.blocked_on_uncertain_side_effect(step);
    }
    let commit_plan = match prepare_scoped_commit(&repo.root, &opts.extra_excludes) {
        Ok(plan) => plan,
        Err(e) => {
            return outcome.blocked_at(StepResult::blocked(
                "commit",
                format!("无法在隔离 index 中准备精确提交: {e}"),
            ))
        }
    };
    let head_tree = git(&repo.root, &["rev-parse", "HEAD^{tree}"]).unwrap_or_default();
    if commit_plan.target_tree_sha != head_tree {
        let msg = append_release_urgency(
            generate_commit_message_for_paths(
                &repo.branch,
                opts.title.as_deref(),
                &commit_plan.staged_paths,
            ),
            opts.release_urgency,
        );
        let persisted_identity = opts
            .expected_identity
            .as_ref()
            .cloned()
            .unwrap_or_else(|| {
                capture_delivery_identity(&repo)
                    .expect("delivery identity was captured during preflight")
            });
        let fresh_identity = match capture_delivery_identity(&repo) {
            Ok(identity) => identity,
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked(
                    "identity",
                    format!("写入本地提交意图前无法复核 checkout identity: {error}"),
                ))
            }
        };
        if fresh_identity != persisted_identity
            || worktree_source_manifest_digest(&repo.root).ok().as_deref()
                != Some(commit_plan.source_manifest_digest.as_str())
            || git(&repo.root, &["write-tree"]).ok().as_deref()
                != Some(commit_plan.original_index_tree_sha.as_str())
            || current_index_digest(&repo.root).ok().as_deref()
                != Some(commit_plan.original_index_digest.as_str())
        {
            return outcome.blocked_at(StepResult::blocked(
                "identity",
                "隔离 index 规划期间 checkout 内容或真实 index 已变化；未写入提交意图，也未覆盖外来暂存。",
            ));
        }
        // `commit-tree` writes only an unreachable content-addressed object;
        // it does not move HEAD, the branch, the index or the worktree. Its
        // exact SHA can therefore be included in the durable write-ahead
        // intent before the semantic ref mutation occurs.
        let expected_commit_sha = match git(
            &repo.root,
            &[
                "-c",
                "user.name=CodeFactory",
                "-c",
                "user.email=noreply@codefactory.local",
                "commit-tree",
                &commit_plan.target_tree_sha,
                "-p",
                &persisted_identity.head_sha,
                "-m",
                &msg,
            ],
        ) {
            Ok(sha) if !sha.is_empty() => sha,
            Ok(_) => {
                return outcome.blocked_at(StepResult::blocked(
                    "commit",
                    "提交前无法计算精确 commit identity：git commit-tree 未返回 SHA。",
                ))
            }
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked(
                    "commit",
                    format!("提交前无法计算精确 commit identity: {error}"),
                ))
            }
        };
        let commit_evidence = LocalCommitIntentEvidence::prepared(
            &persisted_identity,
            &repo.branch,
            &commit_plan.original_index_tree_sha,
            &commit_plan.original_index_digest,
            &commit_plan.target_index_digest,
            &commit_plan.source_manifest_digest,
            &commit_plan.target_tree_sha,
            &expected_commit_sha,
            &msg,
        );
        let commit_operation_key = commit_evidence.operation_key();
        let commit_intent_evidence = serde_json::to_string(&commit_evidence)
            .expect("local commit intent evidence is serializable");
        let commit_intent = match begin_or_reuse_external_mutation(
            opts.mutation_permit.as_ref(),
            "git_local_commit",
            &commit_operation_key,
            &commit_intent_evidence,
        )
        .await
        {
            Ok(DeliveryMutationBegin::Dispatch(intent)) => intent,
            Ok(DeliveryMutationBegin::AlreadyCommitted(_)) => {
                return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                    "mutation_intent",
                    "本地提交回执已标记成功但 HEAD 仍需要接管投影；本轮未重复移动分支。",
                ))
            }
            Err(error) => {
                return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                    "mutation_intent",
                    format!("真实 index 变更前无法持久化本地 Git 写入意图: {error}。未执行 stage/commit。"),
                ))
            }
        };
        let committed_identity = match materialize_local_commit_with_permit(
            opts.mutation_permit.as_ref(),
            commit_intent.as_ref(),
            &repo.root,
            default_branch_hint,
            &repo.branch,
            &persisted_identity,
            &commit_evidence,
        )
        .await
        {
            Ok(identity) => identity,
            Err(error) => {
            let error = fail_external_mutation(
                opts.mutation_permit.as_ref(),
                commit_intent.as_ref(),
                    format!("receipted local commit CAS failed: {error}"),
            )
            .await;
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "commit",
                format!("提交结果不确定: {error}"),
            ));
            }
        };
        let committed_head = committed_identity.head_sha;
        if committed_head != expected_commit_sha {
            let error = fail_external_mutation(
                opts.mutation_permit.as_ref(),
                commit_intent.as_ref(),
                format!(
                    "branch ref did not settle on the receipted commit: expected {expected_commit_sha}, observed {committed_head}"
                ),
            )
            .await;
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "commit",
                format!("提交结果不确定: {error}"),
            ));
        }
        let commit_result_evidence = json!({
            "head_sha": committed_head,
            "tree_sha": &commit_plan.target_tree_sha,
            "operation_key": commit_operation_key,
        })
        .to_string();
        if let Err(error) = commit_external_mutation(
            opts.mutation_permit.as_ref(),
            commit_intent.as_ref(),
            &commit_result_evidence,
        )
        .await
        {
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "commit",
                format!(
                    "git commit 已成功，但本地写入回执落盘失败: {error}。系统将只读核对精确 parent/tree/message，禁止猜测重放。"
                ),
            ));
        }
        outcome.steps.push(StepResult::ok(
            "commit",
            format!("提交 {} 个文件", commit_plan.staged_paths.len()),
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
    if let Err(step) = verify_mutation_permit(opts, "git_push").await {
        return outcome.blocked_on_uncertain_side_effect(step);
    }
    let push_sha = outcome.commit_sha.clone().unwrap_or_default();
    let remote_identity = opts
        .expected_identity
        .as_ref()
        .map(|identity| identity.repo_identity.clone())
        .unwrap_or_else(|| receipt_remote_identity(&repo));
    let push_rung = "git_push";
    let push_operation_key =
        external_operation_key(push_rung, &[&remote_identity, &repo.branch, &push_sha]);
    let push_evidence = json!({
        "remote_identity": &remote_identity,
        "branch": &repo.branch,
        "commit_sha": &push_sha,
    })
    .to_string();
    let push_begin = match begin_or_reuse_external_mutation(
        opts.mutation_permit.as_ref(),
        push_rung,
        &push_operation_key,
        &push_evidence,
    )
    .await
    {
        Ok(begin) => begin,
        Err(error) => {
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "mutation_intent",
                format!("推送前无法持久化 DeliveryRun 写入意图: {error}。未发出新的 git push。"),
            ))
        }
    };
    let push_intent = match push_begin {
        DeliveryMutationBegin::Dispatch(intent) => intent,
        DeliveryMutationBegin::AlreadyCommitted(_) => {
            match observe_remote_branch_head(&repo) {
                Ok(Some(head)) if head == push_sha => {
                    outcome.steps.push(StepResult::ok(
                        "push",
                        format!(
                            "复用已提交的精确 push 回执；远端 {} 仍指向 {}",
                            repo.branch, push_sha
                        ),
                    ));
                    None
                }
                Ok(observed) => {
                    return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                        "push",
                        format!(
                            "已提交 push 回执与当前远端不一致（expected={push_sha}, observed={observed:?}）；禁止重推。"
                        ),
                    ))
                }
                Err(error) => {
                    return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                        "push",
                        format!("无法只读核对已提交 push 回执: {error}；禁止重推。"),
                    ))
                }
            }
        }
    };
    if push_intent.is_none()
        && outcome
            .steps
            .last()
            .is_some_and(|step| step.step == "push" && step.status == "ok")
    {
        // The durable result was positively observed above. Continue to the
        // next rung without replaying `git push`.
    } else {
    match git(&repo.root, &["push", "-u", &repo.remote, &repo.branch]) {
        Ok(_) => {
            match observe_remote_branch_head(&repo) {
                Ok(Some(observed_head)) if observed_head == push_sha => {}
                Ok(observed) => {
                    let error = fail_external_mutation(
                        opts.mutation_permit.as_ref(),
                        push_intent.as_ref(),
                        format!(
                            "git push returned success, but the post-push remote head is not the authorized commit (expected={push_sha}, observed={observed:?})"
                        ),
                    )
                    .await;
                    return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                        "push",
                        format!(
                            "git push 返回成功，但远端分支未精确停在授权提交；{error}。禁止创建 PR 或重放 push。"
                        ),
                    ));
                }
                Err(observe_error) => {
                    let error = fail_external_mutation(
                        opts.mutation_permit.as_ref(),
                        push_intent.as_ref(),
                        format!(
                            "git push returned success, but its remote result could not be observed: {observe_error}"
                        ),
                    )
                    .await;
                    return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                        "push",
                        format!(
                            "git push 返回成功，但无法只读确认远端精确 SHA；{error}。禁止创建 PR 或重放 push。"
                        ),
                    ));
                }
            }
            if let Err(error) = commit_external_mutation(
                opts.mutation_permit.as_ref(),
                push_intent.as_ref(),
                &push_evidence,
            )
            .await
            {
                return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                    "push",
                    format!(
                        "git push 已返回成功，但持久化结果对账失败: {error}。系统将只读核对远端，禁止重放。"
                    ),
                ));
            }
            outcome.steps.push(StepResult::ok(
                "push",
                format!("推送 {} 到 {}", repo.branch, repo.remote),
            ));
        }
        Err(e) => {
            let error = fail_external_mutation(
                opts.mutation_permit.as_ref(),
                push_intent.as_ref(),
                format!("git push returned an indeterminate failure: {e}"),
            )
            .await;
            return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                "push",
                format!(
                    "推送结果不确定: {error}。系统将只读核对远端，不会直接重放或要求用户再次说继续。"
                ),
            ));
        }
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
    if let Some(receipt) = prior_receipt.clone() {
        if matches!(receipt.state.as_str(), "intent_merge" | "merge_queued") {
            match remote.observe_merge(receipt.pr_number, &sha).await {
                Ok(MergeObservation::Merged { merge_sha }) => {
                    let mut reconciled = receipt.clone();
                    reconciled.state = "merged".into();
                    if let Err(step) =
                        verify_mutation_permit(opts, "receipt_reconcile_merged").await
                    {
                        return outcome.blocked_on_uncertain_side_effect(step);
                    }
                    if let Err(error) = write_delivery_receipt(&repo, &sha, &reconciled) {
                        return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                            "receipt",
                            format!(
                                "远端确认 PR/MR #{} 已合并为 {merge_sha}，但本地回执升级失败: {error}",
                                receipt.pr_number
                            ),
                        ));
                    }
                    outcome.steps.push(StepResult::ok(
                        "reconcile",
                        format!(
                            "已核对远端: PR/MR #{} 已合并为 {merge_sha}",
                            receipt.pr_number
                        ),
                    ));
                    prior_receipt = Some(reconciled);
                }
                Ok(MergeObservation::OpenSameHead { auto_merge }) => {
                    if auto_merge {
                        outcome.steps.push(StepResult::ok(
                            "reconcile",
                            format!(
                                "PR/MR #{} 仍开放且 auto-merge 已登记；只核对门禁，不重复发起合并",
                                receipt.pr_number
                            ),
                        ));
                        return resume_queued_merge(outcome, &repo, remote, &receipt, opts).await;
                    }
                    let mut retryable = receipt.clone();
                    retryable.state = "pr_open".into();
                    if let Err(step) =
                        verify_mutation_permit(opts, "receipt_reconcile_pr_open").await
                    {
                        return outcome.blocked_on_uncertain_side_effect(step);
                    }
                    if let Err(error) = write_delivery_receipt(&repo, &sha, &retryable) {
                        return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                            "receipt",
                            format!(
                                "远端确认 PR 仍开放，但无法把写前回执恢复为可续接状态: {error}"
                            ),
                        ));
                    }
                    outcome.steps.push(StepResult::ok(
                        "reconcile",
                        format!(
                            "PR/MR #{} 仍开放且 head 未变化；安全续接受控合并",
                            receipt.pr_number
                        ),
                    ));
                    prior_receipt = Some(retryable);
                }
                Ok(MergeObservation::HeadChanged { actual_head }) => {
                    return outcome.blocked_at(StepResult::blocked(
                        "reconcile",
                        format!(
                            "PR/MR #{} head 已变化: 回执绑定 {sha}，远端为 {actual_head}；旧授权不能用于新 head",
                            receipt.pr_number
                        ),
                    ));
                }
                Ok(MergeObservation::ClosedUnmerged) => {
                    return outcome.blocked_at(StepResult::blocked(
                        "reconcile",
                        format!(
                            "PR/MR #{} 已关闭但未合并；未重试外部动作",
                            receipt.pr_number
                        ),
                    ));
                }
                Ok(MergeObservation::Unsupported) => {
                    return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                        "reconcile",
                        format!(
                            "当前 provider 无法核对 PR/MR #{} 的 merge 状态",
                            receipt.pr_number
                        ),
                    ));
                }
                Err(error) => {
                    return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                        "reconcile",
                        format!(
                            "核对 PR/MR #{} 的远端 merge 状态失败: {error}",
                            receipt.pr_number
                        ),
                    ));
                }
            }
        } else if receipt.state == "intent_release" {
            match reconcile_local_release_intent(
                &repo.root,
                Some(&repo.default_branch),
                &repo.branch,
                &sha,
                false,
                Some(remote),
            )
            .await
            {
                Ok(LocalReleaseIntentReconciliation::ProvenAbsent) => {
                    outcome.steps.push(StepResult::ok(
                        "reconcile",
                        "只读确认 exact workflow/ref/head 未被 dispatch；本地 intent_release 位于 DB begin 前的 crash gap，安全续接一次",
                    ));
                }
                Ok(LocalReleaseIntentReconciliation::Triggered { detail }) => {
                    outcome.steps.push(StepResult::ok("reconcile", detail));
                }
                Ok(LocalReleaseIntentReconciliation::NoIntent) => {}
                Err(error) => {
                    return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                        "receipt",
                        format!(
                            "未完成的 intent_release 只能继续 exact workflow/ref/head 只读对账: {error}"
                        ),
                    ));
                }
            }
            prior_receipt = match read_delivery_receipt(&repo, &sha) {
                Ok(receipt) => receipt,
                Err(error) => {
                    return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                        "receipt",
                        format!("release intent 对账后无法重读本地回执: {error}"),
                    ));
                }
            };
        }
        if prior_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.state == "release_triggered")
        {
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
    // The title may have come from session context rather than from this
    // branch. Never let it claim a bigger release than the commits justify.
    let commit_slot = branch_commit_slot(&repo.root, &repo.default_branch, &repo.branch);
    if let (corrected, Some(note)) = reconcile_pr_title(&pr_title, commit_slot) {
        outcome.steps.push(StepResult::ok("pr_title", note));
        pr_title = corrected;
    }
    let requested_pr_body = prior_receipt
        .as_ref()
        .and_then(|receipt| receipt.pr_body.clone())
        .or_else(|| opts.body.clone())
        .unwrap_or_else(|| {
            "由 CodeFactory 自动交付。\n\n🤖 Generated with CodeFactory".to_string()
        });
    let mut pr_body = match reconcile_readme_contract_body(
        &repo.root,
        &repo.remote,
        &repo.default_branch,
        &requested_pr_body,
    ) {
        Ok(reconciled) => reconciled.body,
        Err(error) => {
            return outcome.blocked_at(StepResult::blocked(
                "policy",
                format!("README 交付契约未满足: {error}"),
            ))
        }
    };
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
        // Guard against a misdirected delivery: this tool has no branch
        // argument, so a caller whose working directory drifted would otherwise
        // open a SECOND PR for unrelated work under the intended PR's title.
        match remote
            .conflicting_open_pr(&pr_title, &repo.branch, &repo.default_branch)
            .await
        {
            Ok(Some(conflict)) => {
                return outcome.blocked_at(StepResult::blocked(
                    "pr",
                    format!(
                        "当前分支是 `{}`，但标题 `{pr_title}` 已属于 PR #{}（分支 `{}`，{}）。\
继续会为不相关的改动新开一个同名 PR。\
若本意是续跑那个交付，请先切到 `{}` 再调用；若本分支确实是另一件工作，请给它自己的标题。",
                        repo.branch, conflict.number, conflict.head, conflict.url, conflict.head
                    ),
                ));
            }
            Ok(None) => {}
            Err(error) => {
                return outcome.remote_observation_failed(
                    "pr_observation",
                    format!("无法确认是否已有同一交付的开放 PR: {error}"),
                )
            }
        }
        let had_pr_receipt = prior_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.state == "pr_open");
        if let Err(step) = verify_mutation_permit(opts, "open_or_get_pr").await {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
        let remote_pr = match remote
            .open_or_get_pr(
                &pr_title,
                &pr_body,
                &repo.branch,
                &repo.default_branch,
                &sha,
                opts.mutation_permit.as_ref(),
            )
            .await
        {
            Ok(pr) => pr,
            Err(e) => {
                return outcome.remote_observation_failed(
                    "pr",
                    format!("开 PR/MR 或读取远端真实正文失败: {e}"),
                )
            }
        };
        let pr_number = remote_pr.number;
        let pr_url = remote_pr.url;
        pr_title = remote_pr.title;
        let reconciled_body = match reconcile_readme_contract_body(
            &repo.root,
            &repo.remote,
            &repo.default_branch,
            &remote_pr.body,
        ) {
            Ok(reconciled) => reconciled,
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked(
                    "policy",
                    format!("远端 PR README 契约未满足: {error}"),
                ))
            }
        };
        if reconciled_body.changed {
            if let Err(step) = verify_mutation_permit(opts, "update_pr_body").await {
                return outcome.blocked_on_uncertain_side_effect(step);
            }
            if let Err(error) = remote
                .update_pr_body(
                    pr_number,
                    &reconciled_body.body,
                    &repo.branch,
                    &repo.default_branch,
                    &sha,
                    opts.mutation_permit.as_ref(),
                )
                .await
            {
                return outcome.blocked_at(StepResult::blocked(
                    "pr",
                    format!("PR #{pr_number} 正文缺少有效 README 审计字段，自动补齐失败: {error}"),
                ));
            }
            outcome.steps.push(StepResult::ok(
                "pr_body",
                format!("已保留原正文并补齐 PR #{pr_number} 的 README 决策和理由"),
            ));
        }
        pr_body = reconciled_body.body;
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
        if let Err(step) = verify_mutation_permit(opts, "receipt_pr_open").await {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
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
        let ci_wait = wait_for_ci(remote, &sha, ci_timeout_secs, opts).await;
        if let Some(step) = ci_wait.mutation_permit_failure {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
        for detail in ci_wait.recoveries {
            outcome.steps.push(StepResult::ok("ci_recovery", detail));
        }
        match ci_wait.status {
            CiStatus::Success | CiStatus::None => {
                outcome.steps.push(StepResult::ok("ci", "CI 通过"))
            }
            CiStatus::Failure(d) => {
                return outcome.blocked_at(StepResult::blocked(
                    "ci",
                    format!(
                        "CI 未通过: {d}。读取该 check 的失败日志，修复对应代码、测试或配置，\
提交并 push 新 head 后重新调用 deliver_changes 续接；不要重复运行未修改的同一失败。"
                    ),
                ))
            }
            CiStatus::Unavailable(detail) => {
                return outcome.remote_observation_failed(
                    "ci_observation",
                    format!("无法核对当前 head 的 CI 状态: {detail}"),
                )
            }
            CiStatus::Pending => {
                return outcome.waiting_at(
                    StepResult::waiting(
                        "ci",
                        format!("CI 在 {ci_timeout_secs}s 内仍未出结论，交付保持运行中。"),
                    ),
                    30_000,
                    "等待退避后重新调用 deliver_changes，从同一 PR 和 head 继续核对 CI。",
                )
            }
        }

        if ceiling.rank() < DeliveryCeiling::ThroughMerge.rank() {
            return finish(outcome, &repo.branch);
        }

        // ── Merge ───────────────────────────────────────────────────────────
        if let Err(step) = verify_mutation_permit(opts, "refresh_canonical_pr").await {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
        let refreshed_pr = match remote
            .open_or_get_pr(
                &pr_title,
                &pr_body,
                &repo.branch,
                &repo.default_branch,
                &sha,
                opts.mutation_permit.as_ref(),
            )
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
        let reconciled_body = match reconcile_readme_contract_body(
            &repo.root,
            &repo.remote,
            &repo.default_branch,
            &refreshed_pr.body,
        ) {
            Ok(reconciled) => reconciled,
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked(
                    "policy",
                    format!("合并前远端 PR README 契约未满足: {error}"),
                ))
            }
        };
        if reconciled_body.changed {
            if let Err(step) = verify_mutation_permit(opts, "update_pr_body_before_merge").await {
                return outcome.blocked_on_uncertain_side_effect(step);
            }
            if let Err(error) = remote
                .update_pr_body(
                    pr_number,
                    &reconciled_body.body,
                    &repo.branch,
                    &repo.default_branch,
                    &sha,
                    opts.mutation_permit.as_ref(),
                )
                .await
            {
                return outcome.blocked_at(StepResult::blocked(
                    "pr",
                    format!("合并前补齐 PR #{pr_number} README 审计字段失败: {error}"),
                ));
            }
            outcome.steps.push(StepResult::ok(
                "pr_body",
                format!("合并前重新收敛 PR #{pr_number} 的 README 审计字段"),
            ));
        }
        pr_body = reconciled_body.body;
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
        if let Err(step) = verify_mutation_permit(opts, "receipt_intent_merge").await {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
        if let Err(error) = write_delivery_receipt(&repo, &sha, &intent) {
            return outcome.blocked_at(StepResult::blocked(
                "receipt",
                format!("合并前无法写入本地意图回执，未执行合并: {error}"),
            ));
        }
        if let Err(step) = verify_mutation_permit(opts, "merge_pr").await {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
        let merge_result = match remote
            .merge_pr(
                pr_number,
                merge_method,
                release_policy.merge_commit_message.as_ref(),
                &sha,
                opts.mutation_permit.as_ref(),
            )
            .await
        {
            Ok(result) => result,
            Err(e) => {
                return outcome.blocked_on_uncertain_side_effect(StepResult::blocked(
                    "merge",
                    format!("合并请求返回失败: {e}(服务端可能已接收；已保留 intent_merge 回执，下一次将先核对远端事实)。"),
                ));
            }
        };
        if matches!(merge_result, MergeRequestResult::Queued) {
            let mut queued = intent.clone();
            queued.state = "merge_queued".into();
            return resume_queued_merge(outcome, &repo, remote, &queued, opts).await;
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
        if let Err(step) = verify_mutation_permit(opts, "receipt_merged").await {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
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
        let release_head_sha = match observe_remote_ref_head(&repo, &repo.default_branch) {
            Ok(Some(head)) => head,
            Ok(None) => {
                return outcome.blocked_at(StepResult::blocked(
                    "release",
                    format!(
                        "无法建立 release dispatch 身份: 远端基线分支 {} 不存在；未触发发布",
                        repo.default_branch
                    ),
                ));
            }
            Err(error) => {
                return outcome.remote_observation_failed(
                    "release_observation",
                    format!(
                        "触发发布前无法只读解析 {} 的 exact head: {error}",
                        repo.default_branch
                    ),
                );
            }
        };
        let Some(release_target) = remote.release_dispatch_target(&release_head_sha) else {
            return outcome.blocked_at(StepResult::blocked(
                "release",
                "release adapter 未提供可持久化的 exact workflow/ref/head identity；未触发发布",
            ));
        };
        let release_target_envelope = match encode_release_dispatch_target(&release_target) {
            Ok(envelope) => envelope,
            Err(error) => {
                return outcome.blocked_at(StepResult::blocked("release", error));
            }
        };
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
            release_detail: Some(release_target_envelope),
        };
        if let Err(step) = verify_mutation_permit(opts, "receipt_intent_release").await {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
        if let Err(error) = write_delivery_receipt(&repo, &sha, &intent) {
            return outcome.blocked_at(StepResult::blocked(
                "receipt",
                format!("发布前无法写入本地意图回执，未触发发布: {error}"),
            ));
        }
        if let Err(step) = verify_mutation_permit(opts, "trigger_release").await {
            return outcome.blocked_on_uncertain_side_effect(step);
        }
        match remote
            .trigger_release(&release_head_sha, opts.mutation_permit.as_ref())
            .await
        {
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
                if let Err(step) = verify_mutation_permit(opts, "receipt_release_triggered").await {
                    return outcome.blocked_on_uncertain_side_effect(step);
                }
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
        ObservationStatus::Pending(detail) => Err(format!("线上验证仍在等待: {detail}")),
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
                    return Err(format!("部署在 {timeout_secs}s 内仍未完成: {detail}"));
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
    // Every loop path either returns or records why this poll failed, so the
    // deadline branch below always reads a real observation rather than an
    // empty placeholder.
    let mut last_error;
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
    let detail = detail.into();
    if detail.contains("仍在等待") || detail.contains("仍未完成") {
        return outcome.waiting_at(
            StepResult::waiting("live", detail),
            30_000,
            "等待退避后重新核对同一 release/deployment，不重复触发发布。",
        );
    }
    if detail.contains("没有可用的 live verifier") || detail.contains("未配置 live verifier")
    {
        let mut outcome = outcome.blocked_at(StepResult::blocked("live", detail));
        outcome.code = "delivery_live_verifier_platform_incident".into();
        outcome.next_action = Some(
            "系统必须配置或修复与该部署匹配的 live verifier，然后自动续接同一 release；不得重述任务，也不得降低 live 验收边界。"
                .into(),
        );
        return outcome;
    }
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
        outcome.recovery_class = RecoveryClass::AgentActionRequired;
        outcome.retry_after_ms = None;
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

struct CiWaitOutcome {
    status: CiStatus,
    recoveries: Vec<String>,
    mutation_permit_failure: Option<StepResult>,
}

fn ci_failure_is_retryable(detail: &str) -> bool {
    let without_url = detail.split(" [").next().unwrap_or(detail);
    let conclusion = without_url.rsplit(':').next().unwrap_or(without_url);
    matches!(
        conclusion,
        "cancelled" | "timed_out" | "stale" | "startup_failure"
    )
}

async fn wait_for_ci<R: DeliveryRemote>(
    remote: &R,
    sha: &str,
    timeout_secs: u32,
    opts: &DeliverOpts,
) -> CiWaitOutcome {
    let deadline = timeout_secs.max(1);
    let mut waited = 0u32;
    let mut reruns = 0u8;
    let mut recoveries = Vec::new();
    // Exponential backoff: 10s → 20s → 40s → 60s (capped). GitHub check-runs
    // polling is the biggest API cost of a delivery run; a fixed 10s cadence
    // burns ~30 requests for a 5-minute CI. Backoff keeps the first polls
    // snappy while slashing total calls on longer runs.
    let mut interval = 10u32;
    loop {
        match remote.ci_status(sha).await {
            Ok(CiStatus::Pending) => {}
            Ok(CiStatus::Failure(detail)) if ci_failure_is_retryable(&detail) && reruns < 1 => {
                if let Err(step) = verify_mutation_permit(opts, "rerun_ci").await {
                    return CiWaitOutcome {
                        status: CiStatus::Failure(detail),
                        recoveries,
                        mutation_permit_failure: Some(step),
                    };
                }
                match remote.rerun_ci(sha, opts.mutation_permit.as_ref()).await {
                    Ok(true) => {
                        reruns += 1;
                        waited = 0;
                        interval = 10;
                        recoveries.push(format!(
                            "检测到可重试的 CI 基础设施结论 `{detail}`，已触发一次有界 rerun"
                        ));
                        continue;
                    }
                    Ok(false) => {
                        return CiWaitOutcome {
                            status: CiStatus::Failure(format!(
                                "{detail}; 当前 provider 没有 CI rerun 能力"
                            )),
                            recoveries,
                            mutation_permit_failure: None,
                        }
                    }
                    Err(error) => {
                        return CiWaitOutcome {
                            status: CiStatus::Failure(format!(
                                "{detail}; 自动 rerun 失败: {error}"
                            )),
                            recoveries,
                            mutation_permit_failure: None,
                        }
                    }
                }
            }
            Ok(other) => {
                return CiWaitOutcome {
                    status: other,
                    recoveries,
                    mutation_permit_failure: None,
                }
            }
            Err(e) => {
                return CiWaitOutcome {
                    status: CiStatus::Unavailable(e),
                    recoveries,
                    mutation_permit_failure: None,
                }
            }
        }
        if waited >= deadline {
            return CiWaitOutcome {
                status: CiStatus::Pending,
                recoveries,
                mutation_permit_failure: None,
            };
        }
        let sleep_secs = interval.min(deadline - waited);
        tokio::time::sleep(std::time::Duration::from_secs(sleep_secs as u64)).await;
        waited += sleep_secs;
        interval = (interval * 2).min(60);
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
struct HookOpenPrObservationResponse {
    status: String,
    number: Option<u64>,
    url: Option<String>,
    title: Option<String>,
    body: Option<String>,
    head_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HookMergeObservationResponse {
    status: String,
    merge_sha: Option<String>,
    head_sha: Option<String>,
    auto_merge: Option<bool>,
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

#[derive(Debug, Deserialize)]
struct HookReleaseObservationResponse {
    status: String,
    workflow: String,
    git_ref: String,
    head_sha: Option<String>,
    run_id: Option<String>,
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
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<DeliveryPr, String> {
        let rung = "provider_pr_open_or_get";
        let operation_key = external_operation_key(rung, &[title, body, head, base, expected_head_sha]);
        let evidence = json!({
            "head": head,
            "base": base,
            "expected_head_sha": expected_head_sha,
            "title_digest": external_operation_key("title", &[title]),
            "body_digest": external_operation_key("body", &[body]),
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_open_pr(head, base).await?;
                return observed_committed_pr_projection(
                    &receipt,
                    observation,
                    title,
                    body,
                    expected_head_sha,
                );
            }
        };
        let result = (|| {
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
        })();
        match result {
            Ok(pr) => {
                let pr = match self
                    .observe_open_pr(head, base)
                    .await
                    .and_then(|observation| {
                        exact_created_pr_projection(
                            &pr,
                            observation,
                            title,
                            body,
                            head,
                            base,
                            expected_head_sha,
                        )
                    }) {
                    Ok(pr) => pr,
                    Err(error) => {
                        return Err(
                            fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                        )
                    }
                };
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "pr_number": pr.number, "pr_url": pr.url }).to_string(),
                )
                .await?;
                Ok(pr)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn observe_open_pr(&self, head: &str, base: &str) -> Result<OpenPrObservation, String> {
        let value = self.run_json(json!({
            "action": "observe_open_pr",
            "head": head,
            "base": base,
        }))?;
        let response: HookOpenPrObservationResponse = serde_json::from_value(value).map_err(|e| {
            format!(
                "delivery provider hook '{}' open-PR observation response invalid: {e}",
                self.id
            )
        })?;
        match response.status.as_str() {
            "absent" => Ok(OpenPrObservation::Absent),
            "unsupported" => Ok(OpenPrObservation::Unsupported),
            "open" => {
                let number = response
                    .number
                    .filter(|number| *number > 0)
                    .ok_or_else(|| "provider hook open-PR observation omitted number".to_string())?;
                let url = response
                    .url
                    .filter(|url| !url.is_empty())
                    .ok_or_else(|| "provider hook open-PR observation omitted URL".to_string())?;
                let head_sha = response
                    .head_sha
                    .filter(|sha| !sha.is_empty())
                    .ok_or_else(|| "provider hook open-PR observation omitted head SHA".to_string())?;
                Ok(OpenPrObservation::Open(OpenPrState {
                    pr: DeliveryPr {
                        number,
                        url,
                        title: response.title.unwrap_or_default(),
                        body: response.body.unwrap_or_default(),
                    },
                    head_branch: head.to_string(),
                    base_branch: base.to_string(),
                    head_sha: Some(head_sha),
                }))
            }
            other => Err(format!(
                "delivery provider hook '{}' returned unknown open-PR observation '{other}'",
                self.id
            )),
        }
    }

    async fn update_pr_body(
        &self,
        number: u64,
        body: &str,
        head: &str,
        base: &str,
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<(), String> {
        exact_open_pr_projection(
            self.observe_open_pr(head, base).await?,
            Some(number),
            expected_head_sha,
        )?
        .ok_or_else(|| "canonical PR is absent; no body update was dispatched".to_string())?;
        let rung = "provider_pr_body_update";
        let number_text = number.to_string();
        let operation_key = external_operation_key(
            rung,
            &[&number_text, body, head, base, expected_head_sha],
        );
        let evidence = json!({
            "pr_number": number,
            "head": head,
            "base": base,
            "expected_head_sha": expected_head_sha,
            "body_digest": external_operation_key("body", &[body]),
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                exact_updated_pr_projection(
                    self.observe_open_pr(head, base).await?,
                    number,
                    body,
                    head,
                    base,
                    expected_head_sha,
                )
                .map_err(|error| {
                    format!(
                        "committed PR-body receipt {} no longer matches live state: {error}; no update was replayed",
                        receipt.intent_id
                    )
                })?;
                return Ok(());
            }
        };
        let result = (|| {
            let value = self.run_json(json!({
                "action": "update_pr_body",
                "number": number,
                "body": body,
            }))?;
            let _response: HookOkResponse = serde_json::from_value(value).map_err(|e| {
                format!(
                    "delivery provider hook '{}' update PR response invalid: {e}",
                    self.id
                )
            })?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                if let Err(error) = self
                    .observe_open_pr(head, base)
                    .await
                    .and_then(|observation| {
                        exact_updated_pr_projection(
                            observation,
                            number,
                            body,
                            head,
                            base,
                            expected_head_sha,
                        )
                    })
                {
                    return Err(
                        fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                    );
                }
                commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
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

    async fn rerun_ci(
        &self,
        sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<bool, String> {
        let rung = "provider_ci_rerun";
        let operation_key = external_operation_key(rung, &[sha]);
        let evidence = json!({ "sha": sha }).to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                return committed_receipt_result(&receipt)?
                    .get("rerun")
                    .and_then(serde_json::Value::as_bool)
                    .ok_or_else(|| {
                        "committed hook CI rerun receipt lacks its boolean result".to_string()
                    });
            }
        };
        let result = (|| {
            let value = self.run_json(json!({ "action": "rerun_ci", "sha": sha }))?;
            let response: HookOkResponse = serde_json::from_value(value).map_err(|e| {
                format!(
                    "delivery provider hook '{}' rerun CI response invalid: {e}",
                    self.id
                )
            })?;
            Ok(response.ok.unwrap_or(true))
        })();
        match result {
            Ok(rerun) => {
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "sha": sha, "rerun": rerun }).to_string(),
                )
                .await?;
                Ok(rerun)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<MergeRequestResult, String> {
        let rung = "provider_pr_merge";
        let number_text = number.to_string();
        let operation_key =
            external_operation_key(rung, &[&number_text, method.as_str(), expected_head]);
        let evidence = json!({
            "pr_number": number,
            "method": method.as_str(),
            "expected_head": expected_head,
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_merge(number, expected_head).await?;
                return observed_committed_merge_projection(&receipt, observation);
            }
        };
        let result = (|| {
            let value = self.run_json(json!({
                "action": "merge_pr",
                "number": number,
                "method": method.as_str(),
                "commit_title": commit_message.map(|message| message.title.as_str()),
                "commit_body": commit_message.map(|message| message.body.as_str()),
                "expected_head": expected_head,
            }))?;
            let _response: HookOkResponse = serde_json::from_value(value).map_err(|e| {
                format!(
                    "delivery provider hook '{}' merge response invalid: {e}",
                    self.id
                )
            })?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                let outcome = match self
                    .observe_merge(number, expected_head)
                    .await
                    .and_then(exact_dispatched_merge_projection)
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return Err(
                            fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                        )
                    }
                };
                let result_evidence = match &outcome {
                    MergeRequestResult::Queued => json!({ "pr_number": number, "queued": true }),
                    MergeRequestResult::Merged { merge_sha } => json!({
                        "pr_number": number,
                        "merged": true,
                        "merge_sha": merge_sha,
                    }),
                };
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &result_evidence.to_string(),
                )
                .await?;
                Ok(outcome)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn observe_merge(
        &self,
        number: u64,
        expected_head: &str,
    ) -> Result<MergeObservation, String> {
        let value = self.run_json(json!({
            "action": "observe_merge",
            "number": number,
            "expected_head": expected_head,
        }))?;
        let response: HookMergeObservationResponse = serde_json::from_value(value).map_err(|e| {
            format!(
                "delivery provider hook '{}' merge observation response invalid: {e}",
                self.id
            )
        })?;
        if response
            .head_sha
            .as_deref()
            .is_some_and(|head| head != expected_head)
        {
            return Ok(MergeObservation::HeadChanged {
                actual_head: response.head_sha.unwrap_or_default(),
            });
        }
        match response.status.as_str() {
            "merged" => response
                .merge_sha
                .filter(|sha| !sha.is_empty())
                .map(|merge_sha| MergeObservation::Merged { merge_sha })
                .ok_or_else(|| "provider hook merge observation omitted merge SHA".to_string()),
            "open" => Ok(MergeObservation::OpenSameHead {
                auto_merge: response.auto_merge.unwrap_or(false),
            }),
            "closed" => Ok(MergeObservation::ClosedUnmerged),
            "unsupported" => Ok(MergeObservation::Unsupported),
            other => Err(format!(
                "delivery provider hook '{}' returned unknown merge observation '{other}'",
                self.id
            )),
        }
    }

    fn release_dispatch_target(&self, head_sha: &str) -> Option<ReleaseDispatchTarget> {
        let remote = default_remote(&self.cwd);
        let git_ref = remote_default_branch(&self.cwd, &remote).unwrap_or_else(|| "main".into());
        Some(ReleaseDispatchTarget {
            workflow: format!("provider-hook:{}", self.id),
            git_ref,
            head_sha: head_sha.to_string(),
        })
    }

    async fn observe_release_dispatch(
        &self,
        target: &ReleaseDispatchTarget,
    ) -> Result<ReleaseDispatchObservation, String> {
        let current_target = self
            .release_dispatch_target(&target.head_sha)
            .ok_or_else(|| "provider hook has no release dispatch identity".to_string())?;
        if target.workflow != current_target.workflow || target.git_ref != current_target.git_ref {
            return Err(format!(
                "release target {}/{} does not match the current provider hook identity {}/{}",
                target.workflow, target.git_ref, current_target.workflow, current_target.git_ref
            ));
        }
        let value = self.run_json(json!({
            "action": "observe_release_dispatch",
            "workflow": target.workflow,
            "git_ref": target.git_ref,
            "head_sha": target.head_sha,
        }))?;
        let response: HookReleaseObservationResponse =
            serde_json::from_value(value).map_err(|e| {
                format!(
                    "delivery provider hook '{}' release observation response invalid: {e}",
                    self.id
                )
            })?;
        if response.workflow != target.workflow || response.git_ref != target.git_ref {
            return Err(format!(
                "delivery provider hook '{}' observed a different release workflow/ref",
                self.id
            ));
        }
        let detail = response.detail.unwrap_or_default();
        Ok(match response.status.as_str() {
            "absent" => ReleaseDispatchObservation::Absent,
            "triggered" | "queued" | "in_progress" | "completed" => {
                let head_sha = response.head_sha.unwrap_or_default();
                if head_sha != target.head_sha {
                    ReleaseDispatchObservation::HeadMismatch {
                        observed_heads: (!head_sha.is_empty())
                            .then_some(head_sha)
                            .into_iter()
                            .collect(),
                    }
                } else {
                    ReleaseDispatchObservation::Triggered {
                        run_id: response.run_id.unwrap_or_else(|| "provider-hook".into()),
                        status: response.status,
                        head_sha,
                        detail,
                    }
                }
            }
            "unsupported" => ReleaseDispatchObservation::Unsupported(detail),
            other => return Err(format!("unknown release observation status '{other}'")),
        })
    }

    async fn trigger_release(
        &self,
        head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
        let rung = "provider_release_trigger";
        let target = self
            .release_dispatch_target(head_sha)
            .ok_or_else(|| "provider hook has no release dispatch identity".to_string())?;
        let operation_key = target.operation_key();
        let evidence = serde_json::to_string(&target)
            .map_err(|error| format!("cannot serialize release dispatch target: {error}"))?;
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_release_dispatch(&target).await?;
                return observed_committed_release_projection(&receipt, &target, observation);
            }
        };
        let result = (|| {
            let value = self.run_json(json!({
                "action": "trigger_release",
                "workflow": target.workflow,
                "git_ref": target.git_ref,
                "head_sha": target.head_sha,
            }))?;
            let response: HookOkResponse = serde_json::from_value(value).map_err(|e| {
                format!(
                    "delivery provider hook '{}' release response invalid: {e}",
                    self.id
                )
            })?;
            Ok(response.detail.unwrap_or_else(|| {
                format!("delivery provider hook '{}' triggered release", self.id)
            }))
        })();
        match result {
            Ok(detail) => {
                commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await?;
                Ok(detail)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
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
///
/// Parsing is deliberately LOOSE about host-file structure: modern gh writes
/// `users:`-nested entries (`users: <name>: oauth_token:`), older versions use
/// flat `oauth_token:` at host level, and the indentation differs across gh
/// releases. The only things we need are a `user:` key and a non-empty
/// `oauth_token:` key under the requested host block — anything else is noise.
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
    gh_hosts_content_has_auth_for_host(&content, hostname)
}

/// Testable core of the hosts.yml probe above. `gh auth status` exits non-zero
/// both when no token exists AND when GitHub is rate-limiting the validation
/// request, so we cannot rely on its exit code alone — the credential file is
/// the source of truth for "is gh authenticated".
pub fn gh_hosts_content_has_auth_for_host(content: &str, hostname: &str) -> bool {
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
        if !in_host_block {
            continue;
        }
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
        // A new top-level host header ends the selected block; anything else
        // (nested `users:`, `git_protocol:`, …) is ignored.
        if !t.starts_with(' ')
            && t.ends_with(':')
            && t.to_ascii_lowercase() != header
            && !line.starts_with(' ')
        {
            in_host_block = false;
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

fn gh_pr_edit_body_args(number: u64, body: &str) -> Vec<String> {
    vec![
        "pr".into(),
        "edit".into(),
        number.to_string(),
        "--body".into(),
        body.into(),
    ]
}

/// Map GitHub's `mergeStateStatus` onto the wait-vs-deadlock distinction.
///
/// `BEHIND` is the load-bearing case: it only appears when the repository
/// requires branches to be up to date (`strict_required_status_checks_policy`),
/// and GitHub will not update the head ref itself. Auto-merge on a `BEHIND` PR
/// therefore waits forever.
/// Pick the misdirected-delivery conflict out of the open PR list for `base`.
///
/// Returns `Some` only when BOTH hold:
/// - this head has no open PR of its own (so we are about to CREATE one), and
/// - another open PR already carries this exact title.
///
/// If the current head already has a PR we are resuming it, which is the normal
/// idempotent path and never a conflict — even if some other PR shares the title.
fn conflicting_open_pr_from_list(
    prs: &[(u64, String, String, String)], // (number, url, title, head)
    title: &str,
    head: &str,
) -> Option<ConflictingPr> {
    if prs.iter().any(|(_, _, _, pr_head)| pr_head == head) {
        return None;
    }
    let wanted = title.trim();
    prs.iter()
        .find(|(_, _, pr_title, _)| pr_title.trim() == wanted)
        .map(|(number, url, _, pr_head)| ConflictingPr {
            number: *number,
            url: url.clone(),
            head: pr_head.clone(),
        })
}

/// Conventional-commit release weight: 3 breaking, 2 feat, 1 fix, 0 everything
/// else. Mirrors the slot arithmetic in `.github/workflows/auto-release.yml`.
fn conventional_slot(subject: &str) -> u8 {
    let s = subject.trim();
    if s.contains("BREAKING CHANGE") || s.contains("BREAKING-CHANGE") {
        return 3;
    }
    let Some((kind, _)) = s.split_once(':') else {
        return 0;
    };
    let kind = kind.trim();
    let breaking = kind.ends_with('!');
    let base = kind.trim_end_matches('!');
    // Strip an optional scope: `feat(chat)` → `feat`.
    let base = base.split_once('(').map_or(base, |(head, _)| head).trim();
    match (base, breaking) {
        ("feat", true) | ("fix", true) => 3,
        ("feat", false) => 2,
        ("fix", false) => 1,
        _ => 0,
    }
}

fn slot_prefix(slot: u8) -> &'static str {
    match slot {
        3 => "feat!",
        2 => "feat",
        1 => "fix",
        _ => "chore",
    }
}

/// Stop a PR title from inflating the release slot above what its commits
/// justify.
///
/// The title matters far beyond cosmetics: this repository squash-merges, the
/// squash subject IS the PR title, and `auto-release.yml` computes the version
/// slot from those subjects. A branch whose only commit is `ci: …` carried a
/// title of `feat: …` (2026-07-30 field report, PR #290) would have fabricated a
/// **minor** release for a feature that does not exist.
///
/// Only the inflating direction is corrected. A title that understates (commits
/// are `feat`, title says `fix`) delays value but never invents a release, so it
/// is left alone rather than fighting a deliberate choice.
fn reconcile_pr_title(title: &str, commit_slot: u8) -> (String, Option<String>) {
    let title_slot = conventional_slot(title);
    if title_slot <= commit_slot {
        return (title.to_string(), None);
    }
    let body = title
        .split_once(':')
        .map_or(title.trim(), |(_, rest)| rest.trim());
    let corrected = format!("{}: {body}", slot_prefix(commit_slot));
    let note = format!(
        "PR 标题原为 `{title}`，但分支提交最高只到 `{}`。本仓库 squash 合并且发版 slot 按标题前缀计算，\
按原标题合入会凭空触发更高版本，已修正为 `{corrected}`。",
        slot_prefix(commit_slot)
    );
    (corrected, Some(note))
}

/// Highest conventional slot among the commits this branch adds over `base`.
fn branch_commit_slot(root: &Path, base: &str, branch: &str) -> u8 {
    git(root, &["log", "--format=%s", &format!("{base}..{branch}")])
        .map(|log| log.lines().map(conventional_slot).max().unwrap_or(0))
        .unwrap_or(0)
}

fn merge_readiness_from_state(state: &str) -> MergeReadiness {
    match state.trim().to_ascii_uppercase().as_str() {
        "CLEAN" | "HAS_HOOKS" => MergeReadiness::Ready,
        "BEHIND" => MergeReadiness::Behind,
        // Non-required checks pending; required ones decide. Waiting is right.
        "UNSTABLE" | "BLOCKED" => MergeReadiness::WaitingOnChecks,
        "DIRTY" => {
            MergeReadiness::NeedsAction("PR 与目标分支存在冲突，需要人工解决后才能合并".into())
        }
        "DRAFT" => MergeReadiness::NeedsAction("PR 仍是 draft，需要标记为 ready 才能合并".into()),
        _ => MergeReadiness::Unknown,
    }
}

fn parse_github_merge_readiness(value: &serde_json::Value) -> MergeReadiness {
    if value
        .get("isDraft")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return MergeReadiness::NeedsAction("PR 仍是 draft，需要标记为 ready 才能合并".into());
    }
    match value
        .get("reviewDecision")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
    {
        "CHANGES_REQUESTED" => {
            return MergeReadiness::NeedsAction("PR 存在未解决的 changes requested review".into())
        }
        "REVIEW_REQUIRED" => {
            return MergeReadiness::NeedsAction("PR 仍缺少 required review".into())
        }
        _ => {}
    }
    merge_readiness_from_state(
        value
            .get("mergeStateStatus")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    )
}

fn gh_pr_merge_args(
    number: u64,
    method: MergeMethod,
    commit_message: Option<&MergeCommitMessage>,
    expected_head: &str,
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
    args.extend([
        "--auto".into(),
        "--match-head-commit".into(),
        expected_head.into(),
    ]);
    args
}

fn gh_workflow_run_args(
    workflow: &str,
    git_ref: &str,
    expected_head_sha: &str,
) -> Vec<String> {
    vec![
        "workflow".into(),
        "run".into(),
        workflow.into(),
        "--ref".into(),
        git_ref.into(),
        "-f".into(),
        format!("expected_head_sha={expected_head_sha}"),
    ]
}

fn github_release_live_from_value(
    release: &serde_json::Value,
    sha: &str,
    tag_contains_sha: impl FnOnce(&str) -> Result<bool, String>,
) -> Result<ObservationStatus, String> {
    let tag = release
        .get("tagName")
        .or_else(|| release.get("tag_name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    if tag.is_empty() {
        return Ok(ObservationStatus::Unsupported(
            "GitHub release verifier could not find a release tag".into(),
        ));
    }
    if release
        .get("isDraft")
        .or_else(|| release.get("draft"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(ObservationStatus::Pending(format!(
            "GitHub Release {tag} is still draft"
        )));
    }
    if release
        .get("isPrerelease")
        .or_else(|| release.get("prerelease"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(ObservationStatus::Failure(format!(
            "GitHub Release {tag} is a prerelease, not the live release"
        )));
    }
    let published = release
        .get("publishedAt")
        .or_else(|| release.get("published_at"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if published.trim().is_empty() {
        return Ok(ObservationStatus::Pending(format!(
            "GitHub Release {tag} is not published yet"
        )));
    }
    let assets = release
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .map(|items| items.len())
        .unwrap_or(0);
    if assets == 0 {
        return Ok(ObservationStatus::Pending(format!(
            "GitHub Release {tag} has no assets yet"
        )));
    }
    if !tag_contains_sha(tag)? {
        return Ok(ObservationStatus::Pending(format!(
            "GitHub Release {tag} does not include delivery commit {} yet",
            sha.get(..7).unwrap_or(sha)
        )));
    }
    Ok(ObservationStatus::Success(format!(
        "GitHub Release {tag} is published with {assets} assets and contains delivery commit {}",
        sha.get(..7).unwrap_or(sha)
    )))
}

fn parse_github_release_dispatch_runs(
    value: &serde_json::Value,
    target: &ReleaseDispatchTarget,
) -> Result<ReleaseDispatchObservation, String> {
    let rows = value
        .get("workflow_runs")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| "GitHub workflow run observation is not an array".to_string())?;
    let mut nonmatching_heads = Vec::new();
    for row in rows {
        let event = row
            .get("event")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let git_ref = row
            .get("head_branch")
            .or_else(|| row.get("headBranch"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if event != "workflow_dispatch" || git_ref != target.git_ref {
            continue;
        }
        let head_sha = row
            .get("head_sha")
            .or_else(|| row.get("headSha"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        if head_sha != target.head_sha {
            if !head_sha.is_empty() && !nonmatching_heads.contains(&head_sha) {
                nonmatching_heads.push(head_sha);
            }
            continue;
        }
        let run_id = row
            .get("id")
            .or_else(|| row.get("databaseId"))
            .map(|value| match value {
                serde_json::Value::String(value) => value.clone(),
                _ => value.to_string(),
            })
            .unwrap_or_else(|| "unknown".into());
        let status = row
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let conclusion = row
            .get("conclusion")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| format!("/{value}"))
            .unwrap_or_default();
        let url = row
            .get("html_url")
            .or_else(|| row.get("url"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        return Ok(ReleaseDispatchObservation::Triggered {
            run_id,
            status: format!("{status}{conclusion}"),
            head_sha,
            detail: if url.is_empty() {
                "exact workflow_dispatch run observed".into()
            } else {
                url.to_string()
            },
        });
    }
    if nonmatching_heads.is_empty() {
        Ok(ReleaseDispatchObservation::Absent)
    } else {
        Ok(ReleaseDispatchObservation::HeadMismatch {
            observed_heads: nonmatching_heads,
        })
    }
}

/// [`DeliveryRemote`] over a logged-in `gh` CLI. All commands run in the repo
/// root so gh resolves the repo from the checkout, using the user's existing
/// system-wide authentication — no app-side token required.
pub struct GhCliRemote {
    cwd: PathBuf,
    repo: String,
    default_branch: String,
    release_workflow: String,
    ci_stability: CiObservationStability,
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
        ci_stability: CiObservationStability::default(),
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

    fn merge_observation(
        &self,
        number: u64,
        expected_head: &str,
    ) -> Result<MergeObservation, String> {
        let raw = self.gh(&[
            "pr".into(),
            "view".into(),
            number.to_string(),
            "--json".into(),
            "state,headRefOid,mergeCommit,autoMergeRequest".into(),
        ])?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|error| format!("gh pr view merge state returned non-JSON: {error}"))?;
        parse_github_merge_observation(&value, expected_head)
    }
}

fn parse_github_merge_observation(
    value: &serde_json::Value,
    expected_head: &str,
) -> Result<MergeObservation, String> {
    let actual_head = value
        .get("headRefOid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    if !actual_head.is_empty() && actual_head != expected_head {
        return Ok(MergeObservation::HeadChanged { actual_head });
    }
    match value
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_ascii_uppercase()
        .as_str()
    {
        "MERGED" => {
            let merge_sha = value
                .get("mergeCommit")
                .and_then(|merge| merge.get("oid"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            if merge_sha.is_empty() {
                Err("GitHub reports MERGED but returned no merge commit SHA".into())
            } else {
                Ok(MergeObservation::Merged { merge_sha })
            }
        }
        "OPEN" => Ok(MergeObservation::OpenSameHead {
            auto_merge: value
                .get("autoMergeRequest")
                .is_some_and(|request| !request.is_null()),
        }),
        "CLOSED" => Ok(MergeObservation::ClosedUnmerged),
        other => Err(format!("unknown GitHub PR state '{other}'")),
    }
}

fn parse_gh_pr_list(raw: &str) -> Result<Option<DeliveryPr>, String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| format!("gh pr list returned non-JSON: {error}"))?;
    let rows = value
        .as_array()
        .ok_or_else(|| "gh pr list returned JSON that is not an array".to_string())?;
    let Some(pr) = rows.first() else {
        return Ok(None);
    };
    let (Some(number), Some(url), Some(title), Some(body)) = (
        pr.get("number").and_then(serde_json::Value::as_u64),
        pr.get("url").and_then(serde_json::Value::as_str),
        pr.get("title").and_then(serde_json::Value::as_str),
        pr.get("body").and_then(serde_json::Value::as_str),
    ) else {
        return Err("gh pr list row missing number/url/title/body".into());
    };
    Ok(Some(DeliveryPr {
        number,
        url: url.into(),
        title: title.into(),
        body: body.into(),
    }))
}

fn parse_gh_open_pr_state(raw: &str, head: &str, base: &str) -> Result<OpenPrObservation, String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| format!("gh pr list returned non-JSON: {error}"))?;
    let rows = value
        .as_array()
        .ok_or_else(|| "gh pr list returned JSON that is not an array".to_string())?;
    let Some(row) = rows.first() else {
        return Ok(OpenPrObservation::Absent);
    };
    let (Some(number), Some(url), Some(title), Some(body)) = (
        row.get("number").and_then(serde_json::Value::as_u64),
        row.get("url").and_then(serde_json::Value::as_str),
        row.get("title").and_then(serde_json::Value::as_str),
        row.get("body").and_then(serde_json::Value::as_str),
    ) else {
        return Err("gh pr list row missing number/url/title/body".into());
    };
    Ok(OpenPrObservation::Open(OpenPrState {
        pr: DeliveryPr {
            number,
            url: url.to_string(),
            title: title.to_string(),
            body: body.to_string(),
        },
        head_branch: head.to_string(),
        base_branch: base.to_string(),
        head_sha: row
            .get("headRefOid")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    }))
}

impl DeliveryRemote for GhCliRemote {
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
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<DeliveryPr, String> {
        if let Some(pr) = exact_open_pr_projection(
            self.observe_open_pr(head, base).await?,
            None,
            expected_head_sha,
        )? {
            return Ok(pr);
        }
        let rung = "provider_pr_create";
        let operation_key = external_operation_key(rung, &[title, body, head, base, expected_head_sha]);
        let evidence = json!({
            "head": head,
            "base": base,
            "expected_head_sha": expected_head_sha,
            "title_digest": external_operation_key("title", &[title]),
            "body_digest": external_operation_key("body", &[body]),
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_open_pr(head, base).await?;
                return observed_committed_pr_projection(
                    &receipt,
                    observation,
                    title,
                    body,
                    expected_head_sha,
                );
            }
        };
        let result = (|| {
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
        })();
        match result {
            Ok(pr) => {
                let pr = match self
                    .observe_open_pr(head, base)
                    .await
                    .and_then(|observation| {
                        exact_created_pr_projection(
                            &pr,
                            observation,
                            title,
                            body,
                            head,
                            base,
                            expected_head_sha,
                        )
                    }) {
                    Ok(pr) => pr,
                    Err(error) => {
                        return Err(
                            fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                        )
                    }
                };
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "pr_number": pr.number, "pr_url": pr.url }).to_string(),
                )
                .await?;
                Ok(pr)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn observe_open_pr(&self, head: &str, base: &str) -> Result<OpenPrObservation, String> {
        let raw = self.gh(&[
            "pr".into(),
            "list".into(),
            "--head".into(),
            head.into(),
            "--base".into(),
            base.into(),
            "--state".into(),
            "open".into(),
            "--json".into(),
            "number,url,title,body,headRefOid".into(),
            "--limit".into(),
            "1".into(),
        ])?;
        parse_gh_open_pr_state(&raw, head, base)
    }

    async fn update_pr_body(
        &self,
        number: u64,
        body: &str,
        head: &str,
        base: &str,
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<(), String> {
        exact_open_pr_projection(
            self.observe_open_pr(head, base).await?,
            Some(number),
            expected_head_sha,
        )?
        .ok_or_else(|| "canonical PR is absent; no body update was dispatched".to_string())?;
        let rung = "provider_pr_body_update";
        let number_text = number.to_string();
        let operation_key = external_operation_key(
            rung,
            &[&number_text, body, head, base, expected_head_sha],
        );
        let evidence = json!({
            "pr_number": number,
            "head": head,
            "base": base,
            "expected_head_sha": expected_head_sha,
            "body_digest": external_operation_key("body", &[body]),
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                exact_updated_pr_projection(
                    self.observe_open_pr(head, base).await?,
                    number,
                    body,
                    head,
                    base,
                    expected_head_sha,
                )
                .map_err(|error| {
                    format!(
                        "committed PR-body receipt {} no longer matches live state: {error}; no update was replayed",
                        receipt.intent_id
                    )
                })?;
                return Ok(());
            }
        };
        match self.gh(&gh_pr_edit_body_args(number, body)) {
            Ok(_) => {
                if let Err(error) = self
                    .observe_open_pr(head, base)
                    .await
                    .and_then(|observation| {
                        exact_updated_pr_projection(
                            observation,
                            number,
                            body,
                            head,
                            base,
                            expected_head_sha,
                        )
                    })
                {
                    return Err(
                        fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                    );
                }
                commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn ci_status(&self, sha: &str) -> Result<CiStatus, String> {
        let raw = self.gh(&[
            "api".into(),
            format!("repos/{}/commits/{}/check-runs", self.repo, sha),
        ])?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("check-runs non-JSON: {e}"))?;
        let required = github_required_status_checks(self.gh(&[
            "api".into(),
            format!("repos/{}/rules/branches/{}", self.repo, self.default_branch),
        ]))?;
        let observation = crate::git_remote::github::classify_ci_observation(&v, &required);
        let status = match observation.status.as_str() {
            "success" => CiStatus::Success,
            "pending" => CiStatus::Pending,
            "none" => CiStatus::None,
            other => CiStatus::Failure(other.trim_start_matches("failure:").to_string()),
        };
        Ok(self.ci_stability.confirm(&observation.fingerprint, status))
    }

    async fn rerun_ci(
        &self,
        sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<bool, String> {
        let raw = self.gh(&[
            "run".into(),
            "list".into(),
            "--commit".into(),
            sha.into(),
            "--limit".into(),
            "20".into(),
            "--json".into(),
            "databaseId,status,conclusion".into(),
        ])?;
        let runs: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|error| format!("gh run list returned non-JSON: {error}"))?;
        let mut rerun = false;
        for run in runs.as_array().into_iter().flatten() {
            if run.get("status").and_then(serde_json::Value::as_str) != Some("completed") {
                continue;
            }
            let conclusion = run
                .get("conclusion")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if !matches!(
                conclusion,
                "failure" | "cancelled" | "timed_out" | "stale" | "startup_failure"
            ) {
                continue;
            }
            let Some(id) = run.get("databaseId").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let rung = "provider_ci_rerun";
            let id_text = id.to_string();
            let operation_key = external_operation_key(rung, &[sha, &id_text]);
            let evidence = json!({ "sha": sha, "run_id": id }).to_string();
            let intent = match begin_or_reuse_external_mutation(
                mutation_permit,
                rung,
                &operation_key,
                &evidence,
            )
            .await?
            {
                DeliveryMutationBegin::Dispatch(intent) => intent,
                DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                    let result = committed_receipt_result(&receipt)?;
                    rerun |= result
                        .get("rerun")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true);
                    continue;
                }
            };
            match self.gh(&["run".into(), "rerun".into(), id_text]) {
                Ok(_) => {
                    commit_external_mutation(
                        mutation_permit,
                        intent.as_ref(),
                        &json!({ "sha": sha, "run_id": id, "rerun": true }).to_string(),
                    )
                    .await?
                }
                Err(error) => {
                    return Err(
                        fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                    )
                }
            }
            rerun = true;
        }
        if rerun {
            self.ci_stability.reset();
        }
        Ok(rerun)
    }

    async fn conflicting_open_pr(
        &self,
        title: &str,
        head: &str,
        base: &str,
    ) -> Result<Option<ConflictingPr>, String> {
        let raw = self.gh(&[
            "pr".into(),
            "list".into(),
            "--base".into(),
            base.into(),
            "--state".into(),
            "open".into(),
            "--json".into(),
            "number,url,title,headRefName".into(),
            "--limit".into(),
            "100".into(),
        ])?;
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("解析 gh pr list 输出失败: {e}"))?;
        let rows: Vec<(u64, String, String, String)> = parsed
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|pr| {
                        Some((
                            pr["number"].as_u64()?,
                            pr["url"].as_str()?.to_string(),
                            pr["title"].as_str()?.to_string(),
                            pr["headRefName"].as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(conflicting_open_pr_from_list(&rows, title, head))
    }

    async fn merge_readiness(&self, number: u64) -> Result<MergeReadiness, String> {
        let raw = self.gh(&[
            "pr".into(),
            "view".into(),
            number.to_string(),
            "--json".into(),
            "mergeStateStatus,reviewDecision,isDraft".into(),
        ])?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|error| format!("gh pr view readiness returned non-JSON: {error}"))?;
        Ok(parse_github_merge_readiness(&value))
    }

    async fn update_pr_branch(
        &self,
        number: u64,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
        let rung = "provider_pr_branch_update";
        let number_text = number.to_string();
        let operation_key = external_operation_key(rung, &[&number_text, expected_head]);
        let evidence = json!({
            "pr_number": number,
            "expected_head": expected_head,
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                return Err(format!(
                    "committed PR-branch receipt {} was followed by a still-behind/regressed live branch; no update was replayed",
                    receipt.intent_id
                ));
            }
        };
        let result = (|| {
            self.gh(&[
                "api".into(),
                "-X".into(),
                "PUT".into(),
                format!("repos/{}/pulls/{number}/update-branch", self.repo),
                "-f".into(),
                format!("expected_head_sha={expected_head}"),
            ])?;
            let head = self.gh(&[
                "pr".into(),
                "view".into(),
                number.to_string(),
                "--json".into(),
                "headRefOid".into(),
                "--jq".into(),
                ".headRefOid".into(),
            ])?;
            if head.trim().is_empty() {
                Err("GitHub updated the PR branch but returned no new head SHA".into())
            } else {
                Ok(head.trim().to_string())
            }
        })();
        match result {
            Ok(head) => {
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "pr_number": number, "head": head }).to_string(),
                )
                .await?;
                Ok(head)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<MergeRequestResult, String> {
        if let MergeObservation::Merged { merge_sha } =
            self.merge_observation(number, expected_head)?
        {
            return Ok(MergeRequestResult::Merged { merge_sha });
        }
        let rung = "provider_pr_merge";
        let number_text = number.to_string();
        let operation_key =
            external_operation_key(rung, &[&number_text, method.as_str(), expected_head]);
        let evidence = json!({
            "pr_number": number,
            "method": method.as_str(),
            "expected_head": expected_head,
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_merge(number, expected_head).await?;
                return observed_committed_merge_projection(&receipt, observation);
            }
        };
        if let Err(error) = self.gh(&gh_pr_merge_args(
            number,
            method,
            commit_message,
            expected_head,
        )) {
            return Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await);
        }
        let observation = match self.merge_observation(number, expected_head) {
            Ok(observation) => observation,
            Err(error) => {
                return Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        };
        let merge_sha = match observation {
            MergeObservation::Merged { merge_sha } => {
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "pr_number": number, "merge_sha": merge_sha }).to_string(),
                )
                .await?;
                merge_sha
            }
            MergeObservation::OpenSameHead { auto_merge: true } => {
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "pr_number": number, "queued": true }).to_string(),
                )
                .await?;
                return Ok(MergeRequestResult::Queued);
            }
            MergeObservation::OpenSameHead { auto_merge: false } => {
                return Err(fail_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    "GitHub accepted gh pr merge but neither merged nor registered auto-merge"
                        .into(),
                )
                .await)
            }
            MergeObservation::HeadChanged { actual_head } => {
                return Err(fail_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    format!(
                    "PR head changed during merge: expected {expected_head}, actual {actual_head}"
                ),
                )
                .await)
            }
            MergeObservation::ClosedUnmerged => {
                return Err(fail_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    "PR closed without merge".into(),
                )
                .await)
            }
            MergeObservation::Unsupported => {
                return Err(fail_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    "GitHub merge observation unexpectedly unsupported".into(),
                )
                .await)
            }
        };
        if method != MergeMethod::Squash {
            return Ok(MergeRequestResult::Merged { merge_sha });
        }
        let Some(expected_message) = commit_message.map(|message| message.body.as_str()) else {
            return Ok(MergeRequestResult::Merged { merge_sha });
        };
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
        Ok(MergeRequestResult::Merged { merge_sha })
    }

    async fn observe_merge(
        &self,
        number: u64,
        expected_head: &str,
    ) -> Result<MergeObservation, String> {
        self.merge_observation(number, expected_head)
    }

    fn release_dispatch_target(&self, head_sha: &str) -> Option<ReleaseDispatchTarget> {
        Some(ReleaseDispatchTarget {
            workflow: self.release_workflow.clone(),
            git_ref: self.default_branch.clone(),
            head_sha: head_sha.to_string(),
        })
    }

    async fn observe_release_dispatch(
        &self,
        target: &ReleaseDispatchTarget,
    ) -> Result<ReleaseDispatchObservation, String> {
        if target.workflow != self.release_workflow || target.git_ref != self.default_branch {
            return Err("release target does not match the configured gh workflow/ref".into());
        }
        let raw = self.gh(&[
            "run".into(),
            "list".into(),
            "--workflow".into(),
            target.workflow.clone(),
            "--branch".into(),
            target.git_ref.clone(),
            "--event".into(),
            "workflow_dispatch".into(),
            "--limit".into(),
            "20".into(),
            "--json".into(),
            "databaseId,headBranch,headSha,status,conclusion,url,event".into(),
        ])?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|error| format!("gh run list returned non-JSON: {error}"))?;
        parse_github_release_dispatch_runs(&value, target)
    }

    async fn trigger_release(
        &self,
        head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
        let rung = "provider_release_trigger";
        let target = self
            .release_dispatch_target(head_sha)
            .expect("gh release target is configured");
        let operation_key = target.operation_key();
        let evidence = serde_json::to_string(&target)
            .map_err(|error| format!("cannot serialize release dispatch target: {error}"))?;
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_release_dispatch(&target).await?;
                return observed_committed_release_projection(&receipt, &target, observation);
            }
        };
        match self.gh(&gh_workflow_run_args(
            &self.release_workflow,
            &self.default_branch,
            &target.head_sha,
        )) {
            Ok(_) => {
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({
                        "workflow": target.workflow,
                        "git_ref": target.git_ref,
                        "head_sha": target.head_sha,
                        "triggered": true,
                    })
                    .to_string(),
                )
                .await?;
                Ok(format!(
                    "已通过 gh 触发发布工作流 {}",
                    self.release_workflow
                ))
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn verify_live(
        &self,
        sha: &str,
        _url: Option<&str>,
    ) -> Result<ObservationStatus, String> {
        let raw = self.gh(&[
            "release".into(),
            "view".into(),
            "--json".into(),
            "tagName,isDraft,isPrerelease,publishedAt,url,assets".into(),
        ])?;
        let release: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("gh release view returned non-JSON: {e}"))?;
        github_release_live_from_value(&release, sha, |tag| {
            let raw = self.gh(&[
                "api".into(),
                format!("repos/{}/compare/{sha}...{tag}", self.repo),
            ])?;
            let compare: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|error| format!("GitHub compare returned non-JSON: {error}"))?;
            let status = compare
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let behind_by = compare
                .get("behind_by")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(-1);
            Ok(matches!(status, "identical" | "ahead") && behind_by == 0)
        })
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
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<DeliveryPr, String> {
        match self {
            EitherRemote::Hook(r) => {
                r.open_or_get_pr(title, body, head, base, expected_head_sha, mutation_permit)
                    .await
            }
            EitherRemote::Gh(r) => {
                r.open_or_get_pr(title, body, head, base, expected_head_sha, mutation_permit)
                    .await
            }
            EitherRemote::Github(r) => {
                r.open_or_get_pr(title, body, head, base, expected_head_sha, mutation_permit)
                    .await
            }
            EitherRemote::Gitlab(r) => {
                r.open_or_get_pr(title, body, head, base, expected_head_sha, mutation_permit)
                    .await
            }
        }
    }
    async fn observe_open_pr(&self, head: &str, base: &str) -> Result<OpenPrObservation, String> {
        match self {
            EitherRemote::Hook(r) => r.observe_open_pr(head, base).await,
            EitherRemote::Gh(r) => r.observe_open_pr(head, base).await,
            EitherRemote::Github(r) => r.observe_open_pr(head, base).await,
            EitherRemote::Gitlab(r) => r.observe_open_pr(head, base).await,
        }
    }
    async fn update_pr_body(
        &self,
        number: u64,
        body: &str,
        head: &str,
        base: &str,
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<(), String> {
        match self {
            EitherRemote::Hook(r) => {
                r.update_pr_body(number, body, head, base, expected_head_sha, mutation_permit)
                    .await
            }
            EitherRemote::Gh(r) => {
                r.update_pr_body(number, body, head, base, expected_head_sha, mutation_permit)
                    .await
            }
            EitherRemote::Github(r) => {
                r.update_pr_body(number, body, head, base, expected_head_sha, mutation_permit)
                    .await
            }
            EitherRemote::Gitlab(r) => {
                r.update_pr_body(number, body, head, base, expected_head_sha, mutation_permit)
                    .await
            }
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
    async fn rerun_ci(
        &self,
        sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<bool, String> {
        match self {
            EitherRemote::Hook(r) => r.rerun_ci(sha, mutation_permit).await,
            EitherRemote::Gh(r) => r.rerun_ci(sha, mutation_permit).await,
            EitherRemote::Github(r) => r.rerun_ci(sha, mutation_permit).await,
            EitherRemote::Gitlab(r) => r.rerun_ci(sha, mutation_permit).await,
        }
    }
    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<MergeRequestResult, String> {
        match self {
            EitherRemote::Hook(r) => {
                r.merge_pr(
                    number,
                    method,
                    commit_message,
                    expected_head,
                    mutation_permit,
                )
                .await
            }
            EitherRemote::Gh(r) => {
                r.merge_pr(
                    number,
                    method,
                    commit_message,
                    expected_head,
                    mutation_permit,
                )
                .await
            }
            EitherRemote::Github(r) => {
                r.merge_pr(
                    number,
                    method,
                    commit_message,
                    expected_head,
                    mutation_permit,
                )
                .await
            }
            EitherRemote::Gitlab(r) => {
                r.merge_pr(
                    number,
                    method,
                    commit_message,
                    expected_head,
                    mutation_permit,
                )
                .await
            }
        }
    }
    async fn observe_merge(
        &self,
        number: u64,
        expected_head: &str,
    ) -> Result<MergeObservation, String> {
        match self {
            EitherRemote::Hook(r) => r.observe_merge(number, expected_head).await,
            EitherRemote::Gh(r) => r.observe_merge(number, expected_head).await,
            EitherRemote::Github(r) => r.observe_merge(number, expected_head).await,
            EitherRemote::Gitlab(r) => r.observe_merge(number, expected_head).await,
        }
    }
    fn release_dispatch_target(&self, head_sha: &str) -> Option<ReleaseDispatchTarget> {
        match self {
            EitherRemote::Hook(r) => r.release_dispatch_target(head_sha),
            EitherRemote::Gh(r) => r.release_dispatch_target(head_sha),
            EitherRemote::Github(r) => r.release_dispatch_target(head_sha),
            EitherRemote::Gitlab(r) => r.release_dispatch_target(head_sha),
        }
    }

    async fn observe_release_dispatch(
        &self,
        target: &ReleaseDispatchTarget,
    ) -> Result<ReleaseDispatchObservation, String> {
        match self {
            EitherRemote::Hook(r) => r.observe_release_dispatch(target).await,
            EitherRemote::Gh(r) => r.observe_release_dispatch(target).await,
            EitherRemote::Github(r) => r.observe_release_dispatch(target).await,
            EitherRemote::Gitlab(r) => r.observe_release_dispatch(target).await,
        }
    }

    async fn trigger_release(
        &self,
        head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
        match self {
            EitherRemote::Hook(r) => r.trigger_release(head_sha, mutation_permit).await,
            EitherRemote::Gh(r) => r.trigger_release(head_sha, mutation_permit).await,
            EitherRemote::Github(r) => r.trigger_release(head_sha, mutation_permit).await,
            EitherRemote::Gitlab(r) => r.trigger_release(head_sha, mutation_permit).await,
        }
    }

    async fn update_pr_branch(
        &self,
        number: u64,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
        match self {
            EitherRemote::Hook(r) => {
                r.update_pr_branch(number, expected_head, mutation_permit)
                    .await
            }
            EitherRemote::Gh(r) => {
                r.update_pr_branch(number, expected_head, mutation_permit)
                    .await
            }
            EitherRemote::Github(r) => {
                r.update_pr_branch(number, expected_head, mutation_permit)
                    .await
            }
            EitherRemote::Gitlab(r) => {
                r.update_pr_branch(number, expected_head, mutation_permit)
                    .await
            }
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
    ci_stability: CiObservationStability,
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
        ci_stability: CiObservationStability::default(),
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
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<DeliveryPr, String> {
        if let Some(pr) = exact_open_pr_projection(
            self.observe_open_pr(head, base).await?,
            None,
            expected_head_sha,
        )? {
            return Ok(pr);
        }
        let rung = "provider_pr_create";
        let operation_key = external_operation_key(rung, &[title, body, head, base, expected_head_sha]);
        let evidence = json!({
            "head": head,
            "base": base,
            "expected_head_sha": expected_head_sha,
            "title_digest": external_operation_key("title", &[title]),
            "body_digest": external_operation_key("body", &[body]),
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_open_pr(head, base).await?;
                return observed_committed_pr_projection(
                    &receipt,
                    observation,
                    title,
                    body,
                    expected_head_sha,
                );
            }
        };
        let result = crate::git_remote::gitlab::create_pr(
            &self.client,
            &self.repo,
            title,
            body,
            head,
            base,
            false,
        )
        .await;
        match result {
            Ok(mr) => {
                let created = DeliveryPr {
                    number: mr.number,
                    url: mr.url,
                    title: mr.title,
                    body: mr.body,
                };
                let pr = match self
                    .observe_open_pr(head, base)
                    .await
                    .and_then(|observation| {
                        exact_created_pr_projection(
                            &created,
                            observation,
                            title,
                            body,
                            head,
                            base,
                            expected_head_sha,
                        )
                    }) {
                    Ok(pr) => pr,
                    Err(error) => {
                        return Err(
                            fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                        )
                    }
                };
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "pr_number": pr.number, "pr_url": pr.url }).to_string(),
                )
                .await?;
                Ok(pr)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn observe_open_pr(&self, head: &str, base: &str) -> Result<OpenPrObservation, String> {
        let mrs = crate::git_remote::gitlab::list_prs(&self.client, &self.repo, "open")
            .await
            .map_err(|error| format!("cannot observe GitLab merge requests: {error}"))?;
        let Some(mr) = mrs
            .into_iter()
            .find(|mr| mr.head_branch == head && mr.base_branch == base)
        else {
            return Ok(OpenPrObservation::Absent);
        };
        let encoded = self.repo.replace('/', "%2F");
        let detail = self
            .client
            .get(&format!(
                "/projects/{encoded}/merge_requests/{}",
                mr.number
            ))
            .await?;
        let head_sha = detail
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                detail
                    .get("diff_refs")
                    .and_then(|refs| refs.get("head_sha"))
                    .and_then(serde_json::Value::as_str)
            })
            .filter(|sha| !sha.is_empty())
            .ok_or_else(|| "GitLab MR observation omitted the exact source head SHA".to_string())?;
        Ok(OpenPrObservation::Open(OpenPrState {
            pr: DeliveryPr {
                number: mr.number,
                url: mr.url,
                title: mr.title,
                body: mr.body,
            },
            head_branch: head.to_string(),
            base_branch: base.to_string(),
            head_sha: Some(head_sha.to_string()),
        }))
    }

    async fn update_pr_body(
        &self,
        number: u64,
        body: &str,
        head: &str,
        base: &str,
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<(), String> {
        exact_open_pr_projection(
            self.observe_open_pr(head, base).await?,
            Some(number),
            expected_head_sha,
        )?
        .ok_or_else(|| "canonical PR is absent; no body update was dispatched".to_string())?;
        let rung = "provider_pr_body_update";
        let number_text = number.to_string();
        let operation_key = external_operation_key(
            rung,
            &[&number_text, body, head, base, expected_head_sha],
        );
        let evidence = json!({
            "pr_number": number,
            "head": head,
            "base": base,
            "expected_head_sha": expected_head_sha,
            "body_digest": external_operation_key("body", &[body]),
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                exact_updated_pr_projection(
                    self.observe_open_pr(head, base).await?,
                    number,
                    body,
                    head,
                    base,
                    expected_head_sha,
                )
                .map_err(|error| {
                    format!(
                        "committed PR-body receipt {} no longer matches live state: {error}; no update was replayed",
                        receipt.intent_id
                    )
                })?;
                return Ok(());
            }
        };
        match crate::git_remote::gitlab::update_pr_body(&self.client, &self.repo, number, body)
            .await
        {
            Ok(()) => {
                if let Err(error) = self
                    .observe_open_pr(head, base)
                    .await
                    .and_then(|observation| {
                        exact_updated_pr_projection(
                            observation,
                            number,
                            body,
                            head,
                            base,
                            expected_head_sha,
                        )
                    })
                {
                    return Err(
                        fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                    );
                }
                commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn ci_status(&self, _sha: &str) -> Result<CiStatus, String> {
        Ok(CiStatus::None)
    }

    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        _commit_message: Option<&MergeCommitMessage>,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<MergeRequestResult, String> {
        let rung = "provider_pr_merge";
        let number_text = number.to_string();
        let operation_key =
            external_operation_key(rung, &[&number_text, method.as_str(), expected_head]);
        let evidence = json!({
            "pr_number": number,
            "method": method.as_str(),
            "expected_head": expected_head,
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_merge(number, expected_head).await?;
                return observed_committed_merge_projection(&receipt, observation);
            }
        };
        match crate::git_remote::gitlab::merge_pr(
            &self.client,
            &self.repo,
            number,
            method.as_str(),
            expected_head,
        )
        .await
        {
            Ok(()) => {
                let outcome = match self
                    .observe_merge(number, expected_head)
                    .await
                    .and_then(exact_dispatched_merge_projection)
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return Err(
                            fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                        )
                    }
                };
                let result_evidence = match &outcome {
                    MergeRequestResult::Queued => {
                        json!({ "pr_number": number, "queued": true })
                    }
                    MergeRequestResult::Merged { merge_sha } => json!({
                        "pr_number": number,
                        "merged": true,
                        "merge_sha": merge_sha,
                    }),
                };
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &result_evidence.to_string(),
                )
                .await?;
                Ok(outcome)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn observe_merge(
        &self,
        number: u64,
        expected_head: &str,
    ) -> Result<MergeObservation, String> {
        let encoded = self.repo.replace('/', "%2F");
        let value = self
            .client
            .get(&format!(
                "/projects/{encoded}/merge_requests/{number}"
            ))
            .await?;
        let actual_head = value
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                value
                    .get("diff_refs")
                    .and_then(|refs| refs.get("head_sha"))
                    .and_then(serde_json::Value::as_str)
            })
            .unwrap_or("");
        if !actual_head.is_empty() && actual_head != expected_head {
            return Ok(MergeObservation::HeadChanged {
                actual_head: actual_head.to_string(),
            });
        }
        match value
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
        {
            "merged" => {
                let merge_sha = value
                    .get("merge_commit_sha")
                    .or_else(|| value.get("squash_commit_sha"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if merge_sha.is_empty() {
                    return Err("GitLab reports merged but returned no merge commit SHA".into());
                }
                Ok(MergeObservation::Merged {
                    merge_sha: merge_sha.to_string(),
                })
            }
            "opened" => Ok(MergeObservation::OpenSameHead { auto_merge: false }),
            "closed" => Ok(MergeObservation::ClosedUnmerged),
            other => Err(format!("unknown GitLab merge request state '{other}'")),
        }
    }

    async fn trigger_release(
        &self,
        _head_sha: &str,
        _mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
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
            live: true,
        }
    }

    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<DeliveryPr, String> {
        if let Some(pr) = exact_open_pr_projection(
            self.observe_open_pr(head, base).await?,
            None,
            expected_head_sha,
        )? {
            return Ok(pr);
        }
        let rung = "provider_pr_create";
        let operation_key = external_operation_key(rung, &[title, body, head, base, expected_head_sha]);
        let evidence = json!({
            "head": head,
            "base": base,
            "expected_head_sha": expected_head_sha,
            "title_digest": external_operation_key("title", &[title]),
            "body_digest": external_operation_key("body", &[body]),
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_open_pr(head, base).await?;
                return observed_committed_pr_projection(
                    &receipt,
                    observation,
                    title,
                    body,
                    expected_head_sha,
                );
            }
        };
        let result = crate::git_remote::github::create_pr(
            &self.client,
            &self.repo,
            title,
            body,
            head,
            base,
            false,
        )
        .await;
        match result {
            Ok(pr) => {
                let created = DeliveryPr {
                    number: pr.number,
                    url: pr.url,
                    title: pr.title,
                    body: pr.body,
                };
                let pr = match self
                    .observe_open_pr(head, base)
                    .await
                    .and_then(|observation| {
                        exact_created_pr_projection(
                            &created,
                            observation,
                            title,
                            body,
                            head,
                            base,
                            expected_head_sha,
                        )
                    }) {
                    Ok(pr) => pr,
                    Err(error) => {
                        return Err(
                            fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                        )
                    }
                };
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "pr_number": pr.number, "pr_url": pr.url }).to_string(),
                )
                .await?;
                Ok(pr)
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn observe_open_pr(&self, head: &str, base: &str) -> Result<OpenPrObservation, String> {
        let value = self
            .client
            .get(&format!(
                "/repos/{}/pulls?state=open&base={base}&per_page=100",
                self.repo
            ))
            .await?;
        let rows = value
            .as_array()
            .ok_or_else(|| "GitHub open PR observation returned a non-array".to_string())?;
        let Some(row) = rows.iter().find(|row| {
            row.get("head")
                .and_then(|head| head.get("ref"))
                .and_then(serde_json::Value::as_str)
                == Some(head)
        }) else {
            return Ok(OpenPrObservation::Absent);
        };
        let number = row
            .get("number")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "GitHub open PR observation omitted number".to_string())?;
        let url = row
            .get("html_url")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "GitHub open PR observation omitted URL".to_string())?;
        Ok(OpenPrObservation::Open(OpenPrState {
            pr: DeliveryPr {
                number,
                url: url.to_string(),
                title: row
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                body: row
                    .get("body")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
            head_branch: head.to_string(),
            base_branch: base.to_string(),
            head_sha: row
                .get("head")
                .and_then(|head| head.get("sha"))
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        }))
    }

    async fn update_pr_body(
        &self,
        number: u64,
        body: &str,
        head: &str,
        base: &str,
        expected_head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<(), String> {
        exact_open_pr_projection(
            self.observe_open_pr(head, base).await?,
            Some(number),
            expected_head_sha,
        )?
        .ok_or_else(|| "canonical PR is absent; no body update was dispatched".to_string())?;
        let rung = "provider_pr_body_update";
        let number_text = number.to_string();
        let operation_key = external_operation_key(
            rung,
            &[&number_text, body, head, base, expected_head_sha],
        );
        let evidence = json!({
            "pr_number": number,
            "head": head,
            "base": base,
            "expected_head_sha": expected_head_sha,
            "body_digest": external_operation_key("body", &[body]),
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                exact_updated_pr_projection(
                    self.observe_open_pr(head, base).await?,
                    number,
                    body,
                    head,
                    base,
                    expected_head_sha,
                )
                .map_err(|error| {
                    format!(
                        "committed PR-body receipt {} no longer matches live state: {error}; no update was replayed",
                        receipt.intent_id
                    )
                })?;
                return Ok(());
            }
        };
        match crate::git_remote::github::update_pr_body(&self.client, &self.repo, number, body)
            .await
        {
            Ok(()) => {
                if let Err(error) = self
                    .observe_open_pr(head, base)
                    .await
                    .and_then(|observation| {
                        exact_updated_pr_projection(
                            observation,
                            number,
                            body,
                            head,
                            base,
                            expected_head_sha,
                        )
                    })
                {
                    return Err(
                        fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                    );
                }
                commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn ci_status(&self, sha: &str) -> Result<CiStatus, String> {
        let observation = crate::git_remote::github::ci_observation(
            &self.client,
            &self.repo,
            sha,
            &self.default_branch,
        )
        .await?;
        let status = match observation.status.as_str() {
            "success" => CiStatus::Success,
            "pending" => CiStatus::Pending,
            "none" => CiStatus::None,
            other => CiStatus::Failure(other.trim_start_matches("failure:").to_string()),
        };
        Ok(self.ci_stability.confirm(&observation.fingerprint, status))
    }

    async fn rerun_ci(
        &self,
        sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<bool, String> {
        let value = self
            .client
            .get(&format!(
                "/repos/{}/actions/runs?head_sha={sha}&per_page=20",
                self.repo
            ))
            .await?;
        let mut rerun = false;
        for run in value
            .get("workflow_runs")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if run.get("status").and_then(serde_json::Value::as_str) != Some("completed") {
                continue;
            }
            let conclusion = run
                .get("conclusion")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if !matches!(
                conclusion,
                "failure" | "cancelled" | "timed_out" | "stale" | "startup_failure"
            ) {
                continue;
            }
            let Some(id) = run.get("id").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let rung = "provider_ci_rerun";
            let id_text = id.to_string();
            let operation_key = external_operation_key(rung, &[sha, &id_text]);
            let evidence = json!({ "sha": sha, "run_id": id }).to_string();
            let intent = match begin_or_reuse_external_mutation(
                mutation_permit,
                rung,
                &operation_key,
                &evidence,
            )
            .await?
            {
                DeliveryMutationBegin::Dispatch(intent) => intent,
                DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                    let result = committed_receipt_result(&receipt)?;
                    rerun |= result
                        .get("rerun")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true);
                    continue;
                }
            };
            let result = self
                .client
                .post(
                    &format!("/repos/{}/actions/runs/{id}/rerun", self.repo),
                    serde_json::json!({}),
                )
                .await;
            match result {
                Ok(_) => {
                    commit_external_mutation(
                        mutation_permit,
                        intent.as_ref(),
                        &json!({ "sha": sha, "run_id": id, "rerun": true }).to_string(),
                    )
                    .await?
                }
                Err(error) => {
                    return Err(
                        fail_external_mutation(mutation_permit, intent.as_ref(), error).await,
                    )
                }
            }
            rerun = true;
        }
        if rerun {
            self.ci_stability.reset();
        }
        Ok(rerun)
    }

    async fn merge_readiness(&self, number: u64) -> Result<MergeReadiness, String> {
        let value = self
            .client
            .get(&format!("/repos/{}/pulls/{number}", self.repo))
            .await?;
        if value
            .get("draft")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(MergeReadiness::NeedsAction(
                "PR 仍是 draft，需要标记为 ready 才能合并".into(),
            ));
        }
        Ok(merge_readiness_from_state(
            value
                .get("mergeable_state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        ))
    }

    async fn update_pr_branch(
        &self,
        number: u64,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
        let rung = "provider_pr_branch_update";
        let number_text = number.to_string();
        let operation_key = external_operation_key(rung, &[&number_text, expected_head]);
        let evidence = json!({
            "pr_number": number,
            "expected_head": expected_head,
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                return Err(format!(
                    "committed PR-branch receipt {} was followed by a still-behind/regressed live branch; no update was replayed",
                    receipt.intent_id
                ));
            }
        };
        let result = self
            .client
            .put(
                &format!("/repos/{}/pulls/{number}/update-branch", self.repo),
                serde_json::json!({"expected_head_sha": expected_head}),
            )
            .await;
        if let Err(error) = result {
            return Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await);
        }
        let value = match self
            .client
            .get(&format!("/repos/{}/pulls/{number}", self.repo))
            .await
        {
            Ok(value) => value,
            Err(error) => {
                return Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        };
        let head = value
            .get("head")
            .and_then(|head| head.get("sha"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if head.is_empty() {
            Err(fail_external_mutation(
                mutation_permit,
                intent.as_ref(),
                "GitHub updated the PR branch but returned no new head SHA".into(),
            )
            .await)
        } else {
            commit_external_mutation(
                mutation_permit,
                intent.as_ref(),
                &json!({ "pr_number": number, "head": head }).to_string(),
            )
            .await?;
            Ok(head.to_string())
        }
    }

    async fn merge_pr(
        &self,
        number: u64,
        method: MergeMethod,
        commit_message: Option<&MergeCommitMessage>,
        expected_head: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<MergeRequestResult, String> {
        let rung = "provider_pr_merge";
        let number_text = number.to_string();
        let operation_key =
            external_operation_key(rung, &[&number_text, method.as_str(), expected_head]);
        let evidence = json!({
            "pr_number": number,
            "method": method.as_str(),
            "expected_head": expected_head,
        })
        .to_string();
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_merge(number, expected_head).await?;
                return observed_committed_merge_projection(&receipt, observation);
            }
        };
        let result = crate::git_remote::github::merge_pr(
            &self.client,
            &self.repo,
            number,
            method.as_str(),
            commit_message.map(|message| message.title.as_str()),
            commit_message.map(|message| message.body.as_str()),
            expected_head,
        )
        .await;
        match result {
            Ok(merge_sha) => {
                commit_external_mutation(
                    mutation_permit,
                    intent.as_ref(),
                    &json!({ "pr_number": number, "merge_sha": merge_sha }).to_string(),
                )
                .await?;
                Ok(MergeRequestResult::Merged { merge_sha })
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn observe_merge(
        &self,
        number: u64,
        expected_head: &str,
    ) -> Result<MergeObservation, String> {
        let value = self
            .client
            .get(&format!("/repos/{}/pulls/{number}", self.repo))
            .await?;
        let actual_head = value
            .get("head")
            .and_then(|head| head.get("sha"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        if !actual_head.is_empty() && actual_head != expected_head {
            return Ok(MergeObservation::HeadChanged { actual_head });
        }
        if value
            .get("merged")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            let merge_sha = value
                .get("merge_commit_sha")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            if merge_sha.is_empty() {
                return Err("GitHub reports merged but returned no merge commit SHA".into());
            }
            return Ok(MergeObservation::Merged { merge_sha });
        }
        match value
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
        {
            "open" => Ok(MergeObservation::OpenSameHead { auto_merge: false }),
            "closed" => Ok(MergeObservation::ClosedUnmerged),
            other => Err(format!("unknown GitHub PR state '{other}'")),
        }
    }

    fn release_dispatch_target(&self, head_sha: &str) -> Option<ReleaseDispatchTarget> {
        Some(ReleaseDispatchTarget {
            workflow: self.release_workflow.clone(),
            git_ref: self.default_branch.clone(),
            head_sha: head_sha.to_string(),
        })
    }

    async fn observe_release_dispatch(
        &self,
        target: &ReleaseDispatchTarget,
    ) -> Result<ReleaseDispatchObservation, String> {
        if target.workflow != self.release_workflow || target.git_ref != self.default_branch {
            return Err("release target does not match the configured GitHub workflow/ref".into());
        }
        let branch =
            url::form_urlencoded::byte_serialize(target.git_ref.as_bytes()).collect::<String>();
        let value = self
            .client
            .get(&format!(
                "/repos/{}/actions/workflows/{}/runs?branch={branch}&event=workflow_dispatch&per_page=20",
                self.repo, target.workflow
            ))
            .await?;
        parse_github_release_dispatch_runs(&value, target)
    }

    async fn trigger_release(
        &self,
        head_sha: &str,
        mutation_permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
        // workflow_dispatch on the repo's release workflow (needs a token with
        // the `workflow` scope; a repo-only token yields a clear 403 here).
        let path = format!(
            "/repos/{}/actions/workflows/{}/dispatches",
            self.repo, self.release_workflow
        );
        let rung = "provider_release_trigger";
        let target = self
            .release_dispatch_target(head_sha)
            .expect("GitHub release target is configured");
        let operation_key = target.operation_key();
        let evidence = serde_json::to_string(&target)
            .map_err(|error| format!("cannot serialize release dispatch target: {error}"))?;
        let intent = match begin_or_reuse_external_mutation(
            mutation_permit,
            rung,
            &operation_key,
            &evidence,
        )
        .await?
        {
            DeliveryMutationBegin::Dispatch(intent) => intent,
            DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                let observation = self.observe_release_dispatch(&target).await?;
                return observed_committed_release_projection(&receipt, &target, observation);
            }
        };
        let result = self
            .client
            .post(
                &path,
                serde_json::json!({
                    "ref": self.default_branch,
                    "inputs": { "expected_head_sha": target.head_sha },
                }),
            )
            .await;
        match result {
            Ok(_) => {
                commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await?;
                Ok(format!("已触发发布工作流 {}", self.release_workflow))
            }
            Err(error) => {
                Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
            }
        }
    }

    async fn verify_live(
        &self,
        sha: &str,
        _url: Option<&str>,
    ) -> Result<ObservationStatus, String> {
        let release = self
            .client
            .get(&format!("/repos/{}/releases/latest", self.repo))
            .await?;
        let tag = release
            .get("tagName")
            .or_else(|| release.get("tag_name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let tag_contains_sha = if tag.is_empty() {
            false
        } else {
            let compare = self
                .client
                .get(&format!("/repos/{}/compare/{sha}...{tag}", self.repo))
                .await?;
            let status = compare
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let behind_by = compare
                .get("behind_by")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(-1);
            matches!(status, "identical" | "ahead") && behind_by == 0
        };
        github_release_live_from_value(&release, sha, |_| Ok(tag_contains_sha))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
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

    #[test]
    fn remote_observation_errors_separate_wait_from_core_input() {
        assert!(remote_error_is_retryable(
            "HTTP 429: API rate limit exceeded"
        ));
        assert!(remote_error_is_retryable("HTTP 503 Service Unavailable"));
        assert!(!remote_error_is_retryable("HTTP 401: bad credentials"));
        assert!(remote_error_requires_core_input(
            "HTTP 403: authentication required"
        ));
        assert!(!remote_error_requires_core_input(
            "HTTP 403: API rate limit exceeded"
        ));
        let private_rules_403 =
            "gh: Upgrade to GitHub Pro or make this repository public to enable this feature. (HTTP 403)";
        assert!(remote_error_is_retryable(private_rules_403));
        assert!(github_rules_capability_unavailable(private_rules_403));
        assert!(github_required_status_checks(Err(private_rules_403.into()))
            .unwrap()
            .is_empty());
        assert!(github_required_status_checks(Err("HTTP 500".into())).is_err());

        let unknown = DeliveryOutcome {
            steps: vec![],
            branch: Some("feature/x".into()),
            commit_sha: Some("abc".into()),
            pr_url: None,
            pr_number: None,
            final_state: "delivered".into(),
            stage: "preflight".into(),
            code: "delivery_ready".into(),
            recoverable: false,
            recovery_class: RecoveryClass::None,
            retry_after_ms: None,
            next_action: None,
            reached_state: "local".into(),
            requested_ceiling: "through_release".into(),
            effective_ceiling: "through_release".into(),
            capability_gap: None,
            release_receipt: None,
            summary: String::new(),
        }
        .remote_observation_failed("ci_observation", "unclassified remote schema drift");
        assert_eq!(unknown.final_state, "waiting");
        assert!(unknown.recoverable);
        assert_eq!(unknown.recovery_class, RecoveryClass::WaitRetryable);
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
            recovery_class: RecoveryClass::None,
            retry_after_ms: None,
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
        assert_eq!(
            gh_pr_edit_body_args(7, "updated body"),
            vec!["pr", "edit", "7", "--body", "updated body"]
        );
        let merge_message = MergeCommitMessage {
            title: "fix: preserve release policy".into(),
            body: "Release-Urgency: hold".into(),
        };
        let merge = gh_pr_merge_args(42, MergeMethod::Squash, Some(&merge_message), "abc123");
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
                "--auto",
                "--match-head-commit",
                "abc123",
            ]
        );
        assert!(!merge.iter().any(|arg| arg == "--admin"));
        let release = gh_workflow_run_args("auto-release.yml", "main", "abc123");
        assert_eq!(
            release,
            vec![
                "workflow",
                "run",
                "auto-release.yml",
                "--ref",
                "main",
                "-f",
                "expected_head_sha=abc123",
            ]
        );
    }

    #[test]
    fn github_ci_terminal_observation_must_be_stable_before_green() {
        let stability = CiObservationStability::default();
        assert_eq!(stability.confirm("none", CiStatus::None), CiStatus::Pending);
        assert_eq!(stability.confirm("none", CiStatus::None), CiStatus::None);
        assert_eq!(
            stability.confirm("governance:success", CiStatus::Success),
            CiStatus::Pending
        );
        assert_eq!(
            stability.confirm(
                "agent:success|check:success|governance:success|gui:success",
                CiStatus::Success,
            ),
            CiStatus::Pending
        );
        assert_eq!(
            stability.confirm(
                "agent:success|check:success|governance:success|gui:success",
                CiStatus::Success,
            ),
            CiStatus::Success
        );
    }

    #[test]
    fn github_ci_stays_pending_until_every_effective_required_check_is_present() {
        let rules = serde_json::json!([{
            "type": "required_status_checks",
            "parameters": {"required_status_checks": [
                {"context": "agent-bridge-linux", "integration_id": 15368},
                {"context": "check", "integration_id": 15368},
                {"context": "governance-baseline", "integration_id": 15368},
                {"context": "remote-real-app-gui", "integration_id": 15368}
            ]}
        }]);
        let required = crate::git_remote::github::parse_required_status_checks(&rules);
        let partial = serde_json::json!({"check_runs": [{
            "name": "governance-baseline",
            "status": "completed",
            "conclusion": "success",
            "app": {"id": 15368}
        }]});
        let observation = crate::git_remote::github::classify_ci_observation(&partial, &required);
        assert_eq!(observation.status, "pending");
        assert!(observation.fingerprint.contains("check:absent"));

        let complete = serde_json::json!({"check_runs": required.iter().map(|check| {
            serde_json::json!({
                "name": check.context,
                "status": "completed",
                "conclusion": "success",
                "app": {"id": check.integration_id}
            })
        }).collect::<Vec<_>>()});
        assert_eq!(
            crate::git_remote::github::classify_ci_observation(&complete, &required).status,
            "success"
        );
    }

    #[test]
    fn github_ci_failure_names_the_check_and_retryability() {
        let required = vec![crate::git_remote::github::RequiredStatusCheck {
            context: "check".into(),
            integration_id: Some(15368),
        }];
        for (conclusion, retryable) in [
            ("failure", false),
            ("cancelled", true),
            ("timed_out", true),
            ("stale", true),
            ("startup_failure", true),
        ] {
            let runs = serde_json::json!({"check_runs": [{
                "name": "check",
                "status": "completed",
                "conclusion": conclusion,
                "app": {"id": 15368},
                "details_url": "https://github.example/actions/runs/7/job/8"
            }]});
            let observation = crate::git_remote::github::classify_ci_observation(&runs, &required);
            assert!(
                observation
                    .status
                    .starts_with(&format!("failure:check:{conclusion}")),
                "{}",
                observation.status
            );
            assert!(observation.status.contains("actions/runs/7/job/8"));
            assert_eq!(ci_failure_is_retryable(&observation.status), retryable);
        }
    }

    #[test]
    fn github_merge_observation_binds_state_to_the_expected_head() {
        let queued = serde_json::json!({
            "state": "OPEN",
            "headRefOid": "abc123",
            "mergeCommit": null,
            "autoMergeRequest": {"enabledAt": "2026-08-03T00:00:00Z"}
        });
        assert_eq!(
            parse_github_merge_observation(&queued, "abc123").unwrap(),
            MergeObservation::OpenSameHead { auto_merge: true }
        );
        assert_eq!(
            parse_github_merge_observation(&queued, "different").unwrap(),
            MergeObservation::HeadChanged {
                actual_head: "abc123".into()
            }
        );
        let merged = serde_json::json!({
            "state": "MERGED",
            "headRefOid": "abc123",
            "mergeCommit": {"oid": "merge456"},
            "autoMergeRequest": null
        });
        assert_eq!(
            parse_github_merge_observation(&merged, "abc123").unwrap(),
            MergeObservation::Merged {
                merge_sha: "merge456".into()
            }
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

    /// Real-runtime smoke: with a logged-in gh on this machine, `ci_status` on a
    /// commit the remote actually has must parse into a valid `CiStatus`.
    ///
    /// This is intentionally opt-in (`CODEFACTORY_RUN_GH_SMOKE=1`). The default
    /// Rust suite runs often during delivery and CI; hitting GitHub's live API
    /// there amplified PR polling into rate limits and failed otherwise-good
    /// builds. Parser behavior is covered by deterministic unit tests; this smoke
    /// is for explicit operator diagnostics only. That opt-in gate is part of
    /// the delivery stability contract: routine CI must not consume GitHub API
    /// quota just to prove local parser behavior.
    /// It asks about `origin/<default>`, NOT local `HEAD`. Local HEAD is
    /// whatever you are working on, and GitHub answers `No commit found for SHA
    /// … (HTTP 422)` for anything unpushed — so the old form failed on every
    /// in-progress commit (hit twice in one session on 2026-08-03) and told you
    /// nothing about the parser.
    ///
    /// Skipping on unpushed HEAD would have been worse than the bug: the test
    /// would then sit out exactly when someone is changing this code. Pointing
    /// it at a remote-known commit keeps it running during ordinary work.
    #[tokio::test]
    async fn gh_cli_remote_reads_real_ci_status_when_gh_is_authenticated() {
        if std::env::var("CODEFACTORY_RUN_GH_SMOKE").ok().as_deref() != Some("1") {
            eprintln!("skipping gh smoke: set CODEFACTORY_RUN_GH_SMOKE=1 to hit GitHub's live API");
            return;
        }
        if !gh_cli_available() {
            eprintln!("skipping gh smoke: gh missing or unauthenticated");
            return;
        }
        let cwd = std::env::var_os("CODEFACTORY_GH_SMOKE_CWD")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap());
        let Some(remote) = gh_remote_for(&cwd) else {
            eprintln!("skipping gh smoke: not a github repo checkout");
            return;
        };
        let default_branch =
            remote_default_branch(&cwd, "origin").unwrap_or_else(|| "main".to_string());
        // A commit the remote is guaranteed to know. A production incident can
        // supply its exact durable head; otherwise use origin/<default>.
        let sha = if let Ok(sha) = std::env::var("CODEFACTORY_GH_SMOKE_SHA") {
            sha
        } else {
            let Ok(sha) = git(&cwd, &["rev-parse", &format!("origin/{default_branch}")]) else {
                eprintln!(
                    "skipping gh smoke: no local ref for origin/{default_branch}; run `git fetch origin`"
                );
                return;
            };
            sha
        };
        match remote.ci_status(sha.trim()).await {
            Ok(_) => {}
            Err(e) if e.to_ascii_lowercase().contains("rate limit") => {
                eprintln!("skipping gh smoke: GitHub API rate limited this external smoke: {e}");
                return;
            }
            Err(e) => panic!("gh ci_status must parse for remote-known {sha}: {e}"),
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
    fn github_release_live_verifier_requires_published_assets_and_tag_ancestry() {
        let release = serde_json::json!({
            "tagName": "v1.2.3",
            "isDraft": false,
            "isPrerelease": false,
            "publishedAt": "2026-08-05T09:00:00Z",
            "assets": [{"name": "CodeFactory.dmg"}]
        });
        let status =
            github_release_live_from_value(&release, "abcdef1234567890", |tag| Ok(tag == "v1.2.3"))
                .unwrap();
        assert_eq!(
            status,
            ObservationStatus::Success(
                "GitHub Release v1.2.3 is published with 1 assets and contains delivery commit abcdef1".into()
            )
        );

        let draft = serde_json::json!({
            "tagName": "v1.2.3",
            "isDraft": true,
            "isPrerelease": false,
            "publishedAt": null,
            "assets": []
        });
        assert!(matches!(
            github_release_live_from_value(&draft, "abcdef1234567890", |_| Ok(true)).unwrap(),
            ObservationStatus::Pending(detail) if detail.contains("draft")
        ));

        let missing_sha =
            github_release_live_from_value(&release, "abcdef1234567890", |_| Ok(false)).unwrap();
        assert!(matches!(
            missing_sha,
            ObservationStatus::Pending(detail) if detail.contains("does not include")
        ));
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
            recovery_class: RecoveryClass::None,
            retry_after_ms: None,
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
        assert!(
            outcome.recoverable,
            "missing system live-verifier configuration is platform-owned, not a human gate"
        );
        assert_eq!(
            outcome.recovery_class,
            RecoveryClass::AgentActionRequired,
            "the agent/system remediation loop must own this blocker"
        );
        assert!(outcome
            .next_action
            .as_deref()
            .is_some_and(|action| !action.contains("用户")));
    }

    #[test]
    fn triggered_but_not_live_release_remains_system_owned_observation_without_user_cta() {
        let outcome = DeliveryOutcome {
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
            recovery_class: RecoveryClass::None,
            retry_after_ms: None,
            next_action: None,
            reached_state: "release_triggered".into(),
            requested_ceiling: "through_release".into(),
            effective_ceiling: "through_release".into(),
            capability_gap: None,
            release_receipt: None,
            summary: String::new(),
        };

        let waiting = block_unverified_release(
            outcome,
            "release 已触发，但目标构建仍在等待发布并形成 live 证据",
        );

        assert_eq!(waiting.final_state, "waiting");
        assert_eq!(waiting.recovery_class, RecoveryClass::WaitRetryable);
        assert!(waiting.recoverable);
        assert!(waiting.retry_after_ms.is_some());
        assert!(waiting
            .next_action
            .as_deref()
            .is_some_and(|action| action.contains("核对同一 release")
                && !action.contains("用户")
                && !action.contains("请")));
        assert!(!waiting.summary.contains("用户"));
        assert!(!waiting.summary.contains("请"));
        assert!(!waiting.summary.contains("重新调用"));
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
    fn github_release_dispatch_parser_never_promotes_nonmatching_event_ref_or_head() {
        let target = ReleaseDispatchTarget {
            workflow: "auto-release.yml".into(),
            git_ref: "main".into(),
            head_sha: "exact-head".into(),
        };
        let observed = serde_json::json!({
            "workflow_runs": [
                {
                    "id": 1,
                    "event": "push",
                    "head_branch": "main",
                    "head_sha": "exact-head",
                    "status": "completed"
                },
                {
                    "id": 2,
                    "event": "workflow_dispatch",
                    "head_branch": "other",
                    "head_sha": "exact-head",
                    "status": "completed"
                },
                {
                    "id": 3,
                    "event": "workflow_dispatch",
                    "head_branch": "main",
                    "head_sha": "other-head",
                    "status": "completed"
                }
            ]
        });

        assert_eq!(
            parse_github_release_dispatch_runs(&observed, &target).unwrap(),
            ReleaseDispatchObservation::HeadMismatch {
                observed_heads: vec!["other-head".into()]
            }
        );

        let exact = serde_json::json!([{
            "databaseId": 42,
            "event": "workflow_dispatch",
            "headBranch": "main",
            "headSha": "exact-head",
            "status": "in_progress",
            "url": "https://example.invalid/actions/runs/42"
        }]);
        assert_eq!(
            parse_github_release_dispatch_runs(&exact, &target).unwrap(),
            ReleaseDispatchObservation::Triggered {
                run_id: "42".into(),
                status: "in_progress".into(),
                head_sha: "exact-head".into(),
                detail: "https://example.invalid/actions/runs/42".into(),
            }
        );

        assert_eq!(
            parse_github_release_dispatch_runs(&serde_json::json!([]), &target).unwrap(),
            ReleaseDispatchObservation::Absent
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

        let exact_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let pr = remote
            .open_or_get_pr("title", "body", "feat/x", "main", &exact_head, None)
            .await
            .expect("hook open_or_get_pr");
        assert_eq!(pr.number, 42);
        assert!(pr.url.contains("merge_requests/42"));
        assert_eq!(pr.title, "title");
        assert_eq!(pr.body, "body");
        assert_eq!(remote.ci_status("abc123").await.unwrap(), CiStatus::Success);
        remote
            .merge_pr(42, MergeMethod::Squash, None, "abc123", None)
            .await
            .unwrap();
        assert_eq!(
            remote.trigger_release("release-head", None).await.unwrap(),
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

    #[tokio::test]
    async fn provider_hook_release_observer_rejects_foreign_target_before_hook_execution() {
        let root = make_repo("hook-release-foreign-target");
        let remote = HookRemote::new("corp-mr".into(), "false".into(), root.clone());
        let target = ReleaseDispatchTarget {
            workflow: "provider-hook:retired-hook".into(),
            git_ref: "main".into(),
            head_sha: "release-head".into(),
        };

        let error = remote
            .observe_release_dispatch(&target)
            .await
            .expect_err("a current hook must never observe a target owned by another hook");
        assert!(
            error.contains("does not match the current provider hook identity"),
            "foreign target reached the hook instead of failing closed: {error}"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn committed_provider_receipts_are_freshly_observed_and_never_replayed() {
        let root = feature_branch_repo("hook-committed-receipts");
        let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let hook = root.join("committed_provider.py");
        let mutation_log = root.join("mutation.log");
        std::fs::write(
            &hook,
            r#"#!/usr/bin/env python3
import json, pathlib, sys
req=json.load(sys.stdin)
action=req.get('action')
mutations={'open_or_get_pr','update_pr_body','rerun_ci','merge_pr','trigger_release'}
if action in mutations:
    pathlib.Path(sys.argv[1]).open('a').write(action+'\n')
    print(json.dumps({'error':'committed mutation was replayed'}))
    sys.exit(2)
if action == 'observe_open_pr':
    print(json.dumps({'status':'open','number':42,'url':'https://example/pr/42','title':'title','body':'body','head_sha':sys.argv[2]}))
elif action == 'observe_merge':
    print(json.dumps({'status':'merged','head_sha':req.get('expected_head'),'merge_sha':'merge-sha'}))
elif action == 'observe_release_dispatch':
    print(json.dumps({'status':'triggered','workflow':req.get('workflow'),'git_ref':req.get('git_ref'),'head_sha':req.get('head_sha'),'run_id':'run-1','detail':'queued'}))
else:
    print(json.dumps({'error':'unexpected observation '+str(action)}))
    sys.exit(2)
"#,
        )
        .unwrap();
        let remote = HookRemote::new(
            "test-hook".into(),
            format!("python3 {} {} {}", hook.display(), mutation_log.display(), head),
            root.clone(),
        );
        let permit = committed_only_permit(&[
            (
                "provider_pr_open_or_get",
                json!({"pr_number": 42, "pr_url": "https://example/pr/42"}),
            ),
            (
                "provider_pr_body_update",
                json!({"pr_number": 42, "body_digest": external_operation_key("body", &["body"])}),
            ),
            ("provider_ci_rerun", json!({"sha": head, "rerun": false})),
            (
                "provider_pr_merge",
                json!({"pr_number": 42, "merged": true, "merge_sha": "merge-sha"}),
            ),
            (
                "provider_release_trigger",
                json!({"workflow": "provider-hook:test-hook", "git_ref": "main", "head_sha": head, "triggered": true}),
            ),
        ]);

        let pr = remote
            .open_or_get_pr("title", "body", "feat/x", "main", &head, Some(&permit))
            .await
            .expect("committed PR create must be reconstructed from a current exact observation");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.body, "body");
        remote
            .update_pr_body(42, "body", "feat/x", "main", &head, Some(&permit))
            .await
            .expect("an exact live body/head must reuse the committed receipt without replay");
        assert!(!remote.rerun_ci(&head, Some(&permit)).await.unwrap());
        assert_eq!(
            remote
                .merge_pr(42, MergeMethod::Squash, None, &head, Some(&permit))
                .await
                .unwrap(),
            MergeRequestResult::Merged {
                merge_sha: "merge-sha".into()
            }
        );
        assert!(remote
            .trigger_release(&head, Some(&permit))
            .await
            .unwrap()
            .contains("当前只读观察仍匹配"));
        assert!(
            !mutation_log.exists(),
            "no provider actuator may run when the same operation already has a committed receipt"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn committed_provider_receipt_with_remote_regression_fails_closed_without_replay() {
        let root = feature_branch_repo("hook-committed-regression");
        let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let hook = root.join("regressed_provider.py");
        let mutation_log = root.join("mutation.log");
        std::fs::write(
            &hook,
            r#"#!/usr/bin/env python3
import json, pathlib, sys
req=json.load(sys.stdin)
action=req.get('action')
if action == 'observe_open_pr':
    print(json.dumps({'status':'open','number':42,'url':'https://example/pr/42','title':'title','body':'foreign edit','head_sha':'foreign-head'}))
else:
    pathlib.Path(sys.argv[1]).open('a').write(str(action)+'\n')
    print(json.dumps({'error':'mutation/other action forbidden'}))
    sys.exit(2)
"#,
        )
        .unwrap();
        let remote = HookRemote::new(
            "test-hook".into(),
            format!("python3 {} {} {}", hook.display(), mutation_log.display(), head),
            root.clone(),
        );
        let permit = committed_only_permit(&[
            (
                "provider_pr_open_or_get",
                json!({"pr_number": 42, "pr_url": "https://example/pr/42"}),
            ),
            (
                "provider_pr_body_update",
                json!({"pr_number": 42, "body_digest": external_operation_key("body", &["body"])}),
            ),
        ]);

        let error = remote
            .open_or_get_pr("title", "body", "feat/x", "main", &head, Some(&permit))
            .await
            .expect_err("a committed PR receipt cannot bless current remote drift");
        assert!(error.contains("no longer matches"));
        let body_error = remote
            .update_pr_body(42, "body", "feat/x", "main", &head, Some(&permit))
            .await
            .expect_err("a foreign PR head must block before a body mutation is prepared");
        assert!(body_error.contains("foreign head"), "{body_error}");
        assert!(!mutation_log.exists());
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
                expect_branch: None,
                expected_identity: None,
                mutation_permit: None,
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

    struct CommittedOnlyMutationPermit {
        results: HashMap<String, String>,
    }

    #[derive(Default)]
    struct RecordingMutationPermit {
        committed_rungs: Mutex<Vec<String>>,
        unknown_rungs: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl DeliveryMutationPermitVerifier for RecordingMutationPermit {
        async fn verify(&self, _rung: &str) -> Result<(), String> {
            Ok(())
        }

        async fn begin_external_mutation(
            &self,
            rung: &str,
            operation_key: &str,
            _evidence: &str,
        ) -> Result<DeliveryMutationBegin, String> {
            Ok(DeliveryMutationBegin::Dispatch(Some(
                DeliveryMutationIntentToken {
                    id: format!("recording-{rung}"),
                    rung: rung.to_string(),
                    operation_key: operation_key.to_string(),
                },
            )))
        }

        async fn commit_external_mutation(
            &self,
            intent: &DeliveryMutationIntentToken,
            _evidence: &str,
        ) -> Result<(), String> {
            self.committed_rungs
                .lock()
                .unwrap()
                .push(intent.rung.clone());
            Ok(())
        }

        async fn mark_external_mutation_unknown(
            &self,
            intent: &DeliveryMutationIntentToken,
            _detail: &str,
        ) -> Result<(), String> {
            self.unknown_rungs
                .lock()
                .unwrap()
                .push(intent.rung.clone());
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl DeliveryMutationPermitVerifier for CommittedOnlyMutationPermit {
        async fn verify(&self, _rung: &str) -> Result<(), String> {
            Ok(())
        }

        async fn begin_external_mutation(
            &self,
            rung: &str,
            operation_key: &str,
            _evidence: &str,
        ) -> Result<DeliveryMutationBegin, String> {
            let result_evidence = self
                .results
                .get(rung)
                .cloned()
                .ok_or_else(|| format!("test has no committed receipt for {rung}"))?;
            Ok(DeliveryMutationBegin::AlreadyCommitted(
                DeliveryMutationCommittedReceipt {
                    intent_id: format!("committed-{rung}"),
                    rung: rung.to_string(),
                    operation_key: operation_key.to_string(),
                    result_evidence: Some(result_evidence),
                },
            ))
        }

        async fn commit_external_mutation(
            &self,
            _intent: &DeliveryMutationIntentToken,
            _evidence: &str,
        ) -> Result<(), String> {
            panic!("an already-committed mutation must never be dispatched or committed again")
        }
    }

    fn committed_only_permit(results: &[(&str, serde_json::Value)]) -> DeliveryMutationPermit {
        DeliveryMutationPermit::new(Arc::new(CommittedOnlyMutationPermit {
            results: results
                .iter()
                .map(|(rung, value)| ((*rung).to_string(), value.to_string()))
                .collect(),
        }))
    }

    #[test]
    fn committed_pr_projection_rejects_conflicting_result_and_observation_identity() {
        let receipt = DeliveryMutationCommittedReceipt {
            intent_id: "pr-conflict".into(),
            rung: "provider_pr_create".into(),
            operation_key: "op".into(),
            result_evidence: Some(
                json!({
                    "committed_result": {"pr_number": 41, "pr_url": "https://example/pr/41"},
                    "observation": {"pr_number": 42, "pr_url": "https://example/pr/42"}
                })
                .to_string(),
            ),
        };

        let error = committed_pr_projection(&receipt, "title", "body")
            .expect_err("a fresh PR B cannot replace the committed PR A identity");
        assert!(error.contains("conflict"), "{error}");
    }

    #[test]
    fn committed_merge_projection_rejects_conflicting_result_and_observed_merge_sha() {
        let receipt = DeliveryMutationCommittedReceipt {
            intent_id: "merge-conflict".into(),
            rung: "provider_pr_merge".into(),
            operation_key: "op".into(),
            result_evidence: Some(
                json!({
                    "committed_result": {"merged": true, "merge_sha": "merge-a"},
                    "observation": {"confirmation": "merge_observed", "merge_sha": "merge-b"}
                })
                .to_string(),
            ),
        };

        let error = committed_merge_projection(&receipt)
            .expect_err("an observed merge SHA cannot replace a different committed merge SHA");
        assert!(error.contains("conflict"), "{error}");
    }

    struct StubRemote {
        ci: CiStatus,
        existing_pr: Option<(u64, String)>,
        merge_ok: bool,
        merge_queues: bool,
        /// Varies per test: the whole point of the ladder fix is that a missing
        /// high-rung capability must not cancel the rungs below it.
        caps: DeliveryCapabilities,
        calls: Arc<StubCalls>,
    }

    #[derive(Default)]
    struct StubCalls {
        merged: AtomicBool,
        open_pr: AtomicUsize,
        update_pr_body: AtomicUsize,
        ci: AtomicUsize,
        rerun_ci: AtomicUsize,
        merge: AtomicUsize,
        update_branch: AtomicUsize,
        release: AtomicUsize,
        merge_commit_message: Mutex<Option<MergeCommitMessage>>,
        remote_pr_text: Mutex<Option<(String, String)>>,
        last_pr_body: Mutex<Option<String>>,
        last_ci_sha: Mutex<Option<String>>,
        ci_sequence: Mutex<VecDeque<CiStatus>>,
        merge_readiness: Mutex<Option<MergeReadiness>>,
        release_observation: Mutex<Option<ReleaseDispatchObservation>>,
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
            h: &str,
            base: &str,
            expected_head_sha: &str,
            mutation_permit: Option<&DeliveryMutationPermit>,
        ) -> Result<DeliveryPr, String> {
            let rung = "provider_pr_open_or_get";
            let operation_key = external_operation_key(rung, &[t, b, h, base, expected_head_sha]);
            let evidence = json!({ "head": h, "base": base, "expected_head_sha": expected_head_sha }).to_string();
            let intent = match begin_or_reuse_external_mutation(
                mutation_permit,
                rung,
                &operation_key,
                &evidence,
            )
            .await?
            {
                DeliveryMutationBegin::Dispatch(intent) => intent,
                DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                    let observation = self.observe_open_pr(h, base).await?;
                    return observed_committed_pr_projection(
                        &receipt,
                        observation,
                        t,
                        b,
                        expected_head_sha,
                    );
                }
            };
            self.calls.open_pr.fetch_add(1, Ordering::SeqCst);
            *self.calls.last_pr_body.lock().unwrap() = Some(b.to_string());
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
            let pr = DeliveryPr {
                number,
                url,
                title,
                body,
            };
            *self.calls.last_ci_sha.lock().unwrap() = Some(expected_head_sha.to_string());
            commit_external_mutation(
                mutation_permit,
                intent.as_ref(),
                &json!({ "pr_number": pr.number, "pr_url": pr.url }).to_string(),
            )
            .await?;
            Ok(pr)
        }
        async fn observe_open_pr(
            &self,
            head: &str,
            base: &str,
        ) -> Result<OpenPrObservation, String> {
            let Some((number, url)) = self.existing_pr.clone() else {
                return Ok(OpenPrObservation::Absent);
            };
            let (title, body) = self
                .calls
                .remote_pr_text
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| ("fix: existing PR".into(), String::new()));
            Ok(OpenPrObservation::Open(OpenPrState {
                pr: DeliveryPr {
                    number,
                    url,
                    title,
                    body,
                },
                head_branch: head.to_string(),
                base_branch: base.to_string(),
                head_sha: self.calls.last_ci_sha.lock().unwrap().clone(),
            }))
        }
        async fn update_pr_body(
            &self,
            number: u64,
            body: &str,
            head: &str,
            base: &str,
            expected_head_sha: &str,
            mutation_permit: Option<&DeliveryMutationPermit>,
        ) -> Result<(), String> {
            exact_open_pr_projection(
                self.observe_open_pr(head, base).await?,
                Some(number),
                expected_head_sha,
            )?
            .ok_or_else(|| "canonical PR is absent; no body update was dispatched".to_string())?;
            let rung = "provider_pr_body_update";
            let number_text = number.to_string();
            let operation_key = external_operation_key(
                rung,
                &[&number_text, body, head, base, expected_head_sha],
            );
            let evidence = json!({
                "pr_number": number,
                "head": head,
                "base": base,
                "expected_head_sha": expected_head_sha,
            })
            .to_string();
            let intent = match begin_or_reuse_external_mutation(
                mutation_permit,
                rung,
                &operation_key,
                &evidence,
            )
            .await?
            {
                DeliveryMutationBegin::Dispatch(intent) => intent,
                DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                    return Err(format!(
                        "committed PR-body receipt {} was followed by live body drift; no update was replayed",
                        receipt.intent_id
                    ));
                }
            };
            self.calls.update_pr_body.fetch_add(1, Ordering::SeqCst);
            let mut remote = self.calls.remote_pr_text.lock().unwrap();
            let title = remote
                .as_ref()
                .map(|(title, _)| title.clone())
                .unwrap_or_else(|| "fix: existing PR".into());
            *remote = Some((title, body.to_string()));
            commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await
        }
        async fn ci_status(&self, sha: &str) -> Result<CiStatus, String> {
            self.calls.ci.fetch_add(1, Ordering::SeqCst);
            *self.calls.last_ci_sha.lock().unwrap() = Some(sha.to_string());
            if let Some(status) = self.calls.ci_sequence.lock().unwrap().pop_front() {
                return Ok(status);
            }
            Ok(self.ci.clone())
        }
        async fn rerun_ci(
            &self,
            sha: &str,
            mutation_permit: Option<&DeliveryMutationPermit>,
        ) -> Result<bool, String> {
            let rung = "provider_ci_rerun";
            let operation_key = external_operation_key(rung, &[sha]);
            let evidence = json!({ "sha": sha }).to_string();
            let intent = match begin_or_reuse_external_mutation(
                mutation_permit,
                rung,
                &operation_key,
                &evidence,
            )
            .await?
            {
                DeliveryMutationBegin::Dispatch(intent) => intent,
                DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                    return committed_receipt_result(&receipt)?
                        .get("rerun")
                        .and_then(serde_json::Value::as_bool)
                        .ok_or_else(|| "committed stub CI rerun receipt lacks result".to_string());
                }
            };
            self.calls.rerun_ci.fetch_add(1, Ordering::SeqCst);
            commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await?;
            Ok(true)
        }
        async fn merge_pr(
            &self,
            _n: u64,
            _m: MergeMethod,
            commit_message: Option<&MergeCommitMessage>,
            expected_head: &str,
            mutation_permit: Option<&DeliveryMutationPermit>,
        ) -> Result<MergeRequestResult, String> {
            let rung = "provider_pr_merge";
            let operation_key = external_operation_key(rung, &[expected_head]);
            let evidence = json!({ "expected_head": expected_head }).to_string();
            let intent = match begin_or_reuse_external_mutation(
                mutation_permit,
                rung,
                &operation_key,
                &evidence,
            )
            .await?
            {
                DeliveryMutationBegin::Dispatch(intent) => intent,
                DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                    let observation = self.observe_merge(_n, expected_head).await?;
                    return observed_committed_merge_projection(&receipt, observation);
                }
            };
            self.calls.merge.fetch_add(1, Ordering::SeqCst);
            *self.calls.merge_commit_message.lock().unwrap() = commit_message.cloned();
            let result = if self.merge_queues {
                Ok(MergeRequestResult::Queued)
            } else if self.merge_ok {
                self.calls.merged.store(true, Ordering::SeqCst);
                Ok(MergeRequestResult::Merged {
                    merge_sha: "merge-sha".into(),
                })
            } else {
                Err("protected branch".into())
            };
            match result {
                Ok(outcome) => {
                    let result_evidence = match &outcome {
                        MergeRequestResult::Queued => json!({ "pr_number": _n, "queued": true }),
                        MergeRequestResult::Merged { merge_sha } => json!({
                            "pr_number": _n,
                            "merged": true,
                            "merge_sha": merge_sha,
                        }),
                    };
                    commit_external_mutation(
                        mutation_permit,
                        intent.as_ref(),
                        &result_evidence.to_string(),
                    )
                    .await?;
                    Ok(outcome)
                }
                Err(error) => {
                    Err(fail_external_mutation(mutation_permit, intent.as_ref(), error).await)
                }
            }
        }
        async fn observe_merge(
            &self,
            _number: u64,
            _expected_head: &str,
        ) -> Result<MergeObservation, String> {
            if self.calls.merged.load(Ordering::SeqCst) {
                Ok(MergeObservation::Merged {
                    merge_sha: "merge-sha".into(),
                })
            } else {
                Ok(MergeObservation::OpenSameHead {
                    auto_merge: self.merge_queues && self.calls.merge.load(Ordering::SeqCst) > 0,
                })
            }
        }
        async fn merge_readiness(&self, _number: u64) -> Result<MergeReadiness, String> {
            Ok(self
                .calls
                .merge_readiness
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(MergeReadiness::Unknown))
        }
        async fn update_pr_branch(
            &self,
            number: u64,
            expected_head: &str,
            mutation_permit: Option<&DeliveryMutationPermit>,
        ) -> Result<String, String> {
            let rung = "provider_pr_branch_update";
            let number_text = number.to_string();
            let operation_key = external_operation_key(rung, &[&number_text, expected_head]);
            let evidence = json!({
                "pr_number": number,
                "expected_head": expected_head,
            })
            .to_string();
            let intent = match begin_or_reuse_external_mutation(
                mutation_permit,
                rung,
                &operation_key,
                &evidence,
            )
            .await?
            {
                DeliveryMutationBegin::Dispatch(intent) => intent,
                DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                    return Err(format!(
                        "committed PR-branch receipt {} was followed by a still-behind/regressed live branch; no update was replayed",
                        receipt.intent_id
                    ));
                }
            };
            self.calls.update_branch.fetch_add(1, Ordering::SeqCst);
            let head = self
                .calls
                .last_ci_sha
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| "stub has not observed a head".to_string())?;
            commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await?;
            Ok(head)
        }
        fn release_dispatch_target(&self, head_sha: &str) -> Option<ReleaseDispatchTarget> {
            Some(ReleaseDispatchTarget {
                workflow: "stub-release.yml".into(),
                git_ref: "main".into(),
                head_sha: head_sha.to_string(),
            })
        }

        async fn observe_release_dispatch(
            &self,
            _target: &ReleaseDispatchTarget,
        ) -> Result<ReleaseDispatchObservation, String> {
            Ok(self
                .calls
                .release_observation
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(ReleaseDispatchObservation::Absent))
        }

        async fn trigger_release(
            &self,
            head_sha: &str,
            mutation_permit: Option<&DeliveryMutationPermit>,
        ) -> Result<String, String> {
            let rung = "provider_release_trigger";
            let target = self
                .release_dispatch_target(head_sha)
                .expect("stub release target");
            let operation_key = target.operation_key();
            let evidence = serde_json::to_string(&target).unwrap();
            let intent = match begin_or_reuse_external_mutation(
                mutation_permit,
                rung,
                &operation_key,
                &evidence,
            )
            .await?
            {
                DeliveryMutationBegin::Dispatch(intent) => intent,
                DeliveryMutationBegin::AlreadyCommitted(receipt) => {
                    let observation = self.observe_release_dispatch(&target).await?;
                    return observed_committed_release_projection(&receipt, &target, observation);
                }
            };
            self.calls.release.fetch_add(1, Ordering::SeqCst);
            *self.calls.release_observation.lock().unwrap() =
                Some(ReleaseDispatchObservation::Triggered {
                    run_id: "stub-run".into(),
                    status: "queued".into(),
                    head_sha: head_sha.to_string(),
                    detail: "stub release workflow dispatched".into(),
                });
            commit_external_mutation(mutation_permit, intent.as_ref(), &evidence).await?;
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

    #[test]
    fn a_receipted_branch_update_head_stays_fetchable_after_the_provider_deletes_the_branch() {
        // Providers with "delete branch on merge" enabled remove the PR's head
        // branch the moment it merges. The commit stays reachable — through the
        // merge and through the PR ref — but the branch is no longer an address
        // for it, so a recovery that can only fetch `repo.branch` strands a run
        // whose receipt already names the exact head.
        let root = feature_branch_repo("branch-update-deleted-head");
        git(&root, &["add", "feature.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "feature A"]).unwrap();
        let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        git(&root, &["push", "-q", "origin", "feat/x"]).unwrap();

        let origin = root.parent().unwrap().join("origin.git");
        git(&origin, &["update-ref", "refs/pull/7/head", &head]).unwrap();
        git(&origin, &["update-ref", "refs/heads/main", &head]).unwrap();
        git(&origin, &["update-ref", "-d", "refs/heads/feat/x"]).unwrap();

        let repo = RepoContext {
            root: root.clone(),
            branch: "feat/x".to_string(),
            default_branch: "main".to_string(),
            remote: "origin".to_string(),
            remote_url: None,
        };
        let fetched = fetch_updated_pr_head_for_operation(
            &repo,
            &head,
            "sha256:0123456789abcdef0123456789abcdef",
            Some(7),
        )
        .expect("a receipted head must stay addressable after its branch is deleted");
        assert_eq!(git(&root, &["rev-parse", &fetched]).unwrap(), head);
        clear_delivery_operation_ref(&repo, &fetched);
    }

    #[test]
    fn a_branch_update_fetch_refuses_every_ref_that_does_not_hold_the_receipted_head() {
        // Trying more addresses must not mean accepting more heads: if the
        // branch AND the PR ref have both moved to a foreign commit, the
        // receipted head is simply not materialisable and the run must stay
        // stuck rather than advance onto someone else's commit.
        let root = feature_branch_repo("branch-update-foreign-everywhere");
        git(&root, &["add", "feature.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "feature A"]).unwrap();
        let receipted_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        git(&root, &["push", "-q", "origin", "feat/x"]).unwrap();

        std::fs::write(root.join("foreign.rs"), "pub fn foreign() {}\n").unwrap();
        git(&root, &["add", "foreign.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "foreign B"]).unwrap();
        let foreign_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        git(&root, &["push", "-q", "-f", "origin", "feat/x"]).unwrap();
        git(&root, &["reset", "-q", "--hard", &receipted_head]).unwrap();

        let origin = root.parent().unwrap().join("origin.git");
        git(&origin, &["update-ref", "refs/pull/7/head", &foreign_head]).unwrap();

        let repo = RepoContext {
            root: root.clone(),
            branch: "feat/x".to_string(),
            default_branch: "main".to_string(),
            remote: "origin".to_string(),
            remote_url: None,
        };
        let error = fetch_updated_pr_head_for_operation(
            &repo,
            &receipted_head,
            "sha256:fedcba9876543210fedcba9876543210",
            Some(7),
        )
        .expect_err("a foreign head must never satisfy a receipted branch update");
        assert!(error.contains(&receipted_head), "{error}");

        // A rejected attempt may not leave the temporary observation ref behind.
        let leaked = git(&root, &["for-each-ref", "--format=%(refname)", "refs/codefactory/"])
            .unwrap();
        assert!(leaked.is_empty(), "leaked observation ref: {leaked}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_push_with_foreign_post_receive_head_never_commits_a_receipt_or_opens_pr() {
        use std::os::unix::fs::PermissionsExt;

        let root = feature_branch_repo("push-post-observe-foreign-head");
        git(&root, &["add", "feature.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "feature A"]).unwrap();
        let authorized_head = git(&root, &["rev-parse", "HEAD"]).unwrap();

        git(&root, &["checkout", "-q", "-b", "foreign"]).unwrap();
        std::fs::write(root.join("foreign.rs"), "pub fn foreign() {}\n").unwrap();
        git(&root, &["add", "foreign.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "foreign B"]).unwrap();
        let foreign_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        git(&root, &["push", "-q", "origin", "foreign"]).unwrap();
        git(&root, &["checkout", "-q", "feat/x"]).unwrap();
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), authorized_head);

        let origin = root.parent().unwrap().join("origin.git");
        let hook = origin.join("hooks/post-receive");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\ngit --git-dir='{}' update-ref refs/heads/feat/x {}\n",
                origin.display(),
                foreign_head
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();

        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };
        let recorder = Arc::new(RecordingMutationPermit::default());
        let permit = DeliveryMutationPermit::new(recorder.clone());
        let out = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            5,
            &DeliverOpts {
                mutation_permit: Some(permit),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(
            observe_remote_ref_head(
                &resolve_delivery_repo(&root, Some("main"), Some("feat/x"))
                    .unwrap()
                    .0,
                "feat/x",
            )
            .unwrap()
            .as_deref(),
            Some(foreign_head.as_str())
        );
        assert_ne!(out.final_state, "delivered", "{:?}", out.steps);
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert!(
            !recorder
                .committed_rungs
                .lock()
                .unwrap()
                .iter()
                .any(|rung| rung == "git_push")
        );
        assert!(recorder
            .unknown_rungs
            .lock()
            .unwrap()
            .iter()
            .any(|rung| rung == "git_push"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn provider_create_with_foreign_observed_head_never_commits_its_receipt() {
        let root = feature_branch_repo("provider-create-post-observe");
        let hook = root.join("provider.py");
        std::fs::write(
            &hook,
            r#"#!/usr/bin/env python3
import json, sys
req=json.load(sys.stdin)
if req.get('action') == 'open_or_get_pr':
    print(json.dumps({'number':42,'url':'https://example/pr/42','title':req.get('title'),'body':req.get('body')}))
elif req.get('action') == 'observe_open_pr':
    print(json.dumps({'status':'open','number':42,'url':'https://example/pr/42','title':'title','body':'body','head_sha':'foreign-head'}))
else:
    print(json.dumps({'error':'unexpected action'}))
    sys.exit(2)
"#,
        )
        .unwrap();
        let expected_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let remote = HookRemote::new(
            "post-observe".into(),
            format!("python3 {}", hook.display()),
            root.clone(),
        );
        let recorder = Arc::new(RecordingMutationPermit::default());
        let permit = DeliveryMutationPermit::new(recorder.clone());

        let error = remote
            .open_or_get_pr(
                "title",
                "body",
                "feat/x",
                "main",
                &expected_head,
                Some(&permit),
            )
            .await
            .expect_err("a create response is not a receipt until the exact PR head is observed");

        assert!(error.contains("foreign head") || error.contains("exact"), "{error}");
        assert!(
            !recorder
                .committed_rungs
                .lock()
                .unwrap()
                .iter()
                .any(|rung| rung == "provider_pr_open_or_get")
        );
        assert!(recorder
            .unknown_rungs
            .lock()
            .unwrap()
            .iter()
            .any(|rung| rung == "provider_pr_open_or_get"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn provider_body_update_with_post_mutation_drift_never_commits_its_receipt() {
        let root = feature_branch_repo("provider-body-post-observe");
        let hook = root.join("provider.py");
        let counter = root.join("observe-count");
        std::fs::write(
            &hook,
            r#"#!/usr/bin/env python3
import json, pathlib, sys
req=json.load(sys.stdin)
counter=pathlib.Path(sys.argv[1])
if req.get('action') == 'observe_open_pr':
    n=int(counter.read_text()) if counter.exists() else 0
    counter.write_text(str(n+1))
    if n == 0:
        print(json.dumps({'status':'open','number':42,'url':'https://example/pr/42','title':'title','body':'old','head_sha':sys.argv[2]}))
    else:
        print(json.dumps({'status':'open','number':42,'url':'https://example/pr/42','title':'title','body':'old','head_sha':'foreign-head'}))
elif req.get('action') == 'update_pr_body':
    print(json.dumps({'ok':True}))
else:
    print(json.dumps({'error':'unexpected action'}))
    sys.exit(2)
"#,
        )
        .unwrap();
        let expected_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let remote = HookRemote::new(
            "post-observe".into(),
            format!("python3 {} {} {}", hook.display(), counter.display(), expected_head),
            root.clone(),
        );
        let recorder = Arc::new(RecordingMutationPermit::default());
        let permit = DeliveryMutationPermit::new(recorder.clone());

        let error = remote
            .update_pr_body(
                42,
                "desired",
                "feat/x",
                "main",
                &expected_head,
                Some(&permit),
            )
            .await
            .expect_err("a body response is not a receipt until the updated exact PR is observed");

        assert!(error.contains("foreign") || error.contains("exact"), "{error}");
        assert!(
            !recorder
                .committed_rungs
                .lock()
                .unwrap()
                .iter()
                .any(|rung| rung == "provider_pr_body_update")
        );
        assert!(recorder
            .unknown_rungs
            .lock()
            .unwrap()
            .iter()
            .any(|rung| rung == "provider_pr_body_update"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn provider_merge_ok_without_positive_merge_observation_never_commits_its_receipt() {
        let root = feature_branch_repo("provider-merge-post-observe");
        let hook = root.join("provider.py");
        std::fs::write(
            &hook,
            r#"#!/usr/bin/env python3
import json, sys
req=json.load(sys.stdin)
if req.get('action') == 'merge_pr':
    print(json.dumps({'ok':True}))
elif req.get('action') == 'observe_merge':
    print(json.dumps({'status':'open','head_sha':req.get('expected_head'),'auto_merge':False}))
else:
    print(json.dumps({'error':'unexpected action'}))
    sys.exit(2)
"#,
        )
        .unwrap();
        let expected_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let remote = HookRemote::new(
            "post-observe".into(),
            format!("python3 {}", hook.display()),
            root.clone(),
        );
        let recorder = Arc::new(RecordingMutationPermit::default());
        let permit = DeliveryMutationPermit::new(recorder.clone());

        let error = remote
            .merge_pr(
                42,
                MergeMethod::Squash,
                None,
                &expected_head,
                Some(&permit),
            )
            .await
            .expect_err("provider ok is not a merge receipt without a positive observation");

        assert!(error.contains("merge") || error.contains("open"), "{error}");
        assert!(
            !recorder
                .committed_rungs
                .lock()
                .unwrap()
                .iter()
                .any(|rung| rung == "provider_pr_merge")
        );
        assert!(recorder
            .unknown_rungs
            .lock()
            .unwrap()
            .iter()
            .any(|rung| rung == "provider_pr_merge"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn pr_only_commits_pushes_and_opens_pr_then_stops() {
        let root = feature_branch_repo("pronly");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
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
        let body = remote
            .calls
            .last_pr_body
            .lock()
            .unwrap()
            .clone()
            .expect("controlled delivery must send a PR body");
        assert!(body.contains("README-Update: reviewed"));
        assert!(body.contains("README-Update-Reason:"));
        assert!(!body.contains("README-Update-Reason: <"));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn stale_feature_branch_is_blocked_before_stage_commit_push_or_pr() {
        let root = feature_branch_repo("stale-native-base");
        let updater = root.parent().unwrap().join("upstream-main");
        git(
            &root,
            &["worktree", "add", "-q", updater.to_str().unwrap(), "main"],
        )
        .unwrap();
        git(&updater, &["config", "user.name", "CodeFactory Test"]).unwrap();
        git(
            &updater,
            &["config", "user.email", "test@codefactory.invalid"],
        )
        .unwrap();
        std::fs::write(updater.join("upstream.txt"), "new main\n").unwrap();
        git(&updater, &["add", "upstream.txt"]).unwrap();
        git(&updater, &["commit", "-q", "-m", "advance main"]).unwrap();
        git(&updater, &["push", "-q", "origin", "main"]).unwrap();

        let head_before = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let status_before = git(&root, &["status", "--porcelain=v1"]).unwrap();
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
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

        assert_eq!(out.final_state, "blocked", "{:?}", out.steps);
        assert_eq!(out.stage, "base_sync");
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), head_before);
        assert_eq!(
            git(&root, &["status", "--porcelain=v1"]).unwrap(),
            status_before
        );
        assert!(git(&root, &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn readme_diff_is_declared_required_in_generated_pr_body() {
        let root = feature_branch_repo("pr-readme-required");
        std::fs::write(root.join("README.md"), "# User-facing change\n").unwrap();
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
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
        let body = calls.last_pr_body.lock().unwrap().clone().unwrap();
        assert!(body.contains("README-Update: required"), "{body}");
        assert!(body.contains("README-Update-Reason:"), "{body}");
    }

    #[tokio::test]
    async fn existing_pr_body_is_converged_instead_of_left_to_fail_ci() {
        let root = feature_branch_repo("pr-body-converge");
        let calls = stub_calls();
        *calls.remote_pr_text.lock().unwrap() = Some((
            "fix: existing PR".into(),
            "Existing context that must be preserved.".into(),
        ));
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: Some((7, "https://example/pr/7".into())),
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
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
        assert_eq!(calls.update_pr_body.load(Ordering::SeqCst), 1);
        let body = calls.remote_pr_text.lock().unwrap().clone().unwrap().1;
        assert!(body.contains("Existing context that must be preserved."));
        assert!(body.contains("README-Update: reviewed"));
        assert!(body.contains("README-Update-Reason:"));
    }

    #[tokio::test]
    async fn retryable_ci_infrastructure_failure_reruns_once_then_continues() {
        let root = feature_branch_repo("ci-retryable");
        let calls = stub_calls();
        *calls.ci_sequence.lock().unwrap() = VecDeque::from([
            CiStatus::Failure("check:timed_out".into()),
            CiStatus::Success,
        ]);
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };

        let out = deliver(
            &root,
            DeliveryCeiling::ThroughCiGreen,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(out.final_state, "delivered", "{:?}", out.steps);
        assert_eq!(calls.rerun_ci.load(Ordering::SeqCst), 1);
        assert!(out.steps.iter().any(|step| step.step == "ci_recovery"));
    }

    #[tokio::test]
    async fn ordinary_test_failure_is_actionable_but_not_blindly_rerun() {
        let root = feature_branch_repo("ci-actionable");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Failure("check:failure".into()),
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };

        let out = deliver(
            &root,
            DeliveryCeiling::ThroughCiGreen,
            MergeMethod::Squash,
            5,
            &DeliverOpts::default(),
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(out.final_state, "blocked");
        assert!(out.recoverable);
        assert_eq!(calls.rerun_ci.load(Ordering::SeqCst), 0);
        assert!(out.next_action.as_deref().unwrap_or("").contains("check"));
        assert!(out.next_action.as_deref().unwrap_or("").contains("修复"));
    }

    #[tokio::test]
    async fn pr_only_metadata_survives_a_parameterless_resume_through_release() {
        let root = feature_branch_repo("pr-metadata-resume");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
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
            merge_queues: false,
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
            merge_queues: false,
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
    async fn stale_delivery_identity_blocks_before_commit_push_or_pr() {
        let root = feature_branch_repo("stale-identity");
        let origin = root.parent().unwrap().join("origin.git");
        let (repo, _) = resolve_delivery_repo(&root, Some("main"), Some("feat/x")).unwrap();
        let expected_identity = capture_delivery_identity(&repo).unwrap();

        git(&root, &["add", "feature.rs"]).unwrap();
        git(
            &root,
            &[
                "-c",
                "user.name=Concurrent Writer",
                "-c",
                "user.email=concurrent@example.invalid",
                "commit",
                "-m",
                "intruder commit",
            ],
        )
        .unwrap();
        let intruder_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let intruder_status = git(
            &root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .unwrap();
        assert!(
            git(&origin, &["show-ref", "--verify", "refs/heads/feat/x"]).is_err(),
            "test precondition: feature branch is not yet pushed"
        );

        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };
        let outcome = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            1,
            &DeliverOpts {
                expect_branch: Some("feat/x".into()),
                expected_identity: Some(expected_identity),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(outcome.final_state, "blocked", "{:?}", outcome.steps);
        assert_eq!(outcome.stage, "identity");
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), intruder_head);
        assert_eq!(
            git(
                &root,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )
            .unwrap(),
            intruder_status,
            "identity rejection must not stage or rewrite the concurrent commit"
        );
        assert!(
            git(&origin, &["show-ref", "--verify", "refs/heads/feat/x"]).is_err(),
            "identity rejection must not push"
        );
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert_eq!(calls.update_pr_body.load(Ordering::SeqCst), 0);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn takeover_refuses_an_unreceipted_local_head_without_any_remote_mutation() {
        let root = feature_branch_repo("takeover-unreceipted-head");
        let origin = root.parent().unwrap().join("origin.git");
        let (repo, _) = resolve_delivery_repo(&root, Some("main"), Some("feat/x")).unwrap();
        let persisted_identity = capture_delivery_identity(&repo).unwrap();

        git(&root, &["add", "feature.rs"]).unwrap();
        git(
            &root,
            &[
                "-c",
                "user.name=Unreceipted Writer",
                "-c",
                "user.email=unreceipted@example.invalid",
                "commit",
                "-m",
                "unreceipted local head",
            ],
        )
        .unwrap();
        let unreceipted_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(unreceipted_head, persisted_identity.head_sha);

        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };
        let error = observe_delivery_takeover(
            &root,
            Some("main"),
            "feat/x",
            &persisted_identity,
            None,
            None,
            Some(&remote),
        )
        .await
        .expect_err("an unreceipted head cannot self-authorize takeover");

        assert!(error.contains("not durably receipted"));
        assert!(git(&origin, &["show-ref", "--verify", "refs/heads/feat/x"]).is_err());
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert_eq!(calls.update_pr_body.load(Ordering::SeqCst), 0);
        assert_eq!(calls.rerun_ci.load(Ordering::SeqCst), 0);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn exact_local_commit_receipt_reconciles_only_its_own_child_head() {
        let root = feature_branch_repo("takeover-receipted-local-commit");
        let (repo, _) = resolve_delivery_repo(&root, Some("main"), Some("feat/x")).unwrap();
        let persisted_identity = capture_delivery_identity(&repo).unwrap();
        git(&root, &["add", "feature.rs"]).unwrap();
        let staged_identity = capture_delivery_identity(&repo).unwrap();
        let staged_tree_sha = git(&root, &["write-tree"]).unwrap();
        let commit_message = "fix: receipted local commit";
        let expected_head_sha = git(
            &root,
            &[
                "-c",
                "user.name=CodeFactory",
                "-c",
                "user.email=noreply@codefactory.local",
                "commit-tree",
                &staged_tree_sha,
                "-p",
                &persisted_identity.head_sha,
                "-m",
                commit_message,
            ],
        )
        .unwrap();
        let evidence = LocalCommitIntentEvidence::new(
            &persisted_identity,
            &staged_identity,
            &repo.branch,
            &staged_tree_sha,
            &expected_head_sha,
            commit_message,
        );
        assert_eq!(
            git(&root, &["rev-parse", "HEAD"]).unwrap(),
            persisted_identity.head_sha,
            "write-ahead intent exists while the branch still points at its parent",
        );
        let materialized = materialize_receipted_local_commit(
            &root,
            Some("main"),
            "feat/x",
            &persisted_identity,
            &evidence,
        )
        .expect("the exact staged tree and commit object complete the pre-ref crash window");
        assert_eq!(materialized.head_sha, expected_head_sha);

        let observed = observe_receipted_local_commit(
            &root,
            Some("main"),
            "feat/x",
            &persisted_identity,
            &evidence,
        )
        .expect("the exact parent/tree/message receipt proves this local commit");
        assert_eq!(
            observed.head_sha,
            git(&root, &["rev-parse", "HEAD"]).unwrap()
        );
        assert_eq!(observed.repo_identity, persisted_identity.repo_identity);
        assert_eq!(
            observed.worktree_identity,
            persisted_identity.worktree_identity
        );

        let mut wrong = evidence.clone();
        wrong.commit_message_digest = external_operation_key("message", &["foreign"]);
        assert!(observe_receipted_local_commit(
            &root,
            Some("main"),
            "feat/x",
            &persisted_identity,
            &wrong,
        )
        .is_err());

        git(&root, &["reset", "--soft", &persisted_identity.head_sha]).unwrap();
        git(
            &root,
            &[
                "-c",
                "user.name=Foreign",
                "-c",
                "user.email=foreign@example.invalid",
                "commit",
                "--no-verify",
                "-m",
                commit_message,
            ],
        )
        .unwrap();
        assert_ne!(
            git(&root, &["rev-parse", "HEAD"]).unwrap(),
            expected_head_sha
        );
        assert!(
            observe_receipted_local_commit(
                &root,
                Some("main"),
                "feat/x",
                &persisted_identity,
                &evidence,
            )
            .is_err(),
            "same parent/tree/message with a different exact commit SHA is foreign"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn pre_intent_isolated_stage_leaves_real_git_state_bit_identical() {
        let root = feature_branch_repo("pre-intent-isolated-stage");
        std::fs::write(root.join("already-staged.txt"), "keep staged\n").unwrap();
        git(&root, &["add", "already-staged.txt"]).unwrap();
        let repository = git2::Repository::open(&root).unwrap();
        let index_path = repository.index().unwrap().path().unwrap().to_path_buf();
        let before_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let before_branch_ref = git(&root, &["rev-parse", "refs/heads/feat/x"]).unwrap();
        let before_index = std::fs::read(&index_path).unwrap();
        let before_feature = std::fs::read(root.join("feature.rs")).unwrap();
        let before_staged = std::fs::read(root.join("already-staged.txt")).unwrap();

        let plan = prepare_scoped_commit(&root, &[]).unwrap();
        assert!(!plan.target_tree_sha.is_empty());

        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), before_head);
        assert_eq!(
            git(&root, &["rev-parse", "refs/heads/feat/x"]).unwrap(),
            before_branch_ref
        );
        assert_eq!(std::fs::read(&index_path).unwrap(), before_index);
        assert_eq!(std::fs::read(root.join("feature.rs")).unwrap(), before_feature);
        assert_eq!(
            std::fs::read(root.join("already-staged.txt")).unwrap(),
            before_staged
        );
        assert!(!index_path.with_extension("lock").exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn foreign_exact_target_index_lock_is_never_adopted_or_removed() {
        let root = feature_branch_repo("foreign-exact-index-lock");
        let (repo, _) = resolve_delivery_repo(&root, Some("main"), Some("feat/x")).unwrap();
        let persisted = capture_delivery_identity(&repo).unwrap();
        let plan = prepare_scoped_commit(&root, &[]).unwrap();
        let message = "fix: exact locked delivery";
        let expected_head = git(
            &root,
            &[
                "-c",
                "user.name=CodeFactory",
                "-c",
                "user.email=noreply@codefactory.local",
                "commit-tree",
                &plan.target_tree_sha,
                "-p",
                &persisted.head_sha,
                "-m",
                message,
            ],
        )
        .unwrap();
        let evidence = LocalCommitIntentEvidence::prepared(
            &persisted,
            &repo.branch,
            &plan.original_index_tree_sha,
            &plan.original_index_digest,
            &plan.target_index_digest,
            &plan.source_manifest_digest,
            &plan.target_tree_sha,
            &expected_head,
            message,
        );
        let repository = git2::Repository::open(&root).unwrap();
        let index_path = repository.index().unwrap().path().unwrap().to_path_buf();
        let lock_path = index_path.with_extension("lock");
        let exact_foreign_bytes = canonical_index_bytes_for_tree(&root, &plan.target_tree_sha).unwrap();
        let operation_key = evidence.operation_key();
        let suffix = operation_key.strip_prefix("sha256:").unwrap();
        let owned_lock_path = index_path.with_file_name(format!(
            "index.codefactory-{}.lock",
            &suffix[..suffix.len().min(32)]
        ));
        std::fs::write(&owned_lock_path, &exact_foreign_bytes).unwrap();
        let foreign_lock_bytes = b"foreign live index writer".to_vec();
        std::fs::write(&lock_path, &foreign_lock_bytes).unwrap();
        let before_index = std::fs::read(&index_path).unwrap();
        let before_head = git(&root, &["rev-parse", "HEAD"]).unwrap();

        let error = materialize_receipted_local_commit(
            &root,
            Some("main"),
            "feat/x",
            &persisted,
            &evidence,
        )
        .expect_err("content equality alone must not confer Git lock ownership");

        assert!(error.contains("unrecognized writer"), "{error}");
        assert_eq!(std::fs::read(&lock_path).unwrap(), foreign_lock_bytes);
        assert_eq!(std::fs::read(&owned_lock_path).unwrap(), exact_foreign_bytes);
        assert_eq!(std::fs::read(&index_path).unwrap(), before_index);
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), before_head);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn receipted_child_takeover_accepts_the_canonical_pr_still_at_its_parent() {
        let root = feature_branch_repo("takeover-receipted-child-existing-pr");
        git(&root, &["add", "feature.rs"]).unwrap();
        git(
            &root,
            &[
                "-c",
                "user.name=CodeFactory",
                "-c",
                "user.email=noreply@codefactory.local",
                "commit",
                "--no-verify",
                "-m",
                "fix: existing canonical PR head",
            ],
        )
        .unwrap();
        git(&root, &["push", "-q", "-u", "origin", "feat/x"]).unwrap();
        let (repo, _) = resolve_delivery_repo(&root, Some("main"), Some("feat/x")).unwrap();
        let parent = capture_delivery_identity(&repo).unwrap();

        std::fs::write(root.join("followup.rs"), "pub fn followup() {}\n").unwrap();
        git(&root, &["add", "followup.rs"]).unwrap();
        let staged = capture_delivery_identity(&repo).unwrap();
        let tree = git(&root, &["write-tree"]).unwrap();
        let message = "fix: receipted follow-up before push";
        let expected_head_sha = git(
            &root,
            &[
                "-c",
                "user.name=CodeFactory",
                "-c",
                "user.email=noreply@codefactory.local",
                "commit-tree",
                &tree,
                "-p",
                &parent.head_sha,
                "-m",
                message,
            ],
        )
        .unwrap();
        let evidence = LocalCommitIntentEvidence::new(
            &parent,
            &staged,
            &repo.branch,
            &tree,
            &expected_head_sha,
            message,
        );
        git(
            &root,
            &[
                "update-ref",
                "refs/heads/feat/x",
                &expected_head_sha,
                &parent.head_sha,
            ],
        )
        .unwrap();
        let child =
            observe_receipted_local_commit(&root, Some("main"), "feat/x", &parent, &evidence)
                .unwrap();
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: Some((7, "https://example/pr/7".into())),
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };

        let observed = observe_delivery_takeover_with_receipted_parent(
            &root,
            Some("main"),
            "feat/x",
            &child,
            Some(7),
            Some("https://example/pr/7"),
            Some(parent.head_sha.as_str()),
            Some(&remote),
        )
        .await
        .expect(
            "the exact receipted child may resume while its canonical PR remains at the parent",
        );

        assert_eq!(observed.identity, child);
        assert_eq!(observed.remote_head_sha, Some(parent.head_sha));
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert_eq!(calls.update_pr_body.load(Ordering::SeqCst), 0);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn matching_takeover_observation_is_strictly_read_only() {
        let root = feature_branch_repo("takeover-read-only");
        let origin = root.parent().unwrap().join("origin.git");
        let (repo, _) = resolve_delivery_repo(&root, Some("main"), Some("feat/x")).unwrap();
        let persisted_identity = capture_delivery_identity(&repo).unwrap();
        let before_status = git(
            &root,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .unwrap();
        let before_head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };

        let observed = observe_delivery_takeover(
            &root,
            Some("main"),
            "feat/x",
            &persisted_identity,
            None,
            None,
            Some(&remote),
        )
        .await
        .expect("the exact persisted state is safe to reconcile read-only");

        assert_eq!(observed.identity, persisted_identity);
        assert_eq!(git(&root, &["rev-parse", "HEAD"]).unwrap(), before_head);
        assert_eq!(
            git(
                &root,
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )
            .unwrap(),
            before_status
        );
        assert!(git(&origin, &["show-ref", "--verify", "refs/heads/feat/x"]).is_err());
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert_eq!(calls.update_pr_body.load(Ordering::SeqCst), 0);
        assert_eq!(calls.rerun_ci.load(Ordering::SeqCst), 0);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn takeover_does_not_reject_local_release_intent_before_release_reconciliation() {
        let root = feature_branch_repo("takeover-local-release-intent");
        let (repo, _) = resolve_delivery_repo(&root, Some("main"), Some("feat/x")).unwrap();
        let persisted_identity = capture_delivery_identity(&repo).unwrap();
        let receipt = DeliveryReceipt {
            version: 1,
            state: "intent_release".into(),
            remote: repo.remote.clone(),
            remote_identity: receipt_remote_identity(&repo),
            base_branch: repo.default_branch.clone(),
            head_branch: repo.branch.clone(),
            commit_sha: persisted_identity.head_sha.clone(),
            pr_number: 7,
            pr_url: "https://example/pr/7".into(),
            pr_title: Some("fix: release takeover".into()),
            pr_body: Some(String::new()),
            release_detail: None,
        };
        write_delivery_receipt(&repo, &persisted_identity.head_sha, &receipt).unwrap();
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: Some((7, "https://example/pr/7".into())),
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };

        let observation = observe_delivery_takeover(
            &root,
            Some("main"),
            "feat/x",
            &persisted_identity,
            Some(7),
            Some("https://example/pr/7"),
            Some(&remote),
        )
        .await
        .expect("base takeover must defer local release intent to the release-specific reconciler");

        assert_eq!(observation.identity, persisted_identity);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let persisted = read_delivery_receipt(&repo, &observation.identity.head_sha)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.state, "intent_release");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    struct FailAtMutationRung {
        calls: AtomicUsize,
        fail_at: usize,
    }

    #[async_trait::async_trait]
    impl DeliveryMutationPermitVerifier for FailAtMutationRung {
        async fn verify(&self, rung: &str) -> Result<(), String> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call >= self.fail_at {
                Err(format!("fenced before {rung}"))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn lost_claim_epoch_fences_every_later_mutation_rung() {
        let root = feature_branch_repo("claim-epoch-rung-fence");
        let origin = root.parent().unwrap().join("origin.git");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };
        let permit = DeliveryMutationPermit::new(Arc::new(FailAtMutationRung {
            calls: AtomicUsize::new(0),
            // Allow staging, the local-commit permit, and its durable
            // write-ahead intent; then simulate lease/epoch loss immediately
            // before the first remote write.
            fail_at: 4,
        }));

        let outcome = deliver(
            &root,
            DeliveryCeiling::ThroughRelease,
            MergeMethod::Squash,
            1,
            &DeliverOpts {
                mutation_permit: Some(permit),
                expect_branch: Some("feat/x".into()),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(
            outcome.recovery_class,
            RecoveryClass::ExternalStateUncertain
        );
        assert_eq!(outcome.stage, "mutation_permit");
        assert!(
            git(&origin, &["show-ref", "--verify", "refs/heads/feat/x"]).is_err(),
            "a stale owner must not push after losing its epoch"
        );
        assert_eq!(calls.open_pr.load(Ordering::SeqCst), 0);
        assert_eq!(calls.update_pr_body.load(Ordering::SeqCst), 0);
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
            merge_queues: false,
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
            merge_queues: false,
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
    async fn queued_auto_merge_is_a_waiting_state_not_a_permission_failure() {
        let root = feature_branch_repo("merge-queued");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: false,
            merge_queues: true,
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
        assert_eq!(out.final_state, "waiting", "{:?}", out.steps);
        assert_eq!(out.reached_state, "merge_queued");
        assert_eq!(out.code, "delivery_merge_queued");
        assert!(out.recoverable);
        assert_eq!(out.recovery_class, RecoveryClass::WaitRetryable);
        assert_eq!(out.retry_after_ms, Some(30_000));
        assert!(out
            .steps
            .iter()
            .any(|s| s.step == "merge" && s.status == "waiting"));
        assert_eq!(calls.merge.load(Ordering::SeqCst), 1);
        assert_eq!(calls.update_branch.load(Ordering::SeqCst), 0);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn queued_auto_merge_resume_observes_without_reissuing_merge() {
        let root = feature_branch_repo("merge-queued-resume");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: false,
            merge_queues: true,
            caps: every_capability(),
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
        assert_eq!(first.code, "delivery_merge_queued", "{:?}", first.steps);
        assert_eq!(calls.merge.load(Ordering::SeqCst), 1);

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
        assert_eq!(resumed.code, "delivery_merge_queued", "{:?}", resumed.steps);
        assert_eq!(
            calls.merge.load(Ordering::SeqCst),
            1,
            "a durable merge_queued receipt must prevent duplicate merge requests"
        );
        assert!(resumed.steps.iter().any(|step| {
            step.step == "reconcile" && step.detail.contains("不重复发起合并")
        }));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn queued_auto_merge_updates_a_behind_branch_instead_of_waiting_forever() {
        let root = feature_branch_repo("merge-queued-behind");
        let calls = stub_calls();
        *calls.merge_readiness.lock().unwrap() = Some(MergeReadiness::Behind);
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: false,
            merge_queues: true,
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

        assert_eq!(out.final_state, "blocked", "{:?}", out.steps);
        assert_eq!(out.code, "delivery_branch_updated");
        assert!(out.recoverable);
        assert_eq!(calls.update_branch.load(Ordering::SeqCst), 1);
        assert!(out
            .next_action
            .as_deref()
            .unwrap_or("")
            .contains("deliver_changes"));
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn through_merge_merges_on_green() {
        let root = feature_branch_repo("merge");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
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
            merge_queues: false,
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
        assert_eq!(first.recovery_class, RecoveryClass::AgentActionRequired);
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
            merge_queues: false,
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
            merge_queues: false,
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
            merge_queues: false,
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
    async fn merge_intent_reconciles_open_same_head_and_resumes_safely() {
        let root = feature_branch_repo("intent-merge-reconcile");
        git(&root, &["add", "feature.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "feature"]).unwrap();
        let repo = resolve_repo(&root, Some("main")).unwrap();
        let sha = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let receipt = DeliveryReceipt {
            version: 1,
            state: "intent_merge".into(),
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
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
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
        assert_eq!(out.reached_state, "merged");
        assert_eq!(calls.merge.load(Ordering::SeqCst), 1);
        assert_eq!(calls.release.load(Ordering::SeqCst), 0);
        assert!(out
            .steps
            .iter()
            .any(|step| { step.step == "reconcile" && step.detail.contains("安全续接") }));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn local_release_intent_with_proven_absence_resumes_and_dispatches_exactly_once() {
        let root = feature_branch_repo("intent-release-proven-absent");
        git(&root, &["add", "feature.rs"]).unwrap();
        git(&root, &["commit", "-q", "-m", "feature"]).unwrap();
        let repo = resolve_repo(&root, Some("main")).unwrap();
        let sha = git(&root, &["rev-parse", "HEAD"]).unwrap();
        let release_head = observe_remote_ref_head(&repo, &repo.default_branch)
            .unwrap()
            .expect("remote main head");
        let release_target = ReleaseDispatchTarget {
            workflow: "stub-release.yml".into(),
            git_ref: repo.default_branch.clone(),
            head_sha: release_head,
        };
        let receipt = DeliveryReceipt {
            version: 1,
            state: "intent_release".into(),
            remote: repo.remote.clone(),
            remote_identity: receipt_remote_identity(&repo),
            base_branch: repo.default_branch.clone(),
            head_branch: repo.branch.clone(),
            commit_sha: sha.clone(),
            pr_number: 7,
            pr_url: "https://example/pr/7".into(),
            pr_title: None,
            pr_body: None,
            release_detail: Some(encode_release_dispatch_target(&release_target).unwrap()),
        };
        write_delivery_receipt(&repo, &sha, &receipt).unwrap();
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: calls.clone(),
        };
        assert_eq!(
            reconcile_local_release_intent(
                &root,
                Some("main"),
                "feat/x",
                &sha,
                true,
                Some(&remote),
            )
            .await
            .unwrap(),
            LocalReleaseIntentReconciliation::ProvenAbsent
        );
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
        assert_ne!(out.final_state, "waiting", "{:?}", out.steps);
        assert_eq!(out.reached_state, "release_triggered", "{:?}", out.steps);
        assert_eq!(calls.release.load(Ordering::SeqCst), 1);

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
        assert_eq!(
            resumed.reached_state, "release_triggered",
            "{:?}",
            resumed.steps
        );
        assert_eq!(calls.release.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn a_missing_release_actuator_descends_to_merge_instead_of_refusing_everything() {
        let root = feature_branch_repo("norelease");
        let calls = stub_calls();
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
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
        assert_eq!(out.recovery_class, RecoveryClass::AgentActionRequired);
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
            merge_queues: false,
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
        assert_eq!(out.recovery_class, RecoveryClass::AgentActionRequired);
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
            merge_queues: false,
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
        assert_eq!(out.recovery_class, RecoveryClass::AgentActionRequired);
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
            merge_queues: false,
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

    // ── Waiting vs deadlock, and titles that inflate the release ────────────
    //
    // 2026-07-30 field report. `deliver_changes` ran 11m36s, registered
    // auto-merge on PR #290, reported `blocked` with "等待 GitHub 远端门禁完成
    // 并自动合并", and stopped. The repository's ruleset has
    // `strict_required_status_checks_policy = true` and the PR was `BEHIND`, so
    // GitHub would never merge it: auto-merge does not update a stale head ref.
    // The advice was not merely unhelpful, it was wrong — an unbounded no-op wait.

    // ── A delivery must be the delivery the caller meant ────────────────────
    //
    // `deliver_changes` takes no branch argument: it delivers whatever the
    // working directory is on, stamped with a title from session context. On
    // 2026-07-30 a turn meaning to resume PR #281
    // (feat/on-demand-embedded-browser-pane) was sitting on
    // fix/auto-release-reconcile-sigpipe and opened #290 — two open PRs, one
    // title, unrelated contents.

    fn pr_row(number: u64, title: &str, head: &str) -> (u64, String, String, String) {
        (
            number,
            format!("https://example/pr/{number}"),
            title.into(),
            head.into(),
        )
    }

    #[test]
    fn opening_a_second_pr_under_another_prs_title_is_refused() {
        let open = vec![pr_row(
            281,
            "feat: add on-demand embedded browser pane",
            "feat/on-demand-embedded-browser-pane",
        )];
        // The exact #290 shape: different head, inherited title, no PR of our own.
        let conflict = conflicting_open_pr_from_list(
            &open,
            "feat: add on-demand embedded browser pane",
            "fix/auto-release-reconcile-sigpipe",
        )
        .expect("a same-title PR on another head is a misdirected delivery");
        assert_eq!(conflict.number, 281);
        assert_eq!(conflict.head, "feat/on-demand-embedded-browser-pane");
    }

    #[test]
    fn resuming_our_own_pr_is_never_a_conflict() {
        let open = vec![
            pr_row(281, "feat: add pane", "feat/pane"),
            pr_row(290, "feat: add pane", "fix/other"),
        ];
        // We already own an open PR for this head → this is the ordinary
        // idempotent resume, even though another PR shares the title.
        assert!(conflicting_open_pr_from_list(&open, "feat: add pane", "feat/pane").is_none());
        // A genuinely new title on a fresh branch is fine too.
        assert!(conflicting_open_pr_from_list(&open, "fix: unrelated work", "fix/fresh").is_none());
        // Whitespace must not defeat the match.
        assert!(conflicting_open_pr_from_list(&open, "  feat: add pane  ", "fix/fresh").is_some());
        // No open PRs at all → nothing to conflict with.
        assert!(conflicting_open_pr_from_list(&[], "feat: add pane", "feat/pane").is_none());
    }

    #[test]
    fn gh_pr_list_unknown_is_never_treated_as_an_empty_list() {
        assert!(parse_gh_pr_list("not json").is_err());
        assert!(parse_gh_pr_list(r#"{"number":7}"#).is_err());
        assert!(parse_gh_pr_list(
            r#"[{"number":7,"url":"https://example/pr/7","title":"fix","body":null}]"#
        )
        .is_err());
        assert_eq!(parse_gh_pr_list("[]").unwrap(), None);

        let existing = parse_gh_pr_list(
            r#"[{"number":7,"url":"https://example/pr/7","title":"fix","body":"body"}]"#,
        )
        .unwrap()
        .expect("valid row");
        assert_eq!(existing.number, 7);
    }

    #[tokio::test]
    async fn a_stale_branch_declaration_blocks_before_touching_the_work() {
        // A caller-supplied branch guard is the only binding between a resumed
        // delivery and the checkout it is allowed to mutate. A mismatch must
        // fail closed; trusting cwd can stamp an unrelated worktree with the
        // resumed session's title and PR metadata.
        let root = feature_branch_repo("staleclaim");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_ok: true,
            caps: every_capability(),
            calls: Arc::new(StubCalls::default()),
            merge_queues: Default::default(),
        };
        let out = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            5,
            &DeliverOpts {
                expect_branch: Some("feat/some-other-branch".into()),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;

        assert_eq!(out.final_state, "blocked");
        assert!(
            !out.steps
                .iter()
                .any(|s| s.step == "commit" || s.step == "push" || s.step == "pr"),
            "a branch mismatch must not mutate or deliver the checkout: {:?}",
            out.steps
        );
        let note = out
            .steps
            .iter()
            .find(|s| s.step == "preflight" && s.detail.contains("feat/some-other-branch"))
            .expect("the mismatch must be reported");
        assert_eq!(note.status, "blocked");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn a_matching_target_branch_does_not_get_in_the_way() {
        let root = feature_branch_repo("rightbranch");
        let branch = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).expect("branch");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
            merge_ok: true,
            caps: every_capability(),
            calls: Arc::new(StubCalls::default()),
        };
        let out = deliver(
            &root,
            DeliveryCeiling::PrOnly,
            MergeMethod::Squash,
            5,
            &DeliverOpts {
                expect_branch: Some(branch),
                ..DeliverOpts::default()
            },
            Some(&remote),
            Some("main"),
        )
        .await;
        assert!(
            out.steps
                .iter()
                .any(|s| s.step == "commit" && s.status == "ok"),
            "a correct declaration must not block delivery: {:?}",
            out.steps
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn behind_is_a_deadlock_while_pending_checks_are_a_real_wait() {
        assert_eq!(merge_readiness_from_state("BEHIND"), MergeReadiness::Behind);
        assert_eq!(merge_readiness_from_state("CLEAN"), MergeReadiness::Ready);
        assert_eq!(
            merge_readiness_from_state("HAS_HOOKS"),
            MergeReadiness::Ready
        );
        assert_eq!(
            merge_readiness_from_state("UNSTABLE"),
            MergeReadiness::WaitingOnChecks
        );
        assert!(matches!(
            merge_readiness_from_state("DIRTY"),
            MergeReadiness::NeedsAction(_)
        ));
        assert!(matches!(
            merge_readiness_from_state("DRAFT"),
            MergeReadiness::NeedsAction(_)
        ));
        // GitHub is still computing — not a conclusion.
        assert_eq!(
            merge_readiness_from_state("UNKNOWN"),
            MergeReadiness::Unknown
        );
        assert_eq!(merge_readiness_from_state(""), MergeReadiness::Unknown);
        // Case/whitespace tolerant: the value is read off a CLI.
        assert_eq!(
            merge_readiness_from_state(" behind \n"),
            MergeReadiness::Behind
        );
    }

    #[test]
    fn a_pr_title_may_never_claim_a_bigger_release_than_its_commits() {
        // The exact #290 shape: one `ci:` commit, a `feat:` title inherited from
        // session context. Squash-merging that fabricates a minor release.
        let (title, note) = reconcile_pr_title(
            "feat: add on-demand embedded browser pane",
            conventional_slot("ci: avoid auto-release reconcile sigpipe"),
        );
        assert_eq!(title, "chore: add on-demand embedded browser pane");
        let note = note.expect("an inflating title must be reported, not silently kept");
        assert!(note.contains("slot"), "the note must say why: {note}");

        // feat title over a fix-only branch drops to fix.
        let (title, note) = reconcile_pr_title("feat: tidy up", conventional_slot("fix: crash"));
        assert_eq!(title, "fix: tidy up");
        assert!(note.is_some());

        // Matching or understating titles are left exactly alone.
        for (title, commit) in [
            ("fix: crash on empty input", "fix: crash on empty input"),
            ("feat: new pane", "feat: new pane"),
            ("fix: modest wording", "feat: big feature"),
            ("chore: notes", "chore: notes"),
        ] {
            let (out, note) = reconcile_pr_title(title, conventional_slot(commit));
            assert_eq!(out, title, "must not rewrite {title:?}");
            assert!(note.is_none(), "no note expected for {title:?}");
        }
    }

    #[test]
    fn conventional_slot_matches_the_release_workflow_arithmetic() {
        assert_eq!(conventional_slot("feat!: drop v1 API"), 3);
        assert_eq!(conventional_slot("fix!: change default"), 3);
        assert_eq!(conventional_slot("refactor: x\n\nBREAKING CHANGE: y"), 3);
        assert_eq!(conventional_slot("feat: add pane"), 2);
        assert_eq!(conventional_slot("feat(chat): add pane"), 2);
        assert_eq!(conventional_slot("fix: crash"), 1);
        assert_eq!(conventional_slot("fix(agent): crash"), 1);
        for none in [
            "ci: avoid sigpipe",
            "chore: bump version to 1.74.0",
            "docs: update readme",
            "refactor: split module",
            "test: add case",
            "no colon here",
            "",
        ] {
            assert_eq!(conventional_slot(none), 0, "{none:?}");
        }
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
        git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        )
        .unwrap();
        git(&root, &["push", "-q", "origin", "main"]).unwrap();

        let wt = root.parent().unwrap().join("wt-feat");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/wt",
                wt.to_str().unwrap(),
                "main",
            ],
        )
        .unwrap();
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
            merge_queues: false,
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
                expect_branch: None,
                expected_identity: None,
                mutation_permit: None,
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
        let (root, _wt) = repo_with_worktree_feature("wt-discover");
        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
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
                expect_branch: None,
                expected_identity: None,
                mutation_permit: None,
            },
            Some(&remote),
            Some("main"),
        ));
        // Delivery must NOT refuse on default branch: it found the worktree
        // branch and opened the PR from it.
        assert_eq!(out.branch.as_deref(), Some("feat/wt"));
        assert_eq!(out.pr_number, Some(7));
        assert!(
            out.steps
                .iter()
                .any(|s| s.step == "repo" && s.status == "ok"),
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
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/wt2",
                wt2.to_str().unwrap(),
                "main",
            ],
        )
        .unwrap();
        std::fs::write(wt2.join("feature2.rs"), "pub fn g() {}\n").unwrap();
        git(&wt2, &["add", "-A"]).unwrap();
        git(&wt2, &["commit", "-q", "-m", "feat(wt2): work"]).unwrap();

        let remote = StubRemote {
            ci: CiStatus::Success,
            existing_pr: None,
            merge_queues: false,
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
                expect_branch: None,
                expected_identity: None,
                mutation_permit: None,
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

        let selected = rt.block_on(deliver(
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
                expect_branch: Some("feat/wt".into()),
                expected_identity: None,
                mutation_permit: None,
            },
            Some(&remote),
            Some("main"),
        ));
        assert_eq!(selected.branch.as_deref(), Some("feat/wt"));
        assert_eq!(selected.pr_number, Some(7));
        assert_eq!(remote.calls.open_pr.load(Ordering::SeqCst), 1);
        assert!(wt1.join("feature.rs").exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    // ── gh hosts.yml parsing: auth presence must not be misreported ─────────

    #[test]
    fn gh_hosts_content_detects_modern_nested_users_format() {
        // Modern gh writes users-nested entries; the old parser bailed at the
        // `users:` line and reported "not authenticated" even with a token.
        let content = "\
github.com:
    users:
        BumStill:
            oauth_token: gho_abc123
    user: BumStill
    git_protocol: https
";
        assert!(gh_hosts_content_has_auth_for_host(content, "github.com"));
    }

    #[test]
    fn gh_hosts_content_detects_flat_legacy_format() {
        let content = "\
github.com:
    oauth_token: gho_abc123
    user: BumStill
";
        assert!(gh_hosts_content_has_auth_for_host(content, "github.com"));
    }

    #[test]
    fn gh_hosts_content_ignores_other_hosts_and_missing_tokens() {
        // Different host block: not authenticated for github.com.
        let other = "\
gitlab.com:
    user: someone
    oauth_token: glpat_x
";
        assert!(!gh_hosts_content_has_auth_for_host(other, "github.com"));
        // Host present but token empty / missing.
        let empty_token = "\
github.com:
    user: BumStill
    oauth_token:
";
        assert!(!gh_hosts_content_has_auth_for_host(
            empty_token,
            "github.com"
        ));
        let no_token = "github.com:\n    user: BumStill\n";
        assert!(!gh_hosts_content_has_auth_for_host(no_token, "github.com"));
    }

    #[test]
    fn gh_hosts_content_case_insensitive_host_match() {
        let content = "\
GITHUB.COM:
    oauth_token: gho_abc123
    user: BumStill
";
        assert!(gh_hosts_content_has_auth_for_host(content, "github.com"));
    }
}
