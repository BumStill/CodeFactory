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
use crate::util::no_window::NoWindow;

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
                let named_file =
                    exact_local_append_target(command).or_else(|| fetch_output_target(command));
                if let Some(relative) = named_file {
                    file_plan(cwd, Some(&relative)).await?
                } else if escapes_workspace_observation(command) {
                    None
                } else {
                    // The workspace digest answers the only question recovery
                    // asks — did anything change? — for `git commit` and for a
                    // build script alike. Which binary runs is a permission
                    // decision the user already made; it is not what makes an
                    // effect observable, and treating an allowlist as if it
                    // were left `npm install`, `pip install` and every project
                    // script with no observer that could ever be built.
                    Some(ToolRecoveryPlan {
                        resource_kind: "workspace_git",
                        replay_policy: "never_after_dispatch",
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
                    generic.precondition_digest, generic.replay_policy,
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
        let mut generic_replay_policy = None;
        let observation = if let Ok(kind) = row.try_get::<String, _>("resource_kind") {
            let locator: String = row.try_get("safe_locator_json")?;
            let pre: String = row.try_get("precondition_digest")?;
            generic_replay_policy = row.try_get::<String, _>("replay_policy").ok();
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
                if generic_replay_policy.as_deref() == Some("never_after_dispatch") {
                    return Ok(ToolRecoveryDisposition::ObserveOnly);
                }
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
    let raw = path.to_string_lossy();
    let windows_absolute = raw.starts_with("\\\\")
        || raw.starts_with("//")
        || raw
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    !raw.is_empty()
        && !windows_absolute
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

fn exact_local_append_target(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.is_empty()
        || trimmed.contains(['\n', '\r', ';', '|', '&', '`'])
        || trimmed.contains("$(")
        || trimmed.contains("${")
        || lower.contains("$env:")
        || lower.contains("%userprofile%")
    {
        return None;
    }
    let candidate = if lower.starts_with("add-content ") {
        let words = trimmed.split_whitespace().collect::<Vec<_>>();
        if words.len() != 5
            || !words[1].eq_ignore_ascii_case("-path")
            || !words[3].eq_ignore_ascii_case("-value")
            || words[4].contains(['(', ')', '{', '}', '[', ']'])
        {
            return None;
        }
        words[2].trim_matches(['\'', '"']).to_owned()
    } else if lower.starts_with("printf ") || lower.starts_with("echo ") {
        let (prefix, target) = trimmed.rsplit_once(">>")?;
        if prefix.contains(['<', '>']) || target.contains(['<', '>']) {
            return None;
        }
        target.trim().trim_matches(['\'', '"']).to_owned()
    } else {
        return None;
    };
    safe_relative(Path::new(&candidate)).then_some(candidate)
}

/// A file mutator aimed outside the workspace.
///
/// `rm /tmp/keep` and `Add-Content -Path C:\Temp\effect.log` do land on this
/// machine, but nowhere the workspace digest reads — admitting them would
/// attach an observer that could never see the effect, and an unobservable
/// delete is precisely what the receipt gate exists to hold.
///
/// Scoped to verbs whose arguments *are* paths. A build script that happens to
/// mention `/usr/lib` in a flag is not this, and widening the check to every
/// command containing a slash is what made the old allowlist reject most of the
/// shell.
fn file_mutator_leaves_the_workspace(lower: &str) -> bool {
    let first = lower.split_whitespace().next().unwrap_or_default();
    if !matches!(
        first,
        "rm" | "rmdir"
            | "mv"
            | "cp"
            | "touch"
            | "mkdir"
            | "ln"
            | "add-content"
            | "set-content"
            | "out-file"
            | "remove-item"
            | "move-item"
            | "copy-item"
            | "new-item"
    ) {
        return false;
    }
    lower.contains("../")
        || lower.contains("..\\")
        || lower.contains("%userprofile%")
        || lower.contains("%temp%")
        || lower.split_whitespace().any(|word| {
            let word = word.trim_matches(['\'', '"']);
            (word.starts_with('/') && !word.starts_with("--"))
                || word.starts_with("~/")
                || word.starts_with("~\\")
                || word.starts_with("\\\\")
                || word
                    .as_bytes()
                    .get(1)
                    .is_some_and(|separator| *separator == b':')
        })
}

/// Effects the workspace digest cannot speak for.
///
/// Two families, and only two. A backgrounded command outlives the observation
/// entirely, so its completion is unknowable no matter what it does. Everything
/// else here writes somewhere this machine cannot read back: a remote branch, a
/// registry, a cluster, another host, or an HTTP request carrying a body.
///
/// The list used to be the inverse — an allowlist of a dozen local verbs, with
/// everything unrecognized fenced. That is the wrong polarity for a shell: the
/// long tail of legitimate local work (`npm install`, `pip install`, a project's
/// own build script) is unbounded, and each one settled `Waiting` on an
/// observation contract that no amount of replanning could produce. Fencing an
/// effect is only honest when the effect is genuinely unobservable, not when the
/// verb is merely unfamiliar — which binary may run is what the permission mode
/// decides.
fn escapes_workspace_observation(command: &str) -> bool {
    let trimmed = command.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    if lower.trim().is_empty() {
        return true;
    }
    // `&&` sequences; a lone trailing `&` forks.
    if lower.ends_with('&') && !lower.ends_with("&&") {
        return true;
    }
    if ["nohup ", "start-process ", "start-job "]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return true;
    }
    // A request with a method or a body changes state on the far side, which no
    // local file can attest to. A plain fetch is a read and stays admissible.
    if super::tool_backend::bash_has_explicit_external_mutation(command) {
        return true;
    }
    if file_mutator_leaves_the_workspace(&lower) {
        return true;
    }
    [
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
}

/// Whether a shell metacharacter appears outside quotes.
///
/// `curl "https://host/path?a=1&b=2" -o out` is one foreground command; a bare
/// `&` in the same position forks it. Only the unquoted one changes what runs,
/// and a query string is far too common to read as backgrounding.
fn contains_unquoted(command: &str, needle: char) -> bool {
    let mut quote: Option<char> = None;
    for character in command.chars() {
        match (quote, character) {
            (Some(open), current) if current == open => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(character),
            (None, current) if current == needle => return true,
            (None, _) => {}
        }
    }
    false
}

/// The local file a download names, when it names exactly one.
///
/// A download reads like an unobservable network call, but its entire purpose is
/// a file on disk — and that file is observable by the same `workspace_file`
/// contract `write_file` uses. Without this, `curl` sat on a deny list, no
/// observer could ever be built for it, and "fetch the installer" was not a slow
/// path but an impossible one: every attempt settled `Waiting` and the only
/// suggested alternative — a dedicated observable tool — does not exist for HTTP.
///
/// Deliberately strict about shape. One segment, no substitution, no
/// backgrounding, and a workspace-relative destination; anything more elaborate
/// falls through to the workspace digest rather than claiming a precision it
/// does not have.
fn fetch_output_target(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty()
        || trimmed.contains(['\n', '\r', ';', '|', '`'])
        || contains_unquoted(trimmed, '&')
        || trimmed.contains("$(")
        || trimmed.contains("${")
    {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let client = lower.split_whitespace().next().unwrap_or_default().to_owned();
    if !matches!(client.as_str(), "curl" | "wget" | "invoke-webrequest" | "iwr") {
        return None;
    }
    if escapes_workspace_observation(trimmed) {
        return None;
    }

    let candidate = if let Some((before, after)) = trimmed.split_once('>') {
        // `curl URL > out`. A second redirect or an input redirect means the
        // destination is no longer a single obvious file.
        if after.contains('>') || before.contains('<') || after.trim().is_empty() {
            return None;
        }
        after.trim().trim_matches(['\'', '"']).to_owned()
    } else {
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let mut found: Option<String> = None;
        for (index, word) in words.iter().enumerate() {
            if let Some((name, value)) = word.split_once('=') {
                if matches!(
                    name.trim_start_matches('-').to_ascii_lowercase().as_str(),
                    "output" | "output-document" | "outfile"
                ) && !value.is_empty()
                {
                    found = Some(value.trim_matches(['\'', '"']).to_owned());
                    break;
                }
            }
            // `-O` means "keep the remote name" to curl and "write this path"
            // to wget. Reading it as a path for curl would invent a file the
            // command never writes.
            let takes_path = match client.as_str() {
                "curl" => *word == "-o" || word.eq_ignore_ascii_case("--output"),
                "wget" => {
                    *word == "-O" || word.eq_ignore_ascii_case("--output-document")
                }
                _ => word.eq_ignore_ascii_case("-outfile"),
            };
            if takes_path {
                found = words
                    .get(index + 1)
                    .map(|path| path.trim_matches(['\'', '"']).to_owned());
                break;
            }
        }
        found?
    };

    if candidate.is_empty() {
        return None;
    }
    safe_relative(Path::new(&candidate)).then_some(candidate)
}

async fn workspace_git_digest(cwd: &Path) -> Result<String> {
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String> {
        let root = std::process::Command::new("git")
            .no_window()
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
            .no_window()
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
            .no_window()
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
            .no_window()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A download's destination is the whole point of the command, and a file
    /// digest observes it exactly. Before this, `curl` was on a deny list with
    /// no observer of any kind, so "fetch the installer" could not be attempted
    /// at all — the report that started this fix showed both `curl` and
    /// `Invoke-WebRequest` refused for a plain GET.
    #[test]
    fn a_download_is_observed_by_the_file_it_writes() {
        for (command, expected) in [
            ("curl -fsSL https://example.invalid/install.sh -o install.sh", "install.sh"),
            ("curl --output tools/cli.tar.gz https://example.invalid/cli.tar.gz", "tools/cli.tar.gz"),
            ("curl -fsSL https://example.invalid/install.sh > install.sh", "install.sh"),
            ("wget -O vendor/agentcenter.zip https://example.invalid/a.zip", "vendor/agentcenter.zip"),
            ("wget --output-document=vendor/a.zip https://example.invalid/a.zip", "vendor/a.zip"),
            ("Invoke-WebRequest https://example.invalid/a.zip -OutFile vendor/a.zip", "vendor/a.zip"),
            // A query string is not a fork.
            ("curl \"https://example.invalid/d?a=1&b=2\" -o a.bin", "a.bin"),
        ] {
            assert_eq!(
                fetch_output_target(command).as_deref(),
                Some(expected),
                "{command} names exactly one local destination"
            );
        }
    }

    /// The file observer may only stand in for a *read* that lands locally.
    /// A request carrying a method or a body changes state on the far side, and
    /// a destination outside the workspace is not something the digest can read
    /// back.
    #[test]
    fn a_write_shaped_or_unbounded_fetch_names_no_observable_file() {
        for command in [
            "curl -X POST https://example.invalid/hooks -d '{\"ok\":true}' -o reply.json",
            "curl -T upload.bin https://example.invalid/put -o reply.json",
            "curl -fsSL https://example.invalid/i.sh -o /etc/profile.d/i.sh",
            "curl -fsSL https://example.invalid/i.sh -o ../outside.sh",
            "curl -fsSL https://example.invalid/i.sh | sh",
            "curl -fsSL https://example.invalid/i.sh -o i.sh &",
            "curl -O https://example.invalid/remote-name.tgz",
            "curl -fsSL https://example.invalid/i.sh",
        ] {
            assert_eq!(
                fetch_output_target(command),
                None,
                "{command} must not claim a file observer it cannot honour"
            );
        }
    }

    /// The gate's job is to fence effects this machine cannot read back, not
    /// verbs it has not met before. An allowlist of a dozen local commands made
    /// every unlisted one — `npm install`, a project's own build script —
    /// permanently unrunnable, because no replan could produce an observer for
    /// a command the list simply did not contain.
    #[test]
    fn ordinary_local_work_stays_observable_by_the_workspace_digest() {
        for command in [
            "npm install",
            "pnpm install --frozen-lockfile",
            "pip install -r requirements.txt",
            "python3 tools/generate.py",
            "make build",
            "docker build -t local/app .",
            "cd packages/app && npm run codegen",
            "git commit -m \"fix\"",
            "mkdir -p vendor",
            "sh install.sh",
            "cargo run --bin migrate -- --path $(git rev-parse --show-toplevel)",
        ] {
            assert!(
                !escapes_workspace_observation(command),
                "{command} only touches this machine and must keep a usable observer"
            );
        }
    }

    /// The two families that genuinely outrun the workspace digest: effects
    /// that land on another system, and forks that outlive the observation.
    #[test]
    fn effects_beyond_this_machine_stay_fenced() {
        for command in [
            "git push origin main",
            "gh pr create --fill",
            "gh pr merge 12 --squash",
            "npm publish",
            "docker push registry.invalid/app:1",
            "kubectl apply -f deployment.yaml",
            "helm upgrade app ./chart",
            "ssh host 'systemctl restart app'",
            "scp build.tar host:/srv",
            "rsync -a dist/ host:/srv",
            "curl -X POST https://example.invalid/hooks -d '{\"ok\":true}'",
            "nohup sh -c 'touch launched' >/dev/null 2>&1 &",
            "Start-Process powershell -ArgumentList '-Command whoami'",
        ] {
            assert!(
                escapes_workspace_observation(command),
                "{command} writes where this machine cannot read back"
            );
        }
    }

    #[test]
    fn a_quoted_ampersand_is_not_a_fork() {
        assert!(contains_unquoted("sleep 1 & echo done", '&'));
        assert!(!contains_unquoted("curl \"https://h/d?a=1&b=2\" -o a.bin", '&'));
        assert!(!contains_unquoted("curl 'https://h/d?a=1&b=2' -o a.bin", '&'));
    }
}
