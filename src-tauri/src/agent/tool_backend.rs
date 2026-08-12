// SPDX-License-Identifier: Apache-2.0
//! Desktop tool-execution backend (keystone slice 4.3).
//!
//! The in-process implementation of [`codefactory_agent_loop::tool::ToolBackend`]:
//! it builds a [`crate::tools::ExecCtx`] from the per-call [`ToolCtx`] plus its
//! own long-lived handles and runs the tool MCP-first / native-dispatch, exactly
//! as both provider loop bodies did inline before. Both loops now route through
//! one `execute`, so the duplicated dispatch block lives in a single place.
//!
//! This owns the `AppHandle` privately (under `#[cfg(not(test))]`, mirroring
//! `ExecCtx.app`), so the loop only ever calls it through the trait and the
//! unit-test EXE links no Tauri entrypoints (#166). It is constructed only in
//! `run_openai`/`run_anthropic`, which the test EXE dead-strips.

use codefactory_agent_core::ToolKind;
use codefactory_agent_loop::tool::{
    ToolBackend, ToolCtx, ToolError, ToolExecutionStatus, ToolInvocationResult,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::HashSet;

use crate::openrouter::types::{ToolCall, ToolDefinition};

enum MutationAdmission {
    Unbound,
    Dispatch { receipt_id: Option<String> },
    Replay(ToolInvocationResult),
    Waiting(ToolInvocationResult),
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
        }
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        serde_json::Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn opaque_digest(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn desktop_command_and_kind(tool_name: &str, args: &serde_json::Value) -> (String, ToolKind) {
    let (command, typed_kind) =
        codefactory_agent_loop::policy::completion_command_and_kind(tool_name, args);
    let kind = match tool_name {
        // Shell and browser have argument-sensitive native classifiers. In
        // particular, browser probes remain probes while click/fill/screenshot
        // keep their mutation semantics.
        "bash" | "browser_session" => typed_kind,
        // This is an explicit read-only capability list. Every new native tool
        // defaults to Mutation until it receives an audited typed classifier.
        "read_file" | "glob" | "grep" | "kb_search" | "kb_get_chunk" | "read_pptx"
        | "skill_list" | "skill_search" | "read_xlsx" => ToolKind::ReadOnly,
        _ => ToolKind::Mutation,
    };
    (command, kind)
}

fn bash_has_explicit_external_mutation(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let curl = lower
        .split(|character: char| character.is_whitespace() || matches!(character, ';' | '|' | '&'))
        .any(|word| word == "curl");
    curl && [
        " -x post",
        " -xpost",
        " --request post",
        " -x put",
        " -xput",
        " --request put",
        " -x patch",
        " -xpatch",
        " --request patch",
        " -x delete",
        " -xdelete",
        " --request delete",
        " --data",
        " --json",
        " --form",
        " --upload-file",
        " -d ",
        " -f ",
        " -t ",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn bash_is_explicit_read_only(command: &str) -> bool {
    let lower = command.trim_start().to_ascii_lowercase();
    if lower.contains(['\n', ';', '&', '>']) || lower.contains("| tee ") {
        return false;
    }
    [
        "pwd",
        "ls",
        "rg",
        "grep",
        "find",
        "cat",
        "head",
        "tail",
        "sed -n",
        "stat",
        "wc",
        "du",
        "df",
        "which",
        "command -v",
        "git status",
        "git diff",
        "git log",
        "git show",
        "git rev-parse",
        "git ls-files",
        "git branch --show-current",
        "kubectl get",
        "kubectl describe",
        "kubectl logs",
        "kubectl version",
    ]
    .iter()
    .any(|command_name| {
        lower == *command_name
            || lower
                .strip_prefix(command_name)
                .is_some_and(|suffix| suffix.chars().next().is_some_and(char::is_whitespace))
    })
}

/// Completion evidence and durable side-effect admission answer different
/// questions. A background service remains `BackgroundServiceStart`, a POST
/// probe may remain `RuntimeProbe`, and both still require an Objective-bound
/// receipt before dispatch.
fn native_requires_mutation_receipt(
    tool_name: &str,
    args: &serde_json::Value,
    completion_kind: &ToolKind,
) -> bool {
    match tool_name {
        "bash" => {
            let command = args
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if bash_has_explicit_external_mutation(command) {
                return true;
            }
            match completion_kind {
                ToolKind::Mutation | ToolKind::BackgroundServiceStart => true,
                ToolKind::Verification
                | ToolKind::RuntimeProbe
                | ToolKind::FunctionalProbe { .. } => false,
                ToolKind::ReadOnly => !bash_is_explicit_read_only(command),
            }
        }
        "browser_session" => !matches!(
            args.get("action").and_then(serde_json::Value::as_str),
            Some("snapshot" | "tabs")
        ),
        "read_file" | "glob" | "grep" | "kb_search" | "kb_get_chunk" | "read_pptx"
        | "skill_list" | "skill_search" | "read_xlsx" => false,
        _ => true,
    }
}

fn waiting_result(command: &str, kind: ToolKind, code: &str) -> ToolInvocationResult {
    ToolInvocationResult {
        content: "外部变更未再次发出；系统将核对持久化状态后自动继续。".into(),
        is_error: false,
        status: ToolExecutionStatus::Waiting,
        command: command.to_string(),
        kind,
        return_code: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        metadata: Some(serde_json::json!({
            "code": code,
            "recoverable": true,
            "next_action": "observe_only_reconcile",
            "system_owned": true,
        })),
        next_working_directory: None,
        duration_ms: 0,
    }
}

fn invocation_from_output(
    output: crate::tools::ToolOutput,
    command: String,
    kind: ToolKind,
) -> ToolInvocationResult {
    ToolInvocationResult {
        content: output.content,
        is_error: output.is_error,
        status: match output.status {
            crate::tools::ToolExecutionStatus::Done => ToolExecutionStatus::Done,
            crate::tools::ToolExecutionStatus::Waiting => ToolExecutionStatus::Waiting,
            crate::tools::ToolExecutionStatus::Blocked => ToolExecutionStatus::Blocked,
            crate::tools::ToolExecutionStatus::Error => ToolExecutionStatus::Error,
        },
        command,
        kind,
        return_code: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        metadata: output.metadata,
        next_working_directory: None,
        duration_ms: 0,
    }
}

/// In-process tool backend for the desktop app. Holds the long-lived handles;
/// per-call context (cwd, session, task, knowledge scope) arrives via [`ToolCtx`].
pub(super) struct DesktopToolBackend {
    /// Owned privately so the loop never sees an `AppHandle`. Absent in the
    /// test config — this struct is constructed only in the (dead-stripped)
    /// provider loops, never in a `#[cfg(test)]` test.
    #[cfg(not(test))]
    pub(super) app: Option<tauri::AppHandle>,
    pub(super) db: sqlx::SqlitePool,
    pub(super) mcp_manager: std::sync::Arc<crate::mcp::McpManager>,
    pub(super) settings: std::sync::Arc<tokio::sync::RwLock<crate::config::settings::Settings>>,
    /// `ToolBackend::classify` is synchronous, so MCP discovery refreshes this
    /// conservative cache. Unknown/missing MCP annotations never downgrade a
    /// connected tool to read-only.
    pub(super) mcp_tool_names: std::sync::Arc<std::sync::RwLock<HashSet<String>>>,
}

impl DesktopToolBackend {
    async fn mutation_preflight(
        &self,
        call: &ToolCall,
        args: &serde_json::Value,
        ctx: &ToolCtx,
        command: &str,
        kind: ToolKind,
    ) -> Result<MutationAdmission, ToolError> {
        let resource = if let Some(task_id) = ctx.task_id.as_deref() {
            Some(("task", "task_run", task_id, true))
        } else {
            ctx.root_turn_id
                .as_deref()
                .map(|root_turn_id| ("chat", "chat_root_turn", root_turn_id, false))
        };
        let Some((binding_domain, resource_kind, resource_id, is_task)) = resource else {
            return Ok(MutationAdmission::Waiting(waiting_result(
                command,
                kind,
                "objective_identity_missing",
            )));
        };

        let mut tx = self.db.begin().await.map_err(|error| ToolError {
            message: format!("begin mutation preflight: {error}"),
        })?;
        let objective_id = if is_task {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT objective_id FROM task_runs WHERE id=?",
            )
            .bind(resource_id)
            .fetch_optional(&mut *tx)
            .await
        } else {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT objective_id FROM chat_turn_state WHERE root_turn_id=?",
            )
            .bind(resource_id)
            .fetch_optional(&mut *tx)
            .await
        }
        .map_err(|error| ToolError {
            message: format!("resolve mutation objective: {error}"),
        })?
        .flatten()
        .ok_or_else(|| ToolError {
            message: format!(
                "mutation refused without an opaque Objective binding for {resource_kind}:{resource_id}"
            ),
        })?;

        let objective = sqlx::query(
            "SELECT revision, status, remediation_id
             FROM objectives WHERE id=?",
        )
        .bind(&objective_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("load mutation objective: {error}"),
        })?
        .ok_or_else(|| ToolError {
            message: format!("mutation Objective {objective_id} no longer exists"),
        })?;
        let revision: i64 = objective.get("revision");
        let objective_status: String = objective.get("status");
        let remediation_id: Option<String> = objective.get("remediation_id");

        let binding = sqlx::query(
            "SELECT id, resource_generation FROM objective_bindings
             WHERE objective_id=? AND domain=? AND resource_kind=? AND resource_id=?
             ORDER BY resource_generation DESC LIMIT 1",
        )
        .bind(&objective_id)
        .bind(binding_domain)
        .bind(resource_kind)
        .bind(resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("load mutation Objective binding: {error}"),
        })?
        .ok_or_else(|| ToolError {
            message: format!(
                "mutation refused because Objective {objective_id} has no authoritative {resource_kind} binding"
            ),
        })?;
        let binding_id: String = binding.get("id");
        let resource_generation: i64 = binding.get("resource_generation");
        let canonical_args = canonical_json(args);
        let cwd = ctx.working_directory.to_string_lossy();
        let generation = resource_generation.to_string();
        let action_fingerprint = opaque_digest(&[
            &call.function.name,
            &canonical_args,
            cwd.as_ref(),
            &binding_id,
            &generation,
        ]);

        let trajectory_session_id = ctx
            .trajectory_session_id
            .as_deref()
            .or(ctx.session_id.as_deref())
            .ok_or_else(|| ToolError {
                message: "objective-bound mutation is missing its trajectory session".into(),
            })?;
        let trace_id = crate::trajectory::trace_record_id(trajectory_session_id, &call.id);
        let attributed = sqlx::query(
            "UPDATE tool_calls
             SET objective_id=?, action_signature=?, resource_generation=?
             WHERE id=?
               AND (objective_id IS NULL OR objective_id=?)
               AND (action_signature IS NULL OR action_signature=?)
               AND (resource_generation IS NULL OR resource_generation=?)",
        )
        .bind(&objective_id)
        .bind(&action_fingerprint)
        .bind(resource_generation)
        .bind(&trace_id)
        .bind(&objective_id)
        .bind(&action_fingerprint)
        .bind(resource_generation)
        .execute(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("persist mutation tool attribution: {error}"),
        })?;
        if attributed.rows_affected() != 1 {
            return Err(ToolError {
                message: format!(
                    "normalized tool call {trace_id} is missing or has conflicting Objective attribution"
                ),
            });
        }

        match (objective_status.as_str(), ctx.mutation_permit.as_ref()) {
            ("active", None) => {}
            ("waiting_system", Some(permit)) => {
                let permit_matches = permit.objective_id == objective_id
                    && remediation_id.as_deref() == Some(permit.remediation_id.as_str())
                    && permit.binding_id.as_deref() == Some(binding_id.as_str())
                    && permit.resource_generation == Some(resource_generation);
                if !permit_matches {
                    tx.commit().await.map_err(|error| ToolError {
                        message: format!("persist stale mutation attribution: {error}"),
                    })?;
                    return Ok(MutationAdmission::Waiting(waiting_result(
                        command,
                        kind,
                        "mutation_permit_lost",
                    )));
                }
                let now = chrono::Utc::now().timestamp_millis();
                let remediation = sqlx::query(
                    "UPDATE objective_remediations SET updated_at=updated_at
                     WHERE id=? AND objective_id=? AND binding_id=?
                       AND status='claimed' AND lease_owner=?
                       AND attempt_index=? AND lease_expires_at>?",
                )
                .bind(&permit.remediation_id)
                .bind(&objective_id)
                .bind(&binding_id)
                .bind(&permit.owner)
                .bind(permit.claim_epoch)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|error| ToolError {
                    message: format!("validate mutation remediation permit: {error}"),
                })?;
                let objective_claim = sqlx::query(
                    "UPDATE objectives SET updated_at=updated_at
                     WHERE id=? AND revision=? AND status='waiting_system'
                       AND remediation_id=? AND lease_owner=? AND lease_expires_at>?",
                )
                .bind(&objective_id)
                .bind(revision)
                .bind(&permit.remediation_id)
                .bind(&permit.owner)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|error| ToolError {
                    message: format!("validate mutation Objective permit: {error}"),
                })?;
                if remediation.rows_affected() != 1 || objective_claim.rows_affected() != 1 {
                    tx.commit().await.map_err(|error| ToolError {
                        message: format!("persist expired mutation attribution: {error}"),
                    })?;
                    return Ok(MutationAdmission::Waiting(waiting_result(
                        command,
                        kind,
                        "mutation_permit_lost",
                    )));
                }
            }
            ("waiting_system", None) | ("active", Some(_)) => {
                tx.commit().await.map_err(|error| ToolError {
                    message: format!("persist fenced mutation attribution: {error}"),
                })?;
                return Ok(MutationAdmission::Waiting(waiting_result(
                    command,
                    kind,
                    "mutation_permit_lost",
                )));
            }
            _ => {
                tx.commit().await.map_err(|error| ToolError {
                    message: format!("persist inactive mutation attribution: {error}"),
                })?;
                return Ok(MutationAdmission::Waiting(waiting_result(
                    command,
                    kind,
                    "objective_not_mutable",
                )));
            }
        }

        let objective_started = sqlx::query(
            "UPDATE objectives SET side_effect_started=1
             WHERE id=? AND revision=?",
        )
        .bind(&objective_id)
        .bind(revision)
        .execute(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("mark Objective side effect started: {error}"),
        })?;
        let binding_started = sqlx::query(
            "UPDATE objective_bindings SET side_effect_started=1, updated_at=?
             WHERE id=? AND objective_id=? AND resource_generation=?",
        )
        .bind(chrono::Utc::now().timestamp_millis())
        .bind(&binding_id)
        .bind(&objective_id)
        .bind(resource_generation)
        .execute(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("mark Objective binding side effect started: {error}"),
        })?;
        if objective_started.rows_affected() != 1 || binding_started.rows_affected() != 1 {
            return Err(ToolError {
                message: "Objective identity changed before mutation dispatch".into(),
            });
        }

        // DeliveryRun owns its own epoch, mutation rung and revision receipts.
        // Wrapping it in the generic ledger would turn a legitimate durable
        // Waiting result into `unknown` and block its observation loop. It
        // still receives the exact Objective attribution and permit check above.
        if call.function.name == "deliver_changes" {
            tx.commit().await.map_err(|error| ToolError {
                message: format!("commit delivery Objective attribution: {error}"),
            })?;
            return Ok(MutationAdmission::Dispatch { receipt_id: None });
        }

        // Provider call ids change across forced reprompts and process resume.
        // Durable idempotency is the Objective-bound action itself, not one
        // transport response's ephemeral identifier.
        let idempotency_key =
            opaque_digest(&[&objective_id, &action_fingerprint, &binding_id, &generation]);
        if let Some(existing) = sqlx::query(
            "SELECT status, summary_json FROM side_effect_receipts
             WHERE objective_id=? AND action_fingerprint=? AND idempotency_key=?
             ORDER BY observed_at DESC LIMIT 1",
        )
        .bind(&objective_id)
        .bind(&action_fingerprint)
        .bind(&idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("load mutation replay receipt: {error}"),
        })? {
            let status: String = existing.get("status");
            if matches!(status.as_str(), "committed" | "reconciled") {
                let summary_json: Option<String> = existing.get("summary_json");
                let summary = summary_json
                    .as_deref()
                    .and_then(|summary| serde_json::from_str::<serde_json::Value>(summary).ok())
                    .ok_or_else(|| ToolError {
                        message: "committed mutation receipt has no valid replay summary".into(),
                    })?;
                let status = match summary.get("status").and_then(|value| value.as_str()) {
                    Some("done") => ToolExecutionStatus::Done,
                    _ => {
                        return Err(ToolError {
                            message: "committed mutation receipt has an invalid status".into(),
                        })
                    }
                };
                let replay = ToolInvocationResult {
                    content: "此前相同外部变更已由持久化回执确认完成；未重复执行。".into(),
                    is_error: false,
                    status,
                    command: command.to_string(),
                    kind,
                    return_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    error: None,
                    metadata: Some(serde_json::json!({
                        "receipt_replayed": true,
                        "system_owned": true,
                    })),
                    next_working_directory: None,
                    duration_ms: 0,
                };
                tx.commit().await.map_err(|error| ToolError {
                    message: format!("commit mutation replay attribution: {error}"),
                })?;
                return Ok(MutationAdmission::Replay(replay));
            }
        }

        let uncertain: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND binding_id=?
               AND status IN ('started','unknown')",
        )
        .bind(&objective_id)
        .bind(&binding_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("inspect uncertain mutation receipts: {error}"),
        })?;
        if uncertain > 0 {
            tx.commit().await.map_err(|error| ToolError {
                message: format!("commit uncertain mutation attribution: {error}"),
            })?;
            return Ok(MutationAdmission::Waiting(waiting_result(
                command,
                kind,
                "external_state_uncertain",
            )));
        }

        // The primary key is deliberately deterministic. It gives every new
        // writer a database-enforced cross-revision collision even on schemas
        // whose older composite UNIQUE constraint still included `revision`.
        let receipt_id = opaque_digest(&["side_effect_receipt", &idempotency_key]);
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES (?, ?, ?, ?, ?, ?, 'started', ?, ?)",
        )
        .bind(&receipt_id)
        .bind(&objective_id)
        .bind(&binding_id)
        .bind(revision)
        .bind(&action_fingerprint)
        .bind(&idempotency_key)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("persist started mutation receipt: {error}"),
        })?;
        tx.commit().await.map_err(|error| ToolError {
            message: format!("commit started mutation receipt: {error}"),
        })?;
        Ok(MutationAdmission::Dispatch {
            receipt_id: Some(receipt_id),
        })
    }

    async fn settle_mutation_receipt(
        &self,
        receipt_id: &str,
        result: Option<&ToolInvocationResult>,
    ) -> Result<(), ToolError> {
        let (status, summary_json) = match result {
            Some(result) if result.status == ToolExecutionStatus::Done && !result.is_error => {
                let summary = serde_json::json!({
                    "status": "done",
                });
                ("committed", Some(summary.to_string()))
            }
            _ => ("unknown", None),
        };
        let updated = sqlx::query(
            "UPDATE side_effect_receipts
             SET status=?, summary_json=?, observed_at=?
             WHERE id=? AND status='started'",
        )
        .bind(status)
        .bind(summary_json)
        .bind(chrono::Utc::now().timestamp_millis())
        .bind(receipt_id)
        .execute(&self.db)
        .await
        .map_err(|error| ToolError {
            message: format!("persist mutation receipt outcome: {error}"),
        })?;
        if updated.rows_affected() != 1 {
            return Err(ToolError {
                message: format!("mutation receipt {receipt_id} changed before settlement"),
            });
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ToolBackend for DesktopToolBackend {
    async fn list_schemas(&self) -> Vec<ToolDefinition> {
        // Desktop surface = native tools + every connected MCP tool. (The
        // anonymous KB-tool strip stays in the loop for now — it depends on the
        // run's anonymous flag; folded in when the loop moves in slice 4.6.)
        let mut defs = crate::tools::all_definitions();
        let mcp_tools = self.mcp_manager.list_all_tools().await;
        if let Ok(mut names) = self.mcp_tool_names.write() {
            names.clear();
            names.extend(mcp_tools.iter().map(|tool| tool.name.clone()));
        }
        for mcp_tool in &mcp_tools {
            defs.push(super::mcp_tool_to_definition(mcp_tool));
        }
        defs
    }

    async fn execute(
        &self,
        call: &ToolCall,
        args: &serde_json::Value,
        ctx: &ToolCtx,
    ) -> Result<ToolInvocationResult, ToolError> {
        let mcp_server = self.mcp_manager.find_tool_server(&call.function.name).await;
        let (command, native_kind) = desktop_command_and_kind(&call.function.name, args);
        let kind = if mcp_server.is_some() {
            ToolKind::Mutation
        } else {
            native_kind
        };
        let requires_receipt = mcp_server.is_some()
            || native_requires_mutation_receipt(&call.function.name, args, &kind);
        let admission = if requires_receipt {
            self.mutation_preflight(call, args, ctx, &command, kind.clone())
                .await?
        } else {
            MutationAdmission::Unbound
        };
        let receipt_id = match admission {
            MutationAdmission::Replay(result) | MutationAdmission::Waiting(result) => {
                return Ok(result)
            }
            MutationAdmission::Unbound => None,
            MutationAdmission::Dispatch { receipt_id } => receipt_id,
        };

        let exec_ctx = crate::tools::ExecCtx {
            cwd: ctx.working_directory.clone(),
            #[cfg(not(test))]
            app: self.app.clone(),
            db: Some(self.db.clone()),
            session_id: ctx.session_id.clone(),
            root_turn_id: ctx.root_turn_id.clone(),
            task_id: ctx.task_id.clone(),
            knowledge_library_ids: ctx.knowledge_library_ids.clone(),
            settings: Some(self.settings.read().await.clone()),
        };

        // MCP-first, then native dispatch — precedence and the `Unknown tool`
        // sentinel are preserved. An MCP error becomes an `is_error` result the
        // model sees; a native-dispatch `Err` is FATAL and aborts the turn.
        let output = if let Some(server_id) = mcp_server {
            match self
                .mcp_manager
                .call_tool(&server_id, &call.function.name, args.clone())
                .await
            {
                Ok(text) => crate::tools::ToolOutput::ok(text),
                Err(e) => crate::tools::ToolOutput::err(format!("MCP error: {e}")),
            }
        } else {
            match crate::tools::dispatch(&call.function.name, args.clone(), &exec_ctx).await {
                Ok(output) => output,
                Err(error) => {
                    if let Some(receipt_id) = receipt_id.as_deref() {
                        self.settle_mutation_receipt(receipt_id, None).await?;
                    }
                    return Err(ToolError {
                        message: error.to_string(),
                    });
                }
            }
        };

        let result = invocation_from_output(output, command, kind);
        if let Some(receipt_id) = receipt_id.as_deref() {
            self.settle_mutation_receipt(receipt_id, Some(&result))
                .await?;
        }
        Ok(result)
    }

    fn classify(&self, call: &ToolCall, args: &serde_json::Value) -> (String, ToolKind) {
        if self
            .mcp_tool_names
            .read()
            .is_ok_and(|names| names.contains(&call.function.name))
        {
            (format!("mcp:{}", call.function.name), ToolKind::Mutation)
        } else {
            desktop_command_and_kind(&call.function.name, args)
        }
    }
}

#[cfg(test)]
mod tests {
    //! In `#[cfg(test)]` builds `DesktopToolBackend` has NO `app` field, so
    //! these construct the REAL backend with no `AppHandle` — that headless
    //! constructibility is the whole point of the seam, and it keeps the
    //! unit-test EXE clear of Tauri entrypoints (#166; McpManager/Settings/pool
    //! own no `AppHandle`). This locks the contract the loop relies on: the full
    //! native tool surface runs through `execute`, MCP-first with a native
    //! fallback, and an unknown tool is a clean `is_error` result, not a fatal
    //! `Err`.
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    const TEST_SESSION_ID: &str = "session-tool-fencing";
    const TEST_ROOT_TURN_ID: &str = "root-tool-fencing";
    const TEST_OBJECTIVE_ID: &str = "5cf0bf25-2ed8-4cad-a775-f55cd16f0830";
    const TEST_BINDING_ID: &str = "binding-tool-fencing";

    async fn backend() -> DesktopToolBackend {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        DesktopToolBackend {
            db,
            mcp_manager: std::sync::Arc::new(crate::mcp::McpManager::new()),
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::config::settings::Settings::default(),
            )),
            mcp_tool_names: std::sync::Arc::new(std::sync::RwLock::new(HashSet::new())),
        }
    }

    /// Materialize only the persisted identities that a mutation permit may
    /// trust. The failure-first tests below deliberately exercise the real
    /// backend seam: no test-only dispatcher or fake receipt store can make a
    /// duplicate external launch look safe.
    async fn objective_backend(waiting_with_foreign_lease: bool) -> DesktopToolBackend {
        let backend = backend().await;
        sqlx::raw_sql(include_str!(
            "../../migrations/0007_unified_objective_control_plane.sql"
        ))
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE chat_turn_state (
                 root_turn_id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 objective_id TEXT,
                 revision INTEGER NOT NULL
             );
             CREATE TABLE tool_calls (
                 id TEXT PRIMARY KEY,
                 message_id TEXT NOT NULL,
                 tool_name TEXT NOT NULL,
                 arguments TEXT NOT NULL DEFAULT '{}',
                 result TEXT,
                 metadata TEXT,
                 status TEXT NOT NULL DEFAULT 'pending',
                 error TEXT,
                 duration_ms INTEGER,
                 created_at INTEGER NOT NULL,
                 objective_id TEXT,
                 action_signature TEXT,
                 resource_generation INTEGER
             );",
        )
        .execute(&backend.db)
        .await
        .unwrap();

        let now = chrono::Utc::now().timestamp_millis();
        let status = if waiting_with_foreign_lease {
            "waiting_system"
        } else {
            "active"
        };
        let decision_type = if waiting_with_foreign_lease {
            "waiting"
        } else {
            "continue"
        };
        let remediation_id = waiting_with_foreign_lease.then_some("remediation-tool-fencing");
        let lease_owner = waiting_with_foreign_lease.then_some("replacement-supervisor");
        let lease_expires_at = waiting_with_foreign_lease.then_some(now + 60_000);
        sqlx::query(
            "INSERT INTO objectives
             (id, revision, kind, session_id, root_turn_id, status, decision_type,
              domain, autonomous_completion, requested_acceptance, requires_user_action,
              recovery_owner, remediation_id, lease_owner, lease_expires_at,
              created_surface, created_at, updated_at)
             VALUES (?, 1, 'local_mutation', ?, ?, ?, ?, 'tool', 1,
                     'validated_change', 0, 'objective-supervisor', ?, ?, ?,
                     'test', ?, ?)",
        )
        .bind(TEST_OBJECTIVE_ID)
        .bind(TEST_SESSION_ID)
        .bind(TEST_ROOT_TURN_ID)
        .bind(status)
        .bind(decision_type)
        .bind(remediation_id)
        .bind(lease_owner)
        .bind(lease_expires_at)
        .bind(now)
        .bind(now)
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, created_at, updated_at)
             VALUES (?, ?, 'chat', 'chat_root_turn', ?, 1,
                     'sha256:test-binding', ?, ?)",
        )
        .bind(TEST_BINDING_ID)
        .bind(TEST_OBJECTIVE_ID)
        .bind(TEST_ROOT_TURN_ID)
        .bind(now)
        .bind(now)
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, objective_id, revision)
             VALUES (?, ?, ?, 1)",
        )
        .bind(TEST_ROOT_TURN_ID)
        .bind(TEST_SESSION_ID)
        .bind(TEST_OBJECTIVE_ID)
        .execute(&backend.db)
        .await
        .unwrap();
        if waiting_with_foreign_lease {
            sqlx::query(
                "INSERT INTO objective_remediations
                 (id, objective_id, binding_id, domain, status, failure_code,
                  failure_signature, strategy, approach_index, attempt_index,
                  next_observation_at, lease_owner, lease_expires_at,
                  created_at, updated_at)
                 VALUES ('remediation-tool-fencing', ?, ?, 'tool', 'claimed',
                         'external_state_uncertain', 'sha256:test-failure',
                         'observe_then_resume', 0, 2, ?,
                         'replacement-supervisor', ?, ?, ?)",
            )
            .bind(TEST_OBJECTIVE_ID)
            .bind(TEST_BINDING_ID)
            .bind(now)
            .bind(now + 60_000)
            .bind(now)
            .bind(now)
            .execute(&backend.db)
            .await
            .unwrap();
        }
        backend
    }

    fn call_with_args(id: &str, name: &str, args: &serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            r#type: "function".into(),
            function: crate::openrouter::types::FunctionCall {
                name: name.into(),
                arguments: args.to_string(),
            },
        }
    }

    fn append_once_args() -> serde_json::Value {
        if cfg!(windows) {
            serde_json::json!({"command": "Add-Content -Path effect.log -Value once"})
        } else {
            serde_json::json!({"command": "printf 'once\\n' >> effect.log"})
        }
    }

    fn objective_ctx(dir: &std::path::Path) -> ToolCtx {
        ToolCtx {
            working_directory: dir.to_path_buf(),
            session_id: Some(TEST_SESSION_ID.into()),
            root_turn_id: Some(TEST_ROOT_TURN_ID.into()),
            trajectory_session_id: Some(TEST_SESSION_ID.into()),
            ..Default::default()
        }
    }

    fn current_permit(claim_epoch: i64) -> codefactory_agent_loop::tool::MutationPermit {
        codefactory_agent_loop::tool::MutationPermit {
            objective_id: TEST_OBJECTIVE_ID.into(),
            remediation_id: "remediation-tool-fencing".into(),
            owner: "replacement-supervisor".into(),
            claim_epoch,
            binding_id: Some(TEST_BINDING_ID.into()),
            resource_generation: Some(1),
        }
    }

    async fn register_tool_call(
        backend: &DesktopToolBackend,
        tool_call: &ToolCall,
        args: &serde_json::Value,
    ) {
        sqlx::query(
            "INSERT INTO tool_calls
             (id, message_id, tool_name, arguments, status, created_at)
             VALUES (?, 'assistant-message', ?, ?, 'pending', ?)",
        )
        .bind(crate::trajectory::trace_record_id(
            TEST_SESSION_ID,
            &tool_call.id,
        ))
        .bind(&tool_call.function.name)
        .bind(args.to_string())
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&backend.db)
        .await
        .unwrap();
    }

    async fn assert_objective_bound_call_starts_receipt(
        tool_name: &str,
        call_id: &str,
        args: &serde_json::Value,
    ) {
        let backend = objective_backend(false).await;
        let call = call_with_args(call_id, tool_name, args);
        register_tool_call(&backend, &call, args).await;
        let (command, kind) = backend.classify(&call, args);
        assert!(
            native_requires_mutation_receipt(tool_name, args, &kind),
            "{tool_name} with {args} must enter the durable mutation fence"
        );
        let admission = backend
            .mutation_preflight(
                &call,
                args,
                &objective_ctx(std::path::Path::new(".")),
                &command,
                kind,
            )
            .await
            .expect("mutation preflight must persist its dispatch fence");
        let receipt_id = match admission {
            MutationAdmission::Dispatch {
                receipt_id: Some(receipt_id),
            } => receipt_id,
            _ => panic!("{tool_name} must not dispatch without a generic started receipt"),
        };
        let status: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE id=?")
                .bind(receipt_id)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(status, "started");
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "t".into(),
            r#type: "function".into(),
            function: crate::openrouter::types::FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    #[tokio::test]
    async fn desktop_backend_runs_the_native_surface_headless() {
        let dir = tempfile::tempdir().unwrap();
        let backend = objective_backend(false).await;
        let ctx = objective_ctx(dir.path());
        let write_args = serde_json::json!({ "path": "n.txt", "content": "hello backend" });
        let write_call = call_with_args("headless-native-write", "write_file", &write_args);
        register_tool_call(&backend, &write_call, &write_args).await;

        let out = backend
            .execute(&write_call, &write_args, &ctx)
            .await
            .expect("write is not fatal");
        assert!(!out.is_error, "write via backend: {}", out.content);

        let out = backend
            .execute(
                &call("read_file"),
                &serde_json::json!({ "path": "n.txt" }),
                &ctx,
            )
            .await
            .expect("read is not fatal");
        assert!(!out.is_error && out.content.contains("hello backend"));
    }

    #[tokio::test]
    async fn unknown_tool_is_an_is_error_result_not_a_fatal_err() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = objective_ctx(dir.path());
        let unknown = call("no_such_tool");
        let args = serde_json::json!({});
        register_tool_call(&backend, &unknown, &args).await;
        let out = backend
            .execute(&unknown, &args, &ctx)
            .await
            .expect("unknown tool returns a result, never aborts the turn");
        assert!(out.is_error);
        assert!(out.content.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn cached_mcp_tool_is_mutation_without_calling_list_schemas_first() {
        let backend = backend().await;
        backend
            .mcp_tool_names
            .write()
            .unwrap()
            .insert("mcp_without_annotations".into());
        let (command, kind) = backend.classify(
            &call("mcp_without_annotations"),
            &serde_json::json!({"query": "read-looking but unannotated"}),
        );
        assert_eq!(command, "mcp:mcp_without_annotations");
        assert_eq!(kind, ToolKind::Mutation);
    }

    #[tokio::test]
    async fn every_native_tool_outside_the_read_only_whitelist_defaults_to_mutation() {
        let backend = backend().await;
        let read_only = [
            "read_file",
            "glob",
            "grep",
            "kb_search",
            "kb_get_chunk",
            "read_pptx",
            "skill_list",
            "skill_search",
            "read_xlsx",
        ];
        for definition in crate::tools::all_definitions() {
            let name = definition.function.name;
            if name == "bash" || name == "browser_session" {
                continue;
            }
            let (_, kind) = backend.classify(&call(&name), &serde_json::json!({}));
            if read_only.contains(&name.as_str()) {
                assert_eq!(
                    kind,
                    ToolKind::ReadOnly,
                    "{name} read-only contract drifted"
                );
            } else {
                assert_eq!(
                    kind,
                    ToolKind::Mutation,
                    "{name} must be fenced until explicitly audited read-only"
                );
            }
        }
        assert_eq!(
            backend
                .classify(
                    &call("format_pptx"),
                    &serde_json::json!({"path": "deck.pptx"})
                )
                .1,
            ToolKind::Mutation
        );
        assert_eq!(
            backend
                .classify(&call("update_plan"), &serde_json::json!({"steps": []}))
                .1,
            ToolKind::Mutation
        );
    }

    #[tokio::test]
    async fn skill_fetch_is_fenced_as_a_mutation_and_starts_a_receipt() {
        assert_objective_bound_call_starts_receipt(
            "skill_fetch",
            "skill-fetch-mutation",
            &serde_json::json!({"name": "workspace-maintenance"}),
        )
        .await;
    }

    #[tokio::test]
    async fn side_effecting_bash_commands_cannot_bypass_the_receipt_fence() {
        for (call_id, command) in [
            (
                "bash-curl-post",
                "curl -X POST https://example.invalid/hooks -d '{\"ok\":true}'",
            ),
            ("bash-kubectl-apply", "kubectl apply -f deployment.yaml"),
            (
                "bash-nohup",
                "nohup sh -c 'touch launched.marker' >/dev/null 2>&1 &",
            ),
            (
                "bash-read-prefix-lookalike",
                "lsmalware --perform-side-effect",
            ),
        ] {
            assert_objective_bound_call_starts_receipt(
                "bash",
                call_id,
                &serde_json::json!({"command": command}),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn browser_open_close_and_select_tab_require_mutation_receipts() {
        for action in ["open", "close", "select_tab", "attach"] {
            assert_objective_bound_call_starts_receipt(
                "browser_session",
                &format!("browser-{action}"),
                &serde_json::json!({"action": action, "url": "https://example.invalid"}),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn mutation_without_root_or_task_is_fenced_before_native_dispatch() {
        let backend = backend().await;
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": "must-not-exist.txt", "content": "side effect"});
        let call = call_with_args("unbound-native-mutation", "write_file", &args);
        let outcome = backend
            .execute(
                &call,
                &args,
                &ToolCtx {
                    working_directory: dir.path().to_path_buf(),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            outcome.is_err()
                || outcome
                    .as_ref()
                    .is_ok_and(|result| { matches!(result.status, ToolExecutionStatus::Waiting) }),
            "an identity-free mutation must be rejected or held for system reconciliation"
        );
        assert!(
            !dir.path().join("must-not-exist.txt").exists(),
            "an identity-free mutation must have zero native dispatches"
        );
    }

    #[tokio::test]
    async fn mutation_without_opaque_objective_is_fenced_before_native_dispatch() {
        let backend = backend().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES ('unbound-root', 'unbound-session', NULL)",
        )
        .execute(&backend.db)
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": "must-not-exist.txt", "content": "side effect"});
        let call = call_with_args("opaque-objective-missing", "write_file", &args);
        let outcome = backend
            .execute(
                &call,
                &args,
                &ToolCtx {
                    working_directory: dir.path().to_path_buf(),
                    session_id: Some("unbound-session".into()),
                    root_turn_id: Some("unbound-root".into()),
                    trajectory_session_id: Some("unbound-session".into()),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            outcome.is_err(),
            "missing opaque Objective must fail closed"
        );
        assert!(!dir.path().join("must-not-exist.txt").exists());
    }

    #[tokio::test]
    async fn waiting_tool_outcome_is_persisted_as_waiting_not_trajectory_error() {
        let backend = objective_backend(false).await;
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               role TEXT NOT NULL,
               content TEXT NOT NULL,
               created_at INTEGER NOT NULL
             )",
        )
        .execute(&backend.db)
        .await
        .unwrap();
        let args = append_once_args();
        let call = call_with_args("waiting-trajectory", "bash", &args);
        register_tool_call(&backend, &call, &args).await;

        crate::trajectory::record_terminal_tool_outcome(
            &backend.db,
            TEST_SESSION_ID,
            &call.id,
            "waiting",
            Some("system-owned observation pending"),
            None,
            7,
        )
        .await
        .expect("Waiting is durable lifecycle state, not a trajectory write error");

        let (status, error): (String, Option<String>) =
            sqlx::query_as("SELECT status, error FROM tool_calls WHERE id=?")
                .bind(crate::trajectory::trace_record_id(
                    TEST_SESSION_ID,
                    &call.id,
                ))
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(status, "waiting");
        assert_eq!(error, None);
    }

    #[tokio::test]
    async fn objective_bound_mutation_records_receipt_and_tool_attribution() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let tool_call = call_with_args("mutation-attribution", "bash", &args);
        register_tool_call(&backend, &tool_call, &args).await;
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, created_at, updated_at)
             VALUES ('delivery-domain-collision', ?, 'delivery', 'chat_root_turn', ?, 2,
                     'sha256:other-domain', ?, ?)",
        )
        .bind(TEST_OBJECTIVE_ID)
        .bind(TEST_ROOT_TURN_ID)
        .bind(now)
        .bind(now)
        .execute(&backend.db)
        .await
        .unwrap();

        let out = backend
            .execute(&tool_call, &args, &objective_ctx(dir.path()))
            .await
            .expect("the permitted mutation itself is not fatal");
        assert!(!out.is_error, "mutation output: {}", out.content);

        let attribution: (Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT objective_id, action_signature, resource_generation
             FROM tool_calls WHERE id=?",
        )
        .bind(crate::trajectory::trace_record_id(
            TEST_SESSION_ID,
            &tool_call.id,
        ))
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert_eq!(attribution.0.as_deref(), Some(TEST_OBJECTIVE_ID));
        assert!(
            attribution
                .1
                .as_deref()
                .is_some_and(|signature| signature.starts_with("sha256:")),
            "a mutation must carry a canonical, opaque action signature"
        );
        assert_eq!(attribution.2, Some(1));

        let receipt: (String, String, String) = sqlx::query_as(
            "SELECT objective_id, status, action_fingerprint
             FROM side_effect_receipts",
        )
        .fetch_one(&backend.db)
        .await
        .expect("a receipt must be durable before success is returned");
        assert_eq!(receipt.0, TEST_OBJECTIVE_ID);
        assert_eq!(receipt.1, "committed");
        assert_eq!(Some(receipt.2.as_str()), attribution.1.as_deref());
    }

    #[tokio::test]
    async fn forced_reprompt_reuses_one_committed_receipt_across_provider_call_ids() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let ctx = objective_ctx(dir.path());

        for provider_call_id in ["mutation-before-reprompt", "mutation-after-reprompt"] {
            let tool_call = call_with_args(provider_call_id, "bash", &args);
            register_tool_call(&backend, &tool_call, &args).await;
            let out = backend
                .execute(&tool_call, &args, &ctx)
                .await
                .expect("receipt replay is a normal tool result");
            assert!(!out.is_error, "mutation/replay output: {}", out.content);
        }

        let content = std::fs::read_to_string(dir.path().join("effect.log")).unwrap();
        assert_eq!(
            content.lines().collect::<Vec<_>>(),
            vec!["once"],
            "the same durable tool call must launch its side effect at most once"
        );
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND status='committed'",
        )
        .bind(TEST_OBJECTIVE_ID)
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert_eq!(receipts, 1);
        let summary: String = sqlx::query_scalar(
            "SELECT summary_json FROM side_effect_receipts WHERE objective_id=?",
        )
        .bind(TEST_OBJECTIVE_ID)
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert!(
            !summary.contains("once"),
            "receipt summaries store no raw output"
        );
    }

    #[tokio::test]
    async fn uncertain_prior_mutation_forces_observe_only_instead_of_relaunch() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let first = call_with_args("mutation-before-crash", "bash", &args);
        register_tool_call(&backend, &first, &args).await;
        let ctx = objective_ctx(dir.path());
        backend
            .execute(&first, &args, &ctx)
            .await
            .expect("first mutation completes");

        let changed = sqlx::query(
            "UPDATE side_effect_receipts SET status='unknown'
             WHERE objective_id=? AND status='committed'",
        )
        .bind(TEST_OBJECTIVE_ID)
        .execute(&backend.db)
        .await
        .unwrap();
        assert_eq!(
            changed.rows_affected(),
            1,
            "the first mutation must have established its receipt fence"
        );

        // A resumed model may receive a fresh provider call id for the same
        // action. The unknown fingerprint, not that ephemeral id, is the fence.
        let resumed = call_with_args("mutation-after-takeover", "bash", &args);
        register_tool_call(&backend, &resumed, &args).await;
        let out = backend
            .execute(&resumed, &args, &ctx)
            .await
            .expect("uncertainty is a system-owned waiting result, not a fatal/user handoff");
        assert!(matches!(out.status, ToolExecutionStatus::Waiting));
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("external_state_uncertain")
        );
        let content = std::fs::read_to_string(dir.path().join("effect.log")).unwrap();
        assert_eq!(content.lines().collect::<Vec<_>>(), vec!["once"]);
    }

    #[tokio::test]
    async fn waiting_objective_without_its_current_mutation_permit_cannot_launch() {
        let backend = objective_backend(true).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let stale = call_with_args("mutation-from-stale-runner", "bash", &args);
        register_tool_call(&backend, &stale, &args).await;

        // This context identifies the Objective but carries no owner/epoch
        // permit. The durable row is already claimed by a replacement owner.
        let out = backend
            .execute(&stale, &args, &objective_ctx(dir.path()))
            .await
            .expect("a fenced mutation becomes system-owned waiting");
        assert!(matches!(out.status, ToolExecutionStatus::Waiting));
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("mutation_permit_lost")
        );
        assert!(
            !dir.path().join("effect.log").exists(),
            "the stale runner must be fenced before external dispatch"
        );
    }

    #[tokio::test]
    async fn waiting_objective_with_current_permit_executes_and_commits_receipt() {
        let backend = objective_backend(true).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let call = call_with_args("mutation-current-permit", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let mut ctx = objective_ctx(dir.path());
        ctx.mutation_permit = Some(current_permit(2));

        let out = backend.execute(&call, &args, &ctx).await.unwrap();
        assert_eq!(out.status, ToolExecutionStatus::Done);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("effect.log"))
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            vec!["once"]
        );
        let status: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE objective_id=?")
                .bind(TEST_OBJECTIVE_ID)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(status, "committed");
    }

    #[tokio::test]
    async fn stale_epoch_is_fenced_even_when_owner_string_is_unchanged() {
        let backend = objective_backend(true).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let call = call_with_args("mutation-stale-same-owner", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let mut ctx = objective_ctx(dir.path());
        ctx.mutation_permit = Some(current_permit(1));

        let out = backend.execute(&call, &args, &ctx).await.unwrap();
        assert_eq!(out.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("mutation_permit_lost")
        );
        assert!(!dir.path().join("effect.log").exists());
        let receipts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts WHERE objective_id=?")
                .bind(TEST_OBJECTIVE_ID)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(receipts, 0);
    }

    #[tokio::test]
    async fn expired_current_permit_is_fenced_before_dispatch() {
        let backend = objective_backend(true).await;
        let expired = chrono::Utc::now().timestamp_millis() - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=?")
            .bind(expired)
            .execute(&backend.db)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(TEST_OBJECTIVE_ID)
            .execute(&backend.db)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let call = call_with_args("mutation-expired-permit", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let mut ctx = objective_ctx(dir.path());
        ctx.mutation_permit = Some(current_permit(2));

        let out = backend.execute(&call, &args, &ctx).await.unwrap();
        assert_eq!(out.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("mutation_permit_lost")
        );
        assert!(!dir.path().join("effect.log").exists());
    }
}
