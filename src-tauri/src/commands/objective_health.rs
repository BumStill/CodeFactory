// SPDX-License-Identifier: Apache-2.0

//! Read-only Objective control-plane health aggregates.
//!
//! The snapshot deliberately contains counts and latency aggregates only. It
//! never exposes Objective ids, decision envelopes, or side-effect identities.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::collections::{HashMap, HashSet};
use tauri::State;

use crate::errors::AppError;
use crate::AppState;

const HEALTH_WINDOW_MS: i64 = 86_400_000;
const STALLED_PROGRESS_MS: i64 = 300_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveHealthAvailability {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveHealthMetrics {
    /// All non-terminal, non-quarantined Objectives at query time.
    pub open: i64,
    /// Open Objectives whose next action remains owned by the system.
    pub system_owned: i64,
    /// Open Objectives in one of the three structurally typed attention states.
    pub typed_user_attention: i64,
    /// Distinct Objective revisions that projected technical work to the user.
    pub technical_user_handoff_violations: i64,
    /// The same violation scoped to the trailing production-review window.
    pub technical_user_handoff_violations_24h: i64,
    /// Real user turns attributed to an already-open, system-owned technical objective.
    pub avoidable_user_reprompts_24h: i64,
    /// Due remediation rows without a currently valid owner lease.
    pub overdue_ownerless_remediations: i64,
    pub stalled_system_owned_objectives: i64,
    pub unavailable_domain_adapter_objectives: i64,
    /// Completed Objective rows that do not retain the completion predicate.
    pub invalid_completions: i64,
    pub invalid_completions_24h: i64,
    /// Committed receipts beyond the first for one Objective/action fingerprint.
    pub duplicate_committed_side_effect_receipts: i64,
    pub duplicate_committed_side_effect_receipts_24h: i64,
    /// Delivery attempts whose persisted effective ceiling was below the user request.
    pub requested_ceiling_downgrades_24h: i64,
    /// Lifetime technical recovery decisions in the available journal.
    pub recovery_decisions: i64,
    /// Lifetime distinct Objectives with a later active/completed event.
    pub recovered_objectives: i64,
    pub recovery_latency_p50_ms: Option<i64>,
    pub recovery_latency_p95_ms: Option<i64>,
    /// Technical recovery decisions created in the trailing 24-hour window.
    pub recovery_decisions_24h: i64,
    /// Distinct recovered Objectives whose recovery decision began in that window.
    pub recovered_objectives_24h: i64,
    pub recovery_latency_p50_ms_24h: Option<i64>,
    pub recovery_latency_p95_ms_24h: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveHealthSnapshot {
    pub generated_at_ms: i64,
    pub window_start_ms: i64,
    /// Exact release build when compiled by the release workflow. Development
    /// builds intentionally report `None` and cannot satisfy production proof.
    pub build_git_sha: Option<String>,
    pub availability: ObjectiveHealthAvailability,
    pub unavailable_reason: Option<String>,
    /// `None` is intentional when unavailable; absence must not be read as zero.
    pub metrics: Option<ObjectiveHealthMetrics>,
}

impl ObjectiveHealthSnapshot {
    fn available(now_ms: i64, metrics: ObjectiveHealthMetrics) -> Self {
        Self {
            generated_at_ms: now_ms,
            window_start_ms: now_ms.saturating_sub(HEALTH_WINDOW_MS),
            build_git_sha: release_build_git_sha(),
            availability: ObjectiveHealthAvailability::Available,
            unavailable_reason: None,
            metrics: Some(metrics),
        }
    }

    fn unavailable(now_ms: i64, reason: impl Into<String>) -> Self {
        Self {
            generated_at_ms: now_ms,
            window_start_ms: now_ms.saturating_sub(HEALTH_WINDOW_MS),
            build_git_sha: release_build_git_sha(),
            availability: ObjectiveHealthAvailability::Unavailable,
            unavailable_reason: Some(reason.into()),
            metrics: None,
        }
    }
}

fn release_build_git_sha() -> Option<String> {
    option_env!("CODEFACTORY_BUILD_GIT_SHA")
        .filter(|value| {
            value.len() == 40
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .map(ToOwned::to_owned)
}

const REQUIRED_SCHEMA: &[(&str, &[&str])] = &[
    (
        "objectives",
        &[
            "id",
            "revision",
            "status",
            "decision_type",
            "domain",
            "requires_user_action",
            "recovery_owner",
            "remediation_id",
            "evidence_ref",
            "reached_acceptance",
            "completed_at",
            "lease_owner",
            "lease_expires_at",
            "last_progress_at",
            "created_at",
        ],
    ),
    (
        "objective_decisions",
        &[
            "objective_id",
            "revision",
            "decision_type",
            "requires_user_action",
            "recovery_owner",
            "remediation_id",
            "envelope_json",
            "evidence_ref",
            "created_at",
        ],
    ),
    (
        "objective_events",
        &["objective_id", "revision", "status", "created_at"],
    ),
    (
        "objective_evidence",
        &["objective_id", "revision", "evidence_ref"],
    ),
    (
        "objective_remediations",
        &[
            "status",
            "next_observation_at",
            "lease_owner",
            "lease_expires_at",
        ],
    ),
    (
        "side_effect_receipts",
        &[
            "objective_id",
            "action_fingerprint",
            "status",
            "observed_at",
        ],
    ),
    (
        "chat_turn_state",
        &["objective_id", "user_reprompt_driver", "updated_at"],
    ),
    ("tool_calls", &["tool_name", "metadata", "created_at"]),
];

/// Query the durable Objective health surface at a caller-supplied clock.
///
/// Missing or partial Objective schema returns an explicit unavailable
/// snapshot with no metrics. Query/inspection errors take the same fail-closed
/// path, so callers can never mistake a broken aggregate for a healthy zero.
pub async fn query_objective_health(pool: &SqlitePool, now_ms: i64) -> ObjectiveHealthSnapshot {
    match missing_schema_parts(pool).await {
        Ok(parts) if !parts.is_empty() => {
            return ObjectiveHealthSnapshot::unavailable(
                now_ms,
                format!(
                    "objective health unavailable: missing required schema: {}",
                    parts.join(", ")
                ),
            );
        }
        Ok(_) => {}
        Err(error) => {
            return ObjectiveHealthSnapshot::unavailable(
                now_ms,
                format!("objective health unavailable: schema inspection failed: {error}"),
            );
        }
    }

    match aggregate_health(pool, now_ms).await {
        Ok(metrics) => ObjectiveHealthSnapshot::available(now_ms, metrics),
        Err(error) => ObjectiveHealthSnapshot::unavailable(
            now_ms,
            format!("objective health unavailable: aggregate query failed: {error}"),
        ),
    }
}

/// Read-only Tauri projection for the formal control-plane surface.
///
/// The database handle is cloned while the application state lock is held, so
/// the aggregate itself never blocks writers behind that lock. Aggregate and
/// schema failures remain data (`availability = unavailable`), not a healthy
/// empty response or a user-retryable command error.
#[tauri::command]
pub async fn get_objective_health(
    state: State<'_, AppState>,
) -> Result<ObjectiveHealthSnapshot, AppError> {
    let pool = state.db.read().await.clone();
    Ok(query_objective_health(&pool, chrono::Utc::now().timestamp_millis()).await)
}

async fn missing_schema_parts(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    let mut missing = Vec::new();
    for &(table, required_columns) in REQUIRED_SCHEMA {
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(pool)
                .await?;
        if exists == 0 {
            missing.push(table.to_string());
            continue;
        }

        // Table names come only from REQUIRED_SCHEMA, never from user input.
        let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(pool)
            .await?;
        let present: HashSet<String> = rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("name").ok())
            .collect();
        for &column in required_columns {
            if !present.contains(column) {
                missing.push(format!("{table}.{column}"));
            }
        }
    }
    Ok(missing)
}

#[derive(Debug)]
struct JournalDecision {
    objective_id: String,
    revision: i64,
    decision_type: String,
    requires_user_action: bool,
    recovery_owner: Option<String>,
    remediation_id: Option<String>,
    envelope: EnvelopeProjection,
    created_at: i64,
}

#[derive(Debug, Default)]
struct EnvelopeProjection {
    decision_type: Option<String>,
    status: Option<String>,
    requires_user_action: Option<bool>,
    recovery_owner_present: bool,
    remediation_id_present: bool,
}

impl EnvelopeProjection {
    fn parse(raw: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return Self::default();
        };
        Self {
            decision_type: string_field(&value, "decision_type"),
            status: string_field(&value, "status"),
            requires_user_action: value.get("requires_user_action").and_then(Value::as_bool),
            recovery_owner_present: nonempty_string_field(&value, "recovery_owner"),
            remediation_id_present: nonempty_string_field(&value, "remediation_id"),
        }
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn nonempty_string_field(value: &Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
}

fn is_typed_attention_decision(value: &str) -> bool {
    matches!(
        value,
        "core_input_required" | "authorization_required" | "needs_business_decision"
    )
}

fn is_technical_recovery_decision(value: &str) -> bool {
    matches!(
        value,
        "waiting" | "apply_recommended" | "platform_incident" | "failed_internal"
    )
}

impl JournalDecision {
    fn requires_user_action(&self) -> bool {
        self.requires_user_action || self.envelope.requires_user_action == Some(true)
    }

    fn is_technical(&self) -> bool {
        is_technical_recovery_decision(&self.decision_type)
            || self
                .envelope
                .decision_type
                .as_deref()
                .is_some_and(is_technical_recovery_decision)
    }

    fn is_user_handoff_violation(&self) -> bool {
        if !self.requires_user_action() {
            return false;
        }
        let technical_status = self
            .envelope
            .status
            .as_deref()
            .is_some_and(|status| matches!(status, "active" | "waiting_system"));
        self.is_technical()
            || technical_status
            || self
                .recovery_owner
                .as_deref()
                .is_some_and(|owner| !owner.trim().is_empty())
            || self
                .remediation_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
            || self.envelope.recovery_owner_present
            || self.envelope.remediation_id_present
    }

    fn is_recovery_decision(&self) -> bool {
        self.is_technical() && !self.requires_user_action()
    }
}

#[derive(Debug)]
struct RecoveryEvent {
    revision: i64,
    created_at: i64,
}

async fn aggregate_health(
    pool: &SqlitePool,
    now_ms: i64,
) -> Result<ObjectiveHealthMetrics, sqlx::Error> {
    let window_start_ms = now_ms.saturating_sub(HEALTH_WINDOW_MS);
    let open: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives
         WHERE status NOT IN ('completed','cancelled','legacy_orphan')",
    )
    .fetch_one(pool)
    .await?;
    let system_owned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives
         WHERE status IN ('active','waiting_system')
           AND requires_user_action=0",
    )
    .fetch_one(pool)
    .await?;
    let typed_user_attention: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives
         WHERE requires_user_action<>0 AND (
           (status='waiting_core_input' AND decision_type='core_input_required') OR
           (status='waiting_authorization' AND decision_type='authorization_required') OR
           (status='waiting_business_decision' AND decision_type='needs_business_decision')
         )",
    )
    .fetch_one(pool)
    .await?;
    let overdue_ownerless_remediations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objective_remediations
         WHERE status NOT IN ('completed','cancelled','superseded')
           AND next_observation_at<=?
           AND (
             NULLIF(TRIM(lease_owner),'') IS NULL OR
             lease_expires_at IS NULL OR lease_expires_at<=?
           )",
    )
    .bind(now_ms)
    .bind(now_ms)
    .fetch_one(pool)
    .await?;
    let stalled_system_owned_objectives: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives
         WHERE status IN ('active','waiting_system')
           AND requires_user_action=0
           AND COALESCE(last_progress_at, created_at)<=?",
    )
    .bind(now_ms.saturating_sub(STALLED_PROGRESS_MS))
    .fetch_one(pool)
    .await?;
    let open_domains = sqlx::query_scalar::<_, String>(
        "SELECT domain FROM objectives
         WHERE status IN ('active','waiting_system') AND requires_user_action=0",
    )
    .fetch_all(pool)
    .await?;
    let unavailable_domain_adapter_objectives = open_domains
        .into_iter()
        .filter(|domain| {
            crate::agent::objective::RecoveryDomain::ALL
                .into_iter()
                .find(|candidate| candidate.as_str() == domain)
                .map(|domain| {
                    !crate::agent::objective_supervisor::domain_has_executable_adapter(domain)
                })
                .unwrap_or(true)
        })
        .count() as i64;
    let invalid_completions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives objective
         WHERE objective.status='completed' AND (
           objective.decision_type<>'complete' OR
           objective.requires_user_action<>0 OR
           NULLIF(TRIM(objective.evidence_ref),'') IS NULL OR
           NULLIF(TRIM(objective.reached_acceptance),'') IS NULL OR
           objective.completed_at IS NULL OR
           NULLIF(TRIM(objective.recovery_owner),'') IS NOT NULL OR
           NULLIF(TRIM(objective.remediation_id),'') IS NOT NULL OR
           NULLIF(TRIM(objective.lease_owner),'') IS NOT NULL OR
           objective.lease_expires_at IS NOT NULL OR
           NOT EXISTS (
             SELECT 1 FROM objective_decisions decision
             WHERE decision.objective_id=objective.id
               AND decision.revision=objective.revision
               AND decision.decision_type='complete'
               AND decision.evidence_ref=objective.evidence_ref
               AND json_valid(decision.envelope_json)
               AND json_extract(decision.envelope_json, '$.decision_type')='complete'
               AND json_extract(decision.envelope_json, '$.status')='completed'
               AND json_extract(decision.envelope_json, '$.reached_acceptance')
                   =objective.reached_acceptance
           ) OR
           NOT EXISTS (
             SELECT 1 FROM objective_evidence evidence
             WHERE evidence.objective_id=objective.id
               AND evidence.revision=objective.revision
               AND evidence.evidence_ref=objective.evidence_ref
           )
         )",
    )
    .fetch_one(pool)
    .await?;
    let invalid_completions_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives objective
         WHERE objective.status='completed'
           AND objective.completed_at BETWEEN ? AND ?
           AND (
             objective.decision_type<>'complete' OR
             objective.requires_user_action<>0 OR
             NULLIF(TRIM(objective.evidence_ref),'') IS NULL OR
             NULLIF(TRIM(objective.reached_acceptance),'') IS NULL OR
             NULLIF(TRIM(objective.recovery_owner),'') IS NOT NULL OR
             NULLIF(TRIM(objective.remediation_id),'') IS NOT NULL OR
             NULLIF(TRIM(objective.lease_owner),'') IS NOT NULL OR
             objective.lease_expires_at IS NOT NULL OR
             NOT EXISTS (
               SELECT 1 FROM objective_decisions decision
               WHERE decision.objective_id=objective.id
                 AND decision.revision=objective.revision
                 AND decision.decision_type='complete'
                 AND decision.evidence_ref=objective.evidence_ref
                 AND json_valid(decision.envelope_json)
                 AND json_extract(decision.envelope_json, '$.decision_type')='complete'
                 AND json_extract(decision.envelope_json, '$.status')='completed'
                 AND json_extract(decision.envelope_json, '$.reached_acceptance')
                     =objective.reached_acceptance
             ) OR
             NOT EXISTS (
               SELECT 1 FROM objective_evidence evidence
               WHERE evidence.objective_id=objective.id
                 AND evidence.revision=objective.revision
                 AND evidence.evidence_ref=objective.evidence_ref
             )
           )",
    )
    .bind(window_start_ms)
    .bind(now_ms)
    .fetch_one(pool)
    .await?;
    let duplicate_committed_side_effect_receipts: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(receipt_count - 1), 0) FROM (
           SELECT COUNT(*) AS receipt_count
           FROM side_effect_receipts
           WHERE status='committed'
           GROUP BY objective_id, action_fingerprint
           HAVING COUNT(*) > 1
         )",
    )
    .fetch_one(pool)
    .await?;
    let duplicate_committed_side_effect_receipts_24h: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(receipt_count - 1), 0) FROM (
           SELECT COUNT(*) AS receipt_count
           FROM side_effect_receipts
           WHERE status='committed' AND observed_at BETWEEN ? AND ?
           GROUP BY objective_id, action_fingerprint
           HAVING COUNT(*) > 1
         )",
    )
    .bind(window_start_ms)
    .bind(now_ms)
    .fetch_one(pool)
    .await?;

    let mut handoff_revisions: HashSet<(String, i64)> = HashSet::new();
    let current_handoffs = sqlx::query(
        "SELECT id, revision, status, decision_type, requires_user_action,
                recovery_owner, remediation_id
         FROM objectives WHERE requires_user_action<>0",
    )
    .fetch_all(pool)
    .await?;
    for row in current_handoffs {
        let status: String = row.try_get("status")?;
        let decision_type: String = row.try_get("decision_type")?;
        let recovery_owner: Option<String> = row.try_get("recovery_owner")?;
        let remediation_id: Option<String> = row.try_get("remediation_id")?;
        let typed_pair = matches!(
            (status.as_str(), decision_type.as_str()),
            ("waiting_core_input", "core_input_required")
                | ("waiting_authorization", "authorization_required")
                | ("waiting_business_decision", "needs_business_decision")
        );
        let hybrid_owner = recovery_owner
            .as_deref()
            .is_some_and(|owner| !owner.trim().is_empty())
            || remediation_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty());
        if !typed_pair || hybrid_owner || !is_typed_attention_decision(&decision_type) {
            handoff_revisions.insert((row.try_get("id")?, row.try_get("revision")?));
        }
    }

    let rows = sqlx::query(
        "SELECT objective_id, revision, decision_type, requires_user_action,
                recovery_owner, remediation_id, envelope_json, created_at
         FROM objective_decisions WHERE created_at<=?",
    )
    .bind(now_ms)
    .fetch_all(pool)
    .await?;
    let mut decisions = Vec::with_capacity(rows.len());
    for row in rows {
        let raw_envelope: String = row.try_get("envelope_json")?;
        decisions.push(JournalDecision {
            objective_id: row.try_get("objective_id")?,
            revision: row.try_get("revision")?,
            decision_type: row.try_get("decision_type")?,
            requires_user_action: row.try_get::<i64, _>("requires_user_action")? != 0,
            recovery_owner: row.try_get("recovery_owner")?,
            remediation_id: row.try_get("remediation_id")?,
            envelope: EnvelopeProjection::parse(&raw_envelope),
            created_at: row.try_get("created_at")?,
        });
    }
    for decision in &decisions {
        if decision.is_user_handoff_violation() {
            handoff_revisions.insert((decision.objective_id.clone(), decision.revision));
        }
    }

    let technical_user_handoff_violations_24h = decisions
        .iter()
        .filter(|decision| {
            decision.created_at >= window_start_ms && decision.is_user_handoff_violation()
        })
        .map(|decision| (decision.objective_id.as_str(), decision.revision))
        .collect::<HashSet<_>>()
        .len() as i64;
    let avoidable_user_reprompts_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_turn_state
         WHERE user_reprompt_driver IN (
           'recoverable_waiting_open',
           'completion_arbitration_open',
           'system_owned_remediation_open',
           'authorized_objective_still_open'
         )
           AND objective_id IS NOT NULL
           AND updated_at BETWEEN ? AND ?",
    )
    .bind(window_start_ms)
    .bind(now_ms)
    .fetch_one(pool)
    .await?;
    let requested_ceiling_downgrades_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tool_calls
         WHERE tool_name='deliver_changes'
           AND created_at BETWEEN ? AND ?
           AND metadata IS NOT NULL
           AND json_valid(metadata)
           AND NULLIF(json_extract(metadata, '$.requested_ceiling'), '') IS NOT NULL
           AND NULLIF(json_extract(metadata, '$.effective_ceiling'), '') IS NOT NULL
           AND json_extract(metadata, '$.requested_ceiling')
               <> json_extract(metadata, '$.effective_ceiling')",
    )
    .bind(window_start_ms)
    .bind(now_ms)
    .fetch_one(pool)
    .await?;

    let event_rows = sqlx::query(
        "SELECT objective_id, revision, created_at FROM objective_events
         WHERE created_at<=? AND status IN ('active','completed')
         ORDER BY objective_id, created_at",
    )
    .bind(now_ms)
    .fetch_all(pool)
    .await?;
    let mut events_by_objective: HashMap<String, Vec<RecoveryEvent>> = HashMap::new();
    for row in event_rows {
        events_by_objective
            .entry(row.try_get("objective_id")?)
            .or_default()
            .push(RecoveryEvent {
                revision: row.try_get("revision")?,
                created_at: row.try_get("created_at")?,
            });
    }

    let recovery_decisions: Vec<&JournalDecision> = decisions
        .iter()
        .filter(|decision| decision.is_recovery_decision())
        .collect();
    let recovery_decisions_24h: Vec<&JournalDecision> = recovery_decisions
        .iter()
        .copied()
        .filter(|decision| decision.created_at >= window_start_ms)
        .collect();
    let (recovered_objectives, mut recovery_latencies) =
        recovery_results(&recovery_decisions, &events_by_objective);
    let (recovered_objectives_24h, mut recovery_latencies_24h) =
        recovery_results(&recovery_decisions_24h, &events_by_objective);
    recovery_latencies.sort_unstable();
    recovery_latencies_24h.sort_unstable();

    Ok(ObjectiveHealthMetrics {
        open,
        system_owned,
        typed_user_attention,
        technical_user_handoff_violations: handoff_revisions.len() as i64,
        technical_user_handoff_violations_24h,
        avoidable_user_reprompts_24h,
        overdue_ownerless_remediations,
        stalled_system_owned_objectives,
        unavailable_domain_adapter_objectives,
        invalid_completions,
        invalid_completions_24h,
        duplicate_committed_side_effect_receipts,
        duplicate_committed_side_effect_receipts_24h,
        requested_ceiling_downgrades_24h,
        recovery_decisions: recovery_decisions.len() as i64,
        recovered_objectives,
        recovery_latency_p50_ms: nearest_rank_percentile(&recovery_latencies, 50),
        recovery_latency_p95_ms: nearest_rank_percentile(&recovery_latencies, 95),
        recovery_decisions_24h: recovery_decisions_24h.len() as i64,
        recovered_objectives_24h,
        recovery_latency_p50_ms_24h: nearest_rank_percentile(&recovery_latencies_24h, 50),
        recovery_latency_p95_ms_24h: nearest_rank_percentile(&recovery_latencies_24h, 95),
    })
}

fn recovery_results(
    decisions: &[&JournalDecision],
    events_by_objective: &HashMap<String, Vec<RecoveryEvent>>,
) -> (i64, Vec<i64>) {
    let mut recovered_objectives = HashSet::new();
    let mut latencies = Vec::new();
    for decision in decisions {
        let recovered_at = events_by_objective
            .get(&decision.objective_id)
            .and_then(|events| {
                events
                    .iter()
                    .filter(|event| {
                        event.revision > decision.revision
                            && event.created_at >= decision.created_at
                    })
                    .map(|event| event.created_at)
                    .min()
            });
        if let Some(recovered_at) = recovered_at {
            recovered_objectives.insert(decision.objective_id.as_str());
            latencies.push(recovered_at.saturating_sub(decision.created_at));
        }
    }
    (recovered_objectives.len() as i64, latencies)
}

fn nearest_rank_percentile(sorted_values: &[i64], percentile: usize) -> Option<i64> {
    if sorted_values.is_empty() {
        return None;
    }
    let rank = (percentile * sorted_values.len()).div_ceil(100);
    sorted_values.get(rank.saturating_sub(1)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;

    const NOW_MS: i64 = 100_000_000;

    async fn pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    async fn install_health_schema(pool: &SqlitePool) {
        sqlx::raw_sql(
            "CREATE TABLE objectives (
                id TEXT PRIMARY KEY,
                revision INTEGER NOT NULL,
                status TEXT NOT NULL,
                decision_type TEXT NOT NULL,
                domain TEXT NOT NULL DEFAULT 'chat',
                requires_user_action INTEGER NOT NULL,
                recovery_owner TEXT,
                remediation_id TEXT,
                evidence_ref TEXT,
                reached_acceptance TEXT,
                completed_at INTEGER,
                lease_owner TEXT,
                lease_expires_at INTEGER,
                last_progress_at INTEGER,
                created_at INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE objective_decisions (
                id TEXT PRIMARY KEY,
                objective_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                decision_type TEXT NOT NULL,
                requires_user_action INTEGER NOT NULL,
                recovery_owner TEXT,
                remediation_id TEXT,
                envelope_json TEXT NOT NULL,
                evidence_ref TEXT,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE objective_events (
                objective_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                status TEXT,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE objective_evidence (
                objective_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                evidence_ref TEXT NOT NULL
             );
             CREATE TABLE objective_remediations (
                status TEXT NOT NULL,
                next_observation_at INTEGER NOT NULL,
                lease_owner TEXT,
                lease_expires_at INTEGER
             );
             CREATE TABLE side_effect_receipts (
                objective_id TEXT NOT NULL,
                action_fingerprint TEXT NOT NULL,
                status TEXT NOT NULL,
                observed_at INTEGER NOT NULL
             );
             CREATE TABLE chat_turn_state (
                objective_id TEXT,
                user_reprompt_driver TEXT,
                updated_at INTEGER NOT NULL
             );
             CREATE TABLE tool_calls (
                tool_name TEXT NOT NULL,
                metadata TEXT,
                created_at INTEGER NOT NULL
             );",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_objective(
        pool: &SqlitePool,
        id: &str,
        revision: i64,
        status: &str,
        decision_type: &str,
        requires_user_action: bool,
        recovery_owner: Option<&str>,
        remediation_id: Option<&str>,
        evidence_ref: Option<&str>,
        reached_acceptance: Option<&str>,
        completed_at: Option<i64>,
    ) {
        sqlx::query(
            "INSERT INTO objectives
             (id, revision, status, decision_type, requires_user_action,
              recovery_owner, remediation_id, evidence_ref, reached_acceptance,
              completed_at, lease_owner, lease_expires_at, last_progress_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?)",
        )
        .bind(id)
        .bind(revision)
        .bind(status)
        .bind(decision_type)
        .bind(i64::from(requires_user_action))
        .bind(recovery_owner)
        .bind(remediation_id)
        .bind(evidence_ref)
        .bind(reached_acceptance)
        .bind(completed_at)
        .bind(NOW_MS)
        .bind(NOW_MS)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_decision(
        pool: &SqlitePool,
        id: &str,
        objective_id: &str,
        revision: i64,
        decision_type: &str,
        requires_user_action: bool,
        recovery_owner: Option<&str>,
        remediation_id: Option<&str>,
        envelope: serde_json::Value,
        evidence_ref: Option<&str>,
        created_at: i64,
    ) {
        sqlx::query(
            "INSERT INTO objective_decisions
             (id, objective_id, revision, decision_type, requires_user_action,
              recovery_owner, remediation_id, envelope_json, evidence_ref, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(objective_id)
        .bind(revision)
        .bind(decision_type)
        .bind(i64::from(requires_user_action))
        .bind(recovery_owner)
        .bind(remediation_id)
        .bind(envelope.to_string())
        .bind(evidence_ref)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn missing_schema_is_explicitly_unavailable_instead_of_reporting_zeroes() {
        let pool = pool().await;

        let snapshot = query_objective_health(&pool, NOW_MS).await;

        assert_eq!(
            snapshot.availability,
            ObjectiveHealthAvailability::Unavailable
        );
        assert!(snapshot.metrics.is_none());
        assert!(snapshot
            .unavailable_reason
            .as_deref()
            .unwrap_or_default()
            .contains("missing"));
        let serialized = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(serialized["availability"], "unavailable");
        assert!(serialized["metrics"].is_null());
    }

    #[tokio::test]
    async fn partial_schema_is_unavailable_instead_of_running_partial_aggregates() {
        let pool = pool().await;
        sqlx::query("CREATE TABLE objectives (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();

        let snapshot = query_objective_health(&pool, NOW_MS).await;

        assert_eq!(
            snapshot.availability,
            ObjectiveHealthAvailability::Unavailable
        );
        assert!(snapshot.metrics.is_none());
        let reason = snapshot.unavailable_reason.unwrap_or_default();
        assert!(reason.contains("objectives.revision"));
        assert!(reason.contains("objective_decisions"));
    }

    #[tokio::test]
    async fn empty_complete_schema_reports_available_zeroes_and_null_latency() {
        let pool = pool().await;
        install_health_schema(&pool).await;

        let snapshot = query_objective_health(&pool, NOW_MS).await;
        let metrics = snapshot.metrics.expect("available metrics");

        assert_eq!(
            snapshot.availability,
            ObjectiveHealthAvailability::Available
        );
        assert_eq!(snapshot.window_start_ms, NOW_MS - 86_400_000);
        assert_eq!(metrics.open, 0);
        assert_eq!(metrics.system_owned, 0);
        assert_eq!(metrics.typed_user_attention, 0);
        assert_eq!(metrics.technical_user_handoff_violations, 0);
        assert_eq!(metrics.technical_user_handoff_violations_24h, 0);
        assert_eq!(metrics.avoidable_user_reprompts_24h, 0);
        assert_eq!(metrics.overdue_ownerless_remediations, 0);
        assert_eq!(metrics.stalled_system_owned_objectives, 0);
        assert_eq!(metrics.unavailable_domain_adapter_objectives, 0);
        assert_eq!(metrics.invalid_completions, 0);
        assert_eq!(metrics.invalid_completions_24h, 0);
        assert_eq!(metrics.duplicate_committed_side_effect_receipts, 0);
        assert_eq!(metrics.duplicate_committed_side_effect_receipts_24h, 0);
        assert_eq!(metrics.requested_ceiling_downgrades_24h, 0);
        assert_eq!(metrics.recovery_decisions, 0);
        assert_eq!(metrics.recovered_objectives, 0);
        assert_eq!(metrics.recovery_latency_p50_ms, None);
        assert_eq!(metrics.recovery_latency_p95_ms, None);
        assert_eq!(metrics.recovery_decisions_24h, 0);
        assert_eq!(metrics.recovered_objectives_24h, 0);
        assert_eq!(metrics.recovery_latency_p50_ms_24h, None);
        assert_eq!(metrics.recovery_latency_p95_ms_24h, None);
    }

    #[tokio::test]
    async fn stalled_and_non_executable_domain_work_is_visible_to_the_release_gate() {
        let pool = pool().await;
        install_health_schema(&pool).await;

        insert_objective(
            &pool,
            "stalled-browser",
            2,
            "waiting_system",
            "platform_incident",
            false,
            Some("objective-supervisor:browser"),
            Some("remediation-stalled"),
            None,
            None,
            None,
        )
        .await;
        sqlx::query(
            "UPDATE objectives
             SET domain='browser', last_progress_at=?, created_at=?
             WHERE id='stalled-browser'",
        )
        .bind(NOW_MS - 300_001)
        .bind(NOW_MS - 300_001)
        .execute(&pool)
        .await
        .unwrap();

        insert_objective(
            &pool,
            "fresh-chat",
            2,
            "waiting_system",
            "waiting",
            false,
            Some("objective-supervisor:chat"),
            Some("remediation-fresh"),
            None,
            None,
            None,
        )
        .await;
        sqlx::query(
            "UPDATE objectives SET domain='chat', last_progress_at=?, created_at=?
             WHERE id='fresh-chat'",
        )
        .bind(NOW_MS - 1)
        .bind(NOW_MS - 1)
        .execute(&pool)
        .await
        .unwrap();

        let metrics = query_objective_health(&pool, NOW_MS)
            .await
            .metrics
            .expect("available metrics");
        assert_eq!(metrics.stalled_system_owned_objectives, 1);
        assert_eq!(metrics.unavailable_domain_adapter_objectives, 1);
    }

    #[tokio::test]
    async fn production_window_detects_avoidable_reprompts_and_ceiling_downgrades_only_in_24h() {
        let pool = pool().await;
        install_health_schema(&pool).await;

        for (id, driver, observed_at) in [
            ("technical-current", "recoverable_waiting_open", NOW_MS - 1),
            (
                "technical-old",
                "system_owned_remediation_open",
                NOW_MS - HEALTH_WINDOW_MS - 1,
            ),
            ("typed-input", "core_input_response", NOW_MS - 1),
        ] {
            sqlx::query(
                "INSERT INTO chat_turn_state
                 (objective_id, user_reprompt_driver, updated_at) VALUES (?, ?, ?)",
            )
            .bind(id)
            .bind(driver)
            .bind(observed_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (id, requested, effective, observed_at) in [
            (
                "current-downgrade",
                "through_release",
                "through_merge",
                NOW_MS - 1,
            ),
            (
                "old-downgrade",
                "through_release",
                "pr_only",
                NOW_MS - HEALTH_WINDOW_MS - 1,
            ),
            (
                "current-exact",
                "through_release",
                "through_release",
                NOW_MS - 1,
            ),
        ] {
            sqlx::query(
                "INSERT INTO tool_calls (tool_name, metadata, created_at)
                 VALUES ('deliver_changes', ?, ?)",
            )
            .bind(
                json!({
                    "id": id,
                    "requested_ceiling": requested,
                    "effective_ceiling": effective,
                })
                .to_string(),
            )
            .bind(observed_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let metrics = query_objective_health(&pool, NOW_MS)
            .await
            .metrics
            .expect("available metrics");
        assert_eq!(metrics.avoidable_user_reprompts_24h, 1);
        assert_eq!(metrics.requested_ceiling_downgrades_24h, 1);
    }

    #[tokio::test]
    async fn production_window_counts_only_recent_technical_user_handoffs() {
        let pool = pool().await;
        install_health_schema(&pool).await;
        for (id, objective_id, created_at) in [
            ("recent", "objective-recent", NOW_MS - 1),
            ("old", "objective-old", NOW_MS - HEALTH_WINDOW_MS - 1),
        ] {
            insert_decision(
                &pool,
                id,
                objective_id,
                2,
                "platform_incident",
                true,
                Some("objective-supervisor:provider"),
                Some("remediation"),
                json!({
                    "decision_type":"platform_incident",
                    "status":"waiting_system",
                    "requires_user_action":true
                }),
                None,
                created_at,
            )
            .await;
        }

        let metrics = query_objective_health(&pool, NOW_MS)
            .await
            .metrics
            .expect("available metrics");
        assert_eq!(metrics.technical_user_handoff_violations, 2);
        assert_eq!(metrics.technical_user_handoff_violations_24h, 1);
    }

    #[tokio::test]
    async fn actual_runtime_schema_reports_available_zeroes() {
        let pool = pool().await;
        sqlx::raw_sql(include_str!("../../migrations/0001_init.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/0003_session_execution_governance.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/0007_unified_objective_control_plane.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("ALTER TABLE chat_turn_state ADD COLUMN user_reprompt_driver TEXT")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE chat_turn_state ADD COLUMN objective_id TEXT")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN metadata TEXT")
            .execute(&pool)
            .await
            .unwrap();

        let snapshot = query_objective_health(&pool, NOW_MS).await;
        assert_eq!(
            snapshot.availability,
            ObjectiveHealthAvailability::Available,
            "unexpected unavailable reason: {:?}",
            snapshot.unavailable_reason
        );
        assert_eq!(snapshot.metrics.expect("available metrics").open, 0);
    }

    #[tokio::test]
    async fn completion_without_matching_evidence_and_acceptance_is_invalid() {
        let pool = pool().await;
        install_health_schema(&pool).await;

        insert_objective(
            &pool,
            "missing-evidence",
            2,
            "completed",
            "complete",
            false,
            None,
            None,
            Some("evidence://missing"),
            Some("validated_change"),
            Some(NOW_MS - 10),
        )
        .await;
        insert_decision(
            &pool,
            "missing-evidence-decision",
            "missing-evidence",
            2,
            "complete",
            false,
            None,
            None,
            json!({
                "decision_type":"complete",
                "status":"completed",
                "requires_user_action":false,
                "reached_acceptance":"validated_change"
            }),
            Some("evidence://missing"),
            NOW_MS - 10,
        )
        .await;

        insert_objective(
            &pool,
            "mismatched-acceptance",
            2,
            "completed",
            "complete",
            false,
            None,
            None,
            Some("evidence://mismatch"),
            Some("live_verification"),
            Some(NOW_MS - 5),
        )
        .await;
        insert_decision(
            &pool,
            "mismatched-acceptance-decision",
            "mismatched-acceptance",
            2,
            "complete",
            false,
            None,
            None,
            json!({
                "decision_type":"complete",
                "status":"completed",
                "requires_user_action":false,
                "reached_acceptance":"validated_change"
            }),
            Some("evidence://mismatch"),
            NOW_MS - 5,
        )
        .await;
        sqlx::query(
            "INSERT INTO objective_evidence
             (objective_id, revision, evidence_ref) VALUES (?, ?, ?)",
        )
        .bind("mismatched-acceptance")
        .bind(2_i64)
        .bind("evidence://mismatch")
        .execute(&pool)
        .await
        .unwrap();

        let metrics = query_objective_health(&pool, NOW_MS)
            .await
            .metrics
            .expect("available metrics");
        assert_eq!(metrics.invalid_completions, 2);
    }

    #[tokio::test]
    async fn real_sqlite_journal_aggregates_health_violations_and_recovery_percentiles() {
        let pool = pool().await;
        install_health_schema(&pool).await;

        insert_objective(
            &pool,
            "active",
            2,
            "active",
            "continue",
            false,
            Some("chat-foreground"),
            None,
            None,
            None,
            None,
        )
        .await;
        insert_objective(
            &pool,
            "waiting-system",
            2,
            "waiting_system",
            "waiting",
            false,
            Some("objective-supervisor:chat"),
            Some("remediation-1"),
            None,
            None,
            None,
        )
        .await;
        for (id, status, decision_type) in [
            ("core-input", "waiting_core_input", "core_input_required"),
            (
                "authorization",
                "waiting_authorization",
                "authorization_required",
            ),
            (
                "business-decision",
                "waiting_business_decision",
                "needs_business_decision",
            ),
        ] {
            insert_objective(
                &pool,
                id,
                2,
                status,
                decision_type,
                true,
                None,
                None,
                None,
                None,
                None,
            )
            .await;
        }
        insert_objective(
            &pool,
            "current-handoff",
            4,
            "waiting_system",
            "waiting",
            true,
            Some("objective-supervisor:tool"),
            Some("remediation-current"),
            None,
            None,
            None,
        )
        .await;
        insert_objective(
            &pool,
            "recovered-fast",
            3,
            "completed",
            "complete",
            false,
            None,
            None,
            Some("evidence://fast"),
            Some("delivery"),
            Some(20_000_100),
        )
        .await;
        insert_objective(
            &pool,
            "recovered-slow",
            4,
            "active",
            "continue",
            false,
            Some("chat-foreground"),
            None,
            None,
            None,
            None,
        )
        .await;
        insert_objective(
            &pool,
            "recovered-old",
            3,
            "active",
            "continue",
            false,
            Some("chat-foreground"),
            None,
            None,
            None,
            None,
        )
        .await;
        insert_objective(
            &pool,
            "unresolved",
            2,
            "waiting_system",
            "failed_internal",
            false,
            Some("objective-supervisor:provider"),
            Some("remediation-unresolved"),
            None,
            None,
            None,
        )
        .await;
        insert_objective(
            &pool,
            "invalid-completion",
            2,
            "completed",
            "complete",
            false,
            None,
            None,
            None,
            Some("delivery"),
            Some(50_000_000),
        )
        .await;
        insert_objective(
            &pool,
            "cancelled",
            2,
            "cancelled",
            "cancelled",
            false,
            None,
            None,
            None,
            None,
            Some(50_000_000),
        )
        .await;
        insert_objective(
            &pool,
            "legacy",
            1,
            "legacy_orphan",
            "failed_internal",
            false,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        insert_decision(
            &pool,
            "decision-fast-start",
            "recovered-fast",
            2,
            "waiting",
            false,
            Some("objective-supervisor:delivery"),
            Some("remediation-fast"),
            json!({"decision_type":"waiting","status":"waiting_system","requires_user_action":false}),
            None,
            20_000_000,
        )
        .await;
        insert_decision(
            &pool,
            "decision-fast-complete",
            "recovered-fast",
            3,
            "complete",
            false,
            None,
            None,
            json!({
                "decision_type":"complete",
                "status":"completed",
                "requires_user_action":false,
                "reached_acceptance":"delivery"
            }),
            Some("evidence://fast"),
            20_000_100,
        )
        .await;
        sqlx::query(
            "INSERT INTO objective_evidence
             (objective_id, revision, evidence_ref) VALUES (?, ?, ?)",
        )
        .bind("recovered-fast")
        .bind(3_i64)
        .bind("evidence://fast")
        .execute(&pool)
        .await
        .unwrap();
        insert_decision(
            &pool,
            "decision-slow-start",
            "recovered-slow",
            3,
            "platform_incident",
            false,
            Some("objective-supervisor:chat"),
            Some("remediation-slow"),
            json!({"decision_type":"platform_incident","status":"waiting_system","requires_user_action":false}),
            None,
            30_000_000,
        )
        .await;
        insert_decision(
            &pool,
            "decision-old-start",
            "recovered-old",
            2,
            "failed_internal",
            false,
            Some("objective-supervisor:tool"),
            Some("remediation-old"),
            json!({"decision_type":"failed_internal","status":"waiting_system","requires_user_action":false}),
            None,
            10_000_000,
        )
        .await;
        insert_decision(
            &pool,
            "decision-unresolved",
            "unresolved",
            2,
            "failed_internal",
            false,
            Some("objective-supervisor:provider"),
            Some("remediation-unresolved"),
            json!({"decision_type":"failed_internal","status":"waiting_system","requires_user_action":false}),
            None,
            40_000_000,
        )
        .await;
        insert_decision(
            &pool,
            "historical-handoff",
            "active",
            2,
            "waiting",
            true,
            Some("objective-supervisor:chat"),
            Some("remediation-handoff"),
            json!({"decision_type":"waiting","status":"waiting_system","requires_user_action":true}),
            None,
            60_000_000,
        )
        .await;
        insert_decision(
            &pool,
            "envelope-only-handoff",
            "waiting-system",
            1,
            "waiting",
            false,
            Some("objective-supervisor:chat"),
            Some("remediation-envelope"),
            json!({"decision_type":"waiting","status":"waiting_system","requires_user_action":true}),
            None,
            70_000_000,
        )
        .await;
        insert_decision(
            &pool,
            "current-handoff-journal",
            "current-handoff",
            4,
            "waiting",
            true,
            Some("objective-supervisor:tool"),
            Some("remediation-current"),
            json!({"decision_type":"waiting","status":"waiting_system","requires_user_action":true}),
            None,
            80_000_000,
        )
        .await;
        insert_decision(
            &pool,
            "valid-core-input",
            "core-input",
            2,
            "core_input_required",
            true,
            None,
            None,
            json!({"decision_type":"core_input_required","status":"waiting_core_input","requires_user_action":true}),
            None,
            90_000_000,
        )
        .await;

        for (objective_id, revision, status, created_at) in [
            ("recovered-fast", 3, "completed", 20_000_100),
            ("recovered-slow", 4, "active", 30_000_300),
            ("recovered-old", 3, "active", 10_001_000),
        ] {
            sqlx::query(
                "INSERT INTO objective_events
                 (objective_id, revision, status, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(objective_id)
            .bind(revision)
            .bind(status)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::raw_sql(
            "INSERT INTO objective_remediations
             (status, next_observation_at, lease_owner, lease_expires_at) VALUES
             ('queued', 99999999, NULL, NULL),
             ('claimed', 99999999, 'expired-owner', 99999999),
             ('claimed', 99999999, 'live-owner', 100000001),
             ('waiting', 100000001, NULL, NULL),
             ('completed', 1, NULL, NULL);
             INSERT INTO side_effect_receipts
             (objective_id, action_fingerprint, status, observed_at) VALUES
             ('active', 'deliver:head-a', 'committed', 99999999),
             ('active', 'deliver:head-a', 'committed', 99999999),
             ('active', 'deliver:head-a', 'started', 99999999),
             ('active', 'deliver:head-b', 'committed', 99999999),
             ('waiting-system', 'deliver:head-a', 'committed', 99999999);",
        )
        .execute(&pool)
        .await
        .unwrap();

        let snapshot = query_objective_health(&pool, NOW_MS).await;
        let metrics = snapshot.metrics.expect("available metrics");

        assert_eq!(
            snapshot.availability,
            ObjectiveHealthAvailability::Available
        );
        assert_eq!(metrics.open, 9);
        assert_eq!(metrics.system_owned, 5);
        assert_eq!(metrics.typed_user_attention, 3);
        assert_eq!(metrics.technical_user_handoff_violations, 3);
        assert_eq!(metrics.overdue_ownerless_remediations, 2);
        assert_eq!(metrics.invalid_completions, 1);
        assert_eq!(metrics.invalid_completions_24h, 1);
        assert_eq!(metrics.duplicate_committed_side_effect_receipts, 1);
        assert_eq!(metrics.duplicate_committed_side_effect_receipts_24h, 1);
        assert_eq!(metrics.recovery_decisions, 4);
        assert_eq!(metrics.recovered_objectives, 3);
        assert_eq!(metrics.recovery_latency_p50_ms, Some(300));
        assert_eq!(metrics.recovery_latency_p95_ms, Some(1_000));
        assert_eq!(metrics.recovery_decisions_24h, 3);
        assert_eq!(metrics.recovered_objectives_24h, 2);
        assert_eq!(metrics.recovery_latency_p50_ms_24h, Some(100));
        assert_eq!(metrics.recovery_latency_p95_ms_24h, Some(300));
    }
}
