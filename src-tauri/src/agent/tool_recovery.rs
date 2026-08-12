// SPDX-License-Identifier: Apache-2.0
//! Durable observation and takeover settlement for native tool mutations.
//!
//! A provider tool call is not an execution receipt.  This module records a
//! privacy-preserving resource snapshot before dispatch and is the only place
//! that may turn a crash-left generic receipt into replay or replan authority.

use anyhow::{anyhow, bail, Result};
use codefactory_agent_loop::tool::MutationPermit;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use std::path::{Component, Path, PathBuf};

use super::objective::{ClaimedRemediation, ObjectiveStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolRecoveryDisposition {
    RetryExact,
    ReplanCurrentState,
    ResumeWithoutReceipt,
    ObserveOnly,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolRecoveryPlan {
    pub resource_kind: &'static str,
    pub replay_policy: &'static str,
    pub safe_locator_json: String,
    pub precondition_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotObservation {
    Unchanged,
    Changed,
    Unknown,
}

#[derive(Clone)]
pub(crate) struct ToolRecoveryStore {
    pool: SqlitePool,
}

impl ToolRecoveryStore {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn ensure_schema(pool: &SqlitePool) -> Result<()> {
        sqlx::raw_sql(include_str!(
            "../../migrations/0015_tool_recovery_contracts.sql"
        ))
        .execute(pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn prepare(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        cwd: &Path,
        session_id: Option<&str>,
        root_turn_id: Option<&str>,
    ) -> Result<Option<ToolRecoveryPlan>> {
        let plan = match tool_name {
            "write_pptx" | "write_docx" => {
                file_plan(cwd, args.get("path").and_then(serde_json::Value::as_str)).await?
            }
            "edit_pptx" | "format_pptx" => {
                let requested = args
                    .get("out_path")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| args.get("path").and_then(serde_json::Value::as_str));
                file_plan(cwd, requested).await?
            }
            "edit_xlsx" => {
                file_plan(cwd, args.get("path").and_then(serde_json::Value::as_str)).await?
            }
            "skill_create" | "skill_update" | "skill_delete" | "skill_fetch" => {
                Some(ToolRecoveryPlan {
                    resource_kind: "user_skills",
                    replay_policy: "exact_if_unchanged",
                    safe_locator_json: "{}".into(),
                    precondition_digest: user_skills_digest()?,
                })
            }
            "update_plan" => {
                let (Some(session_id), Some(root_turn_id)) = (session_id, root_turn_id) else {
                    return Ok(None);
                };
                Some(ToolRecoveryPlan {
                    resource_kind: "session_plan",
                    replay_policy: "exact_if_unchanged",
                    safe_locator_json: serde_json::json!({
                        "session_id": session_id,
                        "root_turn_id": root_turn_id,
                    })
                    .to_string(),
                    precondition_digest: session_plan_digest(&self.pool, session_id, root_turn_id)
                        .await?,
                })
            }
            "delegate_tasks" | "dispatch_parallel_tasks" => {
                let Some(session_id) = session_id else {
                    return Ok(None);
                };
                Some(ToolRecoveryPlan {
                    resource_kind: "session_tasks",
                    replay_policy: "exact_if_unchanged",
                    safe_locator_json: serde_json::json!({"session_id": session_id}).to_string(),
                    precondition_digest: session_tasks_digest(&self.pool, session_id).await?,
                })
            }
            "bash" => {
                let command = args
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if !observable_workspace_command(command) {
                    None
                } else {
                    Some(ToolRecoveryPlan {
                        resource_kind: "workspace_git",
                        replay_policy: "exact_if_unchanged",
                        safe_locator_json: "{}".into(),
                        precondition_digest: workspace_git_digest(cwd).await?,
                    })
                }
            }
            // Every future native mutation fails closed until it names a
            // bounded observer. A successful provider return is not enough.
            _ => None,
        };
        Ok(plan)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_contract_in_tx(
        tx: &mut Transaction<'_, Sqlite>,
        receipt_id: &str,
        objective_id: &str,
        objective_revision: i64,
        binding_id: &str,
        resource_generation: i64,
        action_fingerprint: &str,
        tool_call_id: &str,
        plan: ToolRecoveryPlan,
        dispatch_owner: Option<&str>,
        dispatch_claim_epoch: i64,
        now: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO tool_recovery_contracts
             (receipt_id, objective_id, objective_revision, binding_id,
              resource_generation, action_fingerprint, tool_call_id,
              resource_kind, replay_policy, safe_locator_json,
              precondition_digest, state, dispatch_owner,
              dispatch_claim_epoch, dispatch_generation, dispatch_started_at,
              created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'dispatching', ?, ?, 1, ?, ?, ?)",
        )
        .bind(receipt_id)
        .bind(objective_id)
        .bind(objective_revision)
        .bind(binding_id)
        .bind(resource_generation)
        .bind(action_fingerprint)
        .bind(tool_call_id)
        .bind(plan.resource_kind)
        .bind(plan.replay_policy)
        .bind(plan.safe_locator_json)
        .bind(plan.precondition_digest)
        .bind(dispatch_owner)
        .bind(dispatch_claim_epoch.max(0))
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub(crate) async fn settle_foreground(
        &self,
        receipt_id: &str,
        cwd: &Path,
        succeeded: bool,
    ) -> Result<bool> {
        let row = sqlx::query(
            "SELECT resource_kind, safe_locator_json, precondition_digest
             FROM tool_recovery_contracts WHERE receipt_id=?",
        )
        .bind(receipt_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let kind: String = row.try_get("resource_kind")?;
        let locator: String = row.try_get("safe_locator_json")?;
        let pre: String = row.try_get("precondition_digest")?;
        let current = snapshot_digest(&self.pool, cwd, &kind, &locator).await?;
        let now = chrono::Utc::now().timestamp_millis();
        let mut tx = self.pool.begin().await?;
        if succeeded {
            let current = current.ok_or_else(|| anyhow!("post-dispatch resource is unreadable"))?;
            let updated = sqlx::query(
                "UPDATE tool_recovery_contracts
                 SET state='settled_committed', postcondition_digest=?,
                     observation_count=observation_count+1, observed_at=?, settled_at=?, updated_at=?
                 WHERE receipt_id=? AND state IN ('dispatching','unknown')",
            )
            .bind(current)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(receipt_id)
            .execute(&mut *tx)
            .await?;
            let recovered: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM tool_recovery_reconciliations WHERE receipt_id=?",
            )
            .bind(receipt_id)
            .fetch_one(&mut *tx)
            .await?;
            if recovered > 0 {
                terminalize_linked_calls(
                    &mut tx,
                    receipt_id,
                    "done",
                    "系统已确认安全重试完成；旧工具调用已收敛，不会再次执行。",
                    now,
                )
                .await?;
            }
            if updated.rows_affected() != 1 {
                bail!("tool recovery contract changed before foreground settlement");
            }
            sqlx::query(
                "UPDATE side_effect_receipts SET status='committed',
                   summary_json=?, observed_at=?
                 WHERE id=? AND status IN ('started','unknown')",
            )
            .bind(serde_json::json!({"status": "done"}).to_string())
            .bind(now)
            .bind(receipt_id)
            .execute(&mut *tx)
            .await?;
        } else {
            let observed = current.as_deref();
            let state = if observed == Some(pre.as_str()) {
                "observed_unchanged"
            } else if observed.is_some() {
                "observed_changed"
            } else {
                "still_unknown"
            };
            sqlx::query(
                "UPDATE tool_recovery_contracts SET state=?, observation_count=observation_count+1,
                   observed_at=?, updated_at=? WHERE receipt_id=? AND state IN ('dispatching','unknown')",
            )
            .bind(state)
            .bind(now)
            .bind(now)
            .bind(receipt_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE side_effect_receipts SET status='unknown', observed_at=?
                 WHERE id=? AND status IN ('started','unknown')",
            )
            .bind(now)
            .bind(receipt_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub(crate) async fn reconcile_claimed(
        &self,
        claim: &ClaimedRemediation,
        permit: &MutationPermit,
    ) -> Result<ToolRecoveryDisposition> {
        if !ObjectiveStore::new(self.pool.clone())
            .claim_is_current(permit)
            .await?
        {
            bail!("Tool recovery claim is no longer current");
        }
        if let Some(row) = sqlx::query(
            "SELECT reconciliation.disposition, reconciliation.claim_epoch, receipt.status
             FROM tool_recovery_reconciliations reconciliation
             JOIN side_effect_receipts receipt ON receipt.id=reconciliation.receipt_id
             WHERE reconciliation.remediation_id=?",
        )
        .bind(&claim.remediation_id)
        .fetch_optional(&self.pool)
        .await?
        {
            let disposition: String = row.try_get("disposition")?;
            let decision_epoch: i64 = row.try_get("claim_epoch")?;
            let receipt_status: String = row.try_get("status")?;
            if decision_epoch == claim.claim_epoch {
                return match disposition.as_str() {
                    "retry_exact" => Ok(ToolRecoveryDisposition::RetryExact),
                    "replan_current_state" => Ok(ToolRecoveryDisposition::ReplanCurrentState),
                    _ => bail!("invalid durable Tool reconciliation disposition"),
                };
            }
            if matches!(receipt_status.as_str(), "committed" | "reconciled") {
                self.adopt_terminal_reconciliation(claim, permit).await?;
                return Ok(ToolRecoveryDisposition::ReplanCurrentState);
            }
        }
        let Some(binding_id) = claim.binding_id.as_deref() else {
            return Ok(ToolRecoveryDisposition::ObserveOnly);
        };
        let rows = sqlx::query(
            "SELECT receipt.id, receipt.status,
                    generic.resource_kind, generic.safe_locator_json,
                    generic.precondition_digest,
                    file.safe_locator_json AS file_locator,
                    file.precondition_digest AS file_precondition,
                    file.expected_postcondition_digest AS file_expected
             FROM side_effect_receipts receipt
             LEFT JOIN tool_recovery_contracts generic ON generic.receipt_id=receipt.id
             LEFT JOIN side_effect_observation_contracts file ON file.receipt_id=receipt.id
             WHERE receipt.objective_id=? AND receipt.binding_id=?
               AND receipt.status IN ('started','unknown')",
        )
        .bind(&claim.objective.id)
        .bind(binding_id)
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            return Ok(match claim.failure_code.as_str() {
                "tool_observation_contract_missing" | "mutation_permit_lost" => {
                    ToolRecoveryDisposition::ResumeWithoutReceipt
                }
                _ => ToolRecoveryDisposition::ObserveOnly,
            });
        }
        if rows.len() != 1 {
            return Ok(ToolRecoveryDisposition::ObserveOnly);
        }
        let row = &rows[0];
        let receipt_id: String = row.try_get("id")?;
        let cwd = working_directory_for_claim(&self.pool, claim).await?;
        let observation = if let Ok(kind) = row.try_get::<String, _>("resource_kind") {
            let locator: String = row.try_get("safe_locator_json")?;
            let pre: String = row.try_get("precondition_digest")?;
            match snapshot_digest(&self.pool, &cwd, &kind, &locator).await? {
                Some(current) if current == pre => SnapshotObservation::Unchanged,
                Some(_) => SnapshotObservation::Changed,
                None => SnapshotObservation::Unknown,
            }
        } else if let Ok(locator) = row.try_get::<String, _>("file_locator") {
            let pre: String = row.try_get("file_precondition")?;
            let expected: String = row.try_get("file_expected")?;
            match observe_exact_file(&cwd, &locator, &pre, &expected).await {
                Some(current) if current == expected => SnapshotObservation::Changed,
                Some(current) if current == pre => SnapshotObservation::Unchanged,
                Some(_) => SnapshotObservation::Changed,
                None => SnapshotObservation::Unknown,
            }
        } else {
            return Ok(ToolRecoveryDisposition::ObserveOnly);
        };
        match observation {
            SnapshotObservation::Unknown => Ok(ToolRecoveryDisposition::ObserveOnly),
            SnapshotObservation::Unchanged => {
                self.mark_retryable(&receipt_id, claim, permit).await?;
                Ok(ToolRecoveryDisposition::RetryExact)
            }
            SnapshotObservation::Changed => {
                self.settle_reconciled(&receipt_id, claim, permit).await?;
                Ok(ToolRecoveryDisposition::ReplanCurrentState)
            }
        }
    }

    async fn mark_retryable(
        &self,
        receipt_id: &str,
        claim: &ClaimedRemediation,
        permit: &MutationPermit,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut tx = self.pool.begin().await?;
        require_claim_in_tx(&mut tx, claim, permit, now).await?;
        let generic = sqlx::query(
            "UPDATE tool_recovery_contracts SET state='observed_unchanged',
               dispatch_owner=?, dispatch_claim_epoch=?, observation_count=observation_count+1,
               observed_at=?, updated_at=?
             WHERE receipt_id=? AND dispatch_claim_epoch<?
               AND state IN ('dispatching','unknown','observed_unchanged')",
        )
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .bind(now)
        .bind(receipt_id)
        .bind(permit.claim_epoch)
        .execute(&mut *tx)
        .await?;
        let file = if generic.rows_affected() == 0 {
            sqlx::query(
                "UPDATE side_effect_observation_contracts
                 SET state='definitely_not_applied', last_dispatch_epoch=?,
                     observation_count=observation_count+1, observed_at=?
                 WHERE receipt_id=? AND last_dispatch_epoch<?",
            )
            .bind(permit.claim_epoch)
            .bind(now)
            .bind(receipt_id)
            .bind(permit.claim_epoch)
            .execute(&mut *tx)
            .await?
            .rows_affected()
        } else {
            0
        };
        if generic.rows_affected() + file != 1 {
            bail!("Tool retry authority was already consumed or changed");
        }
        terminalize_linked_calls(
            &mut tx,
            receipt_id,
            "waiting",
            "系统确认此前工具副作用未发生；正在以同一目标和新租约安全重试。",
            now,
        )
        .await?;
        sqlx::query(
            "INSERT INTO tool_recovery_reconciliations
             (receipt_id, remediation_id, claim_epoch, disposition, created_at)
             VALUES (?, ?, ?, 'retry_exact', ?)
             ON CONFLICT(remediation_id) DO UPDATE SET
               receipt_id=excluded.receipt_id, claim_epoch=excluded.claim_epoch,
               disposition=excluded.disposition, created_at=excluded.created_at",
        )
        .bind(receipt_id)
        .bind(&claim.remediation_id)
        .bind(claim.claim_epoch)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn settle_reconciled(
        &self,
        receipt_id: &str,
        claim: &ClaimedRemediation,
        permit: &MutationPermit,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut tx = self.pool.begin().await?;
        require_claim_in_tx(&mut tx, claim, permit, now).await?;
        let generic = sqlx::query(
            "UPDATE tool_recovery_contracts SET state='settled_reconciled',
               observation_count=observation_count+1, observed_at=?, settled_at=?, updated_at=?
             WHERE receipt_id=? AND state NOT IN ('settled_committed','settled_reconciled','cancelled')",
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(receipt_id)
        .execute(&mut *tx)
        .await?;
        if generic.rows_affected() == 0 {
            sqlx::query(
                "UPDATE side_effect_observation_contracts SET state='conflict',
                   observation_count=observation_count+1, observed_at=? WHERE receipt_id=?",
            )
            .bind(now)
            .bind(receipt_id)
            .execute(&mut *tx)
            .await?;
        }
        terminalize_linked_calls(
            &mut tx,
            receipt_id,
            "done",
            "系统已对账：资源状态已经变化，旧操作不会重放；正在读取当前状态重新规划。",
            now,
        )
        .await?;
        sqlx::query(
            "UPDATE side_effect_receipts SET status='reconciled',
               summary_json=?,
               observed_at=? WHERE id=? AND status IN ('started','unknown')",
        )
        .bind(
            serde_json::json!({
                "status": "done",
                "recovery": "replan_current_state",
            })
            .to_string(),
        )
        .bind(now)
        .bind(receipt_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO tool_recovery_reconciliations
             (receipt_id, remediation_id, claim_epoch, disposition, created_at)
             VALUES (?, ?, ?, 'replan_current_state', ?)
             ON CONFLICT(remediation_id) DO UPDATE SET
               receipt_id=excluded.receipt_id, claim_epoch=excluded.claim_epoch,
               disposition=excluded.disposition, created_at=excluded.created_at",
        )
        .bind(receipt_id)
        .bind(&claim.remediation_id)
        .bind(claim.claim_epoch)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn adopt_terminal_reconciliation(
        &self,
        claim: &ClaimedRemediation,
        permit: &MutationPermit,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut tx = self.pool.begin().await?;
        require_claim_in_tx(&mut tx, claim, permit, now).await?;
        let receipt_id: String = sqlx::query_scalar(
            "SELECT receipt_id FROM tool_recovery_reconciliations WHERE remediation_id=?",
        )
        .bind(&claim.remediation_id)
        .fetch_one(&mut *tx)
        .await?;
        terminalize_linked_calls(
            &mut tx,
            &receipt_id,
            "done",
            "系统已确认此前工具操作完成；不会重放，正在读取当前状态继续。",
            now,
        )
        .await?;
        sqlx::query(
            "UPDATE tool_recovery_reconciliations
             SET claim_epoch=?, disposition='replan_current_state', created_at=?
             WHERE remediation_id=?",
        )
        .bind(claim.claim_epoch)
        .bind(now)
        .bind(&claim.remediation_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn file_plan(cwd: &Path, requested: Option<&str>) -> Result<Option<ToolRecoveryPlan>> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let workspace = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let path = crate::tools::workspace_path::resolve_writable(&workspace, requested)
        .map_err(|error| anyhow!(error.message()))?;
    let relative = path
        .strip_prefix(&workspace)
        .ok()
        .filter(|path| safe_relative(path))
        .and_then(Path::to_str)
        .map(|path| path.replace('\\', "/"));
    let Some(relative) = relative else {
        return Ok(None);
    };
    let precondition_digest = file_digest(&path).await?;
    Ok(Some(ToolRecoveryPlan {
        resource_kind: "workspace_file",
        replay_policy: "exact_if_unchanged",
        safe_locator_json: serde_json::json!({"workspace_relative_path": relative}).to_string(),
        precondition_digest,
    }))
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn digest(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        let bytes = part.as_ref();
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    format!("sha256:{:x}", hasher.finalize())
}

async fn file_digest(path: &Path) -> Result<String> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(digest([b"file".as_slice(), bytes.as_slice()])),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(digest([b"file_absent".as_slice()]))
        }
        Err(error) => Err(error.into()),
    }
}

fn user_skills_digest() -> Result<String> {
    let root = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodeFactory")
        .join("skills");
    tree_digest(&root)
}

fn tree_digest(root: &Path) -> Result<String> {
    if !root.exists() {
        return Ok(digest([b"tree_absent".as_slice()]));
    }
    let mut files = Vec::new();
    fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
        let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                walk(root, &path, files)?;
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((relative, std::fs::read(path)?));
            }
        }
        Ok(())
    }
    walk(root, root, &mut files)?;
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

async fn session_plan_digest(pool: &SqlitePool, session: &str, root: &str) -> Result<String> {
    let rows = sqlx::query(
        "SELECT revision, plan_json, COALESCE(explanation,''), COALESCE(waiting_reason,''),
                COALESCE(next_action_owner,''), COALESCE(change_reason,'')
         FROM chat_plan_events WHERE session_id=? AND root_turn_id=? ORDER BY revision",
    )
    .bind(session)
    .bind(root)
    .fetch_all(pool)
    .await?;
    Ok(digest(rows.iter().map(|row| {
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}",
            row.get::<i64, _>(0),
            row.get::<String, _>(1),
            row.get::<String, _>(2),
            row.get::<String, _>(3),
            row.get::<String, _>(4),
            row.get::<String, _>(5)
        )
    })))
}

async fn session_tasks_digest(pool: &SqlitePool, session: &str) -> Result<String> {
    let rows = sqlx::query(
        "SELECT id, title, description, status, cwd, COALESCE(parent_task_id,''), attempt_count
         FROM task_runs WHERE session_id=? ORDER BY id",
    )
    .bind(session)
    .fetch_all(pool)
    .await?;
    Ok(digest(rows.iter().map(|row| {
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{}",
            row.get::<String, _>(0),
            row.get::<String, _>(1),
            row.get::<String, _>(2),
            row.get::<String, _>(3),
            row.get::<String, _>(4),
            row.get::<String, _>(5),
            row.get::<i64, _>(6)
        )
    })))
}

async fn snapshot_digest(
    pool: &SqlitePool,
    cwd: &Path,
    kind: &str,
    locator: &str,
) -> Result<Option<String>> {
    let locator: serde_json::Value = serde_json::from_str(locator)?;
    let value = match kind {
        "workspace_file" => {
            let Some(relative) = locator
                .get("workspace_relative_path")
                .and_then(|v| v.as_str())
            else {
                return Ok(None);
            };
            if !safe_relative(Path::new(relative)) {
                return Ok(None);
            }
            let path = crate::tools::workspace_path::resolve_writable(cwd, relative)
                .map_err(|error| anyhow!(error.message()))?;
            file_digest(&path).await?
        }
        "user_skills" => user_skills_digest()?,
        "workspace_git" => workspace_git_digest(cwd).await?,
        "session_plan" => {
            let (Some(session), Some(root)) = (
                locator.get("session_id").and_then(|v| v.as_str()),
                locator.get("root_turn_id").and_then(|v| v.as_str()),
            ) else {
                return Ok(None);
            };
            session_plan_digest(pool, session, root).await?
        }
        "session_tasks" => {
            let Some(session) = locator.get("session_id").and_then(|v| v.as_str()) else {
                return Ok(None);
            };
            session_tasks_digest(pool, session).await?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn observable_workspace_command(command: &str) -> bool {
    let lower = command.trim_start().to_ascii_lowercase();
    if lower.is_empty()
        || lower.contains("nohup ")
        || lower.contains("start-process ")
        || lower.trim_end().ends_with('&')
        || [
            "curl ",
            "wget ",
            "kubectl ",
            "helm ",
            "ssh ",
            "scp ",
            "rsync ",
            "git push",
            "git tag",
            "gh pr create",
            "gh pr merge",
            "gh workflow run",
            "gh release ",
            "npm publish",
            "pnpm publish",
            "cargo publish",
            "docker push",
            "podman push",
            "vercel ",
            "netlify ",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return false;
    }
    [
        "apply_patch",
        "sed -i",
        "perl -pi",
        "tee ",
        "cat >",
        "cat >>",
        "touch ",
        "mkdir ",
        "rm ",
        "mv ",
        "cp ",
        "install ",
        "git add",
        "git commit",
        "git apply",
        "git branch",
        "git switch",
        "git checkout",
        "git fetch",
        "git pull",
        "git stash",
        "cargo fmt",
        "rustfmt ",
        "prettier ",
        "eslint ",
        "biome ",
        ">",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

async fn workspace_git_digest(cwd: &Path) -> Result<String> {
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String> {
        let root = std::process::Command::new("git")
            .args([
                "-C",
                cwd.to_string_lossy().as_ref(),
                "rev-parse",
                "--show-toplevel",
            ])
            .output()?;
        if !root.status.success() {
            return tree_digest(&cwd);
        }
        let root = PathBuf::from(String::from_utf8(root.stdout)?.trim());
        let listed = std::process::Command::new("git")
            .args([
                "-C",
                root.to_string_lossy().as_ref(),
                "ls-files",
                "-co",
                "--exclude-standard",
                "-z",
            ])
            .output()?;
        if !listed.status.success() {
            bail!("unable to enumerate observable workspace files");
        }
        let head = std::process::Command::new("git")
            .args([
                "-C",
                root.to_string_lossy().as_ref(),
                "rev-parse",
                "--verify",
                "HEAD",
            ])
            .output()?;
        let mut hasher = Sha256::new();
        hasher.update(b"workspace_git_v1\0");
        if head.status.success() {
            hasher.update(head.stdout);
        }
        let index = std::process::Command::new("git")
            .args([
                "-C",
                root.to_string_lossy().as_ref(),
                "diff",
                "--cached",
                "--binary",
                "--no-ext-diff",
            ])
            .output()?;
        if !index.status.success() {
            bail!("unable to observe the Git index");
        }
        hasher.update((index.stdout.len() as u64).to_le_bytes());
        hasher.update(index.stdout);
        for raw in listed
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let relative = std::str::from_utf8(raw)?;
            if !safe_relative(Path::new(relative)) {
                bail!("Git returned an unsafe workspace path");
            }
            hasher.update((raw.len() as u64).to_le_bytes());
            hasher.update(raw);
            match std::fs::read(root.join(relative)) {
                Ok(bytes) => {
                    hasher.update((bytes.len() as u64).to_le_bytes());
                    hasher.update(bytes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    hasher.update(u64::MAX.to_le_bytes());
                    hasher.update(b"tracked_path_absent");
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(format!("sha256:{:x}", hasher.finalize()))
    })
    .await?
}

async fn observe_exact_file(
    cwd: &Path,
    locator: &str,
    _pre: &str,
    _expected: &str,
) -> Option<String> {
    let locator: serde_json::Value = serde_json::from_str(locator).ok()?;
    let relative = locator.get("workspace_relative_path")?.as_str()?;
    if !safe_relative(Path::new(relative)) {
        return None;
    }
    let path = crate::tools::workspace_path::resolve_writable(cwd, relative).ok()?;
    match tokio::fs::read(path).await {
        Ok(bytes) => Some(format!("sha256:{:x}", Sha256::digest(bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(super_absent_digest()),
        Err(_) => None,
    }
}

fn super_absent_digest() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"file_content_sha256_v1");
    hasher.update([0]);
    hasher.update(b"absent");
    hasher.update([0]);
    format!("sha256:{:x}", hasher.finalize())
}

async fn working_directory_for_claim(
    pool: &SqlitePool,
    claim: &ClaimedRemediation,
) -> Result<PathBuf> {
    let binding_id = claim
        .binding_id
        .as_deref()
        .ok_or_else(|| anyhow!("Tool claim binding missing"))?;
    let row = sqlx::query(
        "SELECT binding.resource_kind, binding.resource_id,
                CASE WHEN binding.resource_kind='task_run' THEN task.cwd ELSE session.cwd END AS cwd
         FROM objective_bindings binding
         LEFT JOIN task_runs task ON binding.resource_kind='task_run' AND task.id=binding.resource_id
         LEFT JOIN sessions session ON binding.resource_kind='chat_root_turn'
              AND session.id=(SELECT session_id FROM messages WHERE id=binding.resource_id)
         WHERE binding.id=? AND binding.objective_id=? AND binding.resource_generation=?",
    )
    .bind(binding_id)
    .bind(&claim.objective.id)
    .bind(claim.resource_generation)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| anyhow!("Tool claim binding changed"))?;
    let cwd: Option<String> = row.try_get("cwd")?;
    cwd.map(PathBuf::from)
        .ok_or_else(|| anyhow!("Tool recovery working directory missing"))
}

async fn require_claim_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    claim: &ClaimedRemediation,
    permit: &MutationPermit,
    now: i64,
) -> Result<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objective_remediations remediation
         JOIN objectives objective ON objective.id=remediation.objective_id
              AND objective.remediation_id=remediation.id
         JOIN objective_bindings binding ON binding.id=remediation.binding_id
              AND binding.objective_id=objective.id
         WHERE remediation.id=? AND remediation.objective_id=?
           AND remediation.status='claimed' AND remediation.lease_owner=?
           AND remediation.attempt_index=? AND remediation.lease_expires_at>?
           AND objective.status='waiting_system' AND objective.lease_owner=?
           AND objective.lease_expires_at>? AND binding.id=?
           AND binding.resource_generation=?",
    )
    .bind(&permit.remediation_id)
    .bind(&permit.objective_id)
    .bind(&permit.owner)
    .bind(permit.claim_epoch)
    .bind(now)
    .bind(&permit.owner)
    .bind(now)
    .bind(&permit.binding_id)
    .bind(permit.resource_generation)
    .fetch_one(&mut **tx)
    .await?;
    if count != 1 || permit.objective_id != claim.objective.id {
        bail!("Tool recovery mutation permit is stale");
    }
    Ok(())
}

async fn terminalize_linked_calls(
    tx: &mut Transaction<'_, Sqlite>,
    receipt_id: &str,
    status: &str,
    result: &str,
    now: i64,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT tool.id, message.session_id
         FROM tool_recovery_call_links link
         JOIN tool_calls tool ON tool.id=link.tool_call_id
         JOIN messages message ON message.id=tool.message_id
         WHERE link.receipt_id=?",
    )
    .bind(receipt_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let trace_id: String = row.try_get("id")?;
        let session_id: String = row.try_get("session_id")?;
        let provider_id = trace_id
            .strip_prefix(&format!("{session_id}:"))
            .ok_or_else(|| anyhow!("normalized Tool call identity is malformed"))?;
        sqlx::query(
            "UPDATE tool_calls SET status=?, result=?, error=NULL,
             duration_ms=COALESCE(duration_ms,0)
             WHERE id=? AND status IN ('pending','waiting')",
        )
        .bind(status)
        .bind(result)
        .bind(&trace_id)
        .execute(&mut **tx)
        .await?;
        let content = serde_json::json!({
            "tool_call_id": provider_id,
            "content": result,
            "status": status,
        })
        .to_string();
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES (?, ?, 'tool', ?, ?)
             ON CONFLICT(id) DO UPDATE SET session_id=excluded.session_id,
             role='tool', content=excluded.content",
        )
        .bind(format!("{trace_id}:result"))
        .bind(session_id)
        .bind(content)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
