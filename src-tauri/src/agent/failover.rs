// SPDX-License-Identifier: Apache-2.0
//! Per-turn model-route failover.
//!
//! The selected endpoint remains the user's preference. A plan is a snapshot
//! for one agent run; advancing it never mutates `Settings::default_endpoint`.

use crate::config::settings::ApiStyle;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const DEFAULT_ENDPOINT_COOLDOWN: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct RouteCandidate {
    pub endpoint_name: String,
    pub model_id: String,
    pub base_url: String,
    /// Opaque reference resolved lazily by the process-wide credential broker.
    pub credential_ref: Option<String>,
    /// Compatibility seam for headless/tests whose caller already owns a
    /// secret. Desktop route plans never put persisted credentials here.
    pub legacy_inline_api_key: Option<String>,
    pub supports_vision: bool,
    pub api_style: ApiStyle,
}

impl std::fmt::Debug for RouteCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteCandidate")
            .field("endpoint_name", &self.endpoint_name)
            .field("model_id", &self.model_id)
            .field("base_url", &self.base_url)
            .field("credential_ref", &self.credential_ref)
            .field(
                "legacy_inline_api_key",
                &self.legacy_inline_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("supports_vision", &self.supports_vision)
            .field("api_style", &self.api_style)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct RouteCandidatePlan {
    candidates: Vec<RouteCandidate>,
    initial_selection: InitialRouteSelection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitialRouteSelection {
    Preferred,
    HealthAware,
}

impl RouteCandidatePlan {
    /// Prefer the user's selected primary at the start of every new turn.
    /// Fallbacks are still available after a replay-safe failure.
    pub fn new(primary: RouteCandidate) -> Self {
        Self {
            candidates: vec![primary],
            initial_selection: InitialRouteSelection::Preferred,
        }
    }

    /// Let automatic routing skip a primary that is still in the short
    /// endpoint-unavailable cooldown window.
    pub fn new_automatic(primary: RouteCandidate) -> Self {
        Self {
            candidates: vec![primary],
            initial_selection: InitialRouteSelection::HealthAware,
        }
    }

    pub fn push_fallback(&mut self, candidate: RouteCandidate) {
        if self
            .candidates
            .iter()
            .all(|existing| existing.endpoint_name != candidate.endpoint_name)
        {
            self.candidates.push(candidate);
        }
    }

    pub fn candidates(&self) -> &[RouteCandidate] {
        &self.candidates
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderFailureClass {
    EndpointUnavailable,
    RateLimited,
    AuthExpired,
    CredentialUnavailable,
    QuotaExceeded,
    ContextOverflow,
    VisionUnsupported,
    FieldUnsupported,
    Fatal,
}

impl ProviderFailureClass {
    pub fn permits_endpoint_failover(self) -> bool {
        matches!(
            self,
            Self::EndpointUnavailable
                | Self::RateLimited
                | Self::CredentialUnavailable
                | Self::QuotaExceeded
        )
    }
}

/// Classify only failures that are safe to replay on a different endpoint
/// before any streamed output/tool side effect exists.
pub fn classify_provider_failure(message: &str) -> ProviderFailureClass {
    let lower = message.to_ascii_lowercase();
    if codefactory_agent_loop::context::is_context_overflow(message) {
        return ProviderFailureClass::ContextOverflow;
    }
    if codefactory_agent_loop::protocol::is_vision_rejection(message) {
        return ProviderFailureClass::VisionUnsupported;
    }
    if lower.contains("http 400")
        || lower.contains("bad request")
        || lower.contains("unsupported field")
        || lower.contains("max_tokens")
        || lower.contains("max_completion_tokens")
    {
        return ProviderFailureClass::FieldUnsupported;
    }
    if lower.contains("auth_expired")
        || lower.contains("invalid_grant")
        || lower.contains("refresh_token")
        || lower.contains("http 401")
        || lower.contains("401 unauthorized")
    {
        return ProviderFailureClass::AuthExpired;
    }
    if lower.contains("credential_access_required")
        || lower.contains("auth_missing")
        || lower.contains("http 403")
        || lower.contains("403 forbidden")
        || lower.contains("invalid api key")
        || lower.contains("invalid_api_key")
    {
        return ProviderFailureClass::CredentialUnavailable;
    }
    if lower.contains("insufficient_quota") || lower.contains("quota exceeded") {
        return ProviderFailureClass::QuotaExceeded;
    }
    if lower.contains("http 429")
        || lower.contains("429 too many requests")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
    {
        return ProviderFailureClass::RateLimited;
    }
    if lower.contains("biscuit_baker_service_me_circuit_open")
        || lower.contains("circuit_open")
        || lower.contains("http 408")
        || lower.contains("http 409")
        || lower.contains("http 425")
        || lower.contains("http 500")
        || lower.contains("http 502")
        || lower.contains("http 503")
        || lower.contains("http 504")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("error sending request")
        || lower.contains("error decoding response body")
    {
        return ProviderFailureClass::EndpointUnavailable;
    }
    ProviderFailureClass::Fatal
}

#[derive(Debug)]
struct EndpointHealthInner {
    cooldown: Duration,
    unavailable_since: Mutex<HashMap<String, Instant>>,
}

#[derive(Clone, Debug)]
pub struct EndpointHealthRegistry {
    inner: Arc<EndpointHealthInner>,
}

impl EndpointHealthRegistry {
    pub fn new(cooldown: Duration) -> Self {
        Self {
            inner: Arc::new(EndpointHealthInner {
                cooldown,
                unavailable_since: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn is_available(&self, endpoint_name: &str) -> bool {
        let mut states = self
            .inner
            .unavailable_since
            .lock()
            .expect("endpoint health mutex poisoned");
        match states.get(endpoint_name).copied() {
            Some(since) if since.elapsed() < self.inner.cooldown => false,
            Some(_) => {
                states.remove(endpoint_name);
                true
            }
            None => true,
        }
    }

    pub fn mark_unavailable(&self, endpoint_name: &str) {
        self.inner
            .unavailable_since
            .lock()
            .expect("endpoint health mutex poisoned")
            .insert(endpoint_name.to_string(), Instant::now());
    }

    pub fn mark_success(&self, endpoint_name: &str) {
        self.inner
            .unavailable_since
            .lock()
            .expect("endpoint health mutex poisoned")
            .remove(endpoint_name);
    }
}

static SHARED_ENDPOINT_HEALTH: OnceLock<EndpointHealthRegistry> = OnceLock::new();

pub fn shared_endpoint_health() -> &'static EndpointHealthRegistry {
    SHARED_ENDPOINT_HEALTH.get_or_init(|| EndpointHealthRegistry::new(DEFAULT_ENDPOINT_COOLDOWN))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteChange {
    pub from_endpoint: String,
    pub from_model: String,
    pub to_endpoint: String,
    pub to_model: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteAttemptSnapshot {
    pub endpoint: String,
    pub model: String,
    pub status: String,
    pub failure_code: Option<String>,
}

impl RouteChange {
    pub fn notice(&self) -> String {
        format!(
            "{} / {} 暂时不可用，已自动切换到 {} / {}，任务继续执行。",
            endpoint_label(&self.from_endpoint),
            self.from_model,
            endpoint_label(&self.to_endpoint),
            self.to_model,
        )
    }
}

fn endpoint_label(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "chatgpt" => "ChatGPT".into(),
        "deepseek" => "DeepSeek".into(),
        "openrouter" => "OpenRouter".into(),
        _ => name.to_string(),
    }
}

#[derive(Debug)]
struct ActiveRouteInner {
    candidates: Vec<RouteCandidate>,
    current_index: usize,
    failed_indices: HashSet<usize>,
    failures: Vec<(String, String, String)>,
    initial_route_change: Option<RouteChange>,
}

#[derive(Clone, Debug)]
pub struct ActiveRouteState {
    inner: Arc<Mutex<ActiveRouteInner>>,
    health: EndpointHealthRegistry,
}

impl ActiveRouteState {
    pub fn from_plan(plan: RouteCandidatePlan) -> Self {
        Self::from_plan_with_health(plan, shared_endpoint_health().clone())
    }

    pub fn from_plan_with_health(plan: RouteCandidatePlan, health: EndpointHealthRegistry) -> Self {
        assert!(
            !plan.candidates.is_empty(),
            "route candidate plan must contain a primary"
        );
        let current_index = match plan.initial_selection {
            InitialRouteSelection::Preferred => 0,
            InitialRouteSelection::HealthAware => plan
                .candidates
                .iter()
                .position(|candidate| health.is_available(&candidate.endpoint_name))
                .unwrap_or(0),
        };
        let initial_route_change = (current_index > 0).then(|| RouteChange {
            from_endpoint: plan.candidates[0].endpoint_name.clone(),
            from_model: plan.candidates[0].model_id.clone(),
            to_endpoint: plan.candidates[current_index].endpoint_name.clone(),
            to_model: plan.candidates[current_index].model_id.clone(),
            reason: "端点处于临时冷却期".into(),
        });
        Self {
            inner: Arc::new(Mutex::new(ActiveRouteInner {
                candidates: plan.candidates,
                current_index,
                failed_indices: HashSet::new(),
                failures: Vec::new(),
                initial_route_change,
            })),
            health,
        }
    }

    pub fn current(&self) -> RouteCandidate {
        let inner = self.inner.lock().expect("active route mutex poisoned");
        inner.candidates[inner.current_index].clone()
    }

    pub fn take_initial_route_change(&self) -> Option<RouteChange> {
        self.inner
            .lock()
            .expect("active route mutex poisoned")
            .initial_route_change
            .take()
    }

    pub fn advance_after_failure(&self, reason: &str) -> Option<RouteChange> {
        let mut inner = self.inner.lock().expect("active route mutex poisoned");
        let from_index = inner.current_index;
        let from = inner.candidates[from_index].clone();
        inner.failed_indices.insert(from_index);
        inner.failures.push((
            from.endpoint_name.clone(),
            from.model_id.clone(),
            reason.to_string(),
        ));
        if classify_provider_failure(reason) == ProviderFailureClass::EndpointUnavailable {
            self.health.mark_unavailable(&from.endpoint_name);
        }

        let next_index = ((from_index + 1)..inner.candidates.len()).find(|index| {
            !inner.failed_indices.contains(index)
                && self
                    .health
                    .is_available(&inner.candidates[*index].endpoint_name)
        })?;
        let to = inner.candidates[next_index].clone();
        inner.current_index = next_index;
        Some(RouteChange {
            from_endpoint: from.endpoint_name,
            from_model: from.model_id,
            to_endpoint: to.endpoint_name,
            to_model: to.model_id,
            reason: reason.to_string(),
        })
    }

    pub fn record_current_failure(&self, reason: &str) {
        let mut inner = self.inner.lock().expect("active route mutex poisoned");
        let current_index = inner.current_index;
        let current = inner.candidates[current_index].clone();
        inner.failed_indices.insert(current_index);
        if inner
            .failures
            .last()
            .is_none_or(|(endpoint, _, _)| endpoint != &current.endpoint_name)
        {
            inner.failures.push((
                current.endpoint_name.clone(),
                current.model_id,
                reason.to_string(),
            ));
        }
        if classify_provider_failure(reason) == ProviderFailureClass::EndpointUnavailable {
            self.health.mark_unavailable(&current.endpoint_name);
        }
    }

    pub fn mark_current_success(&self) {
        let current = self.current();
        self.health.mark_success(&current.endpoint_name);
    }

    pub fn attempt_snapshots(&self, succeeded: bool) -> Vec<RouteAttemptSnapshot> {
        let inner = self.inner.lock().expect("active route mutex poisoned");
        let mut attempts = inner
            .failures
            .iter()
            .map(|(endpoint, model, reason)| RouteAttemptSnapshot {
                endpoint: endpoint.clone(),
                model: model.clone(),
                status: "failed".into(),
                failure_code: Some(failure_code(reason).into()),
            })
            .collect::<Vec<_>>();
        if succeeded {
            let current = &inner.candidates[inner.current_index];
            attempts.push(RouteAttemptSnapshot {
                endpoint: current.endpoint_name.clone(),
                model: current.model_id.clone(),
                status: "succeeded".into(),
                failure_code: None,
            });
        }
        attempts
    }

    pub fn exhausted_error(&self, final_reason: &str) -> String {
        let mut inner = self.inner.lock().expect("active route mutex poisoned");
        let current = inner.candidates[inner.current_index].clone();
        if inner
            .failures
            .last()
            .is_none_or(|(endpoint, _, _)| endpoint != &current.endpoint_name)
        {
            inner.failures.push((
                current.endpoint_name,
                current.model_id,
                final_reason.to_string(),
            ));
        }
        let attempts = inner
            .failures
            .iter()
            .map(|(endpoint, model, reason)| {
                format!(
                    "{} / {}（{}）",
                    endpoint_label(endpoint),
                    model,
                    concise_reason(reason)
                )
            })
            .collect::<Vec<_>>()
            .join("；");
        format!(
            "所有可用模型端点均不可用：{attempts}。请检查服务状态或额度，或在模型选择器选择其他端点后重试。"
        )
    }
}

fn concise_reason(reason: &str) -> String {
    reason
        .lines()
        .next()
        .unwrap_or(reason)
        .chars()
        .take(160)
        .collect()
}

fn failure_code(reason: &str) -> &'static str {
    match classify_provider_failure(reason) {
        ProviderFailureClass::EndpointUnavailable => "ENDPOINT_UNAVAILABLE",
        ProviderFailureClass::RateLimited => "RATE_LIMITED",
        ProviderFailureClass::AuthExpired => "AUTH_EXPIRED",
        ProviderFailureClass::CredentialUnavailable => "CREDENTIAL_UNAVAILABLE",
        ProviderFailureClass::QuotaExceeded => "QUOTA_EXCEEDED",
        ProviderFailureClass::ContextOverflow => "CONTEXT_OVERFLOW",
        ProviderFailureClass::VisionUnsupported => "IMAGE_INPUT_UNSUPPORTED",
        ProviderFailureClass::FieldUnsupported => "FIELD_UNSUPPORTED",
        ProviderFailureClass::Fatal => "PROVIDER_FATAL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::ApiStyle;
    use std::time::Duration;

    fn route(name: &str, model: &str) -> RouteCandidate {
        RouteCandidate {
            endpoint_name: name.into(),
            model_id: model.into(),
            base_url: format!("https://{name}.example"),
            credential_ref: Some(format!("{name}-key-ref")),
            legacy_inline_api_key: Some(format!("super-secret-inline-{name}")),
            supports_vision: true,
            api_style: ApiStyle::Openai,
        }
    }

    #[test]
    fn classifies_failover_safe_and_actionable_provider_failures() {
        assert_eq!(
            classify_provider_failure(
                r#"HTTP 503 Service Unavailable: {"code":"biscuit_baker_service_me_circuit_open"}"#
            ),
            ProviderFailureClass::EndpointUnavailable
        );
        assert_eq!(
            classify_provider_failure("HTTP 429 Too Many Requests"),
            ProviderFailureClass::RateLimited
        );
        assert_eq!(
            classify_provider_failure("HTTP 401 Unauthorized"),
            ProviderFailureClass::AuthExpired
        );
        assert_eq!(
            classify_provider_failure("HTTP 403 Forbidden"),
            ProviderFailureClass::CredentialUnavailable
        );
        assert_eq!(
            classify_provider_failure("HTTP 400 Bad Request: max_tokens is unsupported"),
            ProviderFailureClass::FieldUnsupported
        );
    }

    #[test]
    fn route_candidate_debug_redacts_credentials() {
        let candidate = route("deepseek", "deepseek-v4-pro");
        let rendered = format!("{candidate:?}");
        assert!(!rendered.contains("super-secret-inline"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn active_route_skips_a_cooled_down_primary_and_stays_on_fallback() {
        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        health.mark_unavailable("chatgpt");
        let mut plan = RouteCandidatePlan::new_automatic(route("chatgpt", "gpt-5.5"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));

        let state = ActiveRouteState::from_plan_with_health(plan, health);

        assert_eq!(state.current().endpoint_name, "deepseek");
        assert_eq!(state.current().endpoint_name, "deepseek");
    }

    #[test]
    fn preferred_route_still_starts_with_the_users_selected_primary() {
        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        health.mark_unavailable("chatgpt");
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-5.5"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));

        let state = ActiveRouteState::from_plan_with_health(plan, health);

        assert_eq!(state.current().endpoint_name, "chatgpt");
        assert!(state.take_initial_route_change().is_none());
    }

    #[test]
    fn advancing_is_monotonic_and_success_clears_the_active_breaker() {
        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-5.5"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
        let state = ActiveRouteState::from_plan_with_health(plan, health.clone());

        let change = state
            .advance_after_failure("HTTP 503 Service Unavailable")
            .expect("fallback exists");
        assert_eq!(change.from_endpoint, "chatgpt");
        assert_eq!(change.to_endpoint, "deepseek");
        assert_eq!(state.current().endpoint_name, "deepseek");
        assert!(health.is_available("deepseek"));

        health.mark_unavailable("deepseek");
        assert!(!health.is_available("deepseek"));
        state.mark_current_success();
        assert!(health.is_available("deepseek"));
    }

    #[test]
    fn visits_three_routes_once_without_returning_to_a_failed_endpoint() {
        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        let mut plan = RouteCandidatePlan::new(route("a", "model-a"));
        plan.push_fallback(route("b", "model-b"));
        plan.push_fallback(route("c", "model-c"));
        let state = ActiveRouteState::from_plan_with_health(plan, health);

        assert_eq!(state.current().endpoint_name, "a");
        assert_eq!(
            state
                .advance_after_failure("HTTP 503")
                .expect("b is available")
                .to_endpoint,
            "b"
        );
        assert_eq!(
            state
                .advance_after_failure("HTTP 503")
                .expect("c is available")
                .to_endpoint,
            "c"
        );
        assert_eq!(state.current().endpoint_name, "c");
        assert!(state.advance_after_failure("HTTP 503").is_none());
        assert_eq!(state.current().endpoint_name, "c");
    }

    #[test]
    fn route_attempt_snapshot_records_failed_and_effective_routes_without_secrets() {
        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-5.5"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
        let state = ActiveRouteState::from_plan_with_health(plan, health);

        state
            .advance_after_failure("HTTP 503 Service Unavailable")
            .expect("fallback exists");
        let attempts = state.attempt_snapshots(true);

        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].endpoint, "chatgpt");
        assert_eq!(attempts[0].status, "failed");
        assert_eq!(
            attempts[0].failure_code.as_deref(),
            Some("ENDPOINT_UNAVAILABLE")
        );
        assert_eq!(attempts[1].endpoint, "deepseek");
        assert_eq!(attempts[1].status, "succeeded");
        assert!(attempts
            .iter()
            .all(|attempt| !format!("{attempt:?}").contains("deepseek-key")));
    }

    #[test]
    fn cooled_primary_records_an_initial_route_change_notice() {
        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        health.mark_unavailable("chatgpt");
        let mut plan = RouteCandidatePlan::new_automatic(route("chatgpt", "gpt-5.5"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
        let state = ActiveRouteState::from_plan_with_health(plan, health);

        let change = state
            .take_initial_route_change()
            .expect("cooldown skip is user-visible");
        assert_eq!(change.from_endpoint, "chatgpt");
        assert_eq!(change.to_endpoint, "deepseek");
        assert!(change.notice().contains("已自动切换到"));
        assert!(state.take_initial_route_change().is_none());
    }
}
