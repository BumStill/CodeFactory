// SPDX-License-Identifier: Apache-2.0
//! Durable delivery identity, lease ownership, and restart planning.
//!
//! Startup recovery deliberately stops at a database-only, observe-first plan.
//! Owning a lease proves that this process may reconcile the run; it does not
//! authorize a push, PR mutation, merge, release, or any other external side
//! effect. Those actions remain behind the ordinary delivery authorization and
//! remote-state reconciliation path.

use sqlx::{Row, SqlitePool};

use crate::errors::Result;

const NON_TERMINAL_PREDICATE: &str =
    "status NOT IN ('completed', 'failed', 'cancelled', 'rejected')";
const STABLE_IDENTITY_PREDICATE: &str = "(
    ((session_id IS NOT NULL AND session_id <> '' AND root_turn_id IS NOT NULL AND root_turn_id <> '')
      OR (task_id IS NOT NULL AND task_id <> ''))
    AND repo_identity IS NOT NULL AND repo_identity <> ''
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
    pub run_kind: String,
    pub session_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub task_segment_id: Option<String>,
    pub task_id: Option<String>,
    pub workspace_path: String,
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
    pub workspace_path: String,
    pub repo_identity: String,
    pub base_branch: String,
    pub head_branch: String,
    pub change_set_digest: String,
    pub expected_head_sha: String,
    pub requested_ceiling: String,
    pub autonomous_completion: bool,
    pub canonical_pr_number: Option<i64>,
    pub canonical_head_sha: Option<String>,
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

pub async fn ensure_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS delivery_runs (
            id                     TEXT PRIMARY KEY,
            run_kind               TEXT NOT NULL,
            session_id             TEXT,
            root_turn_id           TEXT,
            task_segment_id        TEXT,
            task_id                TEXT,
            workspace_path         TEXT,
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
            last_observed_at       INTEGER NOT NULL,
            last_progress_at       INTEGER NOT NULL,
            progress_revision      INTEGER NOT NULL DEFAULT 0 CHECK(progress_revision >= 0),
            app_version            TEXT NOT NULL,
            app_build              TEXT NOT NULL,
            process_instance       TEXT NOT NULL,
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

    for index in [
        "CREATE INDEX IF NOT EXISTS idx_delivery_runs_recovery ON delivery_runs(status, lease_expires_at)",
        "CREATE INDEX IF NOT EXISTS idx_delivery_runs_session ON delivery_runs(session_id, updated_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_delivery_runs_pr ON delivery_runs(canonical_pr_number, canonical_head_sha)",
        "CREATE INDEX IF NOT EXISTS idx_delivery_run_events_run ON delivery_run_events(run_id, created_at)",
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
) -> Result<()> {
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

    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO delivery_runs (
            id, run_kind, session_id, root_turn_id, task_segment_id, task_id, workspace_path,
            repo_identity, base_branch, head_branch, change_set_digest, expected_head_sha,
            canonical_pr_number, canonical_pr_url, canonical_head_sha,
            requested_ceiling, reached_ceiling, stage, status, wait_class, next_action,
            next_action_authorized, autonomous_completion,
            failure_signature, stage_attempt, lease_owner, lease_expires_at,
            last_observed_at, last_progress_at, progress_revision, app_version,
            app_build, process_instance, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?)",
    )
    .bind(&run.id)
    .bind(&run.run_kind)
    .bind(&run.session_id)
    .bind(&run.root_turn_id)
    .bind(&run.task_segment_id)
    .bind(&run.task_id)
    .bind(&run.workspace_path)
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
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if inserted == 0 {
        let renewed = sqlx::query(
            "UPDATE delivery_runs
             SET lease_owner=?, lease_expires_at=?, process_instance=?, app_version=?, app_build=?,
                 next_action_authorized=MAX(next_action_authorized, ?),
                 autonomous_completion=MAX(autonomous_completion, ?), updated_at=?
             WHERE id=? AND repo_identity=? AND root_turn_id IS ?
               AND (lease_owner=? OR lease_expires_at IS NULL OR lease_expires_at <= ?)",
        )
        .bind(&process.instance_id)
        .bind(now.saturating_add(lease_ttl))
        .bind(&process.instance_id)
        .bind(&process.app_version)
        .bind(&process.app_build)
        .bind(i64::from(run.next_action_authorized))
        .bind(i64::from(run.autonomous_completion))
        .bind(now)
        .bind(&run.id)
        .bind(&run.repo_identity)
        .bind(&run.root_turn_id)
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
    tx.commit().await?;
    Ok(())
}

/// Persist one delivery observation. Liveness-only observations renew the
/// lease but do not move `last_progress_at` or `progress_revision`.
pub async fn record_delivery_observation(
    pool: &SqlitePool,
    run_id: &str,
    process: &ProcessIdentity,
    observation: &DeliveryObservation,
    now: i64,
    lease_ttl: i64,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let previous = sqlx::query(
        "SELECT head_branch, stage, status, reached_ceiling, expected_head_sha,
                canonical_pr_number, canonical_pr_url, canonical_head_sha, progress_revision
         FROM delivery_runs WHERE id=? AND lease_owner=?",
    )
    .bind(run_id)
    .bind(&process.instance_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        crate::errors::AppError::Other(
            "delivery observation rejected because this process does not own the lease".into(),
        )
    })?;

    let previous_reached = previous.try_get::<String, _>("reached_ceiling")?;
    let reached_ceiling =
        monotonic_reached_ceiling(&previous_reached, &observation.reached_ceiling);
    let canonical_pr_number = observation
        .canonical_pr_number
        .or(previous.try_get::<Option<i64>, _>("canonical_pr_number")?);
    let canonical_pr_url = observation
        .canonical_pr_url
        .clone()
        .or(previous.try_get::<Option<String>, _>("canonical_pr_url")?);
    let canonical_head_sha = observation
        .canonical_head_sha
        .clone()
        .or(previous.try_get::<Option<String>, _>("canonical_head_sha")?);
    let progressed = previous.try_get::<String, _>("head_branch")? != observation.head_branch
        || previous.try_get::<String, _>("stage")? != observation.stage
        || previous.try_get::<String, _>("status")? != observation.status
        || previous_reached != reached_ceiling
        || previous.try_get::<String, _>("expected_head_sha")? != observation.expected_head_sha
        || previous.try_get::<Option<i64>, _>("canonical_pr_number")? != canonical_pr_number
        || previous.try_get::<Option<String>, _>("canonical_head_sha")? != canonical_head_sha;
    let progress_revision = previous.try_get::<i64, _>("progress_revision")?;
    let has_core_input = observation.core_input.is_some();
    let core_input = observation.core_input.as_ref();

    sqlx::query(
        "UPDATE delivery_runs
         SET head_branch=?, stage=?, status=?, wait_class=?, next_action=?, reached_ceiling=?,
             expected_head_sha=?, canonical_pr_number=?, canonical_pr_url=?, canonical_head_sha=?,
             failure_signature=?, stage_attempt=CASE
               WHEN ? IS NULL THEN 0
               WHEN failure_signature = ? THEN stage_attempt
               ELSE 1 END,
             core_input_request_key=CASE WHEN ? THEN ? ELSE core_input_request_key END,
             core_inputs_json=CASE WHEN ? THEN ? ELSE core_inputs_json END,
             core_input_attempts_json=CASE WHEN ? THEN ? ELSE core_input_attempts_json END,
             core_input_resume_stage=CASE WHEN ? THEN ? ELSE core_input_resume_stage END,
             core_input_request_count=CASE WHEN ? THEN 1 ELSE core_input_request_count END,
             lease_expires_at=?, last_observed_at=?,
             last_progress_at=CASE WHEN ? THEN ? ELSE last_progress_at END,
             progress_revision=?, process_instance=?, app_version=?, app_build=?, updated_at=?
         WHERE id=? AND lease_owner=?",
    )
    .bind(&observation.head_branch)
    .bind(&observation.stage)
    .bind(&observation.status)
    .bind(&observation.wait_class)
    .bind(&observation.next_action)
    .bind(&reached_ceiling)
    .bind(&observation.expected_head_sha)
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
    .bind(now.saturating_add(lease_ttl))
    .bind(now)
    .bind(progressed)
    .bind(now)
    .bind(progress_revision + i64::from(progressed))
    .bind(&process.instance_id)
    .bind(&process.app_version)
    .bind(&process.app_build)
    .bind(now)
    .bind(run_id)
    .bind(&process.instance_id)
    .execute(&mut *tx)
    .await?;

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
        "SELECT id, workspace_path, repo_identity, base_branch, head_branch, change_set_digest,
                expected_head_sha, canonical_pr_number, canonical_head_sha, stage, status,
                wait_class, next_action, next_action_authorized, failure_signature,
                stage_attempt, progress_revision, requested_ceiling, autonomous_completion
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
                 last_observed_at=?, updated_at=?
             WHERE id=? AND {NON_TERMINAL_PREDICATE}
               AND (lease_expires_at IS NULL OR lease_expires_at <= ?)"
        );
        let changed = sqlx::query(&claim_sql)
            .bind(&process.instance_id)
            .bind(now.saturating_add(lease_ttl))
            .bind(&process.instance_id)
            .bind(&process.app_version)
            .bind(&process.app_build)
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

        claimed.push(ClaimedRecovery {
            run_id,
            workspace_path: row.try_get("workspace_path")?,
            repo_identity: row.try_get("repo_identity")?,
            base_branch: row.try_get("base_branch")?,
            head_branch: row.try_get("head_branch")?,
            change_set_digest: row.try_get("change_set_digest")?,
            expected_head_sha: row.try_get("expected_head_sha")?,
            requested_ceiling: row.try_get("requested_ceiling")?,
            autonomous_completion: row.try_get::<i64, _>("autonomous_completion")? != 0,
            canonical_pr_number: row.try_get("canonical_pr_number")?,
            canonical_head_sha: row.try_get("canonical_head_sha")?,
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
            run_kind: "chat_delivery".into(),
            session_id: None,
            root_turn_id: None,
            task_segment_id: None,
            task_id: None,
            workspace_path: "/workspace".into(),
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
        };
        assert!(
            record_delivery_observation(&pool, "new-run", &process, &heartbeat, 110, 30)
                .await
                .unwrap()
        );
        assert!(
            !record_delivery_observation(&pool, "new-run", &process, &heartbeat, 120, 30)
                .await
                .unwrap()
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
    async fn later_uncertain_observations_cannot_erase_remote_identity_or_regress_progress() {
        let pool = pool().await;
        let process = ProcessIdentity::new("process", "1.79.0", "17900");
        let run = NewDeliveryRun {
            id: "monotonic-run".into(),
            run_kind: "chat_delivery".into(),
            session_id: Some("session".into()),
            root_turn_id: Some("turn".into()),
            task_segment_id: None,
            task_id: None,
            workspace_path: "/workspace".into(),
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
        };
        record_delivery_observation(
            &pool,
            "monotonic-run",
            &process,
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
                id, run_kind, session_id, root_turn_id, workspace_path, repo_identity, base_branch,
                head_branch, change_set_digest, expected_head_sha, requested_ceiling, reached_ceiling,
                stage, status, wait_class, next_action, next_action_authorized, stage_attempt,
                lease_owner, lease_expires_at, last_observed_at, last_progress_at,
                progress_revision, app_version, app_build, process_instance, created_at, updated_at
             ) VALUES (?, 'chat_delivery', ?, ?, '/workspace', 'example.invalid/repo', 'main',
                       'feature', 'digest', 'abc', 'through_release', 'local',
                       'deliver', ?, 'recoverable', 'observe_remote', 0, 1, 'process-old', ?, 1, 1, 1,
                       '1.78.4', '17804', 'process-old', 1, 1)",
        )
        .bind(id)
        .bind(session_id)
        .bind(root_turn_id)
        .bind(status)
        .bind(lease_expires_at)
        .execute(pool)
        .await
        .unwrap();
    }
}
