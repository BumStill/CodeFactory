// SPDX-License-Identifier: Apache-2.0
//! Structured, append-only execution-route updates for long chat turns.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
#[cfg(not(test))]
use tauri::Emitter;
use uuid::Uuid;

use super::{ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

const MAX_PLAN_STEPS: usize = 8;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct UpdatePlanArgs {
    steps: Vec<PlanStep>,
    #[serde(default)]
    explanation: Option<String>,
    #[serde(default)]
    waiting_reason: Option<String>,
    #[serde(default)]
    next_action_owner: codefactory_agent_loop::types::NextActionOwner,
    #[serde(default)]
    change_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct PlanStep {
    id: String,
    title: String,
    kind: String,
    status: String,
    #[serde(default)]
    external_job_id: Option<String>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "update_plan".into(),
            description: "Publish or update the bounded execution route for an approved non-trivial chat task. Call before the first operational tool, whenever a step starts/completes, when work is waiting, and whenever the step set/order changes. This is UI state, not a user message. Do not provide percentages or ETA.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "minItems": 2,
                        "maxItems": MAX_PLAN_STEPS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Stable short id within this root turn" },
                                "title": { "type": "string", "description": "Concise user-facing step title" },
                                "kind": {
                                    "type": "string",
                                    "enum": ["analysis", "implementation", "verification", "delivery", "external_job", "other"]
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                },
                                "external_job_id": {
                                    "type": ["string", "null"],
                                    "description": "Existing task/job id only when kind=external_job"
                                }
                            },
                            "required": ["id", "title", "kind", "status"],
                            "additionalProperties": false
                        }
                    },
                    "explanation": { "type": ["string", "null"] },
                    "waiting_reason": { "type": ["string", "null"], "description": "Concrete current wait; null when not waiting" },
                    "next_action_owner": {
                        "type": "string",
                        "enum": ["system", "external", "user"],
                        "default": "system",
                        "description": "Who owns the next action while waiting. Use user only for an explicit human action; never infer it from waiting_reason text."
                    },
                    "change_reason": { "type": ["string", "null"], "description": "Required when an existing plan changes step ids, titles, kinds, or order" }
                },
                "required": ["steps"],
                "additionalProperties": false
            }),
        },
    }
}

fn non_empty(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|text| !text.trim().is_empty())
}

fn validate(args: &UpdatePlanArgs) -> std::result::Result<(), String> {
    if !(2..=MAX_PLAN_STEPS).contains(&args.steps.len()) {
        return Err(format!("update_plan requires 2-{MAX_PLAN_STEPS} steps"));
    }
    let mut ids = HashSet::new();
    let mut active = 0;
    for step in &args.steps {
        if step.id.trim().is_empty() || step.title.trim().is_empty() {
            return Err("every plan step needs a non-empty id and title".into());
        }
        if !ids.insert(step.id.as_str()) {
            return Err(format!("duplicate plan step id '{}'", step.id));
        }
        if !matches!(
            step.kind.as_str(),
            "analysis" | "implementation" | "verification" | "delivery" | "external_job" | "other"
        ) {
            return Err(format!("unsupported plan step kind '{}'", step.kind));
        }
        if !matches!(
            step.status.as_str(),
            "pending" | "in_progress" | "completed"
        ) {
            return Err(format!("unsupported plan step status '{}'", step.status));
        }
        if step.status == "in_progress" {
            active += 1;
        }
        if step.kind != "external_job" && step.external_job_id.is_some() {
            return Err("external_job_id is valid only for external_job steps".into());
        }
    }
    if active > 1 {
        return Err("at most one plan step may be in_progress".into());
    }
    Ok(())
}

fn structure_changed(previous: &[PlanStep], next: &[PlanStep]) -> bool {
    previous
        .iter()
        .map(|step| (&step.id, &step.title, &step.kind, &step.external_job_id))
        .ne(next
            .iter()
            .map(|step| (&step.id, &step.title, &step.kind, &step.external_job_id)))
}

fn sanitize(args: &mut UpdatePlanArgs) {
    for step in &mut args.steps {
        step.id = crate::trajectory::redact_text(step.id.trim(), 80);
        step.title = crate::trajectory::redact_text(step.title.trim(), 160);
        step.external_job_id = step
            .external_job_id
            .take()
            .map(|value| crate::trajectory::redact_text(value.trim(), 160));
    }
    args.explanation = args
        .explanation
        .take()
        .map(|value| crate::trajectory::redact_text(value.trim(), 500));
    args.waiting_reason = args
        .waiting_reason
        .take()
        .map(|value| crate::trajectory::redact_text(value.trim(), 500));
    args.change_reason = args
        .change_reason
        .take()
        .map(|value| crate::trajectory::redact_text(value.trim(), 500));
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let mut args: UpdatePlanArgs = serde_json::from_value(args)?;
    if let Err(message) = validate(&args) {
        return Ok(ToolOutput::err(message));
    }
    let Some(db) = ctx.db.clone() else {
        return Ok(ToolOutput::err("update_plan requires persisted chat state"));
    };
    let Some(session_id) = ctx.session_id.as_deref() else {
        return Ok(ToolOutput::err("update_plan requires a current session"));
    };
    let Some(root_turn_id) = ctx.root_turn_id.as_deref() else {
        return Ok(ToolOutput::err(
            "update_plan requires a real user root turn",
        ));
    };
    #[cfg(not(test))]
    let Some(app) = ctx.app.clone() else {
        return Ok(ToolOutput::err("update_plan is unavailable in this runtime"));
    };

    sanitize(&mut args);
    if let Err(message) = validate(&args) {
        return Ok(ToolOutput::err(message));
    }

    let mut tx = db.begin().await?;
    let previous: Option<(i64, String)> = sqlx::query_as(
        "SELECT revision, plan_json FROM chat_plan_events
         WHERE session_id = ? AND root_turn_id = ?
         ORDER BY revision DESC LIMIT 1",
    )
    .bind(session_id)
    .bind(root_turn_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((_, previous_json)) = &previous {
        let previous_steps: Vec<PlanStep> = serde_json::from_str(previous_json).unwrap_or_default();
        if structure_changed(&previous_steps, &args.steps) && !non_empty(&args.change_reason) {
            return Ok(ToolOutput::err(
                "change_reason is required when plan steps or order change",
            ));
        }
    }

    let revision = previous.map_or(1, |(revision, _)| revision + 1);
    let created_at = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO chat_plan_events
         (id, session_id, root_turn_id, revision, plan_json, explanation,
          waiting_reason, next_action_owner, change_reason, created_at)
         VALUES (?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(session_id)
    .bind(root_turn_id)
    .bind(revision)
    .bind(serde_json::to_string(&args.steps)?)
    .bind(&args.explanation)
    .bind(&args.waiting_reason)
    .bind(args.next_action_owner.as_str())
    .bind(&args.change_reason)
    .bind(created_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let steps: Vec<codefactory_agent_loop::types::PlanStepEvent> = args
        .steps
        .iter()
        .map(|step| codefactory_agent_loop::types::PlanStepEvent {
            id: step.id.clone(),
            title: step.title.clone(),
            kind: step.kind.clone(),
            status: step.status.clone(),
            external_job_id: step.external_job_id.clone(),
        })
        .collect();
    #[cfg(not(test))]
    {
        let event_name = format!("stream:{session_id}");
        app.emit(
            &event_name,
            codefactory_agent_loop::types::StreamEvent::PlanUpdated {
                root_turn_id: root_turn_id.to_string(),
                revision,
                steps,
                explanation: args.explanation,
                waiting_reason: args.waiting_reason,
                next_action_owner: args.next_action_owner,
                change_reason: args.change_reason,
                created_at,
            },
        )
        .ok();
    }
    #[cfg(test)]
    let _ = steps;

    Ok(ToolOutput::ok(format!("Plan revision {revision} saved.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, status: &str) -> PlanStep {
        PlanStep {
            id: id.into(),
            title: id.into(),
            kind: "implementation".into(),
            status: status.into(),
            external_job_id: None,
        }
    }

    #[test]
    fn structured_plan_rejects_multiple_active_steps() {
        let args = UpdatePlanArgs {
            steps: vec![step("a", "in_progress"), step("b", "in_progress")],
            explanation: None,
            waiting_reason: None,
            next_action_owner: Default::default(),
            change_reason: None,
        };
        assert_eq!(
            validate(&args).unwrap_err(),
            "at most one plan step may be in_progress"
        );
    }

    #[test]
    fn structural_change_is_detected_independently_of_status() {
        let before = vec![step("a", "in_progress"), step("b", "pending")];
        let same = vec![step("a", "completed"), step("b", "in_progress")];
        let changed = vec![step("a", "completed"), step("c", "in_progress")];
        assert!(!structure_changed(&before, &same));
        assert!(structure_changed(&before, &changed));
    }

    #[test]
    fn sanitized_step_ids_remain_unique() {
        let common_prefix = "a".repeat(80);
        let mut args = UpdatePlanArgs {
            steps: vec![
                step(&format!("{common_prefix}x"), "in_progress"),
                step(&format!("{common_prefix}y"), "pending"),
            ],
            explanation: None,
            waiting_reason: None,
            next_action_owner: Default::default(),
            change_reason: None,
        };
        assert!(validate(&args).is_ok());

        sanitize(&mut args);

        assert!(validate(&args).is_err());
    }

    #[test]
    fn update_plan_schema_exposes_structured_next_action_owner() {
        let parameters = definition().function.parameters;
        assert_eq!(
            parameters["properties"]["next_action_owner"]["enum"],
            json!(["system", "external", "user"]),
        );
    }

    #[test]
    fn missing_next_action_owner_fails_safe_to_system() {
        let args: UpdatePlanArgs = serde_json::from_value(json!({
            "steps": [
                {"id": "a", "title": "a", "kind": "analysis", "status": "completed"},
                {"id": "b", "title": "b", "kind": "verification", "status": "pending"}
            ],
            "waiting_reason": "需要检查权限配置"
        }))
        .expect("legacy arguments remain valid");

        assert_eq!(
            args.next_action_owner,
            codefactory_agent_loop::types::NextActionOwner::System,
        );
    }
}
