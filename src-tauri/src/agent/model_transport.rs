// SPDX-License-Identifier: Apache-2.0
//! Desktop model transport (keystone slice 4.5a).
//!
//! The OpenAI-family transport — `call_openai_transport` (dispatch +
//! required→auto fallback), `call_chatgpt_model` (ChatGPT Responses SSE), and
//! `call_openai_model` (OpenAI-compatible chat SSE) — extracted verbatim out of
//! `AgentLoop` into `DesktopModelTransport`. This is a PURE mechanical move
//! (zero behaviour change): inherent methods, NOT the agent-loop `ModelTransport`
//! trait yet (that is slice 4.5b). The reactive retries (context-overflow,
//! vision-strip) stay in the loop wrapping the call; the `max_tokens →
//! max_completion_tokens` adaptation moves with `call_openai_model` (it is
//! internal to it).
//!
//! The struct owns only the transport's slice of `AgentLoop` state — no
//! `AppHandle` (#166), no `db`/`settings` (DB-pure since 4.4d; `reasoning_effort`
//! is a pre-resolved `&str` param). `cancel` is the SAME `Arc<AtomicBool>` the
//! loop holds (cloned, not fresh), so mid-stream and post-stream cancellation
//! observe identical state. Shared helpers live in the parent `agent` module and
//! are reached via `super::` (this is a child module, so it sees them).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use codefactory_agent_core::{provider_rejects_required_tool_choice, sanitize_completion_summary};
use reqwest::Client;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use codefactory_agent_loop::transport::{
    EffectiveRoute, ModelResponse, ModelTransport, RoundOptions, RouteChange as LoopRouteChange,
    TransportError,
};

use super::events::EventSink;
use super::failover::{classify_provider_failure, ActiveRouteState, RouteCandidate};
use super::provider_recovery::{
    OverloadBudgetDecision, ProviderAttemptSpec, ProviderEpisodeSpec, ProviderMutation,
    ProviderOwnerPermit, ProviderRecoveryStore,
};
use super::{next_stream_item, openai_tool_controls, validate_openai_sse_completion, StreamPoll};
use crate::config::settings::ApiStyle;
use crate::errors::Result;
use crate::openrouter::types::{
    ChatMessage, ChatRequest, FunctionCall, StreamChunk, StreamEvent, StreamOptions, ToolCall,
    ToolDefinition, Usage,
};

/// The transport's slice of `AgentLoop` state. Built once per run by
/// `AgentLoop::model_transport` (clones only Arc/Client handles + small
/// Strings). Owns no `AppHandle` and no `db`/`settings`.
pub(super) struct DesktopModelTransport {
    pub(super) http: Client,
    pub(super) events: Arc<dyn EventSink>,
    pub(super) model_id: String,
    pub(super) session_id: String,
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) api_style: ApiStyle,
    pub(super) cancel: Option<Arc<AtomicBool>>,
    /// Internal sidecars such as session-title generation need a strict output
    /// ceiling. `None` preserves the interactive transport's existing limits.
    pub(super) max_output_tokens: Option<u32>,
    /// Metadata prompts must not be echoed from transient Provider bodies into
    /// logs or retry events. Interactive requests retain their diagnostics.
    pub(super) retry_response_body: crate::http_util::RetryResponseBody,
    /// Present only for Objective-bound interactive/recovery work. It owns the
    /// exact write-ahead provider attempt and fences every POST/output/commit.
    pub(super) provider_attempt: Option<ProviderAttemptRuntime>,
}

pub(super) struct RoutedDesktopModelTransport {
    pub(super) http: Client,
    pub(super) events: Arc<dyn EventSink>,
    pub(super) session_id: String,
    pub(super) route_state: ActiveRouteState,
    pub(super) cancel: Option<Arc<AtomicBool>>,
    pub(super) turn_output_started: Arc<AtomicBool>,
    pub(super) turn_side_effect_started: Arc<AtomicBool>,
    pub(super) db: SqlitePool,
    pub(super) root_turn_id: Option<String>,
    pub(super) mutation_permit: Option<codefactory_agent_loop::tool::MutationPermit>,
    pub(super) context_authorization: Option<super::context_recovery::ContextRecoveryAuthorization>,
    pub(super) anonymous: bool,
    /// Foreground/recovered chat must never silently fall back to unreceipted
    /// provider I/O. Legacy task/subagent surfaces remain optional until their
    /// scheduler owner is represented by the unified mutation permit.
    pub(super) durable_provider_required: bool,
}

#[derive(Clone)]
pub(super) struct ProviderAttemptRuntime {
    store: ProviderRecoveryStore,
    permit: ProviderOwnerPermit,
    attempt_id: String,
    post_admitted: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProviderFailureAction {
    RetrySafe,
    DurableWaiting,
}

struct TrackingEventSink {
    delegate: Arc<dyn EventSink>,
    output_started: Arc<AtomicBool>,
    turn_output_started: Arc<AtomicBool>,
    turn_side_effect_started: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl EventSink for TrackingEventSink {
    fn emit(&self, event: StreamEvent) {
        if matches!(
            event,
            StreamEvent::TextDelta { .. }
                | StreamEvent::ToolCallStart { .. }
                | StreamEvent::ToolResult { .. }
        ) {
            self.output_started.store(true, Ordering::SeqCst);
            self.turn_output_started.store(true, Ordering::SeqCst);
            if matches!(
                event,
                StreamEvent::ToolCallStart { .. } | StreamEvent::ToolResult { .. }
            ) {
                self.turn_side_effect_started.store(true, Ordering::SeqCst);
            }
        }
        self.delegate.emit(event);
    }

    fn usage_recorded(&self, session_id: &str) {
        self.delegate.usage_recorded(session_id);
    }
}

fn effective_route(route: &RouteCandidate) -> EffectiveRoute {
    EffectiveRoute {
        endpoint_name: route.endpoint_name.clone(),
        model_id: route.model_id.clone(),
        base_url: route.base_url.clone(),
        is_chatgpt: matches!(route.api_style, ApiStyle::Chatgpt),
    }
}

/// The ChatGPT subscription Codex backend rejects `max_output_tokens` on the
/// Responses route even though the public Responses API accepts that field.
/// Keep the local metadata marker on the transport, but do not put its output
/// ceiling on this wire contract. Metadata generation remains bounded by its
/// deadline, short-title prompt, and strict local output validation.
fn chatgpt_wire_output_ceiling(_local_ceiling: Option<u32>) -> Option<u32> {
    None
}

fn build_chatgpt_responses_body(
    model_id: &str,
    instructions: String,
    input: Vec<serde_json::Value>,
    tools: Vec<serde_json::Value>,
    require_tool: bool,
    reasoning_effort: &str,
    local_output_ceiling: Option<u32>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model_id,
        "instructions": instructions,
        "input": input,
        "tool_choice": if tools.is_empty() {
            "none"
        } else if require_tool {
            "required"
        } else {
            "auto"
        },
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
        "reasoning": { "effort": reasoning_effort, "summary": "auto" },
    });
    if let Some(ceiling) = chatgpt_wire_output_ceiling(local_output_ceiling) {
        body["max_output_tokens"] = serde_json::json!(ceiling);
    }
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }
    body
}

fn classify_transport_error(message: String) -> TransportError {
    if classify_provider_failure(&message).permits_endpoint_failover() {
        TransportError::Retryable(message)
    } else {
        TransportError::Fatal(message)
    }
}

fn provider_digest(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn provider_transport_error(context: &str, error: impl std::fmt::Display) -> TransportError {
    TransportError::Fatal(format!("{context}: {error}"))
}

fn require_provider_applied<T>(
    mutation: ProviderMutation<T>,
    rung: &str,
) -> std::result::Result<T, TransportError> {
    match mutation {
        ProviderMutation::Applied(value) => Ok(value),
        ProviderMutation::Fenced => Err(TransportError::Fatal(format!(
            "PROVIDER_OWNER_FENCED: durable owner changed before {rung}"
        ))),
    }
}

impl ProviderAttemptRuntime {
    pub(super) async fn admit_post(&self) -> std::result::Result<(), TransportError> {
        let mutation = self
            .store
            .mark_in_flight(
                &self.permit,
                &self.attempt_id,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|error| provider_transport_error("provider POST admission failed", error))?;
        require_provider_applied(mutation, "provider POST")?;
        self.post_admitted.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub(super) async fn checkpoint_text(
        &self,
        content: &str,
    ) -> std::result::Result<(), TransportError> {
        let mutation = self
            .store
            .append_partial_output(
                &self.permit,
                &self.attempt_id,
                content,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|error| {
                provider_transport_error("provider output checkpoint failed", error)
            })?;
        require_provider_applied(mutation, "provider output emit")?;
        Ok(())
    }

    pub(super) async fn checkpoint_output_event(&self) -> std::result::Result<(), TransportError> {
        let mutation = self
            .store
            .latch_output_event(
                &self.permit,
                &self.attempt_id,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|error| provider_transport_error("provider output latch failed", error))?;
        require_provider_applied(mutation, "provider output emit")?;
        Ok(())
    }

    async fn commit_response(
        &self,
        response: &ModelResponse,
    ) -> std::result::Result<(), TransportError> {
        let tool_identity = response
            .tool_calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "id": call.id,
                    "name": call.function.name,
                    "arguments_digest": provider_digest(&[call.function.arguments.as_bytes()]),
                })
            })
            .collect::<Vec<_>>();
        let response_identity = serde_json::to_vec(&serde_json::json!({
            "text_digest": provider_digest(&[response.text.as_bytes()]),
            "tools": tool_identity,
            "reasoning_digest": response.reasoning.as_deref().map(|value| provider_digest(&[value.as_bytes()])),
        }))
        .map_err(|error| provider_transport_error("provider response digest failed", error))?;
        let response_digest = provider_digest(&[&response_identity]);
        let mutation = self
            .store
            .commit_response(
                &self.permit,
                &self.attempt_id,
                &response_digest,
                &response.text,
                false,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|error| provider_transport_error("provider response commit failed", error))?;
        require_provider_applied(mutation, "provider response commit")?;
        Ok(())
    }

    async fn settle_failure(
        &self,
        error: &TransportError,
    ) -> std::result::Result<ProviderFailureAction, TransportError> {
        let text = error.message();
        let overloaded = codefactory_agent_loop::context::is_provider_overloaded(text);
        let explicit_response = text.to_ascii_lowercase().contains("http ")
            || text.contains("后端请求失败（")
            || text.contains("Bad Request");
        let replayable = !self.post_admitted.load(Ordering::SeqCst) || explicit_response;
        let (failure_class, failure_code) = if overloaded {
            ("provider_overload", "provider_overloaded")
        } else if text.to_ascii_lowercase().contains("auth")
            || text.contains("401")
            || text.contains("credential")
        {
            ("provider_auth", "provider_auth_unavailable")
        } else if replayable {
            ("provider_rejected", "provider_request_rejected")
        } else {
            ("provider_transport", "provider_external_state_uncertain")
        };
        let mutation = self
            .store
            .record_failure(
                &self.permit,
                &self.attempt_id,
                failure_class,
                failure_code,
                replayable,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|store_error| {
                provider_transport_error("provider failure receipt failed", store_error)
            })?;
        let decision = require_provider_applied(mutation, "provider failure settle")?;
        Ok(match decision {
            OverloadBudgetDecision::RetryAfter { .. } => ProviderFailureAction::RetrySafe,
            OverloadBudgetDecision::DurableWaiting { .. } => ProviderFailureAction::DurableWaiting,
        })
    }
}

impl RoutedDesktopModelTransport {
    async fn resolve_provider_owner(
        &self,
    ) -> std::result::Result<Option<ProviderOwnerPermit>, TransportError> {
        if self.anonymous {
            return Ok(None);
        }
        let Some(root_turn_id) = self.root_turn_id.as_deref() else {
            if self.durable_provider_required {
                return Err(TransportError::Fatal(
                    "PROVIDER_DURABLE_IDENTITY_MISSING: chat has no root turn".into(),
                ));
            }
            return Ok(None);
        };

        if let Some(permit) = self.mutation_permit.as_ref() {
            let Some(binding_id) = permit.binding_id.as_deref() else {
                return Err(TransportError::Fatal(
                    "PROVIDER_DURABLE_IDENTITY_MISSING: remediation has no binding".into(),
                ));
            };
            let Some(resource_generation) = permit.resource_generation else {
                return Err(TransportError::Fatal(
                    "PROVIDER_DURABLE_IDENTITY_MISSING: remediation has no binding generation"
                        .into(),
                ));
            };
            let row = sqlx::query(
                "SELECT o.revision, o.session_id,
                        COALESCE(NULLIF(o.resume_cursor, ''), o.root_turn_id) AS active_root_turn_id
                 FROM objectives o
                 JOIN objective_bindings b ON b.id=? AND b.objective_id=o.id
                 WHERE o.id=? AND b.resource_generation=?
                   AND b.resource_kind='chat_root_turn'
                   AND b.resource_id=COALESCE(NULLIF(o.resume_cursor, ''), o.root_turn_id)",
            )
            .bind(binding_id)
            .bind(&permit.objective_id)
            .bind(resource_generation)
            .fetch_optional(&self.db)
            .await
            .map_err(|error| provider_transport_error("resolve provider remediation", error))?
            .ok_or_else(|| {
                TransportError::Fatal(
                    "PROVIDER_DURABLE_IDENTITY_MISSING: remediation binding disappeared".into(),
                )
            })?;
            let session_id: Option<String> = row.get("session_id");
            let objective_root: Option<String> = row.get("active_root_turn_id");
            if session_id.as_deref() != Some(self.session_id.as_str())
                || objective_root.as_deref() != Some(root_turn_id)
            {
                return Err(TransportError::Fatal(
                    "PROVIDER_DURABLE_IDENTITY_MISMATCH: remediation session/root changed".into(),
                ));
            }
            return Ok(Some(ProviderOwnerPermit::remediation(
                &permit.objective_id,
                row.get::<i64, _>("revision"),
                binding_id,
                resource_generation,
                &permit.remediation_id,
                &permit.owner,
                permit.claim_epoch,
            )));
        }

        let row = sqlx::query(
            "SELECT o.id AS objective_id, o.revision, b.id AS binding_id,
                    b.resource_generation, c.run_instance_id
             FROM chat_run_controls c
             JOIN objectives o ON o.id=c.objective_id AND o.revision=c.objective_revision
             JOIN objective_bindings b ON b.objective_id=o.id
                AND b.resource_kind='chat_root_turn' AND b.resource_id=c.root_turn_id
             WHERE c.session_id=? AND c.root_turn_id=? AND c.status='active'
               AND o.session_id=c.session_id AND o.root_turn_id=c.root_turn_id
               AND o.status='active'
             ORDER BY b.resource_generation DESC LIMIT 2",
        )
        .bind(&self.session_id)
        .bind(root_turn_id)
        .fetch_all(&self.db)
        .await
        .map_err(|error| provider_transport_error("resolve foreground provider owner", error))?;
        if row.len() != 1 {
            if self.durable_provider_required {
                return Err(TransportError::Fatal(format!(
                    "PROVIDER_DURABLE_IDENTITY_MISSING: expected one active chat binding, found {}",
                    row.len()
                )));
            }
            return Ok(None);
        }
        let row = &row[0];
        let revision = row.get::<i64, _>("revision");
        Ok(Some(ProviderOwnerPermit::chat_run(
            row.get::<String, _>("objective_id"),
            revision,
            row.get::<String, _>("binding_id"),
            row.get::<i64, _>("resource_generation"),
            row.get::<String, _>("run_instance_id"),
            revision,
        )))
    }

    async fn prepare_provider_attempt(
        &self,
        route: &RouteCandidate,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        opts: &RoundOptions,
    ) -> std::result::Result<Option<ProviderAttemptRuntime>, TransportError> {
        if let Some(authorization) = self.context_authorization.as_ref() {
            let Some(permit) = self.mutation_permit.as_ref() else {
                return Err(TransportError::Fatal(
                    "CONTEXT_RECOVERY_FENCED: durable authorization has no mutation permit".into(),
                ));
            };
            let current = super::context_recovery::ContextRecoveryStore::new(self.db.clone())
                .authorization_is_current(authorization, permit)
                .await
                .map_err(|error| {
                    provider_transport_error("context provider admission failed", error)
                })?;
            if !current {
                return Err(TransportError::Fatal(
                    "CONTEXT_RECOVERY_FENCED: objective or cursor changed before provider request"
                        .into(),
                ));
            }
        }
        let Some(permit) = self.resolve_provider_owner().await? else {
            return Ok(None);
        };
        let latches = sqlx::query(
            "SELECT b.output_started, b.side_effect_started,
                    o.output_started AS objective_output_started,
                    o.side_effect_started AS objective_side_effect_started
             FROM objective_bindings b
             JOIN objectives o ON o.id=b.objective_id
             WHERE b.id=? AND b.objective_id=? AND b.resource_generation=?",
        )
        .bind(permit.binding_id())
        .bind(permit.objective_id())
        .bind(permit.resource_generation())
        .fetch_one(&self.db)
        .await
        .map_err(|error| provider_transport_error("load provider safety latches", error))?;
        if latches.get::<i64, _>("output_started") != 0
            || latches.get::<i64, _>("objective_output_started") != 0
        {
            self.turn_output_started.store(true, Ordering::SeqCst);
        }
        if latches.get::<i64, _>("side_effect_started") != 0
            || latches.get::<i64, _>("objective_side_effect_started") != 0
        {
            self.turn_side_effect_started.store(true, Ordering::SeqCst);
        }
        let root_turn_id = self.root_turn_id.as_deref().ok_or_else(|| {
            TransportError::Fatal("PROVIDER_DURABLE_IDENTITY_MISSING: root turn missing".into())
        })?;
        let candidate_snapshot = self
            .route_state
            .candidate_identity_snapshot()
            .into_iter()
            .map(|(endpoint, model)| serde_json::json!({"endpoint": endpoint, "model": model}))
            .collect::<Vec<_>>();
        let candidate_snapshot_json =
            serde_json::to_string(&candidate_snapshot).map_err(|error| {
                provider_transport_error("serialize provider route snapshot", error)
            })?;
        let candidate_snapshot_digest = provider_digest(&[candidate_snapshot_json.as_bytes()]);
        let policy =
            sqlx::query_scalar::<_, String>("SELECT model_policy FROM sessions WHERE id=?")
                .bind(&self.session_id)
                .fetch_optional(&self.db)
                .await
                .map_err(|error| provider_transport_error("load provider route policy", error))?
                .filter(|policy| matches!(policy.as_str(), "fixed" | "prefer" | "auto"))
                .unwrap_or_else(|| "fixed".into());
        let episode_material = format!(
            "{}\0{}\0{}\0{}\0{}",
            permit.objective_id(),
            permit.objective_revision(),
            permit.binding_id(),
            permit.resource_generation(),
            candidate_snapshot_digest
        );
        let episode_hash = provider_digest(&[episode_material.as_bytes()]);
        let episode_id = format!(
            "provider-episode-{}",
            episode_hash.trim_start_matches("sha256:")
        );
        let store = ProviderRecoveryStore::new(self.db.clone());
        let episode = ProviderEpisodeSpec {
            id: episode_id.clone(),
            session_id: self.session_id.clone(),
            root_turn_id: root_turn_id.to_string(),
            policy,
            candidate_snapshot_digest,
            candidate_snapshot_json,
            resume_cursor: root_turn_id.to_string(),
        };
        let opened = store
            .open_episode(&permit, &episode, chrono::Utc::now().timestamp_millis())
            .await
            .map_err(|error| provider_transport_error("open provider episode", error))?;
        require_provider_applied(opened, "provider episode open")?;

        let request_identity = serde_json::to_vec(&serde_json::json!({
            "endpoint": route.endpoint_name,
            "model": route.model_id,
            "messages": messages,
            "tools": tools,
            "require_tool": opts.require_tool,
            "reasoning_effort": opts.reasoning_effort,
            "tool_outcomes_so_far": opts.tool_outcomes_so_far,
        }))
        .map_err(|error| provider_transport_error("digest provider request", error))?;
        let attempt_id = Uuid::new_v4().to_string();
        let attempt = ProviderAttemptSpec {
            id: attempt_id.clone(),
            episode_id,
            endpoint: route.endpoint_name.clone(),
            model: route.model_id.clone(),
            request_digest: provider_digest(&[&request_identity]),
            resume_cursor: root_turn_id.to_string(),
        };
        let begun = store
            .begin_attempt(&permit, &attempt, chrono::Utc::now().timestamp_millis())
            .await
            .map_err(|error| provider_transport_error("prepare provider attempt", error))?;
        require_provider_applied(begun, "provider attempt prepare")?;
        Ok(Some(ProviderAttemptRuntime {
            store,
            permit,
            attempt_id,
            post_admitted: Arc::new(AtomicBool::new(false)),
        }))
    }

    async fn transport_for(
        &self,
        route: &RouteCandidate,
        output_started: Arc<AtomicBool>,
        provider_attempt: Option<ProviderAttemptRuntime>,
    ) -> std::result::Result<DesktopModelTransport, TransportError> {
        let api_key = if matches!(route.api_style, ApiStyle::Chatgpt) {
            String::new()
        } else if let Some(api_key) = route.legacy_inline_api_key.as_ref() {
            api_key.clone()
        } else if let Some(key_ref) = route.credential_ref.as_deref() {
            match crate::credential_broker::CredentialBroker::global()
                .get(key_ref)
                .await
            {
                Ok(Some(secret)) if !secret.trim().is_empty() => secret,
                Ok(_) => {
                    return Err(TransportError::Retryable(format!(
                        "AUTH_MISSING: {} 尚未配置凭据，请在模型设置中保存后重试",
                        route.endpoint_name
                    )))
                }
                Err(error) => {
                    let action = match error.kind {
                        crate::credential_broker::CredentialErrorKind::Unavailable => {
                            "系统仍在等待密钥访问授权"
                        }
                        crate::credential_broker::CredentialErrorKind::Store => {
                            "系统未能读取已保存的密钥"
                        }
                    };
                    return Err(TransportError::Retryable(format!(
                        "CREDENTIAL_ACCESS_REQUIRED: {}。请允许一次系统密钥访问，或在模型设置中重新保存该端点凭据",
                        action
                    )));
                }
            }
        } else {
            return Err(TransportError::Retryable(format!(
                "AUTH_MISSING: {} 尚未配置凭据，请在模型设置中保存后重试",
                route.endpoint_name
            )));
        };

        Ok(DesktopModelTransport {
            http: self.http.clone(),
            events: Arc::new(TrackingEventSink {
                delegate: self.events.clone(),
                output_started,
                turn_output_started: self.turn_output_started.clone(),
                turn_side_effect_started: self.turn_side_effect_started.clone(),
            }),
            model_id: route.model_id.clone(),
            session_id: self.session_id.clone(),
            base_url: route.base_url.clone(),
            api_key,
            api_style: route.api_style.clone(),
            cancel: self.cancel.clone(),
            max_output_tokens: None,
            retry_response_body: crate::http_util::RetryResponseBody::Include,
            provider_attempt,
        })
    }
}

impl DesktopModelTransport {
    /// Mirrors `AgentLoop::is_cancelled` exactly, off the SHARED cancel flag —
    /// so the cancellation-skips-SSE-validation invariant is byte-identical.
    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    fn provider_http_attempt_budget(&self) -> usize {
        if self.provider_attempt.is_some() {
            1
        } else {
            3
        }
    }

    async fn admit_provider_post(&self) -> Result<()> {
        if let Some(attempt) = self.provider_attempt.as_ref() {
            if attempt.post_admitted.load(Ordering::SeqCst) {
                return Err(crate::errors::AppError::Other(
                    "PROVIDER_REQUEST_REWRITE_REQUIRES_NEW_ATTEMPT: a durable attempt admits exactly one POST"
                        .into(),
                ));
            }
            attempt
                .admit_post()
                .await
                .map_err(|error| crate::errors::AppError::Other(error.to_string()))?;
        }
        Ok(())
    }

    async fn checkpoint_provider_text(&self, text: &str) -> Result<()> {
        if let Some(attempt) = self.provider_attempt.as_ref() {
            attempt
                .checkpoint_text(text)
                .await
                .map_err(|error| crate::errors::AppError::Other(error.to_string()))?;
        }
        Ok(())
    }

    pub(super) async fn call_openai_transport(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        require_tool: bool,
        reasoning_effort: &str,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>, Option<String>)> {
        let first = match self.api_style {
            ApiStyle::Chatgpt => {
                self.call_chatgpt_model(messages, tool_defs, require_tool, reasoning_effort)
                    .await
            }
            _ => {
                self.call_openai_model(messages, tool_defs, require_tool, reasoning_effort)
                    .await
            }
        };
        let required_choice_unsupported = first.as_ref().err().is_some_and(|error| {
            require_tool && provider_rejects_required_tool_choice(&error.to_string())
        });
        if !required_choice_unsupported {
            return first;
        }
        match self.api_style {
            ApiStyle::Chatgpt => {
                self.call_chatgpt_model(messages, tool_defs, false, reasoning_effort)
                    .await
            }
            _ => {
                self.call_openai_model(messages, tool_defs, false, reasoning_effort)
                    .await
            }
        }
    }

    async fn call_chatgpt_model(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        require_tool: bool,
        // Pre-resolved by the loop once per round (keystone slice 4.4d), so this
        // transport reads no DB — a step toward the DB-pure ModelTransport (4.5).
        reasoning_effort: &str,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>, Option<String>)> {
        let finalization_response = tool_defs.is_empty();
        let reasoning_effort = normalize_chatgpt_reasoning_effort(reasoning_effort);

        let (mut access_token, mut account_id) = crate::codex_auth::valid_access_token().await?;
        // The ChatGPT backend URL is fixed — use the canonical constant rather
        // than the endpoint's base_url so the request always lands correctly.
        let url = format!("{}/responses", crate::codex_auth::CHATGPT_BASE_URL);

        // ── ChatMessage history → Responses instructions + input items ──
        let mut instructions = String::new();
        let mut input: Vec<serde_json::Value> = Vec::new();
        for m in messages {
            match m.role.as_str() {
                "system" => instructions = super::AgentLoop::content_to_text(&m.content),
                "tool" => input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": super::AgentLoop::content_to_text(&m.content),
                })),
                "assistant" => {
                    let text = super::AgentLoop::content_to_text(&m.content);
                    if !text.is_empty() {
                        input.push(serde_json::json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}],
                        }));
                    }
                    if let Some(tcs) = &m.tool_calls {
                        for tc in tcs {
                            input.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": tc.id,
                                "name": tc.function.name,
                                "arguments": tc.function.arguments,
                            }));
                        }
                    }
                }
                _ => input.push(serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": super::AgentLoop::content_to_chatgpt_user_parts(&m.content),
                })),
            }
        }

        // Tools → Responses shape (function fields flattened, no "function" nest).
        let tools: Vec<serde_json::Value> = tool_defs
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": t.function.parameters,
                })
            })
            .collect();

        let body = build_chatgpt_responses_body(
            &self.model_id,
            instructions,
            input,
            tools,
            require_tool,
            reasoning_effort,
            self.max_output_tokens,
        );

        self.admit_provider_post().await?;
        let mut response = crate::http_util::send_with_attempt_budget_and_notify_policy(
            "ChatGPT Responses stream request",
            || {
                let mut request = self
                    .http
                    .post(&url)
                    .bearer_auth(&access_token)
                    .header("OpenAI-Beta", "responses=experimental")
                    .header("originator", "codex_cli_rs")
                    .header("session_id", &self.session_id)
                    .header("Accept", "text/event-stream")
                    .json(&body);
                if let Some(acct) = &account_id {
                    request = request.header("chatgpt-account-id", acct.as_str());
                }
                request
            },
            |notice| super::AgentLoop::emit_transport_retry(self.events.as_ref(), notice),
            self.retry_response_body,
            self.provider_http_attempt_budget(),
        )
        .await?;
        if response.status().as_u16() == 401 && self.provider_attempt.is_some() {
            crate::codex_auth::mark_auth_expired();
            return Err(crate::errors::AppError::Other(
                "AUTH_EXPIRED: ChatGPT 授权已过期；当前 POST 已入账，刷新后由 Auth/Provider 对账续接"
                    .into(),
            ));
        }
        if response.status().as_u16() == 401 {
            (access_token, account_id) = crate::codex_auth::force_refresh_access_token().await?;
            response = crate::http_util::send_with_attempt_budget_and_notify_policy(
                "ChatGPT Responses stream request after forced token refresh",
                || {
                    let mut request = self
                        .http
                        .post(&url)
                        .bearer_auth(&access_token)
                        .header("OpenAI-Beta", "responses=experimental")
                        .header("originator", "codex_cli_rs")
                        .header("session_id", &self.session_id)
                        .header("Accept", "text/event-stream")
                        .json(&body);
                    if let Some(acct) = &account_id {
                        request = request.header("chatgpt-account-id", acct.as_str());
                    }
                    request
                },
                |notice| super::AgentLoop::emit_transport_retry(self.events.as_ref(), notice),
                self.retry_response_body,
                self.provider_http_attempt_budget(),
            )
            .await?;
            if response.status().as_u16() == 401 {
                crate::codex_auth::mark_auth_expired();
                return Err(crate::errors::AppError::Other(
                    "AUTH_EXPIRED: ChatGPT 授权已过期，请重新验证后在原会话继续".into(),
                ));
            }
        }
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(crate::errors::AppError::Other(format!(
                "ChatGPT 后端请求失败（{status}）：{text}"
            )));
        }

        // ── Parse the Responses SSE stream ──
        let mut byte_stream = response.bytes_stream();
        let mut text_buf = String::new();
        let mut reasoning_buf = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut usage: Option<Usage> = None;
        let mut byte_buffer: Vec<u8> = Vec::with_capacity(4096);
        let mut saw_terminal_marker = false;
        let mut malformed_data_lines = 0_usize;

        'sse: loop {
            let chunk = match next_stream_item(&mut byte_stream, self.cancel.as_ref()).await {
                StreamPoll::Item(Some(chunk)) => chunk,
                StreamPoll::Item(None) | StreamPoll::Cancelled => break,
            };
            let bytes = chunk?;
            byte_buffer.extend_from_slice(&bytes);
            while let Some(nl) = byte_buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = byte_buffer.drain(..=nl).collect();
                let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
                let line = line.trim_end_matches('\r');
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    saw_terminal_marker = true;
                    byte_buffer.clear();
                    break 'sse;
                }
                let Ok(ev) = serde_json::from_str::<serde_json::Value>(data) else {
                    malformed_data_lines += 1;
                    continue;
                };
                match ev.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                    "response.output_text.delta" => {
                        if let Some(d) = ev.get("delta").and_then(|v| v.as_str()) {
                            if !d.is_empty() {
                                if !finalization_response {
                                    self.checkpoint_provider_text(d).await?;
                                    self.events.emit(StreamEvent::TextDelta {
                                        content: d.to_string(),
                                    });
                                }
                                text_buf.push_str(d);
                            }
                        }
                    }
                    "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                        if let Some(d) = ev.get("delta").and_then(|v| v.as_str()) {
                            reasoning_buf.push_str(d);
                        }
                    }
                    "response.output_item.done" => {
                        if let Some(item) = ev.get("item") {
                            if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                                let call_id = item
                                    .get("call_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let name = item
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let arguments = item
                                    .get("arguments")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if !call_id.is_empty() && !name.is_empty() {
                                    tool_calls.push(ToolCall {
                                        id: call_id,
                                        r#type: "function".into(),
                                        function: FunctionCall { name, arguments },
                                    });
                                }
                            }
                        }
                    }
                    "response.completed" => {
                        saw_terminal_marker = true;
                        if let Some(u) = ev.get("response").and_then(|r| r.get("usage")) {
                            let inp =
                                u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            let out =
                                u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                            usage = Some(Usage {
                                prompt_tokens: inp,
                                completion_tokens: out,
                                total_tokens: inp + out,
                                cost: u.get("cost").and_then(|v| v.as_f64()),
                                prompt_tokens_details: Some(
                                    crate::openrouter::types::PromptTokenDetails {
                                        cached_tokens: u
                                            .pointer("/input_tokens_details/cached_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                    },
                                ),
                                completion_tokens_details: Some(
                                    crate::openrouter::types::CompletionTokenDetails {
                                        reasoning_tokens: u
                                            .pointer("/output_tokens_details/reasoning_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0)
                                            as u32,
                                    },
                                ),
                            });
                        }
                    }
                    "response.failed" => {
                        let msg = ev
                            .get("response")
                            .and_then(|r| r.get("error"))
                            .and_then(|e| e.get("message"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("response.failed")
                            .to_string();
                        return Err(crate::errors::AppError::Other(format!(
                            "ChatGPT 后端返回错误：{msg}"
                        )));
                    }
                    _ => {}
                }
            }
        }

        if self.is_cancelled() {
            let reasoning = (!reasoning_buf.is_empty()).then_some(reasoning_buf);
            return Ok((text_buf, Vec::new(), usage, reasoning));
        }

        validate_openai_sse_completion(
            saw_terminal_marker,
            byte_buffer.len(),
            malformed_data_lines,
        )
        .map_err(crate::errors::AppError::Other)?;

        if finalization_response {
            if self.max_output_tokens.is_none() {
                text_buf = sanitize_completion_summary(&text_buf);
            }
            tool_calls.clear();
            if !text_buf.is_empty() {
                self.checkpoint_provider_text(&text_buf).await?;
            }
            self.events.emit(StreamEvent::TextDelta {
                content: text_buf.clone(),
            });
        }
        let reasoning = if reasoning_buf.is_empty() {
            None
        } else {
            Some(reasoning_buf)
        };
        Ok((text_buf, tool_calls, usage, reasoning))
    }

    async fn call_openai_model(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        require_tool: bool,
        reasoning_effort: &str,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>, Option<String>)> {
        let finalization_response = tool_defs.is_empty();
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // Strip OpenRouter-style "vendor/" prefix when talking to a direct
        // provider API. Defensive against ids that linger from earlier
        // OpenRouter use after the user switches endpoint.
        let outbound_model =
            crate::config::settings::normalize_model_id(&self.model_id, &self.base_url);

        let (tools, tool_choice) = openai_tool_controls(tool_defs, require_tool);
        let req = ChatRequest {
            model: outbound_model,
            messages: messages.to_vec(),
            tools,
            tool_choice: Some(tool_choice),
            stream: true,
            temperature: 0.2,
            max_tokens: self.max_output_tokens.unwrap_or(8192),
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
        };

        // Send the request as-is — including `max_tokens` + `temperature`. We do
        // NOT pre-rewrite by model name: providers and proxies routinely serve
        // GPT-5-named models that accept the legacy fields just fine, and forcing
        // `max_completion_tokens` (plus dropping `temperature`) on them breaks
        // chat — the regression introduced by the name-based v1.19.2 attempt and
        // reported as "1.15 worked, recent builds don't". We adapt REACTIVELY
        // below, only when the server itself rejects `max_tokens`.
        let mut body = serde_json::to_value(&req)?;

        // DeepSeek reasoning models (deepseek.com direct or deepseek/… via
        // OpenRouter) enable thinking and tune its strength through
        // `reasoning_effort` (low|high|max). Attach only for DeepSeek routes —
        // other OpenAI-compatible providers keep their exact payload.
        if let Some(patch) =
            deepseek_reasoning_body_patch(&self.base_url, &self.model_id, reasoning_effort)
        {
            if let Some(obj) = body.as_object_mut() {
                if let Some(extra) = patch.as_object() {
                    for (k, v) in extra {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        self.admit_provider_post().await?;
        let mut response = crate::http_util::send_with_attempt_budget_and_notify_policy(
            "OpenAI-compatible chat stream request",
            || {
                self.http
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .header("X-Title", "CodeFactory")
                    .json(&body)
            },
            |notice| super::AgentLoop::emit_transport_retry(self.events.as_ref(), notice),
            self.retry_response_body,
            self.provider_http_attempt_budget(),
        )
        .await?;

        // Reactive safety net for the GPT-5 / o-series `max_tokens` rejection.
        // The name-based adaptation above handles the common ids, but providers,
        // proxies and Azure deployments expose these models under names we can't
        // anticipate (`gpt5`, custom aliases, deployment ids). When the server
        // itself answers 400 "use 'max_completion_tokens' instead", honor it
        // once — regardless of model name — and resend. Makes the fix
        // name-independent so it can't silently miss a model.
        if response.status().as_u16() == 400
            && body.get("max_tokens").is_some()
            && self.provider_attempt.is_some()
        {
            let err_text = response.text().await.unwrap_or_default();
            return Err(crate::errors::AppError::Other(format!(
                "HTTP 400 Bad Request: durable provider attempt will not rewrite and replay: {err_text}"
            )));
        }
        if response.status().as_u16() == 400 && body.get("max_tokens").is_some() {
            let err_text = response.text().await.unwrap_or_default();
            if err_text.contains("max_completion_tokens") {
                crate::config::settings::force_max_completion_tokens_with_minimum(
                    &mut body,
                    self.max_output_tokens.map(u64::from).unwrap_or(8192),
                );
                response = crate::http_util::send_with_attempt_budget_and_notify_policy(
                    "OpenAI-compatible chat stream request after max_tokens adaptation",
                    || {
                        self.http
                            .post(&url)
                            .bearer_auth(&self.api_key)
                            .header("X-Title", "CodeFactory")
                            .json(&body)
                    },
                    |notice| super::AgentLoop::emit_transport_retry(self.events.as_ref(), notice),
                    self.retry_response_body,
                    self.provider_http_attempt_budget(),
                )
                .await?;
            } else {
                // A different 400 — surface the provider's real reason.
                return Err(crate::errors::AppError::Other(format!(
                    "HTTP 400 Bad Request: {err_text}"
                )));
            }
        }
        // Capture the response body on HTTP errors so the user sees the
        // provider's actual rejection reason (bad model id, unsupported
        // field, etc.) rather than just "HTTP 400".
        let response = crate::http_util::check_status(response).await?;

        let mut byte_stream = response.bytes_stream();
        let mut text_buf = String::new();
        let mut reasoning_buf = String::new();
        let mut tc_map: HashMap<u32, (String, String, String)> = HashMap::new();
        let mut usage: Option<Usage> = None;
        let mut saw_terminal_marker = false;
        let mut malformed_data_lines = 0_usize;

        // SSE line buffering — critical correctness fix.
        //
        // The previous implementation processed each TCP chunk as a self-
        // contained block of lines: `from_utf8_lossy(&bytes).lines()`.
        // When a single SSE event ("data: {...}\n") straddled two chunks,
        // chunk-1 ended with a truncated JSON line that failed to parse
        // and was silently skipped, and chunk-2 started mid-string also
        // failing — the entire event was dropped. The symptoms in
        // production: bash commands missing characters (`Select-Object`
        // becoming `Select-Obj`), file writes losing trailing content,
        // parallel tool-call arguments arriving as malformed JSON, all
        // diagnosed by the user as "the tool corrupted my command/file".
        //
        // The fix: keep a byte buffer across chunks. Cut lines only at
        // real `\n` boundaries. `\n` is a single ASCII byte, so partial
        // UTF-8 sequences never sit on a cut point and from_utf8_lossy
        // never sees an incomplete codepoint.
        let mut byte_buffer: Vec<u8> = Vec::with_capacity(4096);

        'sse: loop {
            let chunk = match next_stream_item(&mut byte_stream, self.cancel.as_ref()).await {
                StreamPoll::Item(Some(chunk)) => chunk,
                StreamPoll::Item(None) | StreamPoll::Cancelled => break,
            };
            let bytes = chunk?;
            byte_buffer.extend_from_slice(&bytes);

            while let Some(nl_pos) = byte_buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = byte_buffer.drain(..=nl_pos).collect();
                let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
                let line = line.trim_end_matches('\r'); // SSE may use CRLF

                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    saw_terminal_marker = true;
                    byte_buffer.clear();
                    break 'sse;
                }
                let Ok(sc) = serde_json::from_str::<StreamChunk>(data) else {
                    tracing::warn!("dropped malformed SSE data line (len={})", data.len());
                    malformed_data_lines += 1;
                    continue;
                };
                if let Some(u) = sc.usage {
                    usage = Some(u);
                }
                for choice in sc.choices {
                    if choice.finish_reason.is_some() {
                        saw_terminal_marker = true;
                    }
                    let delta = choice.delta;
                    if let Some(t) = delta.content.filter(|s| !s.is_empty()) {
                        if !finalization_response {
                            self.checkpoint_provider_text(&t).await?;
                            self.events
                                .emit(StreamEvent::TextDelta { content: t.clone() });
                        }
                        text_buf.push_str(&t);
                    }
                    // DeepSeek reasoner family streams a separate reasoning_content
                    // field. Accumulate it for replay on subsequent turns. We don't
                    // stream it to the UI as TextDelta — keeping the chain-of-thought
                    // out of the visible chat is the right default; expose later via
                    // a "show reasoning" toggle if users want it.
                    if let Some(r) = delta.reasoning_content.filter(|s| !s.is_empty()) {
                        reasoning_buf.push_str(&r);
                    }
                    if let Some(tcs) = delta.tool_calls {
                        for tc in tcs {
                            let e = tc_map.entry(tc.index).or_default();
                            if let Some(id) = tc.id {
                                e.0 = id;
                            }
                            if let Some(f) = tc.function {
                                if let Some(n) = f.name {
                                    e.1 = n;
                                }
                                if let Some(a) = f.arguments {
                                    e.2.push_str(&a);
                                }
                            }
                        }
                    }
                }
            }
        }

        if self.is_cancelled() {
            let reasoning = (!reasoning_buf.is_empty()).then_some(reasoning_buf);
            return Ok((text_buf, Vec::new(), usage, reasoning));
        }

        validate_openai_sse_completion(
            saw_terminal_marker,
            byte_buffer.len(),
            malformed_data_lines,
        )
        .map_err(crate::errors::AppError::Other)?;

        let mut tool_calls: Vec<ToolCall> = tc_map
            .into_iter()
            .filter(|(_, (id, name, _))| !id.is_empty() && !name.is_empty())
            .map(|(_, (id, name, args))| ToolCall {
                id,
                r#type: "function".into(),
                function: FunctionCall {
                    name,
                    arguments: args,
                },
            })
            .collect();
        tool_calls.sort_by_key(|tc| tc.id.clone());

        if finalization_response {
            if self.max_output_tokens.is_none() {
                text_buf = sanitize_completion_summary(&text_buf);
            }
            tool_calls.clear();
            if !text_buf.is_empty() {
                self.checkpoint_provider_text(&text_buf).await?;
            }
            self.events.emit(StreamEvent::TextDelta {
                content: text_buf.clone(),
            });
        }

        let reasoning = if reasoning_buf.is_empty() {
            None
        } else {
            Some(reasoning_buf)
        };
        Ok((text_buf, tool_calls, usage, reasoning))
    }
}

fn normalize_chatgpt_reasoning_effort(value: &str) -> &str {
    match value {
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => value,
        "ultra" => "max",
        _ => "medium",
    }
}

/// DeepSeek's `reasoning_effort` accepts `low` | `high` | `max` (default
/// `high`); `medium`/`xhigh` are compatibility-mapped by the API itself. Map
/// CodeFactory's extended palette onto DeepSeek's three real levels — the
/// picker shows low/high/max for DeepSeek, but a persisted session override
/// may still carry any of our levels.
fn normalize_deepseek_reasoning_effort(value: &str) -> &str {
    match value {
        "minimal" | "low" => "low",
        "medium" | "high" => "high",
        "xhigh" | "max" | "ultra" => "max",
        other => other,
    }
}

/// True when this route speaks the DeepSeek API dialect — either a
/// deepseek.com endpoint or a deepseek-prefixed model id (incl. OpenRouter
/// `deepseek/…` slugs).
fn is_deepseek_route(base_url: &str, model_id: &str) -> bool {
    let base = base_url.to_ascii_lowercase();
    let model = model_id.to_ascii_lowercase();
    base.contains("deepseek.com") || model.starts_with("deepseek")
}

/// The extra body fields to attach for a DeepSeek reasoning model. DeepSeek
/// enables thinking via `thinking: {type: enabled}` and tunes it with
/// `reasoning_effort`. Returns `None` for non-DeepSeek routes so other
/// OpenAI-compatible providers (LMStudio, Ollama, …) keep their exact payload.
fn deepseek_reasoning_body_patch(
    base_url: &str,
    model_id: &str,
    reasoning_effort: &str,
) -> Option<serde_json::Value> {
    if !is_deepseek_route(base_url, model_id) {
        return None;
    }
    Some(serde_json::json!({
        "thinking": { "type": "enabled" },
        "reasoning_effort": normalize_deepseek_reasoning_effort(reasoning_effort),
    }))
}

/// The agent-loop `ModelTransport` seam (keystone slice 4.5b). Wraps the
/// inherent `call_openai_transport` (which the loop still calls directly until
/// slice 4.6 switches it onto `complete`): maps the internal 4-tuple to
/// [`ModelResponse`] and the bin's `AppError` to a fatal [`TransportError`]
/// (message preserved verbatim). `RoundOptions` supplies the per-round
/// `require_tool` + pre-resolved `reasoning_effort`; the sink and cancel handle
/// are the transport's own fields, so `complete` needs neither as a param.
impl DesktopModelTransport {
    /// One Anthropic round (keystone slice 4.7): convert canonical `ChatMessage`
    /// history to the Anthropic wire at the edge, run `stream_anthropic`, and
    /// apply the required→auto tool-choice fallback (moved here from
    /// `AgentLoop::call_anthropic_transport` so the shared loop, which only calls
    /// `complete`, keeps it). `reasoning_effort` is ignored for Anthropic.
    async fn call_anthropic_model(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        require_tool: bool,
    ) -> Result<super::anthropic_client::AnthropicResponse> {
        let (system, wire_messages) = chat_messages_to_anthropic(messages);
        self.admit_provider_post().await?;
        let first = super::anthropic_client::stream_anthropic(
            &self.http,
            &self.base_url,
            &self.api_key,
            &self.model_id,
            &system,
            wire_messages.clone(),
            tools,
            require_tool,
            self.max_output_tokens.unwrap_or(8096),
            self.max_output_tokens.map(|_| 0.2),
            self.retry_response_body,
            self.max_output_tokens.is_none(),
            self.cancel.as_ref(),
            self.events.as_ref(),
            self.provider_attempt.as_ref(),
            self.provider_http_attempt_budget(),
        )
        .await;
        let required_choice_unsupported = first.as_ref().err().is_some_and(|error| {
            require_tool && provider_rejects_required_tool_choice(&error.to_string())
        });
        if !required_choice_unsupported {
            return first;
        }
        if self.provider_attempt.is_some() {
            return first;
        }
        super::anthropic_client::stream_anthropic(
            &self.http,
            &self.base_url,
            &self.api_key,
            &self.model_id,
            &system,
            wire_messages,
            tools,
            false,
            self.max_output_tokens.unwrap_or(8096),
            self.max_output_tokens.map(|_| 0.2),
            self.retry_response_body,
            self.max_output_tokens.is_none(),
            self.cancel.as_ref(),
            self.events.as_ref(),
            self.provider_attempt.as_ref(),
            self.provider_http_attempt_budget(),
        )
        .await
    }
}

/// Map the Anthropic streamed answer onto the provider-independent
/// `ModelResponse` (keystone slice 4.7). Usage is gated on `(input>0||output>0)`
/// — the same guard `record_usage_event_for_round` uses; `reasoning` is `None`
/// (Anthropic thinking is neither requested nor parsed); `tool_calls` are
/// cleared on cancel for parity with the OpenAI path (unobservable — the loop
/// breaks first); the per-response `cancelled` bool is dropped (cancellation
/// flows through the shared `Arc`).
fn anthropic_response_to_model_response(
    resp: super::anthropic_client::AnthropicResponse,
) -> ModelResponse {
    let usage = (resp.input_tokens > 0 || resp.output_tokens > 0).then(|| {
        let input = resp.input_tokens.max(0) as u32;
        let output = resp.output_tokens.max(0) as u32;
        codefactory_agent_loop::types::Usage {
            prompt_tokens: input,
            completion_tokens: output,
            total_tokens: input + output,
            cost: None,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        }
    });
    let tool_calls = if resp.cancelled {
        Vec::new()
    } else {
        resp.tool_calls
    };
    ModelResponse {
        text: resp.text,
        tool_calls,
        usage,
        reasoning: None,
        effective_route: None,
        route_change: None,
    }
}

#[async_trait::async_trait]
impl ModelTransport for DesktopModelTransport {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        opts: &RoundOptions,
    ) -> std::result::Result<ModelResponse, TransportError> {
        // Dispatch on the provider dialect. The OpenAI path's `_ =>` would
        // silently mis-route Anthropic to call_openai_model — MUST branch here.
        match self.api_style {
            ApiStyle::Anthropic => {
                let resp = self
                    .call_anthropic_model(messages, tools, opts.require_tool)
                    .await
                    .map_err(|e| classify_transport_error(e.to_string()))?;
                Ok(anthropic_response_to_model_response(resp))
            }
            _ => {
                let (text, tool_calls, usage, reasoning) = self
                    .call_openai_transport(
                        messages,
                        tools,
                        opts.require_tool,
                        &opts.reasoning_effort,
                    )
                    .await
                    .map_err(|e| classify_transport_error(e.to_string()))?;
                Ok(ModelResponse {
                    text,
                    tool_calls,
                    usage,
                    reasoning,
                    effective_route: None,
                    route_change: None,
                })
            }
        }
    }
}

#[async_trait::async_trait]
impl ModelTransport for RoutedDesktopModelTransport {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        opts: &RoundOptions,
    ) -> std::result::Result<ModelResponse, TransportError> {
        let mut transitions = self
            .route_state
            .take_initial_route_change()
            .into_iter()
            .collect::<Vec<_>>();
        loop {
            let route = self.route_state.current();
            let output_started = Arc::new(AtomicBool::new(false));
            let provider_attempt = self
                .prepare_provider_attempt(&route, messages, tools, opts)
                .await?;
            let transport = match self
                .transport_for(&route, output_started.clone(), provider_attempt.clone())
                .await
            {
                Ok(transport) => transport,
                Err(TransportError::Fatal(reason)) => {
                    if let Some(attempt) = provider_attempt.as_ref() {
                        let error = TransportError::Fatal(reason.clone());
                        let _ = attempt.settle_failure(&error).await?;
                    }
                    self.route_state.record_current_failure(&reason);
                    return Err(TransportError::Fatal(reason));
                }
                Err(TransportError::Retryable(reason)) => {
                    let action = match provider_attempt.as_ref() {
                        Some(attempt) => {
                            attempt
                                .settle_failure(&TransportError::Retryable(reason.clone()))
                                .await?
                        }
                        None => ProviderFailureAction::RetrySafe,
                    };
                    if action == ProviderFailureAction::DurableWaiting {
                        return Err(TransportError::Retryable(
                            "PROVIDER_RECOVERY_WAITING: provider overloaded".into(),
                        ));
                    }
                    if self.turn_output_started.load(Ordering::SeqCst)
                        || self.turn_side_effect_started.load(Ordering::SeqCst)
                    {
                        self.route_state.record_current_failure(&reason);
                        return Err(TransportError::Retryable(reason));
                    }
                    let Some(change) = self.route_state.advance_after_failure(&reason) else {
                        return Err(TransportError::Retryable(
                            self.route_state.exhausted_error(&reason),
                        ));
                    };
                    transitions.push(change);
                    continue;
                }
            };
            match transport.complete(messages, tools, opts).await {
                Ok(mut response) => {
                    if let Some(attempt) = provider_attempt.as_ref() {
                        attempt.commit_response(&response).await?;
                    }
                    self.route_state.mark_current_success();
                    response.effective_route = Some(effective_route(&route));
                    if let Some(first) = transitions.first() {
                        let reason = transitions
                            .iter()
                            .map(|change| change.reason.as_str())
                            .collect::<Vec<_>>()
                            .join("；");
                        let combined = super::failover::RouteChange {
                            from_endpoint: first.from_endpoint.clone(),
                            from_model: first.from_model.clone(),
                            to_endpoint: route.endpoint_name.clone(),
                            to_model: route.model_id.clone(),
                            reason,
                        };
                        response.route_change = Some(LoopRouteChange {
                            from_endpoint: combined.from_endpoint.clone(),
                            from_model: combined.from_model.clone(),
                            to_endpoint: combined.to_endpoint.clone(),
                            to_model: combined.to_model.clone(),
                            reason: combined.reason.clone(),
                            notice: combined.notice(),
                        });
                    }
                    return Ok(response);
                }
                Err(error @ TransportError::Fatal(_)) => {
                    if let Some(attempt) = provider_attempt.as_ref() {
                        let _ = attempt.settle_failure(&error).await?;
                    }
                    self.route_state.record_current_failure(&error.to_string());
                    return Err(error);
                }
                Err(TransportError::Retryable(reason)) => {
                    let action = match provider_attempt.as_ref() {
                        Some(attempt) => {
                            attempt
                                .settle_failure(&TransportError::Retryable(reason.clone()))
                                .await?
                        }
                        None => ProviderFailureAction::RetrySafe,
                    };
                    if action == ProviderFailureAction::DurableWaiting {
                        return Err(TransportError::Retryable(
                            "PROVIDER_RECOVERY_WAITING: provider overloaded".into(),
                        ));
                    }
                    // A provider can fail after yielding visible SSE. Replaying
                    // on another model would mix answers and can duplicate tool
                    // intent, so fail visibly without switching.
                    if output_started.load(Ordering::SeqCst)
                        || self.turn_output_started.load(Ordering::SeqCst)
                        || self.turn_side_effect_started.load(Ordering::SeqCst)
                    {
                        self.route_state.record_current_failure(&reason);
                        return Err(TransportError::Retryable(reason));
                    }
                    let Some(change) = self.route_state.advance_after_failure(&reason) else {
                        return Err(TransportError::Retryable(
                            self.route_state.exhausted_error(&reason),
                        ));
                    };
                    transitions.push(change);
                }
            }
        }
    }
}

/// Convert canonical `ChatMessage` history into the Anthropic wire shape
/// (keystone slice 4.7): returns the extracted top-level `system` string and the
/// `messages` array. The INVERSE of the old `build_anthropic_messages` plus the
/// live tool_result batching — the single representation boundary for Anthropic,
/// EDGE-only (`run_agent_loop` never sees `Value`).
///
/// - a leading `role:"system"` ChatMessage → the `system` string (not emitted);
/// - a maximal run of consecutive `role:"tool"` ChatMessages → ONE
///   `{role:"user", content:[tool_result…]}` (the deliberate non-transparent
///   merge — matches the live loop's already-batched shape);
/// - assistant → text block if non-empty, then `tool_use` blocks
///   (`input = from_str(args).unwrap_or({})`); empty-both → `[{text:""}]`;
/// - user `Text` → a bare JSON string; user `Parts` → `[text | image]` blocks,
///   `image_url` data-URLs split back to `{type:image, source:{base64,…}}`.
fn chat_messages_to_anthropic(
    messages: &[codefactory_agent_loop::types::ChatMessage],
) -> (String, Vec<serde_json::Value>) {
    use codefactory_agent_loop::types::MessageContent;
    let mut system = String::new();
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let m = &messages[i];
        match m.role.as_str() {
            "system" => {
                system = super::AgentLoop::content_to_text(&m.content);
                i += 1;
            }
            "tool" => {
                // Merge the maximal run of consecutive tool messages into ONE
                // user message of N tool_result blocks (never absorb a following
                // non-tool message).
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                while i < messages.len() && messages[i].role == "tool" {
                    let tm = &messages[i];
                    blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tm.tool_call_id.clone().unwrap_or_default(),
                        "content": super::AgentLoop::content_to_text(&tm.content),
                    }));
                    i += 1;
                }
                out.push(serde_json::json!({ "role": "user", "content": blocks }));
            }
            "assistant" => {
                let mut content_blocks: Vec<serde_json::Value> = Vec::new();
                let text = super::AgentLoop::content_to_text(&m.content);
                if !text.is_empty() {
                    content_blocks.push(serde_json::json!({ "type": "text", "text": text }));
                }
                for tc in m.tool_calls.as_deref().unwrap_or_default() {
                    let input: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(serde_json::json!({}));
                    content_blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.function.name,
                        "input": input,
                    }));
                }
                if content_blocks.is_empty() {
                    content_blocks.push(serde_json::json!({ "type": "text", "text": "" }));
                }
                out.push(serde_json::json!({ "role": "assistant", "content": content_blocks }));
                i += 1;
            }
            _ => {
                let content = match &m.content {
                    MessageContent::Text(s) => serde_json::Value::String(s.clone()),
                    MessageContent::Parts(parts) => {
                        let blocks: Vec<serde_json::Value> = parts
                            .iter()
                            .map(|p| {
                                if p.r#type == "image_url" {
                                    let url =
                                        p.image_url.as_ref().map(|u| u.url.as_str()).unwrap_or("");
                                    match parse_data_url(url) {
                                        Some((media_type, data)) => serde_json::json!({
                                            "type": "image",
                                            "source": {
                                                "type": "base64",
                                                "media_type": media_type,
                                                "data": data,
                                            },
                                        }),
                                        // Defensive: a non-data image_url (raw http)
                                        // is never produced by attachments today, but
                                        // must degrade without corrupting the request.
                                        None => serde_json::json!({
                                            "type": "image",
                                            "source": { "type": "url", "url": url },
                                        }),
                                    }
                                } else {
                                    serde_json::json!({
                                        "type": "text",
                                        "text": p.text.clone().unwrap_or_default(),
                                    })
                                }
                            })
                            .collect();
                        serde_json::Value::Array(blocks)
                    }
                };
                out.push(serde_json::json!({ "role": m.role, "content": content }));
                i += 1;
            }
        }
    }
    (system, out)
}

/// Split a `data:<media_type>;base64,<data>` URL into `(media_type, data)`.
fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(";base64,")?;
    Some((media_type.to_string(), data.to_string()))
}

#[cfg(test)]
mod tests {
    //! `DesktopModelTransport` owns no `AppHandle`, so it constructs from bare
    //! handles in a test (#166-safe). We prove the `ModelTransport` trait is
    //! satisfied and object-safe (`Arc<dyn ModelTransport>`); `complete` itself
    //! is a network call, exercised end-to-end by the desktop app, not here.
    use super::*;
    use codefactory_agent_loop::types::{
        ChatMessage, ContentPart, FunctionCall, ImageUrl, MessageContent, ToolCall,
    };
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    fn unused_provider_db() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("lazy sqlite pool")
    }

    // ── chat_messages_to_anthropic golden pins (keystone slice 4.7 step 4) ──
    // The Anthropic representation switch is fully pinned HERE, before it is
    // wired, so any drift in the edge conversion fails a unit test.

    fn cm(role: &str, text: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Text(text.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    #[test]
    fn chatgpt_request_never_sends_an_empty_reasoning_effort() {
        assert_eq!(normalize_chatgpt_reasoning_effort(""), "medium");
        assert_eq!(normalize_chatgpt_reasoning_effort("ultra"), "max");
        assert_eq!(normalize_chatgpt_reasoning_effort("high"), "high");
    }

    #[test]
    fn deepseek_effort_maps_our_palette_onto_low_high_max() {
        assert_eq!(normalize_deepseek_reasoning_effort("minimal"), "low");
        assert_eq!(normalize_deepseek_reasoning_effort("low"), "low");
        assert_eq!(normalize_deepseek_reasoning_effort("medium"), "high");
        assert_eq!(normalize_deepseek_reasoning_effort("high"), "high");
        assert_eq!(normalize_deepseek_reasoning_effort("xhigh"), "max");
        assert_eq!(normalize_deepseek_reasoning_effort("max"), "max");
        assert_eq!(normalize_deepseek_reasoning_effort("ultra"), "max");
    }

    #[test]
    fn deepseek_route_detection_covers_direct_and_openrouter_slugs() {
        assert!(is_deepseek_route(
            "https://api.deepseek.com",
            "deepseek-v4-pro"
        ));
        assert!(is_deepseek_route(
            "https://openrouter.ai/api/v1",
            "deepseek/deepseek-v4-pro"
        ));
        // Non-DeepSeek OpenAI-compatible providers must NOT get the patch.
        assert!(!is_deepseek_route(
            "http://localhost:1234/v1",
            "qwen2.5-coder"
        ));
        assert!(!is_deepseek_route("https://api.openai.com/v1", "gpt-5.6"));
    }

    #[test]
    fn deepseek_body_patch_enables_thinking_with_mapped_effort() {
        let patch =
            deepseek_reasoning_body_patch("https://api.deepseek.com", "deepseek-v4-pro", "medium")
                .expect("deepseek route gets a patch");
        assert_eq!(patch["thinking"]["type"], "enabled");
        assert_eq!(patch["reasoning_effort"], "high");

        let max_patch = deepseek_reasoning_body_patch(
            "https://openrouter.ai/api/v1",
            "deepseek/deepseek-v4-pro",
            "xhigh",
        )
        .expect("openrouter deepseek slug gets a patch");
        assert_eq!(max_patch["reasoning_effort"], "max");

        assert!(
            deepseek_reasoning_body_patch("http://localhost:1234/v1", "qwen2.5-coder", "high")
                .is_none()
        );
    }
    fn tool_cm(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: "tool".into(),
            content: MessageContent::Text(content.into()),
            tool_calls: None,
            tool_call_id: Some(id.into()),
            name: None,
            reasoning_content: None,
        }
    }
    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            r#type: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: args.into(),
            },
        }
    }

    #[test]
    fn edge_extracts_leading_system_and_emits_no_system_message() {
        let (system, msgs) =
            chat_messages_to_anthropic(&[cm("system", "you are helpful"), cm("user", "hi")]);
        assert_eq!(system, "you are helpful");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "hi");
    }

    #[test]
    fn edge_assistant_text_only() {
        let (_, msgs) = chat_messages_to_anthropic(&[cm("assistant", "hello")]);
        assert_eq!(msgs[0]["content"], json!([{"type":"text","text":"hello"}]));
    }

    #[test]
    fn edge_assistant_tool_only_has_no_text_block() {
        let mut a = cm("assistant", "");
        a.tool_calls = Some(vec![call("t1", "bash", r#"{"cmd":"ls"}"#)]);
        let (_, msgs) = chat_messages_to_anthropic(&[a]);
        assert_eq!(
            msgs[0]["content"],
            json!([{"type":"tool_use","id":"t1","name":"bash","input":{"cmd":"ls"}}])
        );
    }

    #[test]
    fn edge_assistant_text_then_tool_preserves_order() {
        let mut a = cm("assistant", "running it");
        a.tool_calls = Some(vec![call("t1", "bash", r#"{"cmd":"ls"}"#)]);
        let (_, msgs) = chat_messages_to_anthropic(&[a]);
        assert_eq!(
            msgs[0]["content"],
            json!([
                {"type":"text","text":"running it"},
                {"type":"tool_use","id":"t1","name":"bash","input":{"cmd":"ls"}},
            ])
        );
    }

    #[test]
    fn edge_assistant_empty_both_gets_placeholder_text_block() {
        let (_, msgs) = chat_messages_to_anthropic(&[cm("assistant", "")]);
        assert_eq!(msgs[0]["content"], json!([{"type":"text","text":""}]));
    }

    #[test]
    fn edge_malformed_tool_args_become_empty_object() {
        let mut a = cm("assistant", "");
        a.tool_calls = Some(vec![call("t1", "bash", "not json")]);
        let (_, msgs) = chat_messages_to_anthropic(&[a]);
        assert_eq!(msgs[0]["content"][0]["input"], json!({}));
    }

    #[test]
    fn edge_merges_consecutive_tool_results_into_one_user_message() {
        // THE deliberate non-transparent merge: N tool rows → ONE user message
        // of N tool_result blocks (matches the live loop's batched shape).
        let (_, msgs) = chat_messages_to_anthropic(&[tool_cm("t1", "ok1"), tool_cm("t2", "ok2")]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(
            msgs[0]["content"],
            json!([
                {"type":"tool_result","tool_use_id":"t1","content":"ok1"},
                {"type":"tool_result","tool_use_id":"t2","content":"ok2"},
            ])
        );
    }

    #[test]
    fn edge_tool_run_then_user_progress_stays_two_user_messages() {
        // The merge must NEVER absorb a following non-tool message.
        let (_, msgs) = chat_messages_to_anthropic(&[tool_cm("t1", "ok"), cm("user", "continue")]);
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0]["content"],
            json!([{"type":"tool_result","tool_use_id":"t1","content":"ok"}])
        );
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "continue");
    }

    #[test]
    fn edge_user_image_data_url_becomes_base64_source() {
        let user = ChatMessage {
            role: "user".into(),
            content: MessageContent::Parts(vec![
                ContentPart {
                    r#type: "text".into(),
                    text: Some("look".into()),
                    image_url: None,
                },
                ContentPart {
                    r#type: "image_url".into(),
                    text: None,
                    image_url: Some(ImageUrl {
                        url: "data:image/png;base64,AAAB".into(),
                    }),
                },
            ]),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        };
        let (_, msgs) = chat_messages_to_anthropic(&[user]);
        assert_eq!(
            msgs[0]["content"],
            json!([
                {"type":"text","text":"look"},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAB"}},
            ])
        );
    }

    #[test]
    fn edge_user_text_is_a_bare_json_string() {
        let (_, msgs) = chat_messages_to_anthropic(&[cm("user", "just text")]);
        assert_eq!(
            msgs[0]["content"],
            serde_json::Value::String("just text".into())
        );
    }

    #[test]
    fn anthropic_response_maps_usage_reasoning_and_cancel() {
        use crate::agent::anthropic_client::AnthropicResponse;
        // input/output>0 → Some usage; reasoning always None; tool_calls kept.
        let mr = anthropic_response_to_model_response(AnthropicResponse {
            text: "hi".into(),
            tool_calls: vec![call("t1", "bash", "{}")],
            input_tokens: 5,
            output_tokens: 7,
            cancelled: false,
        });
        assert_eq!(mr.text, "hi");
        assert_eq!(mr.tool_calls.len(), 1);
        assert!(mr.reasoning.is_none());
        let u = mr.usage.expect("usage present when tokens > 0");
        assert_eq!(
            (u.prompt_tokens, u.completion_tokens, u.total_tokens),
            (5, 7, 12)
        );
        // (0,0) tokens → None (matches the record_usage guard).
        let none = anthropic_response_to_model_response(AnthropicResponse {
            text: String::new(),
            tool_calls: vec![],
            input_tokens: 0,
            output_tokens: 0,
            cancelled: false,
        });
        assert!(none.usage.is_none());
        // cancelled → tool_calls cleared (OpenAI parity, unobservable).
        let cancelled = anthropic_response_to_model_response(AnthropicResponse {
            text: String::new(),
            tool_calls: vec![call("t1", "bash", "{}")],
            input_tokens: 1,
            output_tokens: 1,
            cancelled: true,
        });
        assert!(cancelled.tool_calls.is_empty());
    }
    use std::sync::Arc;

    fn transport() -> DesktopModelTransport {
        DesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            model_id: "m".into(),
            session_id: "s".into(),
            base_url: "http://127.0.0.1:0".into(),
            api_key: "k".into(),
            api_style: ApiStyle::Openai,
            cancel: None,
            max_output_tokens: None,
            retry_response_body: crate::http_util::RetryResponseBody::Include,
            provider_attempt: None,
        }
    }

    #[test]
    fn chatgpt_metadata_request_omits_unsupported_output_cap() {
        let body = build_chatgpt_responses_body(
            "gpt-test",
            "system prompt".into(),
            vec![serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "name this task"}],
            })],
            Vec::new(),
            false,
            "low",
            Some(64),
        );
        assert_eq!(body["model"], serde_json::json!("gpt-test"));
        assert_eq!(body["instructions"], serde_json::json!("system prompt"));
        assert_eq!(body["input"][0]["type"], serde_json::json!("message"));
        assert_eq!(body["tool_choice"], serde_json::json!("none"));
        assert_eq!(body["parallel_tool_calls"], serde_json::json!(false));
        assert_eq!(body["reasoning"]["effort"], serde_json::json!("low"));
        assert_eq!(body["store"], serde_json::json!(false));
        assert_eq!(body["stream"], serde_json::json!(true));
        assert!(body.get("max_output_tokens").is_none());

        let interactive = build_chatgpt_responses_body(
            "gpt-test",
            String::new(),
            Vec::new(),
            Vec::new(),
            false,
            "medium",
            None,
        );
        assert!(interactive.get("max_output_tokens").is_none());
    }

    #[test]
    fn desktop_transport_is_object_safe_as_dyn_model_transport() {
        // The shared loop (4.6) holds Arc<dyn ModelTransport>; prove the desktop
        // impl coerces and constructs with no AppHandle.
        let _t: Arc<dyn ModelTransport> = Arc::new(transport());
    }

    fn serve_responses(
        responses: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let base_url = format!("http://{}", listener.local_addr().expect("fixture addr"));
        let hits = Arc::new(AtomicUsize::new(0));
        let fixture_hits = hits.clone();
        std::thread::spawn(move || {
            for (status, content_type, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept fixture request");
                fixture_hits.fetch_add(1, Ordering::SeqCst);
                let mut request = [0_u8; 16 * 1024];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write fixture response");
            }
        });
        (base_url, hits)
    }

    fn openai_candidate(name: &str, model: &str, base_url: String) -> RouteCandidate {
        RouteCandidate {
            endpoint_name: name.into(),
            model_id: model.into(),
            base_url,
            credential_ref: None,
            legacy_inline_api_key: Some("test-key".into()),
            supports_vision: true,
            api_style: ApiStyle::Openai,
        }
    }

    fn test_client() -> Client {
        // Local fixtures must never leak through a developer or CI machine's
        // ambient HTTP proxy. A proxy-generated 502 would otherwise look like
        // a provider failure while the fixture server receives zero requests.
        Client::builder()
            .no_proxy()
            .build()
            .expect("build fixture HTTP client")
    }

    fn serve_truncated_chunked_sse() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind truncated fixture");
        let base_url = format!("http://{}", listener.local_addr().expect("fixture addr"));
        let hits = Arc::new(AtomicUsize::new(0));
        let fixture_hits = hits.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept truncated request");
            fixture_hits.fetch_add(1, Ordering::SeqCst);
            let mut request = [0_u8; 16 * 1024];
            let _ = stream.read(&mut request);
            let body =
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write truncated response");
            // Deliberately omit the terminal zero-length chunk.
        });
        (base_url, hits)
    }

    fn serve_request_probe() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind request probe");
        listener
            .set_nonblocking(true)
            .expect("set request probe nonblocking");
        let base_url = format!("http://{}", listener.local_addr().expect("probe addr"));
        let hits = Arc::new(AtomicUsize::new(0));
        let probe_hits = hits.clone();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((_stream, _)) => {
                        probe_hits.fetch_add(1, Ordering::SeqCst);
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        (base_url, hits)
    }

    async fn durable_foreground_fixture(
        suffix: &str,
    ) -> (SqlitePool, crate::agent::objective::ObjectiveSnapshot) {
        use crate::agent::objective::{
            CreateObjective, ObjectiveKind, ObjectiveStore, RecoveryDomain,
        };

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY, model_policy TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let session_id = format!("durable-{suffix}-session");
        let root_turn_id = format!("durable-{suffix}-root");
        let objective_id = format!("durable-{suffix}-objective");
        let binding_id = format!("durable-{suffix}-binding");
        let run_id = format!("durable-{suffix}-run");
        sqlx::query("INSERT INTO sessions(id, model_policy) VALUES (?, 'fixed')")
            .bind(&session_id)
            .execute(&pool)
            .await
            .unwrap();
        let objective = ObjectiveStore::new(pool.clone())
            .create(CreateObjective {
                id: objective_id,
                kind: ObjectiveKind::Informational,
                session_id: Some(session_id.clone()),
                root_turn_id: Some(root_turn_id.clone()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES (?, ?, 'chat', 'chat_root_turn', ?, 1, ?, ?, ?, ?)",
        )
        .bind(&binding_id)
        .bind(&objective.id)
        .bind(&root_turn_id)
        .bind(format!("sha256:{suffix}-binding"))
        .bind(&root_turn_id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id,
              objective_revision, status, created_process_instance, created_at, updated_at)
             VALUES (?, ?, ?, ?, 1, 'active', 'test-process', ?, ?)",
        )
        .bind(run_id)
        .bind(session_id)
        .bind(root_turn_id)
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        (pool, objective)
    }

    async fn durable_context_transport_fixture() -> (
        SqlitePool,
        crate::agent::objective::ObjectiveSnapshot,
        codefactory_agent_loop::tool::MutationPermit,
        crate::agent::context_recovery::ContextRecoveryAuthorization,
    ) {
        use crate::agent::objective::{
            CreateObjective, DecisionRouter, ObjectiveKind, ObjectiveStore, RecoveryDomain,
            RouteSignal,
        };

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "context-transport-objective".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("context-transport-session".into()),
                root_turn_id: Some("context-transport-anchor".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "context-transport-test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES ('context-transport-current', 'context-transport-session', ?)",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES ('context-transport-binding', ?, 'chat', 'chat_root_turn',
                     'context-transport-current', 2, 'sha256:context-transport',
                     'context-transport-current', ?, ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Context,
                failure_code: "context_overflow_after_compaction".into(),
                failure_signature: "sha256:context-transport".into(),
                next_observation_at: now - 1,
                resume_cursor: Some("context-transport-current".into()),
            },
        )
        .unwrap();
        store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO objective_recovery_attempts
             (id, objective_id, root_turn_id, delivery_run_id, domain, attempt_index,
              failure_code, failure_class, output_started, side_effect_started,
              queue_wait_ms, runtime_ms, process_instance, resume_owner,
              terminal_decision, created_at)
             VALUES ('context-transport-attempt', ?, 'context-transport-current', NULL,
                     'context', 1, 'CONTEXT_OVERFLOW_AFTER_COMPACTION',
                     'context_capacity', 0, 0, NULL, NULL, 'prior-process',
                     'agent_loop', 'waiting_system', ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let claim = store
            .claim_due_remediations("context-transport-owner", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id.clone(),
            owner: "context-transport-owner".into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id.clone(),
            resource_generation: claim.resource_generation,
        };
        let authorization =
            match crate::agent::context_recovery::ContextRecoveryStore::new(pool.clone())
                .reserve_claimed_recovery(&claim, &permit)
                .await
                .unwrap()
            {
                crate::agent::context_recovery::ContextRecoveryReservation::Authorized(value) => {
                    value
                }
                other => panic!("expected Context transport authorization, got {other:?}"),
            };
        (pool, claim.objective, permit, authorization)
    }

    #[tokio::test]
    async fn stale_context_cursor_is_fenced_before_any_provider_post() {
        let (pool, objective, permit, authorization) = durable_context_transport_fixture().await;
        let (base_url, hits) = serve_request_probe();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE objectives SET revision=revision+1, domain='chat', status='active',
                    decision_type='continue', resume_cursor='context-transport-new',
                    remediation_id=NULL, lease_owner=NULL, lease_expires_at=NULL, updated_at=?
             WHERE id=?",
        )
        .bind(now)
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objective_remediations SET status='superseded', lease_owner=NULL,
                    lease_expires_at=NULL, updated_at=? WHERE id=?",
        )
        .bind(now)
        .bind(&permit.remediation_id)
        .execute(&pool)
        .await
        .unwrap();

        let transport = RoutedDesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            session_id: "context-transport-session".into(),
            route_state: ActiveRouteState::from_plan_with_health(
                super::super::failover::RouteCandidatePlan::new(openai_candidate(
                    "must-not-post",
                    "context-model",
                    base_url,
                )),
                super::super::failover::EndpointHealthRegistry::new(
                    std::time::Duration::from_secs(120),
                ),
            ),
            cancel: None,
            turn_output_started: Arc::new(AtomicBool::new(false)),
            turn_side_effect_started: Arc::new(AtomicBool::new(false)),
            db: pool,
            root_turn_id: Some("context-transport-current".into()),
            mutation_permit: Some(permit),
            context_authorization: Some(authorization),
            anonymous: false,
            durable_provider_required: true,
        };
        let error = transport
            .complete(&[], &[], &RoundOptions::default())
            .await
            .expect_err("stale Context authorization must fail before network I/O");
        assert!(error.to_string().contains("CONTEXT_RECOVERY_FENCED"));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn routed_transport_visits_each_candidate_until_the_third_succeeds() {
        const DOWN: (&str, &str, &str) = (
            "503 Service Unavailable",
            "application/json",
            r#"{"error":{"message":"Service Unavailable","code":"circuit_open"}}"#,
        );
        const OK: (&str, &str, &str) = (
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        );
        let (a_url, a_hits) = serve_responses(vec![DOWN, DOWN, DOWN]);
        let (b_url, b_hits) = serve_responses(vec![DOWN, DOWN, DOWN]);
        let (c_url, c_hits) = serve_responses(vec![OK, OK]);
        let mut plan = super::super::failover::RouteCandidatePlan::new(openai_candidate(
            "route-a", "a", a_url,
        ));
        plan.push_fallback(openai_candidate("route-b", "b", b_url));
        plan.push_fallback(openai_candidate("route-c", "c", c_url));
        let transport = RoutedDesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            session_id: "route-test".into(),
            route_state: ActiveRouteState::from_plan_with_health(
                plan,
                super::super::failover::EndpointHealthRegistry::new(
                    std::time::Duration::from_secs(120),
                ),
            ),
            cancel: None,
            turn_output_started: Arc::new(AtomicBool::new(false)),
            turn_side_effect_started: Arc::new(AtomicBool::new(false)),
            db: unused_provider_db(),
            root_turn_id: None,
            mutation_permit: None,
            context_authorization: None,
            anonymous: false,
            durable_provider_required: false,
        };

        let response = transport
            .complete(&[], &[], &RoundOptions::default())
            .await
            .expect("third route succeeds");

        assert_eq!(response.text, "ok");
        assert_eq!(
            response
                .effective_route
                .as_ref()
                .map(|route| route.endpoint_name.as_str()),
            Some("route-c")
        );
        let change = response.route_change.expect("route change metadata");
        assert_eq!(change.from_endpoint, "route-a");
        assert_eq!(change.to_endpoint, "route-c");
        assert!(change.notice.contains("已自动切换到"));
        assert!(change.notice.contains("任务继续执行"));
        let sticky_response = transport
            .complete(&[], &[], &RoundOptions::default())
            .await
            .expect("subsequent round stays on the successful fallback");
        assert_eq!(
            sticky_response
                .effective_route
                .as_ref()
                .map(|route| route.endpoint_name.as_str()),
            Some("route-c")
        );
        assert!(sticky_response.route_change.is_none());
        assert_eq!(a_hits.load(Ordering::SeqCst), 3);
        assert_eq!(b_hits.load(Ordering::SeqCst), 3);
        assert_eq!(c_hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn routed_transport_does_not_switch_after_visible_partial_sse() {
        let (primary_url, primary_hits) = serve_truncated_chunked_sse();
        let (fallback_url, fallback_hits) = serve_responses(vec![(
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"wrong-replay\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        )]);
        let mut plan = super::super::failover::RouteCandidatePlan::new(openai_candidate(
            "partial-primary",
            "a",
            primary_url,
        ));
        plan.push_fallback(openai_candidate("must-not-run", "b", fallback_url));
        let transport = RoutedDesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            session_id: "partial-test".into(),
            route_state: ActiveRouteState::from_plan_with_health(
                plan,
                super::super::failover::EndpointHealthRegistry::new(
                    std::time::Duration::from_secs(120),
                ),
            ),
            cancel: None,
            turn_output_started: Arc::new(AtomicBool::new(false)),
            turn_side_effect_started: Arc::new(AtomicBool::new(false)),
            db: unused_provider_db(),
            root_turn_id: None,
            mutation_permit: None,
            context_authorization: None,
            anonymous: false,
            durable_provider_required: false,
        };
        let tools = vec![ToolDefinition {
            r#type: "function".into(),
            function: codefactory_agent_loop::types::FunctionDefinition {
                name: "noop".into(),
                description: "test".into(),
                parameters: serde_json::json!({"type":"object"}),
            },
        }];

        let error = transport
            .complete(&[], &tools, &RoundOptions::default())
            .await
            .expect_err("truncated stream is visible failure");

        assert!(matches!(error, TransportError::Retryable(_)));
        assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            transport.route_state.current().endpoint_name,
            "partial-primary"
        );
    }

    #[tokio::test]
    async fn durable_provider_overload_stops_after_three_receipted_posts() {
        const DOWN: (&str, &str, &str) = (
            "503 Service Unavailable",
            "application/json",
            r#"{"error":{"message":"Service Unavailable","code":"overloaded"}}"#,
        );
        let (pool, objective) = durable_foreground_fixture("overload").await;
        let (base_url, hits) = serve_responses(vec![DOWN, DOWN, DOWN]);
        let transport = RoutedDesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            session_id: objective.session_id.clone().unwrap(),
            route_state: ActiveRouteState::from_plan_with_health(
                super::super::failover::RouteCandidatePlan::new(openai_candidate(
                    "durable-overload-provider",
                    "durable-overload-model",
                    base_url,
                )),
                super::super::failover::EndpointHealthRegistry::new(
                    std::time::Duration::from_secs(120),
                ),
            ),
            cancel: None,
            turn_output_started: Arc::new(AtomicBool::new(false)),
            turn_side_effect_started: Arc::new(AtomicBool::new(false)),
            db: pool.clone(),
            root_turn_id: objective.root_turn_id.clone(),
            mutation_permit: None,
            context_authorization: None,
            anonymous: false,
            durable_provider_required: true,
        };

        for attempt in 1..=3 {
            let error = transport
                .complete(&[], &[], &RoundOptions::default())
                .await
                .expect_err("overload must remain system-owned");
            if attempt == 3 {
                assert!(error.to_string().contains("PROVIDER_RECOVERY_WAITING"));
                assert!(
                    codefactory_agent_loop::context::is_provider_overloaded(&error.to_string()),
                    "the routed transport must preserve overload classification so the outer loop receipts its third terminal attempt"
                );
            }
        }
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM provider_route_attempts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            3
        );
        assert!(matches!(
            ProviderRecoveryStore::new(pool.clone())
                .observe(&objective.id)
                .await
                .unwrap(),
            super::super::provider_recovery::ProviderRecoveryDisposition::DurableWaiting { .. }
        ));

        transport
            .complete(&[], &[], &RoundOptions::default())
            .await
            .expect_err("waiting episode cannot issue a fourth POST");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn durable_partial_sse_restart_is_checkpointed_and_never_posts_again() {
        use crate::agent::objective::{
            CreateObjective, ObjectiveKind, ObjectiveStore, RecoveryDomain,
        };

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::agent::objective::ensure_schema(&pool).await.unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY, model_policy TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions(id, model_policy) VALUES ('durable-partial-session', 'fixed')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let objective_store = ObjectiveStore::new(pool.clone());
        let objective = objective_store
            .create(CreateObjective {
                id: "durable-partial-objective".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("durable-partial-session".into()),
                root_turn_id: Some("durable-partial-root".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, resume_cursor, created_at, updated_at)
             VALUES ('durable-partial-binding', ?, 'chat', 'chat_root_turn', ?, 1,
                     'sha256:durable-partial-binding', ?, ?, ?)",
        )
        .bind(&objective.id)
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id,
              objective_revision, status, created_process_instance, created_at, updated_at)
             VALUES ('durable-partial-run', ?, ?, ?, 1, 'active', 'old-process', ?, ?)",
        )
        .bind(objective.session_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let (partial_url, partial_hits) = serve_truncated_chunked_sse();
        let route = openai_candidate("durable-provider", "durable-model", partial_url);
        let transport = RoutedDesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            session_id: "durable-partial-session".into(),
            route_state: ActiveRouteState::from_plan_with_health(
                super::super::failover::RouteCandidatePlan::new(route),
                super::super::failover::EndpointHealthRegistry::new(
                    std::time::Duration::from_secs(120),
                ),
            ),
            cancel: None,
            turn_output_started: Arc::new(AtomicBool::new(false)),
            turn_side_effect_started: Arc::new(AtomicBool::new(false)),
            db: pool.clone(),
            root_turn_id: Some("durable-partial-root".into()),
            mutation_permit: None,
            context_authorization: None,
            anonymous: false,
            durable_provider_required: true,
        };
        let tools = vec![ToolDefinition {
            r#type: "function".into(),
            function: codefactory_agent_loop::types::FunctionDefinition {
                name: "noop".into(),
                description: "test".into(),
                parameters: serde_json::json!({"type":"object"}),
            },
        }];
        transport
            .complete(&[], &tools, &RoundOptions::default())
            .await
            .expect_err("truncated durable stream must wait for observation");
        assert_eq!(partial_hits.load(Ordering::SeqCst), 1);
        let checkpoint: (String, String) =
            sqlx::query_as("SELECT state, content FROM provider_output_checkpoints LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(checkpoint, ("partial".into(), "partial".into()));

        sqlx::query(
            "UPDATE chat_run_controls SET status='completed', settled_at=?, updated_at=?
             WHERE run_instance_id='durable-partial-run'",
        )
        .bind(now + 10)
        .bind(now + 10)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            super::super::objective_supervisor::reconcile_provider_recovery_on_startup(&pool)
                .await
                .unwrap(),
            1
        );
        let claim = objective_store
            .claim_due_remediations("durable-partial-owner", 1, 60_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let mutation_permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id,
            owner: "durable-partial-owner".into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id,
            resource_generation: claim.resource_generation,
        };
        let (must_not_post_url, must_not_post_hits) = serve_responses(vec![(
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"duplicate\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        )]);
        let resumed = RoutedDesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            session_id: "durable-partial-session".into(),
            route_state: ActiveRouteState::from_plan_with_health(
                super::super::failover::RouteCandidatePlan::new(openai_candidate(
                    "durable-provider",
                    "durable-model",
                    must_not_post_url,
                )),
                super::super::failover::EndpointHealthRegistry::new(
                    std::time::Duration::from_secs(120),
                ),
            ),
            cancel: None,
            turn_output_started: Arc::new(AtomicBool::new(false)),
            turn_side_effect_started: Arc::new(AtomicBool::new(false)),
            db: pool,
            root_turn_id: Some("durable-partial-root".into()),
            mutation_permit: Some(mutation_permit),
            context_authorization: None,
            anonymous: false,
            durable_provider_required: true,
        };
        resumed
            .complete(&[], &tools, &RoundOptions::default())
            .await
            .expect_err("partial output may not be replayed after restart");
        assert_eq!(must_not_post_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn routed_transport_does_not_switch_in_a_later_round_after_turn_output_started() {
        const DOWN: (&str, &str, &str) = (
            "503 Service Unavailable",
            "application/json",
            r#"{"error":{"message":"Service Unavailable","code":"circuit_open"}}"#,
        );
        const OK: (&str, &str, &str) = (
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"round-one\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        );
        let (primary_url, primary_hits) = serve_responses(vec![OK, DOWN, DOWN, DOWN]);
        let (fallback_url, fallback_hits) = serve_responses(vec![(
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"unsafe-replay\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        )]);
        let mut plan = super::super::failover::RouteCandidatePlan::new(openai_candidate(
            "turn-primary",
            "a",
            primary_url,
        ));
        plan.push_fallback(openai_candidate(
            "must-not-run-after-output",
            "b",
            fallback_url,
        ));
        let transport = RoutedDesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            session_id: "turn-latch-test".into(),
            route_state: ActiveRouteState::from_plan_with_health(
                plan,
                super::super::failover::EndpointHealthRegistry::new(
                    std::time::Duration::from_secs(120),
                ),
            ),
            cancel: None,
            turn_output_started: Arc::new(AtomicBool::new(false)),
            turn_side_effect_started: Arc::new(AtomicBool::new(false)),
            db: unused_provider_db(),
            root_turn_id: None,
            mutation_permit: None,
            context_authorization: None,
            anonymous: false,
            durable_provider_required: false,
        };

        transport
            .complete(&[], &[], &RoundOptions::default())
            .await
            .expect("first round emits visible output");
        let error = transport
            .complete(&[], &[], &RoundOptions::default())
            .await
            .expect_err("later round must not replay the root turn elsewhere");

        assert!(matches!(error, TransportError::Retryable(_)));
        assert_eq!(primary_hits.load(Ordering::SeqCst), 4);
        assert_eq!(fallback_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            transport.route_state.current().endpoint_name,
            "turn-primary"
        );
    }

    #[tokio::test]
    async fn routed_transport_does_not_switch_after_a_prior_tool_side_effect_without_visible_text()
    {
        const DOWN: (&str, &str, &str) = (
            "503 Service Unavailable",
            "application/json",
            r#"{"error":{"message":"Service Unavailable","code":"circuit_open"}}"#,
        );
        let (primary_url, primary_hits) = serve_responses(vec![DOWN, DOWN, DOWN]);
        let (fallback_url, fallback_hits) = serve_responses(vec![(
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"unsafe-replay\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        )]);
        let mut plan = super::super::failover::RouteCandidatePlan::new(openai_candidate(
            "turn-primary",
            "a",
            primary_url,
        ));
        plan.push_fallback(openai_candidate("must-not-replay", "b", fallback_url));
        let transport = RoutedDesktopModelTransport {
            http: test_client(),
            events: Arc::new(super::super::events::CollectingEventSink::new()),
            session_id: "turn-side-effect-latch-test".into(),
            route_state: ActiveRouteState::from_plan_with_health(
                plan,
                super::super::failover::EndpointHealthRegistry::new(
                    std::time::Duration::from_secs(120),
                ),
            ),
            cancel: None,
            turn_output_started: Arc::new(AtomicBool::new(false)),
            turn_side_effect_started: Arc::new(AtomicBool::new(true)),
            db: unused_provider_db(),
            root_turn_id: None,
            mutation_permit: None,
            context_authorization: None,
            anonymous: false,
            durable_provider_required: false,
        };

        let error = transport
            .complete(&[], &[], &RoundOptions::default())
            .await
            .expect_err("unknown prior side effect must block provider replay");

        assert!(matches!(error, TransportError::Retryable(_)));
        assert_eq!(primary_hits.load(Ordering::SeqCst), 3);
        assert_eq!(fallback_hits.load(Ordering::SeqCst), 0);
    }
}
