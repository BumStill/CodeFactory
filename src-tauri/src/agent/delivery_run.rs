// SPDX-License-Identifier: Apache-2.0
//! Durable delivery identity, lease ownership, and restart planning.
//!
//! Startup recovery deliberately stops at a database-only, observe-first plan.
//! Owning a lease proves that this process may reconcile the run; it does not
//! authorize a push, PR mutation, merge, release, or any other external side
//! effect. Those actions remain behind the ordinary delivery authorization and
//! remote-state reconciliation path.

use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use crate::errors::Result;

const NON_TERMINAL_PREDICATE: &str =
    "status NOT IN ('completed', 'failed', 'cancelled', 'rejected')";
const STABLE_IDENTITY_PREDICATE: &str = "(
    objective_id IS NOT NULL AND objective_id <> ''
    AND ((session_id IS NOT NULL AND session_id <> '' AND root_turn_id IS NOT NULL AND root_turn_id <> '')
      OR (task_id IS NOT NULL AND task_id <> ''))
    AND repo_identity IS NOT NULL AND repo_identity <> ''
    AND worktree_identity IS NOT NULL AND worktree_identity <> ''
    AND base_branch IS NOT NULL AND base_branch <> ''
    AND head_branch IS NOT NULL AND head_branch <> ''
    AND change_set_digest IS NOT NULL AND change_set_digest <> ''
    AND expected_head_sha IS NOT NULL AND expected_head_sha <> ''
    AND workspace_path IS NOT NULL AND workspace_path <> ''
)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub instance_id: String,
    pub app_version: String,
    pub app_build: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDeliveryRun {
    pub id: String,
    pub objective_id: String,
    pub run_kind: String,
    pub session_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub task_segment_id: Option<String>,
    pub task_id: Option<String>,
    pub workspace_path: String,
    pub worktree_identity: String,
    pub repo_identity: String,
    pub base_branch: String,
    pub head_branch: String,
    pub change_set_digest: String,
    pub expected_head_sha: String,
    pub canonical_pr_number: Option<i64>,
    pub canonical_pr_url: Option<String>,
    pub canonical_head_sha: Option<String>,
    pub requested_ceiling: String,
    pub reached_ceiling: String,
    pub stage: String,
    pub status: String,
    pub wait_class: Option<String>,
    pub next_action: Option<String>,
    pub next_action_authorized: bool,
    pub autonomous_completion: bool,
}

impl ProcessIdentity {
    pub fn new(
        instance_id: impl Into<String>,
        app_version: impl Into<String>,
        app_build: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            app_version: app_version.into(),
            app_build: app_build.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Re-read local and remote state before deciding whether any continuation
    /// is both necessary and already authorized.
    ObserveOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedRecovery {
    pub run_id: String,
    /// Monotonic fencing token for this exact DeliveryRun ownership claim.
    /// A lease owner without the current epoch may observe, but may not mutate.
    pub claim_epoch: i64,
    pub objective_id: String,
    pub workspace_path: String,
    pub worktree_identity: String,
    pub repo_identity: String,
    pub base_branch: String,
    pub head_branch: String,
    pub change_set_digest: String,
    pub expected_head_sha: String,
    pub requested_ceiling: String,
    pub autonomous_completion: bool,
    pub canonical_pr_number: Option<i64>,
    pub canonical_pr_url: Option<String>,
    pub canonical_head_sha: Option<String>,
    pub reached_ceiling: String,
    pub stage: String,
    pub status: String,
    pub wait_class: Option<String>,
    pub next_action: Option<String>,
    pub next_action_authorized: bool,
    pub failure_signature: Option<String>,
    pub stage_attempt: i64,
    pub progress_revision: i64,
    pub action: RecoveryAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartupRecoveryPlan {
    pub claimed: Vec<ClaimedRecovery>,
    /// Non-terminal expired rows without a durable source identity are
    /// reported, but not leased. This prevents historic/incomplete data from
    /// becoming authority for an external mutation.
    pub fail_closed_identity_missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryObservation {
    pub head_branch: String,
    pub stage: String,
    pub status: String,
    pub wait_class: Option<String>,
    pub next_action: Option<String>,
    pub reached_ceiling: String,
    pub expected_head_sha: String,
    pub canonical_pr_number: Option<i64>,
    pub canonical_pr_url: Option<String>,
    pub canonical_head_sha: Option<String>,
    pub failure_signature: Option<String>,
    pub core_input: Option<CoreInputRequest>,
    /// Required whenever an observation advances the expected git head. The
    /// receipt binds the revision to the stable objective/repo/worktree and to
    /// both sides of the change-set transition; lease ownership alone is not
    /// sufficient authority to rewrite delivery identity.
    pub identity_revision: Option<DeliveryIdentityRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryIdentityRevision {
    pub receipt_id: String,
    pub objective_id: String,
    pub repo_identity: String,
    pub worktree_identity: String,
    pub previous_expected_head_sha: String,
    pub previous_change_set_digest: String,
    pub next_expected_head_sha: String,
    pub next_change_set_digest: String,
}

/// Durable write-ahead record for one external delivery mutation.
///
/// `started` and `unknown` are deliberately unresolved. Either state fences
/// every later mutation until the same effect is positively reconciled; the
/// absence of remote state is never enough to delete or downgrade the record.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DeliveryMutationIntent {
    pub intent_id: String,
    pub run_id: String,
    pub claim_epoch: i64,
    pub rung: String,
    pub operation_key: String,
    pub status: String,
    pub process_instance: String,
    pub evidence_json: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
}

pub fn delivery_identity_revision_receipt_id(
    run_id: &str,
    revision: &DeliveryIdentityRevision,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        "delivery-identity-revision-v1",
        run_id,
        revision.objective_id.as_str(),
        revision.repo_identity.as_str(),
        revision.worktree_identity.as_str(),
        revision.previous_expected_head_sha.as_str(),
        revision.previous_change_set_digest.as_str(),
        revision.next_expected_head_sha.as_str(),
        revision.next_change_set_digest.as_str(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreInputRequest {
    pub request_key: String,
    pub inputs_json: String,
    pub attempts_json: String,
    pub resume_stage: String,
}

fn delivery_progress_rank(state: &str) -> Option<u8> {
    match state {
        "local" => Some(0),
        "committed" => Some(1),
        "pushed" => Some(2),
        "pr_open" => Some(3),
        "ci_green" => Some(4),
        "merge_queued" => Some(5),
        "merged" => Some(6),
        "release_triggered" => Some(7),
        "deployment_succeeded" => Some(8),
        "live_verified" => Some(9),
        _ => None,
    }
}

fn monotonic_reached_ceiling(previous: &str, observed: &str) -> String {
    match (
        delivery_progress_rank(previous),
        delivery_progress_rank(observed),
    ) {
        (Some(previous_rank), Some(observed_rank)) if previous_rank > observed_rank => {
            previous.to_string()
        }
        (Some(_), None) => previous.to_string(),
        _ => observed.to_string(),
    }
}

async fn ensure_delivery_run_column(pool: &SqlitePool, name: &str, definition: &str) -> Result<()> {
    let columns = sqlx::query("PRAGMA table_info(delivery_runs)")
        .fetch_all(pool)
        .await?;
    if columns
        .iter()
        .any(|column| column.try_get::<String, _>("name").ok().as_deref() == Some(name))
    {
        return Ok(());
    }
    sqlx::query(&format!(
        "ALTER TABLE delivery_runs ADD COLUMN {name} {definition}"
    ))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn ensure_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS delivery_runs (
            id                     TEXT PRIMARY KEY,
            objective_id           TEXT,
            run_kind               TEXT NOT NULL,
            session_id             TEXT,
            root_turn_id           TEXT,
            task_segment_id        TEXT,
            task_id                TEXT,
            workspace_path         TEXT,
            worktree_identity      TEXT,
            repo_identity          TEXT,
            base_branch            TEXT,
            head_branch            TEXT,
            change_set_digest      TEXT,
            expected_head_sha      TEXT,
            canonical_pr_number    INTEGER,
            canonical_pr_url       TEXT,
            canonical_head_sha     TEXT,
            requested_ceiling      TEXT NOT NULL,
            reached_ceiling        TEXT NOT NULL,
            stage                  TEXT NOT NULL,
            status                 TEXT NOT NULL,
            wait_class             TEXT,
            next_action            TEXT,
            next_action_authorized INTEGER NOT NULL DEFAULT 0 CHECK(next_action_authorized IN (0, 1)),
            autonomous_completion  INTEGER NOT NULL DEFAULT 0 CHECK(autonomous_completion IN (0, 1)),
            decision_policy        TEXT NOT NULL DEFAULT 'apply_recommended' CHECK(decision_policy IN ('apply_recommended', 'require_irreversible_decision')),
            failure_signature      TEXT,
            stage_attempt          INTEGER NOT NULL DEFAULT 0 CHECK(stage_attempt >= 0),
            lease_owner            TEXT,
            lease_expires_at       INTEGER,
            claim_epoch            INTEGER NOT NULL DEFAULT 0 CHECK(claim_epoch >= 0),
            reconciled_claim_epoch INTEGER NOT NULL DEFAULT 0 CHECK(reconciled_claim_epoch >= 0 AND reconciled_claim_epoch <= claim_epoch),
            last_observed_at       INTEGER NOT NULL,
            last_progress_at       INTEGER NOT NULL,
            progress_revision      INTEGER NOT NULL DEFAULT 0 CHECK(progress_revision >= 0),
            app_version            TEXT NOT NULL,
            app_build              TEXT NOT NULL,
            process_instance       TEXT NOT NULL,
            created_app_version    TEXT,
            created_app_build      TEXT,
            created_process_instance TEXT,
            last_observed_app_version TEXT,
            last_observed_app_build TEXT,
            last_observed_process_instance TEXT,
            recovery_attempt       INTEGER NOT NULL DEFAULT 0 CHECK(recovery_attempt >= 0),
            failure_code           TEXT,
            failure_class          TEXT,
            queue_wait_ms          INTEGER,
            runtime_ms             INTEGER,
            remediation_id         TEXT,
            business_decision_key  TEXT,
            decision_options_json  TEXT,
            recommended_option     TEXT,
            safe_default_action    TEXT,
            decision_reason        TEXT,
            core_input_request_key TEXT,
            core_inputs_json       TEXT,
            core_input_attempts_json TEXT,
            core_input_resume_stage TEXT,
            core_input_request_count INTEGER NOT NULL DEFAULT 0 CHECK(core_input_request_count BETWEEN 0 AND 1),
            created_at             INTEGER NOT NULL,
            updated_at             INTEGER NOT NULL,
            CHECK(status <> 'needs_user'),
            CHECK(
              status <> 'needs_business_decision'
              OR (
                decision_policy = 'require_irreversible_decision'
                AND business_decision_key IS NOT NULL AND business_decision_key <> ''
                AND decision_options_json IS NOT NULL AND decision_options_json <> ''
                AND recommended_option IS NOT NULL AND recommended_option <> ''
                AND safe_default_action IS NOT NULL AND safe_default_action <> ''
                AND decision_reason IS NOT NULL AND decision_reason <> ''
              )
            ),
            CHECK(
              status <> 'core_input_required'
              OR (
                core_input_request_key IS NOT NULL AND core_input_request_key <> ''
                AND core_inputs_json IS NOT NULL AND core_inputs_json <> ''
                AND core_input_attempts_json IS NOT NULL AND core_input_attempts_json <> ''
                AND core_input_resume_stage IS NOT NULL AND core_input_resume_stage <> ''
                AND core_input_request_count = 1
              )
            )
        )",
    )
    .execute(pool)
    .await?;

    for (name, definition) in [
        ("objective_id", "TEXT"),
        ("worktree_identity", "TEXT"),
        ("created_app_version", "TEXT"),
        ("created_app_build", "TEXT"),
        ("created_process_instance", "TEXT"),
        ("last_observed_app_version", "TEXT"),
        ("last_observed_app_build", "TEXT"),
        ("last_observed_process_instance", "TEXT"),
        (
            "recovery_attempt",
            "INTEGER NOT NULL DEFAULT 0 CHECK(recovery_attempt >= 0)",
        ),
        ("failure_code", "TEXT"),
        ("failure_class", "TEXT"),
        ("queue_wait_ms", "INTEGER"),
        ("runtime_ms", "INTEGER"),
        ("remediation_id", "TEXT"),
        (
            "claim_epoch",
            "INTEGER NOT NULL DEFAULT 0 CHECK(claim_epoch >= 0)",
        ),
        (
            "reconciled_claim_epoch",
            "INTEGER NOT NULL DEFAULT 0 CHECK(reconciled_claim_epoch >= 0 AND reconciled_claim_epoch <= claim_epoch)",
        ),
    ] {
        ensure_delivery_run_column(pool, name, definition).await?;
    }

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS delivery_run_events (
            id                 TEXT PRIMARY KEY,
            run_id             TEXT NOT NULL,
            event_kind         TEXT NOT NULL,
            stage              TEXT NOT NULL,
            status             TEXT NOT NULL,
            wait_class         TEXT,
            detail_json        TEXT,
            process_instance   TEXT NOT NULL,
            created_at         INTEGER NOT NULL,
            FOREIGN KEY(run_id) REFERENCES delivery_runs(id) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS objective_recovery_attempts (
            id TEXT PRIMARY KEY,
            objective_id TEXT,
            root_turn_id TEXT,
            delivery_run_id TEXT,
            domain TEXT NOT NULL,
            attempt_index INTEGER NOT NULL CHECK(attempt_index >= 1),
            failure_code TEXT NOT NULL,
            failure_class TEXT NOT NULL,
            output_started INTEGER NOT NULL DEFAULT 0 CHECK(output_started IN (0, 1)),
            side_effect_started INTEGER NOT NULL DEFAULT 0 CHECK(side_effect_started IN (0, 1)),
            queue_wait_ms INTEGER,
            runtime_ms INTEGER,
            process_instance TEXT NOT NULL,
            resume_owner TEXT NOT NULL,
            terminal_decision TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(delivery_run_id) REFERENCES delivery_runs(id) ON DELETE SET NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS delivery_identity_revisions (
            receipt_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            objective_id TEXT NOT NULL,
            repo_identity TEXT NOT NULL,
            worktree_identity TEXT NOT NULL,
            previous_expected_head_sha TEXT NOT NULL,
            previous_change_set_digest TEXT NOT NULL,
            next_expected_head_sha TEXT NOT NULL,
            next_change_set_digest TEXT NOT NULL,
            process_instance TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY(run_id) REFERENCES delivery_runs(id) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS delivery_mutation_intents (
            intent_id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            claim_epoch INTEGER NOT NULL CHECK(claim_epoch > 0),
            rung TEXT NOT NULL CHECK(rung <> ''),
            operation_key TEXT NOT NULL CHECK(operation_key <> ''),
            status TEXT NOT NULL CHECK(status IN (
                'started', 'committed', 'unknown', 'reconciled_committed'
            )),
            process_instance TEXT NOT NULL CHECK(process_instance <> ''),
            evidence_json TEXT,
            started_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY(run_id) REFERENCES delivery_runs(id) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;

    for index in [
        "CREATE INDEX IF NOT EXISTS idx_delivery_runs_recovery ON delivery_runs(status, lease_expires_at)",
        "CREATE INDEX IF NOT EXISTS idx_delivery_runs_session ON delivery_runs(session_id, updated_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_delivery_runs_pr ON delivery_runs(canonical_pr_number, canonical_head_sha)",
        "CREATE INDEX IF NOT EXISTS idx_delivery_run_events_run ON delivery_run_events(run_id, created_at)",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_delivery_runs_active_objective_repo ON delivery_runs(objective_id, repo_identity) WHERE objective_id IS NOT NULL AND objective_id <> '' AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')",
        "CREATE INDEX IF NOT EXISTS idx_objective_recovery_attempts_objective ON objective_recovery_attempts(objective_id, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_objective_recovery_attempts_turn ON objective_recovery_attempts(root_turn_id, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_delivery_identity_revisions_run ON delivery_identity_revisions(run_id, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_delivery_mutation_intents_run ON delivery_mutation_intents(run_id, started_at)",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_delivery_mutation_intents_one_unresolved ON delivery_mutation_intents(run_id) WHERE status IN ('started', 'unknown')",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_delivery_mutation_intents_operation ON delivery_mutation_intents(run_id, rung, operation_key)",
    ] {
        sqlx::query(index).execute(pool).await?;
    }
    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS trg_delivery_runs_requested_ceiling_immutable
         BEFORE UPDATE OF requested_ceiling ON delivery_runs
         WHEN NEW.requested_ceiling <> OLD.requested_ceiling
         BEGIN
           SELECT RAISE(ABORT, 'delivery requested ceiling is immutable');
         END",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Persists a newly-authoritative delivery run and its initial audit event.
/// Legacy/imported rows may lack source identity so they can be represented and
/// failed closed, but new product writes must always carry either a chat-turn
/// identity or a task identity.
pub async fn create_delivery_run(
    pool: &SqlitePool,
    run: &NewDeliveryRun,
    process: &ProcessIdentity,
    now: i64,
    lease_ttl: i64,
) -> Result<i64> {
    let chat_identity = run
        .session_id
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        && run
            .root_turn_id
            .as_deref()
            .is_some_and(|value| !value.is_empty());
    let task_identity = run
        .task_id
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    if !chat_identity && !task_identity {
        return Err(crate::errors::AppError::Other(
            "durable delivery run requires a chat-turn or task identity".into(),
        ));
    }
    if run.objective_id.is_empty() || run.worktree_identity.is_empty() {
        return Err(crate::errors::AppError::Other(
            "durable delivery run requires stable objective and worktree identity".into(),
        ));
    }
    if [
        run.repo_identity.as_str(),
        run.workspace_path.as_str(),
        run.base_branch.as_str(),
        run.head_branch.as_str(),
        run.change_set_digest.as_str(),
        run.expected_head_sha.as_str(),
    ]
    .iter()
    .any(|value| value.is_empty())
    {
        return Err(crate::errors::AppError::Other(
            "durable delivery run requires repo/base/head/change-set/expected-head identity".into(),
        ));
    }
    if run.canonical_pr_number.is_some() != run.canonical_pr_url.is_some()
        || run
            .canonical_pr_url
            .as_deref()
            .is_some_and(|url| url.trim().is_empty())
    {
        return Err(crate::errors::AppError::Other(
            "durable delivery run requires canonical PR number and URL to be first-bound together"
                .into(),
        ));
    }

    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO delivery_runs (
            id, objective_id, run_kind, session_id, root_turn_id, task_segment_id, task_id, workspace_path,
            worktree_identity, repo_identity, base_branch, head_branch, change_set_digest, expected_head_sha,
            canonical_pr_number, canonical_pr_url, canonical_head_sha,
            requested_ceiling, reached_ceiling, stage, status, wait_class, next_action,
            next_action_authorized, autonomous_completion,
            failure_signature, stage_attempt, lease_owner, lease_expires_at,
            last_observed_at, last_progress_at, progress_revision, app_version,
            app_build, process_instance, created_app_version, created_app_build,
            created_process_instance, last_observed_app_version, last_observed_app_build,
            last_observed_process_instance, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&run.id)
    .bind(&run.objective_id)
    .bind(&run.run_kind)
    .bind(&run.session_id)
    .bind(&run.root_turn_id)
    .bind(&run.task_segment_id)
    .bind(&run.task_id)
    .bind(&run.workspace_path)
    .bind(&run.worktree_identity)
    .bind(&run.repo_identity)
    .bind(&run.base_branch)
    .bind(&run.head_branch)
    .bind(&run.change_set_digest)
    .bind(&run.expected_head_sha)
    .bind(run.canonical_pr_number)
    .bind(&run.canonical_pr_url)
    .bind(&run.canonical_head_sha)
    .bind(&run.requested_ceiling)
    .bind(&run.reached_ceiling)
    .bind(&run.stage)
    .bind(&run.status)
    .bind(&run.wait_class)
    .bind(&run.next_action)
    .bind(i64::from(run.next_action_authorized))
    .bind(i64::from(run.autonomous_completion))
    .bind(&process.instance_id)
    .bind(now.saturating_add(lease_ttl))
    .bind(now)
    .bind(now)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(&process.instance_id)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(&process.instance_id)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(&process.instance_id)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if inserted == 0 {
        let existing = sqlx::query(
            "SELECT objective_id, run_kind, session_id, task_id,
                    workspace_path, worktree_identity, repo_identity, base_branch, head_branch,
                    change_set_digest, expected_head_sha, requested_ceiling,
                    lease_owner, lease_expires_at, claim_epoch
             FROM delivery_runs WHERE id=?",
        )
        .bind(&run.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "delivery run identity disappeared during collision reconciliation".into(),
            )
        })?;
        let identity_matches = existing.try_get::<String, _>("objective_id")? == run.objective_id
            && existing.try_get::<String, _>("run_kind")? == run.run_kind
            && existing.try_get::<Option<String>, _>("session_id")? == run.session_id
            && existing.try_get::<Option<String>, _>("task_id")? == run.task_id
            && existing.try_get::<String, _>("workspace_path")? == run.workspace_path
            && existing.try_get::<String, _>("worktree_identity")? == run.worktree_identity
            && existing.try_get::<String, _>("repo_identity")? == run.repo_identity
            && existing.try_get::<String, _>("base_branch")? == run.base_branch
            && existing.try_get::<String, _>("head_branch")? == run.head_branch
            && existing.try_get::<String, _>("change_set_digest")? == run.change_set_digest
            && existing.try_get::<String, _>("expected_head_sha")? == run.expected_head_sha
            && existing.try_get::<String, _>("requested_ceiling")? == run.requested_ceiling;
        if !identity_matches {
            return Err(crate::errors::AppError::Other(
                "delivery run identity collision: objective/worktree/change-set changed; refused before side effects"
                    .into(),
            ));
        }
        let existing_owner = existing.try_get::<Option<String>, _>("lease_owner")?;
        let existing_lease_expires_at = existing.try_get::<Option<i64>, _>("lease_expires_at")?;
        if existing_owner.as_deref() != Some(process.instance_id.as_str())
            && existing_lease_expires_at.is_some_and(|expires_at| expires_at > now)
        {
            return Err(crate::errors::AppError::Other(
                "delivery run already has an active invocation; attach to the existing objective instead of starting a concurrent worktree mutation"
                    .into(),
            ));
        }
        // A foreground retry in the same live process is an idempotent lease
        // renewal, not a new ownership claim. Advancing the epoch here would
        // strand the normal execution path behind its own observe-only fence.
        // Expired or different-owner claims still advance monotonically and
        // therefore require takeover reconciliation before mutation.
        let same_live_claim = existing_owner.as_deref() == Some(process.instance_id.as_str())
            && existing_lease_expires_at.is_some_and(|expires_at| expires_at > now);
        let current_claim_epoch = existing.try_get::<i64, _>("claim_epoch")?;
        let next_claim_epoch = if same_live_claim {
            current_claim_epoch
        } else {
            current_claim_epoch.saturating_add(1)
        };
        let renewed = sqlx::query(
            "UPDATE delivery_runs
             SET lease_owner=?, lease_expires_at=?, process_instance=?, app_version=?, app_build=?,
                 claim_epoch=?,
                 root_turn_id=COALESCE(?, root_turn_id), task_segment_id=COALESCE(?, task_segment_id),
                 last_observed_app_version=?, last_observed_app_build=?, last_observed_process_instance=?,
                 next_action_authorized=MAX(next_action_authorized, ?),
                 autonomous_completion=MAX(autonomous_completion, ?), updated_at=?
             WHERE id=? AND objective_id=? AND repo_identity=?
               AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
               AND (lease_owner=? OR lease_expires_at IS NULL OR lease_expires_at <= ?)",
        )
        .bind(&process.instance_id)
        .bind(now.saturating_add(lease_ttl))
        .bind(&process.instance_id)
        .bind(&process.app_version)
        .bind(&process.app_build)
        .bind(next_claim_epoch)
        .bind(&run.root_turn_id)
        .bind(&run.task_segment_id)
        .bind(&process.app_version)
        .bind(&process.app_build)
        .bind(&process.instance_id)
        .bind(i64::from(run.next_action_authorized))
        .bind(i64::from(run.autonomous_completion))
        .bind(now)
        .bind(&run.id)
        .bind(&run.objective_id)
        .bind(&run.repo_identity)
        .bind(&process.instance_id)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if renewed == 0 {
            return Err(crate::errors::AppError::Other(
                "delivery run id collision or source identity changed".into(),
            ));
        }
    } else {
        // Epoch zero is reserved for legacy rows that have never held a
        // mutation-capable claim. A newly-created run is already reconciled
        // against the identity captured before any side effect.
        sqlx::query(
            "UPDATE delivery_runs
             SET claim_epoch=1, reconciled_claim_epoch=1
             WHERE id=? AND lease_owner=?",
        )
        .bind(&run.id)
        .bind(&process.instance_id)
        .execute(&mut *tx)
        .await?;
    }

    let objectives_exist: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type='table' AND name='objectives'",
    )
    .fetch_one(&mut *tx)
    .await?;
    if objectives_exist == 1 {
        let linked = sqlx::query(
            "UPDATE objectives
             SET delivery_run_id=?, updated_at=?
             WHERE id=? AND status IN ('active','waiting_system')
               AND (delivery_run_id IS NULL OR delivery_run_id=?)",
        )
        .bind(&run.id)
        .bind(now)
        .bind(&run.objective_id)
        .bind(&run.id)
        .execute(&mut *tx)
        .await?;
        if linked.rows_affected() != 1 {
            return Err(crate::errors::AppError::Other(format!(
                "durable DeliveryRun pointer conflict for Objective {}; run creation rolled back before external mutation",
                run.objective_id
            )));
        }
    }

    if inserted > 0 {
        sqlx::query(
        "INSERT INTO delivery_run_events
         (id, run_id, event_kind, stage, status, wait_class, detail_json, process_instance, created_at)
         VALUES (?, ?, 'created', ?, ?, ?, NULL, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&run.id)
    .bind(&run.stage)
    .bind(&run.status)
    .bind(&run.wait_class)
    .bind(&process.instance_id)
    .bind(now)
    .execute(&mut *tx)
        .await?;
    }
    let claim_epoch: i64 =
        sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=? AND lease_owner=?")
            .bind(&run.id)
            .bind(&process.instance_id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(claim_epoch)
}

/// Extend a live DeliveryRun lease without changing business progress. The
/// owner and non-expired predicates form the CAS: a former or competing owner
/// cannot revive or steal a run through the heartbeat path.
pub async fn renew_delivery_lease(
    pool: &SqlitePool,
    run_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    now: i64,
    lease_ttl: i64,
) -> Result<bool> {
    if lease_ttl <= 0 {
        return Err(crate::errors::AppError::Other(
            "delivery lease heartbeat requires a positive TTL".into(),
        ));
    }
    if claim_epoch <= 0 {
        return Ok(false);
    }
    let renewed = sqlx::query(
        "UPDATE delivery_runs
         SET lease_expires_at=?, last_observed_at=?, updated_at=?,
             process_instance=?, app_version=?, app_build=?,
             last_observed_app_version=?, last_observed_app_build=?,
             last_observed_process_instance=?
         WHERE id=? AND lease_owner=? AND claim_epoch=? AND lease_expires_at>?
           AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
           AND COALESCE(wait_class, '') <> 'legacy_orphan'",
    )
    .bind(now.saturating_add(lease_ttl))
    .bind(now)
    .bind(now)
    .bind(&process.instance_id)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(&process.instance_id)
    .bind(run_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .bind(now)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(renewed == 1)
}

fn normalized_mutation_evidence(
    evidence_json: Option<&str>,
    required: bool,
) -> Result<Option<String>> {
    let Some(evidence_json) = evidence_json else {
        if required {
            return Err(crate::errors::AppError::Other(
                "positive delivery mutation reconciliation requires evidence JSON".into(),
            ));
        }
        return Ok(None);
    };
    if evidence_json.trim().is_empty() {
        return Err(crate::errors::AppError::Other(
            "delivery mutation evidence JSON cannot be empty".into(),
        ));
    }
    let evidence: serde_json::Value = serde_json::from_str(evidence_json)?;
    if required && evidence.is_null() {
        return Err(crate::errors::AppError::Other(
            "positive delivery mutation reconciliation requires non-null evidence".into(),
        ));
    }
    Ok(Some(evidence.to_string()))
}

fn validated_positive_mutation_reconciliation_evidence(
    evidence_json: Option<&str>,
    intent: &DeliveryMutationIntent,
) -> Result<String> {
    let normalized = normalized_mutation_evidence(evidence_json, true)?
        .ok_or_else(|| crate::errors::AppError::Other("missing reconciliation evidence".into()))?;
    let envelope: serde_json::Value = serde_json::from_str(&normalized)?;
    if envelope.get("rung").and_then(serde_json::Value::as_str) != Some(intent.rung.as_str())
        || envelope
            .get("operation_key")
            .and_then(serde_json::Value::as_str)
            != Some(intent.operation_key.as_str())
    {
        return Err(crate::errors::AppError::Other(
            "positive delivery mutation evidence must bind the persisted rung and operation key"
                .into(),
        ));
    }
    let observation = envelope
        .get("observation")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            crate::errors::AppError::Other(
                "positive delivery mutation evidence requires a structured observation".into(),
            )
        })?;
    let confirmation = observation
        .get("confirmation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let non_empty = |key: &str| {
        observation
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    };
    let positive = match confirmation {
        "remote_head_matches" => non_empty("remote_head_sha"),
        "open_pr_matches" => {
            observation
                .get("pr_number")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|number| number > 0)
                && non_empty("pr_url")
                && non_empty("head_sha")
        }
        "pr_body_matches" => {
            observation
                .get("pr_number")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|number| number > 0)
                && non_empty("body_digest")
        }
        "merge_observed" => {
            observation
                .get("pr_number")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|number| number > 0)
                && non_empty("merge_sha")
        }
        "auto_merge_observed" => {
            observation
                .get("pr_number")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|number| number > 0)
                && non_empty("head_sha")
        }
        "release_observed" => non_empty("head_sha") && non_empty("detail_digest"),
        _ => false,
    };
    if !positive {
        return Err(crate::errors::AppError::Other(
            "delivery mutation reconciliation requires positive domain evidence; absence or an empty assertion is not sufficient"
                .into(),
        ));
    }
    Ok(normalized)
}

fn mutation_intent_detail_json(
    intent_id: &str,
    claim_epoch: i64,
    rung: &str,
    operation_key: &str,
    status: &str,
    evidence_json: Option<&str>,
) -> String {
    let evidence: Option<serde_json::Value> =
        evidence_json.and_then(|value| serde_json::from_str(value).ok());
    serde_json::json!({
        "intent_id": intent_id,
        "claim_epoch": claim_epoch,
        "rung": rung,
        "operation_key": operation_key,
        "status": status,
        "evidence": evidence,
    })
    .to_string()
}

async fn insert_mutation_intent_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    run_id: &str,
    event_kind: &str,
    detail_json: &str,
    process_instance: &str,
    now: i64,
) -> Result<()> {
    let row = sqlx::query("SELECT stage, status, wait_class FROM delivery_runs WHERE id=?")
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO delivery_run_events
         (id, run_id, event_kind, stage, status, wait_class, detail_json, process_instance, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(run_id)
    .bind(event_kind)
    .bind(row.try_get::<String, _>("stage")?)
    .bind(row.try_get::<String, _>("status")?)
    .bind(row.try_get::<Option<String>, _>("wait_class")?)
    .bind(detail_json)
    .bind(process_instance)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Commit a write-ahead intent before dispatching one Git or remote mutation.
///
/// The insert and audit event share a transaction. Returning `true` therefore
/// means a restart or competing process can already observe the unresolved
/// operation. A stale owner, an unreconciled/expired epoch, or a pre-existing
/// unresolved intent returns `false` without writing anything.
#[allow(clippy::too_many_arguments)]
pub async fn begin_delivery_mutation_intent(
    pool: &SqlitePool,
    intent_id: &str,
    run_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    rung: &str,
    operation_key: &str,
    evidence_json: Option<&str>,
    now: i64,
) -> Result<bool> {
    if intent_id.trim().is_empty()
        || run_id.trim().is_empty()
        || rung.trim().is_empty()
        || operation_key.trim().is_empty()
        || process.instance_id.trim().is_empty()
    {
        return Err(crate::errors::AppError::Other(
            "delivery mutation intent requires non-empty id/run/rung/operation/process identity"
                .into(),
        ));
    }
    if claim_epoch <= 0 {
        return Ok(false);
    }
    let evidence_json = normalized_mutation_evidence(evidence_json, false)?;
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO delivery_mutation_intents
         (intent_id, run_id, claim_epoch, rung, operation_key, status,
          process_instance, evidence_json, started_at, updated_at)
         SELECT ?, delivery_runs.id, ?, ?, ?, 'started', ?, ?, ?, ?
         FROM delivery_runs
         WHERE delivery_runs.id=? AND delivery_runs.lease_owner=?
           AND delivery_runs.claim_epoch=?
           AND delivery_runs.reconciled_claim_epoch=delivery_runs.claim_epoch
           AND delivery_runs.lease_expires_at>?
           AND delivery_runs.status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
           AND COALESCE(delivery_runs.wait_class, '') <> 'legacy_orphan'
           AND NOT EXISTS (
             SELECT 1 FROM delivery_mutation_intents unresolved
             WHERE unresolved.run_id=delivery_runs.id
               AND unresolved.status IN ('started', 'unknown')
           )",
    )
    .bind(intent_id)
    .bind(claim_epoch)
    .bind(rung)
    .bind(operation_key)
    .bind(&process.instance_id)
    .bind(&evidence_json)
    .bind(now)
    .bind(now)
    .bind(run_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        tx.commit().await?;
        return Ok(false);
    }

    // A caller can wait behind a SQLite writer after capturing `now`. Recheck
    // against the wall clock at the commit boundary for production epoch-ms
    // calls so an already-expired owner cannot dispatch using a stale
    // timestamp. Small synthetic clocks remain deterministic in unit tests.
    let commit_boundary_now = if now >= 1_000_000_000_000 {
        chrono::Utc::now().timestamp_millis()
    } else {
        now
    };
    let still_authorized: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_runs
         WHERE id=? AND lease_owner=? AND claim_epoch=?
           AND reconciled_claim_epoch=claim_epoch AND lease_expires_at>?",
    )
    .bind(run_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .bind(commit_boundary_now)
    .fetch_one(&mut *tx)
    .await?;
    if still_authorized != 1 {
        tx.rollback().await?;
        return Ok(false);
    }

    let detail_json = mutation_intent_detail_json(
        intent_id,
        claim_epoch,
        rung,
        operation_key,
        "started",
        evidence_json.as_deref(),
    );
    insert_mutation_intent_event(
        &mut tx,
        run_id,
        "mutation_intent_started",
        &detail_json,
        &process.instance_id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn get_delivery_mutation_intent(
    pool: &SqlitePool,
    intent_id: &str,
) -> Result<Option<DeliveryMutationIntent>> {
    Ok(sqlx::query_as::<_, DeliveryMutationIntent>(
        "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                process_instance, evidence_json, started_at, updated_at
         FROM delivery_mutation_intents WHERE intent_id=?",
    )
    .bind(intent_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_unresolved_delivery_mutation_intents(
    pool: &SqlitePool,
    run_id: &str,
) -> Result<Vec<DeliveryMutationIntent>> {
    Ok(sqlx::query_as::<_, DeliveryMutationIntent>(
        "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                process_instance, evidence_json, started_at, updated_at
         FROM delivery_mutation_intents
         WHERE run_id=? AND status IN ('started', 'unknown')
         ORDER BY started_at, intent_id",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?)
}

/// Settle a successful effect under the owner/epoch that created the intent.
/// Lease expiry is intentionally not a predicate: an effect can complete just
/// after the heartbeat is lost, and recording that success prevents replay.
pub async fn resolve_delivery_mutation_intent_committed(
    pool: &SqlitePool,
    intent_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    evidence_json: Option<&str>,
    now: i64,
) -> Result<bool> {
    settle_original_delivery_mutation_intent(
        pool,
        intent_id,
        process,
        claim_epoch,
        "committed",
        "mutation_intent_committed",
        evidence_json,
        now,
    )
    .await
}

/// Mark a timeout, lease loss, or otherwise indeterminate result. If this
/// transaction itself fails, the original `started` row remains unresolved,
/// which is the same fail-closed replay posture.
pub async fn mark_delivery_mutation_intent_unknown(
    pool: &SqlitePool,
    intent_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    evidence_json: Option<&str>,
    now: i64,
) -> Result<bool> {
    settle_original_delivery_mutation_intent(
        pool,
        intent_id,
        process,
        claim_epoch,
        "unknown",
        "mutation_intent_unknown",
        evidence_json,
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn settle_original_delivery_mutation_intent(
    pool: &SqlitePool,
    intent_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    next_status: &str,
    event_kind: &str,
    evidence_json: Option<&str>,
    now: i64,
) -> Result<bool> {
    if intent_id.trim().is_empty() || claim_epoch <= 0 {
        return Ok(false);
    }
    let evidence_json = normalized_mutation_evidence(evidence_json, false)?;
    let allowed_previous = if next_status == "unknown" {
        "status='started'"
    } else {
        "status IN ('started', 'unknown')"
    };
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(&format!(
        "UPDATE delivery_mutation_intents
         SET status=?, evidence_json=COALESCE(?, evidence_json), updated_at=?
         WHERE intent_id=? AND process_instance=? AND claim_epoch=?
           AND {allowed_previous}"
    ))
    .bind(next_status)
    .bind(&evidence_json)
    .bind(now)
    .bind(intent_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if updated == 0 {
        let already_settled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_mutation_intents
             WHERE intent_id=? AND process_instance=? AND claim_epoch=? AND status=?",
        )
        .bind(intent_id)
        .bind(&process.instance_id)
        .bind(claim_epoch)
        .bind(next_status)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(already_settled == 1);
    }

    let intent = sqlx::query_as::<_, DeliveryMutationIntent>(
        "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                process_instance, evidence_json, started_at, updated_at
         FROM delivery_mutation_intents WHERE intent_id=?",
    )
    .bind(intent_id)
    .fetch_one(&mut *tx)
    .await?;
    let detail_json = mutation_intent_detail_json(
        &intent.intent_id,
        intent.claim_epoch,
        &intent.rung,
        &intent.operation_key,
        &intent.status,
        intent.evidence_json.as_deref(),
    );
    insert_mutation_intent_event(
        &mut tx,
        &intent.run_id,
        event_kind,
        &detail_json,
        &process.instance_id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Resolve an old unresolved intent only after a new, still observe-only
/// owner has positive domain evidence that the effect committed. The intent's
/// original owner remains recorded; the reconciling owner is captured by the
/// audit event.
pub async fn mark_delivery_mutation_intent_reconciled_committed(
    pool: &SqlitePool,
    intent_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    evidence_json: Option<&str>,
    now: i64,
) -> Result<bool> {
    if intent_id.trim().is_empty() || claim_epoch <= 0 {
        return Ok(false);
    }
    let mut tx = pool.begin().await?;
    let Some(existing_intent) = sqlx::query_as::<_, DeliveryMutationIntent>(
        "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                process_instance, evidence_json, started_at, updated_at
         FROM delivery_mutation_intents WHERE intent_id=?",
    )
    .bind(intent_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.commit().await?;
        return Ok(false);
    };
    let evidence_json =
        validated_positive_mutation_reconciliation_evidence(evidence_json, &existing_intent)?;
    if existing_intent.status == "reconciled_committed"
        && existing_intent.evidence_json.as_deref() != Some(evidence_json.as_str())
    {
        return Err(crate::errors::AppError::Other(
            "replayed delivery mutation reconciliation supplied conflicting evidence".into(),
        ));
    }
    let updated = sqlx::query(
        "UPDATE delivery_mutation_intents
         SET status='reconciled_committed', evidence_json=?, updated_at=?
         WHERE intent_id=? AND status IN ('started', 'unknown')
           AND EXISTS (
             SELECT 1 FROM delivery_runs
             WHERE delivery_runs.id=delivery_mutation_intents.run_id
               AND delivery_runs.lease_owner=?
               AND delivery_runs.claim_epoch=?
               AND delivery_runs.claim_epoch>delivery_mutation_intents.claim_epoch
               AND delivery_runs.reconciled_claim_epoch<delivery_runs.claim_epoch
               AND delivery_runs.lease_expires_at>?
               AND delivery_runs.status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
               AND COALESCE(delivery_runs.wait_class, '') <> 'legacy_orphan'
           )",
    )
    .bind(&evidence_json)
    .bind(now)
    .bind(intent_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if updated == 0 {
        let already_reconciled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_mutation_intents
             JOIN delivery_runs ON delivery_runs.id=delivery_mutation_intents.run_id
             WHERE delivery_mutation_intents.intent_id=?
               AND delivery_mutation_intents.status='reconciled_committed'
               AND delivery_runs.lease_owner=? AND delivery_runs.claim_epoch=?
               AND delivery_runs.reconciled_claim_epoch<delivery_runs.claim_epoch
               AND delivery_runs.lease_expires_at>?",
        )
        .bind(intent_id)
        .bind(&process.instance_id)
        .bind(claim_epoch)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(already_reconciled == 1);
    }

    let intent = sqlx::query_as::<_, DeliveryMutationIntent>(
        "SELECT intent_id, run_id, claim_epoch, rung, operation_key, status,
                process_instance, evidence_json, started_at, updated_at
         FROM delivery_mutation_intents WHERE intent_id=?",
    )
    .bind(intent_id)
    .fetch_one(&mut *tx)
    .await?;
    let detail_json = mutation_intent_detail_json(
        &intent.intent_id,
        claim_epoch,
        &intent.rung,
        &intent.operation_key,
        &intent.status,
        intent.evidence_json.as_deref(),
    );
    insert_mutation_intent_event(
        &mut tx,
        &intent.run_id,
        "mutation_intent_reconciled_committed",
        &detail_json,
        &process.instance_id,
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Verify the database-backed mutation permit for one DeliveryRun rung.
///
/// Ownership, the monotonic epoch, an unexpired lease, and a completed
/// observe-only reconciliation must all agree. This is deliberately a fresh
/// read at each mutation boundary; a cached successful check is not authority
/// for a later Git or remote side effect.
pub async fn verify_delivery_mutation_permit(
    pool: &SqlitePool,
    run_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    now: i64,
) -> Result<bool> {
    if claim_epoch <= 0 {
        return Ok(false);
    }
    let permitted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_runs
         WHERE id=? AND lease_owner=? AND claim_epoch=?
           AND reconciled_claim_epoch=claim_epoch
           AND lease_expires_at>?
           AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
           AND COALESCE(wait_class, '') <> 'legacy_orphan'
           AND NOT EXISTS (
             SELECT 1 FROM delivery_mutation_intents
             WHERE delivery_mutation_intents.run_id=delivery_runs.id
               AND delivery_mutation_intents.status IN ('started', 'unknown')
           )",
    )
    .bind(run_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(permitted == 1)
}

/// Promote an observe-only takeover claim into a mutation-capable claim after
/// the caller has reconciled local and remote state without issuing a write.
/// Replays are idempotent; a stale owner or epoch can never reconcile a newer
/// claim.
pub async fn mark_delivery_claim_reconciled(
    pool: &SqlitePool,
    run_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    now: i64,
) -> Result<bool> {
    if claim_epoch <= 0 {
        return Ok(false);
    }
    let mut tx = pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE delivery_runs
         SET reconciled_claim_epoch=claim_epoch, last_observed_at=?, updated_at=?,
             last_observed_app_version=?, last_observed_app_build=?,
             last_observed_process_instance=?
         WHERE id=? AND lease_owner=? AND claim_epoch=?
           AND reconciled_claim_epoch<claim_epoch AND lease_expires_at>?
           AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')
           AND NOT EXISTS (
             SELECT 1 FROM delivery_mutation_intents
             WHERE delivery_mutation_intents.run_id=delivery_runs.id
               AND delivery_mutation_intents.status IN ('started', 'unknown')
           )",
    )
    .bind(now)
    .bind(now)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(&process.instance_id)
    .bind(run_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if changed == 0 {
        let already_reconciled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_runs
             WHERE id=? AND lease_owner=? AND claim_epoch=?
               AND reconciled_claim_epoch=claim_epoch AND lease_expires_at>?
               AND NOT EXISTS (
                 SELECT 1 FROM delivery_mutation_intents
                 WHERE delivery_mutation_intents.run_id=delivery_runs.id
                   AND delivery_mutation_intents.status IN ('started', 'unknown')
               )",
        )
        .bind(run_id)
        .bind(&process.instance_id)
        .bind(claim_epoch)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(already_reconciled == 1);
    }

    let row = sqlx::query("SELECT stage, status, wait_class FROM delivery_runs WHERE id=?")
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO delivery_run_events
         (id, run_id, event_kind, stage, status, wait_class, detail_json, process_instance, created_at)
         VALUES (?, ?, 'claim_reconciled', ?, ?, ?, '{\"mode\":\"observe_only\"}', ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(run_id)
    .bind(row.try_get::<String, _>("stage")?)
    .bind(row.try_get::<String, _>("status")?)
    .bind(row.try_get::<Option<String>, _>("wait_class")?)
    .bind(&process.instance_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Persist one delivery observation. Liveness-only observations renew the
/// lease but do not move `last_progress_at` or `progress_revision`.
pub async fn record_delivery_observation(
    pool: &SqlitePool,
    run_id: &str,
    process: &ProcessIdentity,
    claim_epoch: i64,
    observation: &DeliveryObservation,
    now: i64,
    lease_ttl: i64,
) -> Result<bool> {
    if claim_epoch <= 0 {
        return Err(crate::errors::AppError::Other(
            "delivery observation requires a positive claimed epoch".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let previous = sqlx::query(
        "SELECT objective_id, repo_identity, worktree_identity, head_branch,
                change_set_digest, stage, status, reached_ceiling, expected_head_sha,
                canonical_pr_number, canonical_pr_url, canonical_head_sha, progress_revision,
                lease_expires_at
         FROM delivery_runs WHERE id=? AND lease_owner=? AND claim_epoch=?",
    )
    .bind(run_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        crate::errors::AppError::Other(
            "delivery observation rejected because this process does not own the lease".into(),
        )
    })?;
    if !previous
        .try_get::<Option<i64>, _>("lease_expires_at")?
        .is_some_and(|expires_at| expires_at > now)
    {
        return Err(crate::errors::AppError::Other(
            "delivery observation rejected because the owned lease has expired".into(),
        ));
    }
    if matches!(
        previous.try_get::<String, _>("status")?.as_str(),
        "completed" | "failed" | "cancelled" | "rejected"
    ) {
        return Err(crate::errors::AppError::Other(
            "delivery observation cannot rewrite a terminal durable run".into(),
        ));
    }

    let previous_head_branch = previous.try_get::<String, _>("head_branch")?;
    if previous_head_branch != observation.head_branch {
        return Err(crate::errors::AppError::Other(
            "delivery observation rejected because head branch is immutable for a durable run"
                .into(),
        ));
    }
    let previous_canonical_pr_number = previous.try_get::<Option<i64>, _>("canonical_pr_number")?;
    let previous_canonical_pr_url = previous.try_get::<Option<String>, _>("canonical_pr_url")?;
    if previous_canonical_pr_number.is_some() != previous_canonical_pr_url.is_some() {
        return Err(crate::errors::AppError::Other(
            "delivery observation rejected because the persisted canonical PR identity is incomplete and requires read-only reconciliation"
                .into(),
        ));
    }
    if previous_canonical_pr_number.is_none()
        && (observation.canonical_pr_number.is_some() != observation.canonical_pr_url.is_some())
    {
        return Err(crate::errors::AppError::Other(
            "delivery observation rejected because canonical PR number and URL must be first-bound together"
                .into(),
        ));
    }
    if observation
        .canonical_pr_url
        .as_deref()
        .is_some_and(|url| url.trim().is_empty())
    {
        return Err(crate::errors::AppError::Other(
            "delivery observation rejected because canonical PR URL cannot be empty".into(),
        ));
    }
    if previous_canonical_pr_number.is_some()
        && observation.canonical_pr_number.is_some()
        && previous_canonical_pr_number != observation.canonical_pr_number
    {
        return Err(crate::errors::AppError::Other(
            "delivery observation rejected because canonical PR identity is first-bind-only".into(),
        ));
    }
    if previous_canonical_pr_url.is_some()
        && observation.canonical_pr_url.is_some()
        && previous_canonical_pr_url != observation.canonical_pr_url
    {
        return Err(crate::errors::AppError::Other(
            "delivery observation rejected because canonical PR identity is first-bind-only".into(),
        ));
    }
    let previous_expected_head_sha = previous.try_get::<String, _>("expected_head_sha")?;
    let previous_change_set_digest = previous.try_get::<String, _>("change_set_digest")?;
    let head_changed = previous_expected_head_sha != observation.expected_head_sha;
    let next_change_set_digest = if head_changed {
        let revision = observation.identity_revision.as_ref().ok_or_else(|| {
            crate::errors::AppError::Other(
                "delivery expected-head change requires an identity-bound revision receipt".into(),
            )
        })?;
        let identity_matches = revision.receipt_id
            == delivery_identity_revision_receipt_id(run_id, revision)
            && revision.objective_id == previous.try_get::<String, _>("objective_id")?
            && revision.repo_identity == previous.try_get::<String, _>("repo_identity")?
            && revision.worktree_identity == previous.try_get::<String, _>("worktree_identity")?
            && revision.previous_expected_head_sha == previous_expected_head_sha
            && revision.previous_change_set_digest == previous_change_set_digest
            && revision.next_expected_head_sha == observation.expected_head_sha
            && !revision.next_change_set_digest.is_empty();
        if !identity_matches {
            return Err(crate::errors::AppError::Other(
                "delivery identity revision receipt does not match objective/repo/worktree/change-set"
                    .into(),
            ));
        }
        sqlx::query(
            "INSERT INTO delivery_identity_revisions (
                receipt_id, run_id, objective_id, repo_identity, worktree_identity,
                previous_expected_head_sha, previous_change_set_digest,
                next_expected_head_sha, next_change_set_digest,
                process_instance, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&revision.receipt_id)
        .bind(run_id)
        .bind(&revision.objective_id)
        .bind(&revision.repo_identity)
        .bind(&revision.worktree_identity)
        .bind(&revision.previous_expected_head_sha)
        .bind(&revision.previous_change_set_digest)
        .bind(&revision.next_expected_head_sha)
        .bind(&revision.next_change_set_digest)
        .bind(&process.instance_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        revision.next_change_set_digest.clone()
    } else if let Some(revision) = observation.identity_revision.as_ref() {
        let replay_matches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_identity_revisions
             WHERE receipt_id=? AND run_id=? AND objective_id=? AND repo_identity=?
               AND worktree_identity=? AND next_expected_head_sha=?
               AND next_change_set_digest=? AND previous_expected_head_sha=?
               AND previous_change_set_digest=?",
        )
        .bind(&revision.receipt_id)
        .bind(run_id)
        .bind(&revision.objective_id)
        .bind(&revision.repo_identity)
        .bind(&revision.worktree_identity)
        .bind(&observation.expected_head_sha)
        .bind(&previous_change_set_digest)
        .bind(&revision.previous_expected_head_sha)
        .bind(&revision.previous_change_set_digest)
        .fetch_one(&mut *tx)
        .await?;
        if replay_matches != 1
            || revision.receipt_id != delivery_identity_revision_receipt_id(run_id, revision)
        {
            return Err(crate::errors::AppError::Other(
                "delivery identity revision receipt replay does not match the persisted revision"
                    .into(),
            ));
        }
        previous_change_set_digest.clone()
    } else {
        previous_change_set_digest.clone()
    };
    if observation
        .canonical_head_sha
        .as_deref()
        .is_some_and(|sha| sha != observation.expected_head_sha)
    {
        return Err(crate::errors::AppError::Other(
            "canonical head does not match the durable expected head".into(),
        ));
    }

    let previous_reached = previous.try_get::<String, _>("reached_ceiling")?;
    let reached_ceiling =
        monotonic_reached_ceiling(&previous_reached, &observation.reached_ceiling);
    let canonical_pr_number = observation
        .canonical_pr_number
        .or(previous_canonical_pr_number);
    let canonical_pr_url = observation
        .canonical_pr_url
        .clone()
        .or(previous_canonical_pr_url.clone());
    let canonical_head_sha = observation
        .canonical_head_sha
        .clone()
        .or(previous.try_get::<Option<String>, _>("canonical_head_sha")?);
    let progressed = previous.try_get::<String, _>("stage")? != observation.stage
        || previous.try_get::<String, _>("status")? != observation.status
        || previous_reached != reached_ceiling
        || head_changed
        || previous_change_set_digest != next_change_set_digest
        || previous_canonical_pr_number != canonical_pr_number
        || previous_canonical_pr_url != canonical_pr_url
        || previous.try_get::<Option<String>, _>("canonical_head_sha")? != canonical_head_sha;
    let progress_revision = previous.try_get::<i64, _>("progress_revision")?;
    let has_core_input = observation.core_input.is_some();
    let core_input = observation.core_input.as_ref();

    let updated = sqlx::query(
        "UPDATE delivery_runs
         SET head_branch=?, stage=?, status=?, wait_class=?, next_action=?, reached_ceiling=?,
             expected_head_sha=?, change_set_digest=?,
             canonical_pr_number=?, canonical_pr_url=?, canonical_head_sha=?,
             failure_signature=?, stage_attempt=CASE
               WHEN ? IS NULL THEN 0
               WHEN failure_signature = ? THEN stage_attempt
               ELSE 1 END,
             core_input_request_key=CASE WHEN ? THEN ? ELSE core_input_request_key END,
             core_inputs_json=CASE WHEN ? THEN ? ELSE core_inputs_json END,
             core_input_attempts_json=CASE WHEN ? THEN ? ELSE core_input_attempts_json END,
             core_input_resume_stage=CASE WHEN ? THEN ? ELSE core_input_resume_stage END,
             core_input_request_count=CASE WHEN ? THEN 1 ELSE core_input_request_count END,
             failure_code=?, failure_class=?,
             recovery_attempt=recovery_attempt + CASE
               WHEN ? IS NOT NULL AND COALESCE(failure_signature, '') <> ? THEN 1
               ELSE 0 END,
             lease_expires_at=?, last_observed_at=?,
             last_progress_at=CASE WHEN ? THEN ? ELSE last_progress_at END,
             progress_revision=?, process_instance=?, app_version=?, app_build=?,
             last_observed_app_version=?, last_observed_app_build=?,
             last_observed_process_instance=?, updated_at=?
         WHERE id=? AND lease_owner=? AND claim_epoch=? AND lease_expires_at>?
           AND status NOT IN ('completed', 'failed', 'cancelled', 'rejected')",
    )
    .bind(&observation.head_branch)
    .bind(&observation.stage)
    .bind(&observation.status)
    .bind(&observation.wait_class)
    .bind(&observation.next_action)
    .bind(&reached_ceiling)
    .bind(&observation.expected_head_sha)
    .bind(&next_change_set_digest)
    .bind(canonical_pr_number)
    .bind(&canonical_pr_url)
    .bind(&canonical_head_sha)
    .bind(&observation.failure_signature)
    .bind(&observation.failure_signature)
    .bind(&observation.failure_signature)
    .bind(has_core_input)
    .bind(core_input.map(|value| &value.request_key))
    .bind(has_core_input)
    .bind(core_input.map(|value| &value.inputs_json))
    .bind(has_core_input)
    .bind(core_input.map(|value| &value.attempts_json))
    .bind(has_core_input)
    .bind(core_input.map(|value| &value.resume_stage))
    .bind(has_core_input)
    .bind(&observation.failure_signature)
    .bind(&observation.wait_class)
    .bind(&observation.failure_signature)
    .bind(&observation.failure_signature)
    .bind(now.saturating_add(lease_ttl))
    .bind(now)
    .bind(progressed)
    .bind(now)
    .bind(progress_revision + i64::from(progressed))
    .bind(&process.instance_id)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(&process.instance_id)
    .bind(now)
    .bind(run_id)
    .bind(&process.instance_id)
    .bind(claim_epoch)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(crate::errors::AppError::Other(
            "delivery observation lost its lease before the atomic update".into(),
        ));
    }

    sqlx::query(
        "INSERT INTO delivery_run_events
         (id, run_id, event_kind, stage, status, wait_class, detail_json, process_instance, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(run_id)
    .bind(if progressed { "progressed" } else { "observed" })
    .bind(&observation.stage)
    .bind(&observation.status)
    .bind(&observation.wait_class)
    .bind(
        serde_json::json!({
            "next_action": observation.next_action,
            "failure_signature": observation.failure_signature,
            "identity_revision_receipt_id": observation.identity_revision.as_ref().map(|value| &value.receipt_id),
            "observed_reached_ceiling": observation.reached_ceiling,
            "persisted_reached_ceiling": reached_ceiling,
        })
        .to_string(),
    )
    .bind(&process.instance_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(progressed)
}

/// Atomically claims expired, identified, non-terminal runs for observation.
///
/// The returned plan never represents permission to mutate an external system.
/// `next_action_authorized` is retained only so a later supervisor can combine
/// it with fresh remote reconciliation and the normal policy checks.
pub async fn plan_startup_recovery(
    pool: &SqlitePool,
    process: &ProcessIdentity,
    now: i64,
    lease_ttl: i64,
) -> Result<StartupRecoveryPlan> {
    let mut tx = pool.begin().await?;

    let missing_sql = format!(
        "SELECT id FROM delivery_runs
         WHERE {NON_TERMINAL_PREDICATE}
           AND (lease_expires_at IS NULL OR lease_expires_at <= ?)
           AND NOT {STABLE_IDENTITY_PREDICATE}
         ORDER BY created_at, id"
    );
    let fail_closed_identity_missing = sqlx::query_scalar::<_, String>(&missing_sql)
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;
    let mark_legacy_sql = format!(
        "UPDATE delivery_runs
         SET wait_class='legacy_orphan', last_observed_at=?, updated_at=?
         WHERE {NON_TERMINAL_PREDICATE}
           AND (lease_expires_at IS NULL OR lease_expires_at <= ?)
           AND NOT {STABLE_IDENTITY_PREDICATE}
           AND COALESCE(wait_class, '') <> 'legacy_orphan'"
    );
    sqlx::query(&mark_legacy_sql)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

    let candidate_sql = format!(
        "SELECT id, objective_id, workspace_path, worktree_identity, repo_identity,
                base_branch, head_branch, change_set_digest,
                expected_head_sha, canonical_pr_number, canonical_pr_url, canonical_head_sha, stage, status,
                wait_class, next_action, next_action_authorized, failure_signature,
                stage_attempt, progress_revision, requested_ceiling, reached_ceiling, autonomous_completion,
                claim_epoch
         FROM delivery_runs
         WHERE {NON_TERMINAL_PREDICATE}
           AND (lease_expires_at IS NULL OR lease_expires_at <= ?)
           AND {STABLE_IDENTITY_PREDICATE}
         ORDER BY created_at, id"
    );
    let candidates = sqlx::query(&candidate_sql)
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;

    let mut claimed = Vec::with_capacity(candidates.len());
    for row in candidates {
        let run_id: String = row.try_get("id")?;
        let claim_sql = format!(
            "UPDATE delivery_runs
             SET lease_owner=?, lease_expires_at=?, process_instance=?, app_version=?, app_build=?,
                 claim_epoch=claim_epoch + 1,
                 last_observed_app_version=?, last_observed_app_build=?,
                 last_observed_process_instance=?, last_observed_at=?, updated_at=?
             WHERE id=? AND {NON_TERMINAL_PREDICATE}
               AND (lease_expires_at IS NULL OR lease_expires_at <= ?)"
        );
        let changed = sqlx::query(&claim_sql)
            .bind(&process.instance_id)
            .bind(now.saturating_add(lease_ttl))
            .bind(&process.instance_id)
            .bind(&process.app_version)
            .bind(&process.app_build)
            .bind(&process.app_version)
            .bind(&process.app_build)
            .bind(&process.instance_id)
            .bind(now)
            .bind(now)
            .bind(&run_id)
            .bind(now)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if changed == 0 {
            continue;
        }

        let stage: String = row.try_get("stage")?;
        let status: String = row.try_get("status")?;
        let wait_class: Option<String> = row.try_get("wait_class")?;
        sqlx::query(
            "INSERT INTO delivery_run_events
             (id, run_id, event_kind, stage, status, wait_class, detail_json, process_instance, created_at)
             VALUES (?, ?, 'startup_lease_claimed', ?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&run_id)
        .bind(&stage)
        .bind(&status)
        .bind(&wait_class)
        .bind("{\"action\":\"observe_only\"}")
        .bind(&process.instance_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let claim_epoch = row.try_get::<i64, _>("claim_epoch")?.saturating_add(1);
        claimed.push(ClaimedRecovery {
            run_id,
            claim_epoch,
            objective_id: row.try_get("objective_id")?,
            workspace_path: row.try_get("workspace_path")?,
            worktree_identity: row.try_get("worktree_identity")?,
            repo_identity: row.try_get("repo_identity")?,
            base_branch: row.try_get("base_branch")?,
            head_branch: row.try_get("head_branch")?,
            change_set_digest: row.try_get("change_set_digest")?,
            expected_head_sha: row.try_get("expected_head_sha")?,
            requested_ceiling: row.try_get("requested_ceiling")?,
            autonomous_completion: row.try_get::<i64, _>("autonomous_completion")? != 0,
            canonical_pr_number: row.try_get("canonical_pr_number")?,
            canonical_pr_url: row.try_get("canonical_pr_url")?,
            canonical_head_sha: row.try_get("canonical_head_sha")?,
            reached_ceiling: row.try_get("reached_ceiling")?,
            stage,
            status,
            wait_class,
            next_action: row.try_get("next_action")?,
            next_action_authorized: row.try_get::<i64, _>("next_action_authorized")? != 0,
            failure_signature: row.try_get("failure_signature")?,
            stage_attempt: row.try_get("stage_attempt")?,
            progress_revision: row.try_get("progress_revision")?,
            action: RecoveryAction::ObserveOnly,
        });
    }

    tx.commit().await?;
    Ok(StartupRecoveryPlan {
        claimed,
        fail_closed_identity_missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn startup_claims_only_expired_identified_non_terminal_runs() {
        let pool = pool().await;
        insert_recovery_fixture(
            &pool,
            "expired",
            Some("session"),
            Some("turn"),
            "delivering",
            99,
        )
        .await;
        insert_recovery_fixture(
            &pool,
            "live",
            Some("session"),
            Some("turn"),
            "delivering",
            101,
        )
        .await;
        insert_recovery_fixture(
            &pool,
            "terminal",
            Some("session"),
            Some("turn"),
            "completed",
            99,
        )
        .await;
        insert_recovery_fixture(&pool, "identity-missing", None, None, "delivering", 99).await;

        let plan = plan_startup_recovery(
            &pool,
            &ProcessIdentity::new("process-new", "1.79.0", "17900"),
            100,
            30,
        )
        .await
        .unwrap();

        assert_eq!(
            plan.claimed
                .iter()
                .map(|r| r.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["expired"]
        );
        assert_eq!(plan.fail_closed_identity_missing, vec!["identity-missing"]);
        assert!(plan
            .claimed
            .iter()
            .all(|r| r.action == RecoveryAction::ObserveOnly));

        let owner: Option<String> =
            sqlx::query_scalar("SELECT lease_owner FROM delivery_runs WHERE id='expired'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owner.as_deref(), Some("process-new"));
        let live_owner: Option<String> =
            sqlx::query_scalar("SELECT lease_owner FROM delivery_runs WHERE id='live'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(live_owner.as_deref(), Some("process-old"));
    }

    #[tokio::test]
    async fn new_runs_require_identity_and_persist_initial_event_and_lease() {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.0", "17900");
        let mut run = NewDeliveryRun {
            id: "new-run".into(),
            objective_id: "objective-opaque-new-run".into(),
            run_kind: "chat_delivery".into(),
            session_id: None,
            root_turn_id: None,
            task_segment_id: None,
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:new-run".into(),
            repo_identity: "example.invalid/repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(42),
            canonical_pr_url: Some("https://example.invalid/pr/42".into()),
            canonical_head_sha: Some("abc".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "delivery".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("observe_ci".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        assert!(create_delivery_run(&pool, &run, &process, 100, 30)
            .await
            .is_err());

        run.session_id = Some("session".into());
        run.root_turn_id = Some("turn".into());
        create_delivery_run(&pool, &run, &process, 100, 30)
            .await
            .unwrap();

        let persisted: (String, i64, i64, String) = sqlx::query_as(
            "SELECT requested_ceiling, lease_expires_at, autonomous_completion, decision_policy
             FROM delivery_runs WHERE id='new-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            persisted,
            ("through_release".into(), 130, 1, "apply_recommended".into())
        );
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events WHERE run_id='new-run' AND event_kind='created'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 1);

        let heartbeat = DeliveryObservation {
            head_branch: "feature".into(),
            stage: "ci".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("deliver".into()),
            reached_ceiling: "local".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(42),
            canonical_pr_url: Some("https://example.invalid/pr/42".into()),
            canonical_head_sha: Some("abc".into()),
            failure_signature: None,
            core_input: None,
            identity_revision: None,
        };
        assert!(
            record_delivery_observation(&pool, "new-run", &process, 1, &heartbeat, 110, 30)
                .await
                .unwrap()
        );
        assert!(
            !record_delivery_observation(&pool, "new-run", &process, 1, &heartbeat, 120, 30)
                .await
                .unwrap()
        );
        let successor = ProcessIdentity::new("process-next", "1.80.0", "18000");
        create_delivery_run(&pool, &run, &successor, 200, 30)
            .await
            .unwrap();
        let provenance: (String, String, String, String) = sqlx::query_as(
            "SELECT created_app_version, created_process_instance,
                    last_observed_app_version, last_observed_process_instance
             FROM delivery_runs WHERE id='new-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            provenance,
            (
                "1.79.0".into(),
                "process".into(),
                "1.80.0".into(),
                "process-next".into(),
            )
        );
        let clocks: (i64, i64, i64) = sqlx::query_as(
            "SELECT last_observed_at, last_progress_at, progress_revision
             FROM delivery_runs WHERE id='new-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            clocks,
            (120, 110, 1),
            "heartbeat must not masquerade as progress"
        );
    }

    #[tokio::test]
    async fn objective_pointer_conflict_rolls_back_new_delivery_run_atomically() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE objectives (
               id TEXT PRIMARY KEY,
               status TEXT NOT NULL,
               delivery_run_id TEXT,
               updated_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objectives(id, status, delivery_run_id, updated_at)
             VALUES ('objective-atomic-pointer', 'active', 'different-run', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let run = NewDeliveryRun {
            id: "atomic-pointer-run".into(),
            objective_id: "objective-atomic-pointer".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session-atomic-pointer".into()),
            root_turn_id: Some("turn-atomic-pointer".into()),
            task_segment_id: None,
            task_id: None,
            workspace_path: "/workspace/atomic-pointer".into(),
            worktree_identity: "worktree:atomic-pointer".into(),
            repo_identity: "example.invalid/atomic-pointer".into(),
            base_branch: "main".into(),
            head_branch: "codex/atomic-pointer".into(),
            change_set_digest: "sha256:atomic-pointer".into(),
            expected_head_sha: "atomic-head".into(),
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
        let error = create_delivery_run(
            &pool,
            &run,
            &ProcessIdentity::new("process-atomic-pointer", "test", "test"),
            100,
            90_000,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("DeliveryRun pointer"), "{error}");
        let runs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM delivery_runs WHERE id='atomic-pointer-run'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events WHERE run_id='atomic-pointer-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((runs, events), (0, 0));
    }

    #[tokio::test]
    async fn same_run_id_with_different_workspace_or_change_set_is_rejected_without_lease_renewal()
    {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.1", "17901");
        let original = NewDeliveryRun {
            id: "identity-run".into(),
            objective_id: "objective-opaque-identity".into(),
            run_kind: "chat_delivery".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("objective".into()),
            task_id: None,
            workspace_path: "/workspace-a".into(),
            worktree_identity: "worktree:a".into(),
            repo_identity: "example.invalid/repo".into(),
            base_branch: "main".into(),
            head_branch: "feature-a".into(),
            change_set_digest: "digest-a".into(),
            expected_head_sha: "aaa".into(),
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
        create_delivery_run(&pool, &original, &process, 100, 30)
            .await
            .unwrap();

        let mut mismatched = original.clone();
        mismatched.workspace_path = "/workspace-b".into();
        mismatched.head_branch = "feature-b".into();
        mismatched.change_set_digest = "digest-b".into();
        mismatched.expected_head_sha = "bbb".into();
        let error = create_delivery_run(&pool, &mismatched, &process, 110, 90)
            .await
            .expect_err("identity collision must fail before delivery side effects");
        assert!(error.to_string().contains("identity"));

        let persisted: (String, String, String, String, i64) = sqlx::query_as(
            "SELECT workspace_path, head_branch, change_set_digest, expected_head_sha, lease_expires_at
             FROM delivery_runs WHERE id='identity-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            persisted,
            (
                "/workspace-a".into(),
                "feature-a".into(),
                "digest-a".into(),
                "aaa".into(),
                130,
            )
        );
    }

    #[tokio::test]
    async fn same_owner_live_retry_renews_without_invalidating_the_reconciled_epoch() {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "same-owner-retry".into(),
            objective_id: "objective-opaque-same-owner".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:same-owner".into(),
            repo_identity: "example.invalid/repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
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

        let first_epoch = create_delivery_run(&pool, &run, &process, 100, 30)
            .await
            .unwrap();
        let retry_epoch = create_delivery_run(&pool, &run, &process, 110, 30)
            .await
            .unwrap();

        assert_eq!(first_epoch, 1);
        assert_eq!(retry_epoch, first_epoch);
        assert!(verify_delivery_mutation_permit(
            &pool,
            "same-owner-retry",
            &process,
            retry_epoch,
            111,
        )
        .await
        .unwrap());
        let epochs: (i64, i64, i64) = sqlx::query_as(
            "SELECT claim_epoch, reconciled_claim_epoch, lease_expires_at
             FROM delivery_runs WHERE id='same-owner-retry'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(epochs, (1, 1, 140));
    }

    #[tokio::test]
    async fn migration_0008_enforces_epoch_invariants_on_upgraded_databases() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE delivery_runs (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/0008_delivery_identity_revisions.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO delivery_runs(id) VALUES ('upgraded')")
            .execute(&pool)
            .await
            .unwrap();

        assert!(
            sqlx::query("UPDATE delivery_runs SET claim_epoch=-1 WHERE id='upgraded'")
                .execute(&pool)
                .await
                .is_err(),
            "an upgraded database must reject a negative claim epoch"
        );
        assert!(
            sqlx::query(
                "UPDATE delivery_runs
                 SET claim_epoch=1, reconciled_claim_epoch=2 WHERE id='upgraded'",
            )
            .execute(&pool)
            .await
            .is_err(),
            "reconciled epoch cannot outrun the current claim"
        );
    }

    #[tokio::test]
    async fn migration_0001_through_0009_preserves_rows_tables_indexes_and_integrity() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for migration in [
            include_str!("../../migrations/0001_init.sql"),
            include_str!("../../migrations/0002_knowledge.sql"),
            include_str!("../../migrations/0003_session_execution_governance.sql"),
            include_str!("../../migrations/0004_delivery_runs.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO delivery_runs
             (id, run_kind, requested_ceiling, reached_ceiling, stage, status,
              last_observed_at, last_progress_at, app_version, app_build,
              process_instance, created_at, updated_at)
             VALUES ('legacy-run', 'deliver_changes', 'through_release', 'local',
                     'delivery', 'waiting', 10, 10, '1.79.2', 'legacy-build',
                     'legacy-process', 10, 10)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for migration in [
            include_str!("../../migrations/0005_objective_recovery_control_plane.sql"),
            include_str!("../../migrations/0006_session_auto_title.sql"),
            include_str!("../../migrations/0007_unified_objective_control_plane.sql"),
            include_str!("../../migrations/0008_delivery_identity_revisions.sql"),
            include_str!("../../migrations/0009_chat_run_controls.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }

        // The runtime compatibility path is separately idempotent and must
        // remain safe after sqlx has applied the numbered migration.
        ensure_schema(&pool).await.unwrap();
        ensure_schema(&pool).await.unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();

        let preserved: (String, String, i64, i64) = sqlx::query_as(
            "SELECT requested_ceiling, status, claim_epoch, reconciled_claim_epoch
             FROM delivery_runs WHERE id='legacy-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            preserved,
            ("through_release".into(), "waiting".into(), 0, 0)
        );

        for object in [
            "delivery_identity_revisions",
            "delivery_mutation_intents",
            "idx_delivery_identity_revisions_run",
            "idx_delivery_mutation_intents_run",
            "idx_delivery_mutation_intents_one_unresolved",
            "idx_delivery_mutation_intents_operation",
            "idx_tool_calls_objective_binding",
            "chat_run_controls",
            "idx_chat_run_controls_session",
            "idx_chat_run_controls_cancel_requested",
        ] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE name=? AND type IN ('table', 'index')",
            )
            .bind(object)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(exists, 1, "missing migrated SQLite object {object}");
        }
        let binding_id_columns: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('tool_calls') WHERE name='binding_id'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            binding_id_columns, 1,
            "runtime migration must persist exact Objective binding attribution"
        );
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    #[tokio::test]
    async fn terminal_run_cannot_be_reclaimed_or_rewritten_by_an_identity_replay() {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "terminal-replay".into(),
            objective_id: "objective-opaque-terminal-replay".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:terminal-replay".into(),
            repo_identity: "example.invalid/repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(42),
            canonical_pr_url: Some("https://example.invalid/pr/42".into()),
            canonical_head_sha: Some("abc".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "live_verified".into(),
            stage: "complete".into(),
            status: "running".into(),
            wait_class: None,
            next_action: None,
            next_action_authorized: false,
            autonomous_completion: true,
        };
        let epoch = create_delivery_run(&pool, &run, &process, 100, 30)
            .await
            .unwrap();
        sqlx::query("UPDATE delivery_runs SET status='completed' WHERE id='terminal-replay'")
            .execute(&pool)
            .await
            .unwrap();
        let before: (i64, i64, Option<String>) = sqlx::query_as(
            "SELECT claim_epoch, lease_expires_at, lease_owner
             FROM delivery_runs WHERE id='terminal-replay'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        create_delivery_run(&pool, &run, &process, 110, 30)
            .await
            .expect_err("terminal identity replay cannot reopen a DeliveryRun");
        let observation = DeliveryObservation {
            head_branch: "feature".into(),
            stage: "replay".into(),
            status: "platform_incident".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            reached_ceiling: "live_verified".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(42),
            canonical_pr_url: Some("https://example.invalid/pr/42".into()),
            canonical_head_sha: Some("abc".into()),
            failure_signature: Some("replay".into()),
            core_input: None,
            identity_revision: None,
        };
        record_delivery_observation(
            &pool,
            "terminal-replay",
            &process,
            epoch,
            &observation,
            111,
            30,
        )
        .await
        .expect_err("terminal observation cannot rewrite status");

        let after: (String, i64, i64, Option<String>) = sqlx::query_as(
            "SELECT status, claim_epoch, lease_expires_at, lease_owner
             FROM delivery_runs WHERE id='terminal-replay'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after, ("completed".into(), before.0, before.1, before.2));
    }

    #[tokio::test]
    async fn expected_head_revision_requires_identity_bound_receipt_and_is_atomic() {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "revision-run".into(),
            objective_id: "objective-opaque-revision".into(),
            run_kind: "chat_delivery".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("objective".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:revision".into(),
            repo_identity: "example.invalid/repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest-before".into(),
            expected_head_sha: "aaa".into(),
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
        create_delivery_run(&pool, &run, &process, 100, 30)
            .await
            .unwrap();

        let mut observation = DeliveryObservation {
            head_branch: "feature".into(),
            stage: "commit".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("push".into()),
            reached_ceiling: "committed".into(),
            expected_head_sha: "bbb".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            failure_signature: None,
            core_input: None,
            identity_revision: None,
        };
        let error =
            record_delivery_observation(&pool, "revision-run", &process, 1, &observation, 110, 30)
                .await
                .expect_err("head changes without a receipt must fail closed");
        assert!(error.to_string().contains("revision receipt"));

        let unchanged: (String, String, i64) = sqlx::query_as(
            "SELECT expected_head_sha, change_set_digest, progress_revision
             FROM delivery_runs WHERE id='revision-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(unchanged, ("aaa".into(), "digest-before".into(), 0));

        let mut wrong_repo_revision = DeliveryIdentityRevision {
            receipt_id: String::new(),
            objective_id: "objective-opaque-revision".into(),
            repo_identity: "example.invalid/other".into(),
            worktree_identity: "worktree:revision".into(),
            previous_expected_head_sha: "aaa".into(),
            previous_change_set_digest: "digest-before".into(),
            next_expected_head_sha: "bbb".into(),
            next_change_set_digest: "digest-after".into(),
        };
        wrong_repo_revision.receipt_id =
            delivery_identity_revision_receipt_id("revision-run", &wrong_repo_revision);
        observation.identity_revision = Some(wrong_repo_revision);
        record_delivery_observation(&pool, "revision-run", &process, 1, &observation, 115, 30)
            .await
            .expect_err("a receipt for another repo must fail closed");
        let still_unchanged: (String, String, i64, i64) = sqlx::query_as(
            "SELECT expected_head_sha, change_set_digest, progress_revision,
                    (SELECT COUNT(*) FROM delivery_identity_revisions
                     WHERE run_id='revision-run')
             FROM delivery_runs WHERE id='revision-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            still_unchanged,
            ("aaa".into(), "digest-before".into(), 0, 0)
        );

        let mut valid_revision = DeliveryIdentityRevision {
            receipt_id: String::new(),
            objective_id: "objective-opaque-revision".into(),
            repo_identity: "example.invalid/repo".into(),
            worktree_identity: "worktree:revision".into(),
            previous_expected_head_sha: "aaa".into(),
            previous_change_set_digest: "digest-before".into(),
            next_expected_head_sha: "bbb".into(),
            next_change_set_digest: "digest-after".into(),
        };
        valid_revision.receipt_id =
            delivery_identity_revision_receipt_id("revision-run", &valid_revision);
        let valid_receipt_id = valid_revision.receipt_id.clone();
        observation.identity_revision = Some(valid_revision);
        record_delivery_observation(&pool, "revision-run", &process, 1, &observation, 131, 30)
            .await
            .expect_err("a matching receipt cannot revive an expired lease");
        let expired_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_identity_revisions WHERE run_id='revision-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(expired_receipts, 0, "expired lease must commit no receipt");

        create_delivery_run(&pool, &run, &process, 132, 30)
            .await
            .unwrap();
        assert!(record_delivery_observation(
            &pool,
            "revision-run",
            &process,
            2,
            &observation,
            133,
            30,
        )
        .await
        .unwrap());

        let revised: (String, String, i64, i64) = sqlx::query_as(
            "SELECT expected_head_sha, change_set_digest, progress_revision,
                    (SELECT COUNT(*) FROM delivery_identity_revisions
                     WHERE run_id='revision-run' AND receipt_id=? )
             FROM delivery_runs WHERE id='revision-run'",
        )
        .bind(&valid_receipt_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(revised, ("bbb".into(), "digest-after".into(), 1, 1));

        assert!(
            !record_delivery_observation(&pool, "revision-run", &process, 2, &observation, 140, 30,)
                .await
                .unwrap(),
            "replaying the same receipt is an idempotent observation"
        );
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_identity_revisions WHERE run_id='revision-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(receipt_count, 1);

        let mut forged_replay = observation.clone();
        forged_replay
            .identity_revision
            .as_mut()
            .expect("receipt")
            .previous_change_set_digest = "forged-previous".into();
        record_delivery_observation(&pool, "revision-run", &process, 2, &forged_replay, 145, 30)
            .await
            .expect_err("a replay with a forged prior transition must fail closed");
        let after_forgery: (i64, i64) = sqlx::query_as(
            "SELECT progress_revision,
                    (SELECT COUNT(*) FROM delivery_identity_revisions WHERE run_id='revision-run')
             FROM delivery_runs WHERE id='revision-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after_forgery, (1, 1));
    }

    #[tokio::test]
    async fn later_uncertain_observations_cannot_erase_remote_identity_or_regress_progress() {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.0", "17900");
        let run = NewDeliveryRun {
            id: "monotonic-run".into(),
            objective_id: "objective-opaque-monotonic".into(),
            run_kind: "chat_delivery".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: None,
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:monotonic".into(),
            repo_identity: "example.invalid/repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(42),
            canonical_pr_url: Some("https://example.invalid/pr/42".into()),
            canonical_head_sha: Some("abc".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "merged".into(),
            stage: "release".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("reconcile_receipt".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        create_delivery_run(&pool, &run, &process, 100, 30)
            .await
            .unwrap();

        let uncertain_receipt = DeliveryObservation {
            head_branch: "feature".into(),
            stage: "receipt".into(),
            status: "waiting".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("reconcile_receipt".into()),
            reached_ceiling: "pushed".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            failure_signature: Some("delivery_external_state_uncertain".into()),
            core_input: None,
            identity_revision: None,
        };
        record_delivery_observation(
            &pool,
            "monotonic-run",
            &process,
            1,
            &uncertain_receipt,
            110,
            30,
        )
        .await
        .unwrap();

        let persisted: (String, Option<i64>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT reached_ceiling, canonical_pr_number, canonical_pr_url, canonical_head_sha
             FROM delivery_runs WHERE id='monotonic-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            persisted,
            (
                "merged".into(),
                Some(42),
                Some("https://example.invalid/pr/42".into()),
                Some("abc".into()),
            ),
            "uncertain readback must preserve the highest proven ceiling and canonical remote identity"
        );
    }

    #[tokio::test]
    async fn canonical_pr_identity_is_first_bind_only_and_conflicts_are_atomic() {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "canonical-pr-run".into(),
            objective_id: "objective-opaque-canonical-pr".into(),
            run_kind: "chat_delivery".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("objective".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:canonical-pr".into(),
            repo_identity: "example.invalid/repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(42),
            canonical_pr_url: Some("https://example.invalid/pr/42".into()),
            canonical_head_sha: Some("abc".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "pushed".into(),
            stage: "remote".into(),
            status: "waiting".into(),
            wait_class: Some("remote_checks".into()),
            next_action: Some("observe_ci".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        create_delivery_run(&pool, &run, &process, 100, 30)
            .await
            .unwrap();

        let before: (i64, i64, i64) = sqlx::query_as(
            "SELECT lease_expires_at, progress_revision,
                    (SELECT COUNT(*) FROM delivery_run_events WHERE run_id='canonical-pr-run')
             FROM delivery_runs WHERE id='canonical-pr-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let conflicting_number = DeliveryObservation {
            head_branch: "feature".into(),
            stage: "remote".into(),
            status: "waiting".into(),
            wait_class: Some("remote_checks".into()),
            next_action: Some("observe_ci".into()),
            reached_ceiling: "pushed".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(43),
            canonical_pr_url: Some("https://example.invalid/pr/43".into()),
            canonical_head_sha: Some("abc".into()),
            failure_signature: None,
            core_input: None,
            identity_revision: None,
        };
        let error = record_delivery_observation(
            &pool,
            "canonical-pr-run",
            &process,
            1,
            &conflicting_number,
            110,
            30,
        )
        .await
        .expect_err("a durable run cannot be rebound to a different canonical PR");
        assert!(error.to_string().contains("canonical PR identity"));

        let conflicting_url = DeliveryObservation {
            canonical_pr_number: Some(42),
            canonical_pr_url: Some("https://mirror.invalid/pr/42".into()),
            ..conflicting_number
        };
        let error = record_delivery_observation(
            &pool,
            "canonical-pr-run",
            &process,
            1,
            &conflicting_url,
            115,
            30,
        )
        .await
        .expect_err("a canonical PR URL is immutable after first bind");
        assert!(error.to_string().contains("canonical PR identity"));

        let after: (i64, Option<i64>, Option<String>, Option<String>, i64, i64) = sqlx::query_as(
            "SELECT lease_expires_at, canonical_pr_number, canonical_pr_url,
                        canonical_head_sha, progress_revision,
                        (SELECT COUNT(*) FROM delivery_run_events
                         WHERE run_id='canonical-pr-run')
                 FROM delivery_runs WHERE id='canonical-pr-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            after,
            (
                before.0,
                Some(42),
                Some("https://example.invalid/pr/42".into()),
                Some("abc".into()),
                before.1,
                before.2,
            ),
            "identity conflicts must leave the run, event log, and lease untouched"
        );
    }

    #[tokio::test]
    async fn canonical_pr_number_and_url_are_first_bound_as_one_pair() {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "canonical-pair-run".into(),
            objective_id: "objective-opaque-canonical-pair".into(),
            run_kind: "chat_delivery".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("objective".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:canonical-pair".into(),
            repo_identity: "example.invalid/repo".into(),
            base_branch: "main".into(),
            head_branch: "feature".into(),
            change_set_digest: "digest".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "pushed".into(),
            stage: "remote".into(),
            status: "waiting".into(),
            wait_class: Some("remote_checks".into()),
            next_action: Some("observe_ci".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let mut partial_create = run.clone();
        partial_create.id = "canonical-partial-create".into();
        partial_create.objective_id = "objective-opaque-canonical-partial-create".into();
        partial_create.canonical_pr_number = Some(42);
        create_delivery_run(&pool, &partial_create, &process, 100, 30)
            .await
            .expect_err("a new run cannot persist half of canonical PR identity");

        create_delivery_run(&pool, &run, &process, 100, 30)
            .await
            .unwrap();
        let partial_observation = DeliveryObservation {
            head_branch: "feature".into(),
            stage: "remote".into(),
            status: "waiting".into(),
            wait_class: Some("remote_checks".into()),
            next_action: Some("observe_ci".into()),
            reached_ceiling: "pushed".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(42),
            canonical_pr_url: None,
            canonical_head_sha: Some("abc".into()),
            failure_signature: None,
            core_input: None,
            identity_revision: None,
        };
        record_delivery_observation(
            &pool,
            "canonical-pair-run",
            &process,
            1,
            &partial_observation,
            110,
            30,
        )
        .await
        .expect_err("first bind requires PR number and URL in the same observation");
        let after_partial: (Option<i64>, Option<String>, i64) = sqlx::query_as(
            "SELECT canonical_pr_number, canonical_pr_url, progress_revision
             FROM delivery_runs WHERE id='canonical-pair-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after_partial, (None, None, 0));

        let paired_observation = DeliveryObservation {
            canonical_pr_url: Some("https://example.invalid/pr/42".into()),
            ..partial_observation
        };
        assert!(record_delivery_observation(
            &pool,
            "canonical-pair-run",
            &process,
            1,
            &paired_observation,
            115,
            30,
        )
        .await
        .unwrap());
        let paired: (Option<i64>, Option<String>) = sqlx::query_as(
            "SELECT canonical_pr_number, canonical_pr_url
             FROM delivery_runs WHERE id='canonical-pair-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            paired,
            (Some(42), Some("https://example.invalid/pr/42".into()))
        );
    }

    #[tokio::test]
    async fn startup_plan_preserves_authorization_but_never_schedules_external_mutation() {
        let pool = pool().await;
        insert_recovery_fixture(
            &pool,
            "authorized",
            Some("session"),
            Some("turn"),
            "waiting",
            10,
        )
        .await;
        sqlx::query("UPDATE delivery_runs SET next_action_authorized=1 WHERE id='authorized'")
            .execute(&pool)
            .await
            .unwrap();

        let plan = plan_startup_recovery(
            &pool,
            &ProcessIdentity::new("process-new", "1.79.0", "17900"),
            100,
            30,
        )
        .await
        .unwrap();

        assert_eq!(plan.claimed.len(), 1);
        assert!(plan.claimed[0].next_action_authorized);
        assert_eq!(plan.claimed[0].action, RecoveryAction::ObserveOnly);
    }

    #[tokio::test]
    async fn technical_failures_remain_system_owned_and_generic_needs_user_is_rejected() {
        let pool = pool().await;
        insert_recovery_fixture(
            &pool,
            "internal",
            Some("session"),
            Some("turn"),
            "failed_internal",
            10,
        )
        .await;

        let plan = plan_startup_recovery(
            &pool,
            &ProcessIdentity::new("process-new", "1.79.0", "17900"),
            100,
            30,
        )
        .await
        .unwrap();
        assert_eq!(plan.claimed[0].run_id, "internal");

        let rejected = sqlx::query(
            "INSERT INTO delivery_runs (
                id, run_kind, session_id, root_turn_id, requested_ceiling, reached_ceiling,
                stage, status, next_action_authorized, stage_attempt, last_observed_at,
                last_progress_at, progress_revision, app_version, app_build, process_instance,
                created_at, updated_at
             ) VALUES ('bad-gate', 'chat_delivery', 'session', 'turn', 'through_release',
                       'local', 'deliver', 'needs_user', 0, 0, 1, 1, 0, '1.78.4',
                       '17804', 'old', 1, 1)",
        )
        .execute(&pool)
        .await;
        assert!(
            rejected.is_err(),
            "generic needs_user must not enter durable state"
        );
    }

    #[tokio::test]
    async fn autonomous_defaults_apply_recommended_and_ceiling_cannot_be_lowered() {
        let pool = pool().await;
        insert_recovery_fixture(
            &pool,
            "autonomous",
            Some("session"),
            Some("turn"),
            "delivering",
            10,
        )
        .await;
        sqlx::query("UPDATE delivery_runs SET autonomous_completion=1 WHERE id='autonomous'")
            .execute(&pool)
            .await
            .unwrap();

        let policy: String =
            sqlx::query_scalar("SELECT decision_policy FROM delivery_runs WHERE id='autonomous'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(policy, "apply_recommended");

        let lowered =
            sqlx::query("UPDATE delivery_runs SET requested_ceiling='local' WHERE id='autonomous'")
                .execute(&pool)
                .await;
        assert!(lowered.is_err(), "requested ceiling must remain immutable");
    }

    #[tokio::test]
    async fn autonomous_objective_can_pause_only_for_a_structured_irreversible_business_decision() {
        let pool = pool().await;
        insert_recovery_fixture(
            &pool,
            "business-decision",
            Some("session"),
            Some("turn"),
            "delivering",
            10,
        )
        .await;

        sqlx::query(
            "UPDATE delivery_runs
             SET autonomous_completion=1,
                 decision_policy='require_irreversible_decision',
                 status='needs_business_decision',
                 business_decision_key='irreversible-market-choice',
                 decision_options_json='[\"market-a\",\"market-b\"]',
                 recommended_option='market-a',
                 safe_default_action='preserve-current-market',
                 decision_reason='choice changes the irreversible business outcome'
             WHERE id='business-decision'",
        )
        .execute(&pool)
        .await
        .expect("autonomous execution must retain a real structured business gate");

        let invalid = sqlx::query(
            "UPDATE delivery_runs
             SET decision_policy='apply_recommended', decision_reason=NULL
             WHERE id='business-decision'",
        )
        .execute(&pool)
        .await;
        assert!(
            invalid.is_err(),
            "an unstructured or safely defaultable choice must not masquerade as a business gate"
        );
    }

    #[tokio::test]
    async fn core_input_gate_requires_one_batched_request_and_keeps_run_recoverable() {
        let pool = pool().await;
        insert_recovery_fixture(
            &pool,
            "core-input",
            Some("session"),
            Some("turn"),
            "waiting",
            10,
        )
        .await;
        sqlx::query(
            "UPDATE delivery_runs
             SET status='core_input_required', core_input_request_key='production-identity',
                 core_inputs_json='[\"production_account\"]',
                 core_input_attempts_json='[\"managed_identity\",\"refresh\"]',
                 core_input_resume_stage='release', core_input_request_count=1
             WHERE id='core-input'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let plan = plan_startup_recovery(
            &pool,
            &ProcessIdentity::new("process-new", "1.79.0", "17900"),
            100,
            30,
        )
        .await
        .unwrap();
        assert_eq!(plan.claimed[0].run_id, "core-input");
        assert_eq!(plan.claimed[0].status, "core_input_required");

        let fragmented = sqlx::query(
            "UPDATE delivery_runs SET core_input_request_count=2 WHERE id='core-input'",
        )
        .execute(&pool)
        .await;
        assert!(
            fragmented.is_err(),
            "one objective gets at most one batched core-input request"
        );
    }

    #[tokio::test]
    async fn lease_heartbeat_is_owner_cas_and_never_masquerades_as_progress() {
        let pool = pool().await;
        insert_recovery_fixture(
            &pool,
            "heartbeat-run",
            Some("session"),
            Some("turn"),
            "waiting",
            130,
        )
        .await;
        let owner = ProcessIdentity::new("process-old", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-new", "1.79.2", "17902");

        assert!(
            !renew_delivery_lease(&pool, "heartbeat-run", &owner, 0, 120, 30)
                .await
                .unwrap(),
            "legacy epoch zero is observe-only until startup claim assigns a positive epoch"
        );
        sqlx::query(
            "UPDATE delivery_runs
             SET claim_epoch=1, reconciled_claim_epoch=1
             WHERE id='heartbeat-run'",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            renew_delivery_lease(&pool, "heartbeat-run", &owner, 1, 120, 30)
                .await
                .unwrap()
        );
        let renewed: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT lease_expires_at, last_observed_at, last_progress_at, progress_revision
             FROM delivery_runs WHERE id='heartbeat-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(renewed, (150, 120, 1, 1));

        let while_live = plan_startup_recovery(&pool, &competitor, 140, 30)
            .await
            .unwrap();
        assert!(while_live.claimed.is_empty());
        assert!(
            !renew_delivery_lease(&pool, "heartbeat-run", &competitor, 1, 145, 30)
                .await
                .unwrap(),
            "a different process cannot extend the current owner's lease"
        );
        assert!(
            !renew_delivery_lease(&pool, "heartbeat-run", &owner, 1, 151, 30)
                .await
                .unwrap(),
            "an expired lease cannot be revived by its former owner"
        );

        let after_expiry = plan_startup_recovery(&pool, &competitor, 151, 30)
            .await
            .unwrap();
        assert_eq!(
            after_expiry
                .claimed
                .iter()
                .map(|claimed| claimed.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["heartbeat-run"]
        );
    }

    #[tokio::test]
    async fn claim_epoch_fences_stale_owner_until_takeover_is_reconciled() {
        let pool = pool().await;
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-competitor", "1.79.2", "17902");
        let run = NewDeliveryRun {
            id: "epoch-run".into(),
            objective_id: "objective-opaque-epoch".into(),
            run_kind: "deliver_changes".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: Some("segment".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:epoch".into(),
            repo_identity: "repo:epoch".into(),
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
        let initial_epoch = create_delivery_run(&pool, &run, &owner, 100, 10)
            .await
            .unwrap();
        assert_eq!(initial_epoch, 1);
        assert!(
            verify_delivery_mutation_permit(&pool, "epoch-run", &owner, initial_epoch, 105,)
                .await
                .unwrap()
        );

        let takeover = plan_startup_recovery(&pool, &competitor, 111, 30)
            .await
            .unwrap();
        assert_eq!(takeover.claimed.len(), 1);
        let claimed = &takeover.claimed[0];
        assert_eq!(claimed.claim_epoch, 2);
        assert_eq!(claimed.action, RecoveryAction::ObserveOnly);
        assert!(
            !verify_delivery_mutation_permit(&pool, "epoch-run", &owner, initial_epoch, 112,)
                .await
                .unwrap()
        );
        assert!(!verify_delivery_mutation_permit(
            &pool,
            "epoch-run",
            &competitor,
            claimed.claim_epoch,
            112,
        )
        .await
        .unwrap());

        assert!(mark_delivery_claim_reconciled(
            &pool,
            "epoch-run",
            &competitor,
            claimed.claim_epoch,
            113,
        )
        .await
        .unwrap());
        assert!(verify_delivery_mutation_permit(
            &pool,
            "epoch-run",
            &competitor,
            claimed.claim_epoch,
            114,
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn mutation_intent_is_committed_before_effect_and_serializes_unresolved_rungs() {
        let pool = pool().await;
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let stale_owner = ProcessIdentity::new("process-stale", "1.79.2", "17902");
        let epoch = create_mutation_test_run(&pool, "intent-run", &owner, 100, 30).await;

        assert!(begin_delivery_mutation_intent(
            &pool,
            "intent-1",
            "intent-run",
            &owner,
            epoch,
            "git_push",
            "repo:feature:abc",
            Some(r#"{"expected_head":"abc"}"#),
            105,
        )
        .await
        .unwrap());

        // Returning true is the write-ahead boundary: both the intent and its
        // audit event must already be visible through a fresh database read.
        let persisted = get_delivery_mutation_intent(&pool, "intent-1")
            .await
            .unwrap()
            .expect("started intent must be committed before caller dispatches the effect");
        assert_eq!(persisted.run_id, "intent-run");
        assert_eq!(persisted.claim_epoch, epoch);
        assert_eq!(persisted.rung, "git_push");
        assert_eq!(persisted.operation_key, "repo:feature:abc");
        assert_eq!(persisted.status, "started");
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events
             WHERE run_id='intent-run' AND event_kind='mutation_intent_started'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_count, 1);
        assert!(
            !verify_delivery_mutation_permit(&pool, "intent-run", &owner, epoch, 106)
                .await
                .unwrap(),
            "a started intent consumes the mutation permit until it is settled"
        );
        assert!(
            !mark_delivery_claim_reconciled(&pool, "intent-run", &owner, epoch, 106)
                .await
                .unwrap(),
            "even an already-reconciled claim must report false while an intent is unresolved"
        );

        assert!(
            !begin_delivery_mutation_intent(
                &pool,
                "intent-2",
                "intent-run",
                &owner,
                epoch,
                "open_pr",
                "repo:feature:pr",
                None,
                106,
            )
            .await
            .unwrap(),
            "a run may never have two unresolved mutation intents"
        );
        assert!(
            !begin_delivery_mutation_intent(
                &pool,
                "intent-stale",
                "intent-run",
                &stale_owner,
                epoch,
                "git_push",
                "repo:feature:stale",
                None,
                106,
            )
            .await
            .unwrap(),
            "a non-owner cannot begin an external mutation"
        );

        assert!(resolve_delivery_mutation_intent_committed(
            &pool,
            "intent-1",
            &owner,
            epoch,
            Some(r#"{"remote_head":"abc"}"#),
            107,
        )
        .await
        .unwrap());
        assert!(
            !begin_delivery_mutation_intent(
                &pool,
                "intent-1-replay",
                "intent-run",
                &owner,
                epoch,
                "git_push",
                "repo:feature:abc",
                None,
                108,
            )
            .await
            .unwrap(),
            "a committed external operation key cannot be dispatched again after a projection crash"
        );
        assert!(
            verify_delivery_mutation_permit(&pool, "intent-run", &owner, epoch, 108,)
                .await
                .unwrap()
        );
        assert!(begin_delivery_mutation_intent(
            &pool,
            "intent-2",
            "intent-run",
            &owner,
            epoch,
            "open_pr",
            "repo:feature:pr",
            None,
            109,
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn unknown_intent_requires_positive_takeover_reconciliation() {
        let pool = pool().await;
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let competitor = ProcessIdentity::new("process-competitor", "1.79.2", "17902");
        let epoch = create_mutation_test_run(&pool, "unknown-run", &owner, 100, 10).await;

        assert!(begin_delivery_mutation_intent(
            &pool,
            "intent-unknown",
            "unknown-run",
            &owner,
            epoch,
            "merge_pr",
            "pr:42:head:abc",
            None,
            105,
        )
        .await
        .unwrap());
        assert!(mark_delivery_mutation_intent_unknown(
            &pool,
            "intent-unknown",
            &owner,
            epoch,
            Some(r#"{"reason":"timeout"}"#),
            111,
        )
        .await
        .unwrap());

        let takeover = plan_startup_recovery(&pool, &competitor, 112, 30)
            .await
            .unwrap();
        assert_eq!(takeover.claimed.len(), 1);
        let takeover_epoch = takeover.claimed[0].claim_epoch;
        assert_eq!(takeover_epoch, 2);

        assert_eq!(
            list_unresolved_delivery_mutation_intents(&pool, "unknown-run")
                .await
                .unwrap()
                .iter()
                .map(|intent| intent.status.as_str())
                .collect::<Vec<_>>(),
            vec!["unknown"]
        );
        assert!(!verify_delivery_mutation_permit(
            &pool,
            "unknown-run",
            &competitor,
            takeover_epoch,
            113,
        )
        .await
        .unwrap());
        assert!(
            !mark_delivery_claim_reconciled(
                &pool,
                "unknown-run",
                &competitor,
                takeover_epoch,
                113,
            )
            .await
            .unwrap(),
            "remote absence cannot implicitly clear an unresolved intent"
        );
        assert_eq!(
            get_delivery_mutation_intent(&pool, "intent-unknown")
                .await
                .unwrap()
                .unwrap()
                .status,
            "unknown"
        );
        assert!(
            mark_delivery_mutation_intent_reconciled_committed(
                &pool,
                "intent-unknown",
                &competitor,
                takeover_epoch,
                None,
                114,
            )
            .await
            .is_err(),
            "remote absence or an evidence-free assertion cannot clear uncertainty"
        );
        for invalid in [
            r#"{}"#,
            r#"{"rung":"merge_pr","operation_key":"pr:42:head:abc","observation":{"confirmation":"remote_absent"}}"#,
        ] {
            assert!(
                mark_delivery_mutation_intent_reconciled_committed(
                    &pool,
                    "intent-unknown",
                    &competitor,
                    takeover_epoch,
                    Some(invalid),
                    114,
                )
                .await
                .is_err(),
                "empty or negative observation must never become positive reconciliation evidence"
            );
        }

        assert!(mark_delivery_mutation_intent_reconciled_committed(
            &pool,
            "intent-unknown",
            &competitor,
            takeover_epoch,
            Some(
                r#"{"rung":"merge_pr","operation_key":"pr:42:head:abc","observation":{"confirmation":"merge_observed","pr_number":42,"merge_sha":"merge-abc"}}"#,
            ),
            114,
        )
        .await
        .unwrap());
        assert!(
            mark_delivery_mutation_intent_reconciled_committed(
                &pool,
                "intent-unknown",
                &competitor,
                takeover_epoch,
                Some(
                    r#"{"rung":"merge_pr","operation_key":"pr:42:head:abc","observation":{"confirmation":"merge_observed","pr_number":42,"merge_sha":"different-merge"}}"#,
                ),
                114,
            )
            .await
            .is_err(),
            "an idempotent replay may not replace the first positive evidence"
        );
        assert_eq!(
            get_delivery_mutation_intent(&pool, "intent-unknown")
                .await
                .unwrap()
                .unwrap()
                .status,
            "reconciled_committed"
        );
        assert!(mark_delivery_claim_reconciled(
            &pool,
            "unknown-run",
            &competitor,
            takeover_epoch,
            115,
        )
        .await
        .unwrap());
        assert!(verify_delivery_mutation_permit(
            &pool,
            "unknown-run",
            &competitor,
            takeover_epoch,
            116,
        )
        .await
        .unwrap());
        assert!(
            !begin_delivery_mutation_intent(
                &pool,
                "intent-old-owner",
                "unknown-run",
                &owner,
                epoch,
                "release",
                "release:old-owner",
                None,
                116,
            )
            .await
            .unwrap(),
            "the former epoch cannot begin after takeover"
        );
    }

    #[tokio::test]
    async fn original_intent_owner_can_settle_after_lease_expiry_without_business_progress() {
        let pool = pool().await;
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let epoch = create_mutation_test_run(&pool, "settle-run", &owner, 100, 10).await;
        assert!(begin_delivery_mutation_intent(
            &pool,
            "intent-settle",
            "settle-run",
            &owner,
            epoch,
            "git_push",
            "repo:feature:def",
            None,
            105,
        )
        .await
        .unwrap());
        let before: (String, String, String, i64) = sqlx::query_as(
            "SELECT stage, status, reached_ceiling, progress_revision
             FROM delivery_runs WHERE id='settle-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert!(resolve_delivery_mutation_intent_committed(
            &pool,
            "intent-settle",
            &owner,
            epoch,
            Some(r#"{"remote_head":"def"}"#),
            111,
        )
        .await
        .unwrap());
        let after: (String, String, String, i64) = sqlx::query_as(
            "SELECT stage, status, reached_ceiling, progress_revision
             FROM delivery_runs WHERE id='settle-run'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            after, before,
            "intent settlement must not project business progress"
        );
    }

    #[tokio::test]
    async fn failed_settlement_keeps_started_intent_unresolved() {
        let pool = pool().await;
        let owner = ProcessIdentity::new("process-owner", "1.79.2", "17902");
        let epoch = create_mutation_test_run(&pool, "settle-failure-run", &owner, 100, 30).await;
        assert!(begin_delivery_mutation_intent(
            &pool,
            "intent-settle-failure",
            "settle-failure-run",
            &owner,
            epoch,
            "git_push",
            "repo:feature:ghi",
            None,
            105,
        )
        .await
        .unwrap());

        // Force the audit write to fail after the status UPDATE. The shared
        // transaction must roll the UPDATE back, preserving write uncertainty.
        sqlx::query("DROP TABLE delivery_run_events")
            .execute(&pool)
            .await
            .unwrap();
        assert!(resolve_delivery_mutation_intent_committed(
            &pool,
            "intent-settle-failure",
            &owner,
            epoch,
            Some(r#"{"remote_head":"ghi"}"#),
            106,
        )
        .await
        .is_err());
        assert_eq!(
            get_delivery_mutation_intent(&pool, "intent-settle-failure")
                .await
                .unwrap()
                .unwrap()
                .status,
            "started"
        );
    }

    async fn create_mutation_test_run(
        pool: &SqlitePool,
        id: &str,
        process: &ProcessIdentity,
        now: i64,
        lease_ttl: i64,
    ) -> i64 {
        let run = NewDeliveryRun {
            id: id.into(),
            objective_id: format!("objective-opaque-{id}"),
            run_kind: "deliver_changes".into(),
            session_id: Some(format!("session-{id}")),
            root_turn_id: Some(format!("turn-{id}")),
            task_segment_id: Some(format!("segment-{id}")),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: format!("worktree:{id}"),
            repo_identity: format!("repo:{id}"),
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
        create_delivery_run(pool, &run, process, now, lease_ttl)
            .await
            .unwrap()
    }

    async fn insert_recovery_fixture(
        pool: &sqlx::SqlitePool,
        id: &str,
        session_id: Option<&str>,
        root_turn_id: Option<&str>,
        status: &str,
        lease_expires_at: i64,
    ) {
        sqlx::query(
            "INSERT INTO delivery_runs (
                id, objective_id, run_kind, session_id, root_turn_id, workspace_path, worktree_identity, repo_identity, base_branch,
                head_branch, change_set_digest, expected_head_sha, requested_ceiling, reached_ceiling,
                stage, status, wait_class, next_action, next_action_authorized, stage_attempt,
                lease_owner, lease_expires_at, last_observed_at, last_progress_at,
                progress_revision, app_version, app_build, process_instance, created_at, updated_at
             ) VALUES (?, ?, 'chat_delivery', ?, ?, '/workspace', ?, 'example.invalid/repo', 'main',
                       'feature', 'digest', 'abc', 'through_release', 'local',
                       'deliver', ?, 'recoverable', 'observe_remote', 0, 1, 'process-old', ?, 1, 1, 1,
                       '1.78.4', '17804', 'process-old', 1, 1)",
        )
        .bind(id)
        .bind(
            session_id
                .zip(root_turn_id)
                .map(|_| format!("objective-opaque-{id}")),
        )
        .bind(session_id)
        .bind(root_turn_id)
        .bind(session_id.zip(root_turn_id).map(|_| "worktree:fixture"))
        .bind(status)
        .bind(lease_expires_at)
        .execute(pool)
        .await
        .unwrap();
    }
}
