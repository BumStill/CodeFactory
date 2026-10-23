// SPDX-License-Identifier: Apache-2.0
//! Per-turn model-route failover.
//!
//! The selected endpoint remains the user's preference. A plan is a snapshot
//! for one agent run; advancing it never mutates `Settings::default_endpoint`.

use crate::config::settings::ApiStyle;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
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

impl RouteCandidate {
    /// M86 预过滤：这个候选现在就有可用凭据吗？
    ///
    /// ChatGPT 走订阅登录，不需要 API key；其余端点必须有非空的 inline key
    /// 或 credential ref。过去没有凭据的端点会被"选中"，然后在传输层变成
    /// `AUTH_MISSING`——于是失败说明只写得出真正尝试过的环节，被门禁预先
    /// 过滤掉的端点反而看不见。现在它是一步**记录在案**的预过滤。
    pub fn has_usable_credential(&self) -> bool {
        matches!(self.api_style, ApiStyle::Chatgpt)
            || self.legacy_inline_api_key.as_ref().is_some_and(|key| !key.trim().is_empty())
            || self.credential_ref.as_deref().is_some_and(|reference| !reference.trim().is_empty())
    }
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
    /// CF-QUOTA: endpoints this plan must not start on because they have reached
    /// their configured share of the current metering window, paired with the
    /// plain-language reason the session shows. A skip is a *preference* change,
    /// not a health failure: it never marks the endpoint unavailable, so the
    /// next turn (whose plan is rebuilt) brings it straight back once the window
    /// resets.
    quota_skips: Vec<(String, String)>,
    /// M152：达到"留余量"上限、但没有可用备选时保留首选端点，这里存要给
    /// 会话看的原因。它不改变当前路由，只负责把软偏好被放弃的理由说清楚。
    quota_keep_notice: Option<String>,
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
            quota_skips: Vec::new(),
            quota_keep_notice: None,
        }
    }

    /// Let automatic routing skip a primary that is still in the short
    /// endpoint-unavailable cooldown window.
    pub fn new_automatic(primary: RouteCandidate) -> Self {
        Self {
            candidates: vec![primary],
            initial_selection: InitialRouteSelection::HealthAware,
            quota_skips: Vec::new(),
            quota_keep_notice: None,
        }
    }

    /// CF-QUOTA: record that `endpoint` has reached its configured share of the
    /// current window and must hand the turn to the next usable candidate. The
    /// `notice` is the exact sentence the session shows.
    pub fn skip_for_quota(&mut self, endpoint: &str, notice: String) {
        if self.quota_skips.iter().all(|(name, _)| name != endpoint) {
            self.quota_skips.push((endpoint.to_string(), notice));
        }
    }

    /// Whether `endpoint` was skipped because it reached its window cap.
    pub fn quota_notice(&self, endpoint: &str) -> Option<&str> {
        self.quota_skips
            .iter()
            .find(|(name, _)| name == endpoint)
            .map(|(_, notice)| notice.as_str())
    }

    /// M152/R1：记录"留余量"没能生效的原因。软偏好被放弃时首选端点留在原位，
    /// 这句原因照原样出现在会话里，用户不必猜为什么还在用 GPT。
    pub fn keep_capped_primary(&mut self, notice: String) {
        self.quota_keep_notice = Some(notice);
    }

    pub fn quota_keep_notice(&self) -> Option<&str> {
        self.quota_keep_notice.as_deref()
    }

    pub fn push_fallback(&mut self, candidate: RouteCandidate) {
        if self
            .candidates
            .iter()
            .all(|existing| existing.endpoint_name != candidate.endpoint_name || existing.model_id != candidate.model_id)
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
    /// U25: the request never reached a verdict — the network path itself
    /// (DNS, TCP connect, TLS, read timeout, reset) or an anonymous gateway
    /// error answered instead. Nothing here says the request was wrong, so the
    /// objective waits this out instead of concluding that the task cannot be
    /// done. Measured on 2026-10-08: DeepSeek's transport failed for ~3 minutes
    /// and recovered on its own, while two sessions had already been declared
    /// failed after ~60 seconds.
    TransportUnreachable,
    /// The endpoint *answered*, and the answer was a refusal: the model does not
    /// exist, is deprecated, or the service says it is switched off. Waiting
    /// never changes that from this machine, so this keeps the short path.
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
            Self::TransportUnreachable
                | Self::EndpointUnavailable
                | Self::RateLimited
                | Self::CredentialUnavailable
                | Self::QuotaExceeded
        )
    }
}

/// Classify only failures that are safe to replay on a different endpoint
/// before any streamed output/tool side effect exists.
pub fn classify_provider_failure(message: &str) -> ProviderFailureClass {
    if let Some(rest) = message.strip_prefix("首选端点失败：") {
        if let Some((primary, _)) = rest.split_once('\n') {
            return classify_provider_failure(primary);
        }
    }
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
    // U25 — the endpoint answered, and the answer was a refusal. The model is
    // gone (404 "model not found"), deprecated, or the service says an explicit
    // circuit is open. No amount of waiting from this machine changes any of
    // these, so they keep converging on the short path instead of holding the
    // user in "等待中".
    if lower.contains("model not found")
        || lower.contains("model_not_found")
        || lower.contains("model does not exist")
        || lower.contains("no such model")
        || lower.contains("does not support the model")
        || lower.contains("model has been deprecated")
        || lower.contains("model is deprecated")
        || lower.contains("deprecated")
        || lower.contains("model unavailable")
        || lower.contains("model is unavailable")
        || lower.contains("model_unavailable")
        || (lower.contains("model")
            && (lower.contains("is unavailable")
                || lower.contains("not available")
                || lower.contains("no longer available")))
        || lower.contains("biscuit_baker_service_me_circuit_open")
        || lower.contains("circuit_open")
    {
        return ProviderFailureClass::EndpointUnavailable;
    }
    // U25 — nothing answered: the transport itself failed. A bare 502/503/504
    // (no explicit body saying what is unavailable) lands here too, because a
    // gateway with nothing useful to say is a transport condition as far as this
    // machine can tell.
    if lower.contains("http 408")
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
        || lower.contains("connection closed before message completed")
        || lower.contains("broken pipe")
        || lower.contains("unexpected eof")
        || lower.contains("error sending request")
        || lower.contains("error decoding response body")
        || lower.contains("dns error")
        || lower.contains("dns failure")
        || lower.contains("failed to lookup address")
        || lower.contains("no address found")
        || lower.contains("name or service not known")
        || lower.contains("network is unreachable")
        || lower.contains("network unreachable")
        || lower.contains("tls handshake")
        || lower.contains("tlsv1")
        || lower.contains("certificate verify failed")
        || lower.contains("ssl error")
    {
        return ProviderFailureClass::TransportUnreachable;
    }
    // U25: an explicit overload statement ("our servers are overloaded", "try
    // again later", an upstream 529) is checked *after* the transport signals,
    // because the provider SDK's matcher also fires on a bare "503 Service
    // Unavailable" with no body — and a gateway saying nothing is a transport
    // condition, not a statement about the task. This keeps its short path.
    if codefactory_agent_loop::context::is_provider_overloaded(message) {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteChangeKind {
    /// The previous endpoint failed and a later candidate took over.
    Unavailable,
    /// CF-QUOTA: the previous endpoint reached its configured share of the
    /// current metering window and handed the turn over to leave the user room.
    QuotaCap,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteChange {
    pub from_endpoint: String,
    pub from_model: String,
    pub to_endpoint: String,
    pub to_model: String,
    pub reason: String,
    pub kind: RouteChangeKind,
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
        // A quota handover already carries its own plain-language sentence
        // ("GPT 本五小时窗口已用 80%，为给你留余量已切到 DeepSeek，约 18:20 恢复。"),
        // so it is shown verbatim instead of the generic "temporarily
        // unavailable" line, which would misdescribe a deliberate handover.
        if self.kind == RouteChangeKind::QuotaCap {
            return self.reason.clone();
        }
        format!(
            "{} / {} 暂时不可用，已自动切换到 {} / {}，任务继续执行。",
            endpoint_label(&self.from_endpoint),
            self.from_model,
            endpoint_label(&self.to_endpoint),
            self.to_model,
        )
    }
}

pub(crate) fn endpoint_label(name: &str) -> String {
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
    usage_skips: Vec<RouteAttemptSnapshot>,
    usage_settings: Option<Arc<tokio::sync::RwLock<crate::config::settings::Settings>>>,
    overload_streak: usize,
}

#[derive(Clone, Debug)]
pub struct ActiveRouteState {
    inner: Arc<Mutex<ActiveRouteInner>>,
    health: EndpointHealthRegistry,
}

/// M123: a preflight result, never a provider request failure.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ModelUnavailable {
    pub resume_at_ms: Option<i64>,
    pub notice: String,
}

impl ModelUnavailable {
    const MARKER: &'static str = "MODEL_UNAVAILABLE:";

    pub fn from_decision(decision: super::usage_windows::AvailabilityDecision) -> Self {
        let reasons = decision.causes.iter().map(|cause| cause.sentence()).collect::<Vec<_>>().join("；");
        Self {
            resume_at_ms: decision.earliest_ms.map(|at| at.saturating_add(250)),
            notice: if decision.waitable {
                format!("当前没有可用模型：{reasons}；{}。", decision.notice.split('；').find(|part| part.contains("自动继续")).unwrap_or("届时自动继续"))
            } else {
                format!("已结束：没有可用模型（{reasons}）；请在设置中配置凭据、调整使用时段或选择可用模型。")
            },
        }
    }

    pub fn encode(&self) -> String {
        format!("{}{}", Self::MARKER, serde_json::to_string(self).expect("availability serializes"))
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (_, payload) = text.split_once(Self::MARKER)?;
        serde_json::Deserializer::from_str(payload).into_iter::<Self>().next()?.ok()
    }

    pub fn decision(&self, current: &super::objective::ObjectiveSnapshot, root: &str) -> anyhow::Result<super::objective::DecisionEnvelope> {
        use super::objective::{DecisionRouter, RouteSignal, RecoveryDomain, DecisionType, ObjectiveStatus};
        let mut decision = DecisionRouter::route(current, RouteSignal::CapabilityRestored {
            domain: RecoveryDomain::Chat,
            reason: "no_model_available".into(),
            next_observation_at: self.resume_at_ms.unwrap_or_default(),
            resume_cursor: Some(root.into()),
        })?;
        decision.failure_signature = None;
        if self.resume_at_ms.is_none() {
            decision.status = ObjectiveStatus::Failed;
            decision.decision_type = DecisionType::FailedInternal;
            decision.remediation_id = None;
            decision.next_observation_at = None;
            decision.recovery_owner = None;
            decision.next_action_authorized = false;
        }
        Ok(decision)
    }
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
        let preferred_index = match plan.initial_selection {
            InitialRouteSelection::Preferred => 0,
            InitialRouteSelection::HealthAware => plan
                .candidates
                .iter()
                .position(|candidate| health.is_available(&candidate.endpoint_name))
                .unwrap_or(0),
        };
        // CF-QUOTA: an endpoint that has reached its window cap hands the turn
        // to the next usable candidate — whichever selection mode put it first.
        // This is a preference change, not a health failure: the endpoint stays
        // healthy, so the very next turn (rebuilt from a fresh ledger read)
        // returns to it as soon as the window resets.
        let quota_notice = plan
            .quota_notice(&plan.candidates[preferred_index].endpoint_name)
            .map(str::to_string);
        let keep_notice = plan.quota_keep_notice.clone();
        let current_index = if quota_notice.is_some() {
            let quota_skips = &plan.quota_skips;
            plan.candidates
                .iter()
                .enumerate()
                .skip(preferred_index + 1)
                .find(|(_, candidate)| {
                    health.is_available(&candidate.endpoint_name)
                        // M152/R2：留余量的落点必须真的能用。没有凭据的端点在这里
                        // 就被排除，否则它只会在传输层变成 AUTH_MISSING，连败几次
                        // 之后把整轮任务结束掉。
                        && candidate.has_usable_credential()
                        && quota_skips
                            .iter()
                            .all(|(name, _)| name != &candidate.endpoint_name)
                })
                .map(|(index, _)| index)
                .unwrap_or(preferred_index)
        } else {
            preferred_index
        };
        let initial_route_change = if current_index > 0 {
            Some(RouteChange {
                from_endpoint: plan.candidates[0].endpoint_name.clone(),
                from_model: plan.candidates[0].model_id.clone(),
                to_endpoint: plan.candidates[current_index].endpoint_name.clone(),
                to_model: plan.candidates[current_index].model_id.clone(),
                reason: quota_notice
                    .clone()
                    .unwrap_or_else(|| "端点处于临时冷却期".into()),
                kind: if quota_notice.is_some() {
                    RouteChangeKind::QuotaCap
                } else {
                    RouteChangeKind::Unavailable
                },
            })
        } else {
            // M152/R1：没有可用的留余量落点时留在首选端点上，但要说明原因。
            keep_notice
                .filter(|_| preferred_index == 0)
                .map(|notice| RouteChange {
                    from_endpoint: plan.candidates[0].endpoint_name.clone(),
                    from_model: plan.candidates[0].model_id.clone(),
                    to_endpoint: plan.candidates[0].endpoint_name.clone(),
                    to_model: plan.candidates[0].model_id.clone(),
                    reason: notice,
                    kind: RouteChangeKind::QuotaCap,
                })
        };
        Self {
            inner: Arc::new(Mutex::new(ActiveRouteInner {
                candidates: plan.candidates,
                current_index,
                failed_indices: HashSet::new(),
                failures: Vec::new(),
                initial_route_change,
                usage_skips: Vec::new(),
                usage_settings: None,
                overload_streak: 0,
            })),
            health,
        }
    }

    pub fn attach_usage_settings(&self, settings: Arc<tokio::sync::RwLock<crate::config::settings::Settings>>) {
        self.inner.lock().expect("active route mutex poisoned").usage_settings = Some(settings);
    }

    /// Called before every POST, including retries; waits are not failures.
    pub async fn admit_usage_window(&self, cancel: Option<&AtomicBool>, ignore_windows: bool, events: Option<&dyn crate::agent::events::EventSink>) -> Result<Option<RouteChange>, String> {
        self.admit_availability(cancel, ignore_windows, events, false).await
    }

    pub(crate) async fn admit_model_availability(&self, cancel: Option<&AtomicBool>, ignore_windows: bool, events: Option<&dyn crate::agent::events::EventSink>) -> Result<Option<RouteChange>, String> {
        self.admit_availability(cancel, ignore_windows, events, true).await
    }

    async fn admit_availability(&self, cancel: Option<&AtomicBool>, ignore_windows: bool, events: Option<&dyn crate::agent::events::EventSink>, durable_wait: bool) -> Result<Option<RouteChange>, String> {
        let settings = self.inner.lock().expect("active route mutex poisoned").usage_settings.clone();
        let Some(settings) = settings else { return Ok(None); };
        let mut wait_notified = false;
        loop {
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) { return Err("cancelled".into()); }
            let now = super::usage_windows::now_ms();
            let config = settings.read().await;
            if !durable_wait && config.model_usage_windows.is_empty() && config.endpoint_usage_windows.is_empty() { return Ok(None); }
            let delay = {
            let mut inner = self.inner.lock().expect("active route mutex poisoned");
            let from = inner.candidates[inner.current_index].clone();
            let mut next_available = None;
            let mut skipped = Vec::new();
            // CF-WINDOW-R4/R5：换模型和等待都要如实说出原因，所以分开记两类。
            let mut window_blocked: Vec<String> = Vec::new();
            let mut quota_blocked: Vec<String> = Vec::new();
            // M86：等待说明要列出**全部**原因（时段、额度、缺凭据）与最早可用时间。
            let mut causes: Vec<super::usage_windows::WaitCause> = Vec::new();
            let selected = inner.candidates.iter().enumerate().find_map(|(index, route)| {
                let label = endpoint_label(&route.endpoint_name);
                if inner.failed_indices.contains(&index) || !self.health.is_available(&route.endpoint_name) { return None; }
                if let Some(rule) = super::usage_windows::effective_rule(&config, &route.endpoint_name, &route.model_id) {
                    let status = rule.evaluate(now);
                    if !status.available && !ignore_windows {
                        skipped.push(RouteAttemptSnapshot { endpoint: route.endpoint_name.clone(), model: route.model_id.clone(), status: "skipped".into(), failure_code: Some(format!("outside_usage_window:{}", serde_json::json!({"rule":rule,"next_available_ms":status.next_transition_ms}))) });
                        if let Some(next) = status.next_transition_ms { next_available = Some(next_available.map_or(next, |old: i64| old.min(next))); }
                        causes.push(super::usage_windows::WaitCause::new(label, super::usage_windows::WaitCauseKind::UsageWindow, status.next_transition_ms));
                        window_blocked.push(route.model_id.clone());
                        return None;
                    }
                }
                if config.endpoint_is_subscription(&route.endpoint_name) {
                    let ledger = super::quota_cap::shared_quota_ledger();
                    let cap = config.quota_cap_for(&route.endpoint_name);
                    let budget = super::quota_cap::SubscriptionBudget::default();
                    if let Some(breach) = ledger.evaluate(&route.endpoint_name, cap, budget, now).or_else(|| ledger.evaluate(&route.base_url, cap, budget, now)) {
                        // M152/R2：没有凭据的端点先被排除，绝不能让"留余量"把
                        // 这一轮交给一个只会在传输层变成 AUTH_MISSING 的路由。
                        if !route.has_usable_credential() && !ignore_windows {
                            skipped.push(RouteAttemptSnapshot { endpoint: route.endpoint_name.clone(), model: route.model_id.clone(), status: "skipped".into(), failure_code: Some("route_prefilter:missing_credential".into()) });
                            causes.push(super::usage_windows::WaitCause::new(label, super::usage_windows::WaitCauseKind::MissingCredential, None));
                            return None;
                        }
                        // M86：额度门禁的预过滤同样是一步要写进路由记录的动作。
                        skipped.push(RouteAttemptSnapshot { endpoint: route.endpoint_name.clone(), model: route.model_id.clone(), status: "skipped".into(), failure_code: Some(format!("subscription_quota_cap:{}", serde_json::json!({"resets_at_ms":breach.resets_at_ms}))) });
                        if let Some(next) = breach.resets_at_ms { next_available = Some(next_available.map_or(next, |old: i64| old.min(next))); }
                        causes.push(super::usage_windows::WaitCause::new(format!("{label} 已到你设的 {}% 上限", breach.cap_percent), super::usage_windows::WaitCauseKind::QuotaCap, breach.resets_at_ms));
                        quota_blocked.push(route.model_id.clone());
                        return None;
                    }
                }
                // M86：没有凭据的端点在这里就被挡下，并且留下记录——而不是
                // 被"选中"后在传输层变成 AUTH_MISSING，让失败说明只剩它一条。
                if !route.has_usable_credential() && !ignore_windows {
                    skipped.push(RouteAttemptSnapshot { endpoint: route.endpoint_name.clone(), model: route.model_id.clone(), status: "skipped".into(), failure_code: Some("route_prefilter:missing_credential".into()) });
                    causes.push(super::usage_windows::WaitCause::new(label, super::usage_windows::WaitCauseKind::MissingCredential, None));
                    return None;
                }
                Some(index)
            });
            for skip in skipped {
                if !inner.usage_skips.iter().any(|old| old == &skip) { inner.usage_skips.push(skip); }
            }
            let blocked_causes = |window_blocked: &[String], quota_blocked: &[String], next_available: Option<i64>| {
                let resume = next_available
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|at| at.with_timezone(&chrono_tz::Asia::Shanghai).format("%m-%d %H:%M").to_string());
                let mut causes: Vec<String> = Vec::new();
                if !window_blocked.is_empty() {
                    causes.push(format!(
                        "{} 不在允许使用时段{}",
                        window_blocked.join("、"),
                        resume.as_deref().map(|clock| format!("，{clock} 起可用")).unwrap_or_default()
                    ));
                }
                if !quota_blocked.is_empty() {
                    causes.push(format!("{} 已达订阅用量上限", quota_blocked.join("、")));
                }
                causes
            };
            if let Some(index) = selected {
                inner.current_index = index;
                let to = inner.candidates[index].clone();
                return Ok((from.endpoint_name != to.endpoint_name || from.model_id != to.model_id).then(|| RouteChange {
                    from_endpoint: from.endpoint_name.clone(), from_model: from.model_id,
                    to_endpoint: to.endpoint_name.clone(), to_model: to.model_id.clone(),
                    reason: format!(
                        "已切到 {}{}",
                        to.model_id,
                        blocked_causes(&window_blocked, &quota_blocked, next_available)
                            .first()
                            .map(|cause| format!("：{cause}"))
                            .unwrap_or_default()
                    ),
                    kind: RouteChangeKind::QuotaCap,
                }));
            }
            // CF-WINDOW-R5 / M86：判定"该等"还是"该报错"。时间会让时段到点、
            // 让额度重置；缺凭据不会自己好，所以它只作为原因出现，不构成等待。
            let decision = super::usage_windows::availability_decision(causes);
            if durable_wait && !decision.causes.is_empty() {
                let unavailable = ModelUnavailable::from_decision(decision);
                if let Some(events) = events {
                    events.emit(crate::openrouter::types::StreamEvent::TransportRetry {
                        label: "等唤醒".into(), attempt: 0, max_attempts: 0,
                        delay_ms: unavailable.resume_at_ms.map(|at| at.saturating_sub(now).max(0) as u64).unwrap_or(0),
                        reason: unavailable.notice.clone(),
                    });
                }
                return Err(unavailable.encode());
            }
            if !decision.causes.is_empty() && decision.causes.iter().all(|cause| cause.kind == super::usage_windows::WaitCauseKind::MissingCredential) {
                // 例如全部候选都没配凭据：等也不会变好，交给原有的传输层错误
                // 与失败说明，不在这里空转。
                return Ok(None);
            }
            if !wait_notified {
                if let Some(events) = events {
                    let mut origins: Vec<&str> = Vec::new();
                    if !window_blocked.is_empty() { origins.push("outside_usage_window"); }
                    if !quota_blocked.is_empty() { origins.push("subscription_quota_cap"); }
                    if decision.causes.iter().any(|cause| cause.kind == super::usage_windows::WaitCauseKind::MissingCredential) { origins.push("missing_credential"); }
                    // 会话里显示的就是 reason：全部原因 + 最早可用时间 + 这不
                    // 是技术失败。label 是短标签，界面用 reason 那一行。
                    let _ = &from;
                    events.emit(crate::openrouter::types::StreamEvent::TransportRetry { label: "模型使用时段等待".into(), attempt: 0, max_attempts: 0, delay_ms: 1000, reason: format!("{}（来源：{}）", decision.notice, origins.join(" + ")) });
                }
                wait_notified = true;
            }
            decision.earliest_ms.map(|next| (next - now).clamp(1, 1000) as u64).unwrap_or(1000)
            };
            drop(config);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
    }

    pub fn current(&self) -> RouteCandidate {
        let inner = self.inner.lock().expect("active route mutex poisoned");
        inner.candidates[inner.current_index].clone()
    }

    /// Secret-free, immutable route identity used by the provider write-ahead
    /// ledger. Credential refs, inline API keys, and base URLs are excluded.
    pub fn candidate_identity_snapshot(&self) -> Vec<(String, String)> {
        let inner = self.inner.lock().expect("active route mutex poisoned");
        inner
            .candidates
            .iter()
            .map(|candidate| (candidate.endpoint_name.clone(), candidate.model_id.clone()))
            .collect()
    }

    /// U30: is there a *different* endpoint this run could move to right now?
    ///
    /// Deliberately the same predicate [`Self::advance_after_failure`] uses —
    /// forward-only, not already failed, and not inside the endpoint cooldown —
    /// so "a fallback is available" can never disagree with what advancing
    /// would actually do. A `fixed` plan holds exactly the model the user
    /// pinned, so this is `false` there and a pinned model is never swapped out.
    fn candidate_available(&self, inner: &ActiveRouteInner, index: usize) -> bool {
        let route = &inner.candidates[index];
        if inner.failed_indices.contains(&index) || !self.health.is_available(&route.endpoint_name)
            || !route.has_usable_credential() { return false; }
        let Some(settings) = &inner.usage_settings else { return true; };
        // Fail closed while settings are being edited; admission rechecks before POST.
        let Ok(config) = settings.try_read() else { return false; };
        let now = super::usage_windows::now_ms();
        if super::usage_windows::effective_rule(&config, &route.endpoint_name, &route.model_id)
            .is_some_and(|rule| !rule.evaluate(now).available) { return false; }
        if config.endpoint_is_subscription(&route.endpoint_name) {
            let ledger = super::quota_cap::shared_quota_ledger();
            let cap = config.quota_cap_for(&route.endpoint_name);
            let budget = super::quota_cap::SubscriptionBudget::default();
            if ledger.evaluate(&route.endpoint_name, cap, budget, now)
                .or_else(|| ledger.evaluate(&route.base_url, cap, budget, now)).is_some() { return false; }
        }
        true
    }

    pub fn has_available_fallback(&self) -> bool {
        let inner = self.inner.lock().expect("active route mutex poisoned");
        ((inner.current_index + 1)..inner.candidates.len())
            .any(|index| self.candidate_available(&inner, index))
    }

    /// Count only consecutive overloads on the current route, across complete calls.
    pub fn is_overload(reason: &str) -> bool {
        let lower = reason.to_ascii_lowercase();
        !super::model_transport::is_quota_exhausted(reason)
            && !lower.contains("circuit_open")
            && (lower.contains("overloaded") || lower.contains("http 529")
                || lower.contains("http 503"))
    }

    pub fn overload_switch_due(&self, reason: &str) -> bool {
        let mut inner = self.inner.lock().expect("active route mutex poisoned");
        if Self::is_overload(reason) {
            inner.overload_streak = inner.overload_streak.saturating_add(1);
            inner.overload_streak >= 2
        } else {
            inner.overload_streak = 0;
            true
        }
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
        // U30: every class that permits endpoint failover — unreachable, model
        // gone, sign-in expired, rate limited, quota used up — also puts this
        // endpoint into the cooldown, so the task does not keep hammering a
        // preferred endpoint whose quota is spent before it resets.
        if classify_provider_failure(reason).permits_endpoint_failover() {
            self.health.mark_unavailable(&from.endpoint_name);
        }

        let next_index = ((from_index + 1)..inner.candidates.len())
            .find(|index| self.candidate_available(&inner, *index))?;
        inner.overload_streak = 0;
        let to = inner.candidates[next_index].clone();
        inner.current_index = next_index;
        Some(RouteChange {
            from_endpoint: from.endpoint_name,
            from_model: from.model_id,
            to_endpoint: to.endpoint_name,
            to_model: to.model_id,
            reason: reason.to_string(),
            kind: RouteChangeKind::Unavailable,
        })
    }

    pub fn record_current_failure(&self, reason: &str) {
        // A recoverable saturation is not an exhausted candidate. The durable
        // provider episode already owns the retry time; keep preflight eligible.
        if Self::is_overload(reason) { return; }
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
        if classify_provider_failure(reason).permits_endpoint_failover() {
            self.health.mark_unavailable(&current.endpoint_name);
        }
    }

    pub fn mark_current_success(&self) {
        self.inner.lock().expect("active route mutex poisoned").overload_streak = 0;
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
        attempts.extend(inner.usage_skips.clone());
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

    pub fn primary_failure_reason(&self) -> Option<String> {
        self.inner.lock().expect("active route mutex poisoned")
            .failures.first().map(|(_, _, reason)| reason.clone())
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
        let primary = inner.failures.first().map(|(_, _, reason)| concise_reason(reason))
            .unwrap_or_else(|| concise_reason(final_reason));
        let attempts = inner.candidates.iter().filter_map(|candidate| {
            let label = format!("{} / {}", endpoint_label(&candidate.endpoint_name), candidate.model_id);
            if let Some((_, _, reason)) = inner.failures.iter().find(|(endpoint, model, _)|
                endpoint == &candidate.endpoint_name && model == &candidate.model_id) {
                return Some(format!("{label}（{}）", concise_reason(reason)));
            }
            inner.usage_skips.iter().find(|skip| skip.endpoint == candidate.endpoint_name
                && skip.model == candidate.model_id).map(|skip| {
                let next = skip.failure_code.as_deref()
                    .and_then(|code| code.strip_prefix("outside_usage_window:"))
                    .and_then(|data| serde_json::from_str::<serde_json::Value>(data).ok())
                    .and_then(|data| data["next_available_ms"].as_i64())
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|at| at.with_timezone(&chrono_tz::Asia::Shanghai).format("%m-%d %H:%M").to_string());
                format!("{label}（不在允许使用时段，已跳过{}）",
                    next.map(|clock| format!("，{clock} 起可用")).unwrap_or_default())
            })
        }).collect::<Vec<_>>().join("；");
        format!(
            "首选端点失败：{primary}\n所有可用模型端点均不可用：{attempts}。目标与失败证据已保留；系统将按退避策略重新观测可用路由。"
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
        ProviderFailureClass::TransportUnreachable => "TRANSPORT_UNREACHABLE",
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

    #[tokio::test]
    async fn usage_windows_same_endpoint_models_route_independently_and_audit_skip() {
        let mut plan = RouteCandidatePlan::new(route("synthetic", "pro"));
        plan.push_fallback(route("synthetic", "flash"));
        let state = ActiveRouteState::from_plan_with_health(plan, EndpointHealthRegistry::new(Duration::from_secs(120)));
        let mut settings = crate::config::settings::Settings::default();
        let blocked = super::super::usage_windows::UsageWindow {
            mode: "custom".into(), timezone: Some("UTC".into()), weekdays: vec![], periods: vec![],
            exclude_holidays: false, invert: false, calendar: Default::default(),
        };
        settings.model_usage_windows.insert(super::super::usage_windows::model_key("synthetic", "pro"), blocked);
        let live = Arc::new(tokio::sync::RwLock::new(settings));
        state.attach_usage_settings(live.clone());
        let change = state.admit_usage_window(None, false, None).await.unwrap().unwrap();
        assert_eq!(change.to_model, "flash");
        assert_eq!(state.current().model_id, "flash");
        assert!(state.attempt_snapshots(true).iter().any(|attempt| attempt.status == "skipped" && attempt.model == "pro" && attempt.failure_code.as_deref().unwrap().starts_with("outside_usage_window:")));
        live.write().await.model_usage_windows.get_mut(&super::super::usage_windows::model_key("synthetic", "pro")).unwrap().mode = "anytime".into();
        state.admit_usage_window(None, false, None).await.unwrap();
        assert_eq!(state.current().model_id, "pro", "restored preference on next request");
    }

    #[tokio::test]
    async fn usage_windows_wait_observes_live_config_without_health_failure() {
        let state = ActiveRouteState::from_plan_with_health(RouteCandidatePlan::new(route("synthetic", "pro")), EndpointHealthRegistry::new(Duration::from_secs(120)));
        let mut settings = crate::config::settings::Settings::default();
        let key = super::super::usage_windows::model_key("synthetic", "pro");
        let mut blocked = super::super::usage_windows::presets().remove("deepseek_off_peak").unwrap();
        blocked.mode = "custom".into(); blocked.weekdays.clear(); blocked.invert = false;
        settings.model_usage_windows.insert(key.clone(), blocked);
        let live = Arc::new(tokio::sync::RwLock::new(settings));
        state.attach_usage_settings(live.clone());
        let cloned = state.clone();
        let waiter = tokio::spawn(async move { cloned.admit_usage_window(None, false, None).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished(), "blocked request must wait");
        live.write().await.model_usage_windows.get_mut(&key).unwrap().mode = "anytime".into();
        assert!(tokio::time::timeout(Duration::from_secs(3), waiter).await.unwrap().unwrap().is_ok());
        assert!(state.health.is_available("synthetic"));
        assert!(state.inner.lock().unwrap().failed_indices.is_empty());
    }

    /// CF-WINDOW-R4：一轮进行中跨过时段边界。时钟注入到北京时间 08:59 的
    /// 官方闲时，请求直接放行；把时钟推过 09:00 边界后，**下一次**请求才换
    /// 模型（已发出的请求不中途掐断），会话与端点保持不变，换模型的原因写进
    /// 记录。
    #[tokio::test]
    async fn a_request_that_crosses_the_off_peak_boundary_switches_model_and_audits_why() {
        let _serial = crate::agent::usage_windows::clock_test_lock();
        let clock = |at: &str| {
            chrono::DateTime::parse_from_rfc3339(&format!("2026-10-13T{at}:00+08:00"))
                .unwrap()
                .timestamp_millis()
        };
        let mut plan = RouteCandidatePlan::new(route("deepseek", "deepseek-v4-pro"));
        plan.push_fallback(route("deepseek", "deepseek-flash"));
        let state = ActiveRouteState::from_plan_with_health(plan, EndpointHealthRegistry::new(Duration::from_secs(120)));
        let mut settings = crate::config::settings::Settings::default();
        let pro = crate::agent::usage_windows::model_key("deepseek", "deepseek-v4-pro");
        settings.model_usage_windows.insert(pro.clone(), crate::agent::usage_windows::presets().remove("deepseek_off_peak").unwrap());
        // 同一端点、同一供应商下的另一个模型任何时候都可用。
        settings.model_usage_windows.insert(
            crate::agent::usage_windows::model_key("deepseek", "deepseek-flash"),
            crate::agent::usage_windows::UsageWindow { mode: "anytime".into(), timezone: None, weekdays: vec![], periods: vec![], exclude_holidays: false, invert: false, calendar: Default::default() },
        );
        let live = Arc::new(tokio::sync::RwLock::new(settings));
        state.attach_usage_settings(live.clone());

        crate::agent::usage_windows::set_clock_override_ms(Some(clock("08:59")));
        assert!(state.admit_usage_window(None, false, None).await.unwrap().is_none(), "官方闲时不该换模型");
        assert_eq!(state.current().model_id, "deepseek-v4-pro");

        crate::agent::usage_windows::set_clock_override_ms(Some(clock("09:00")));
        let change = state.admit_usage_window(None, false, None).await.unwrap().expect("跨过边界后必须换模型");
        assert_eq!(change.from_model, "deepseek-v4-pro");
        assert_eq!(change.to_model, "deepseek-flash");
        assert_eq!(state.current().endpoint_name, "deepseek", "只换模型，不换会话/端点");
        assert!(change.reason.contains("deepseek-v4-pro") && change.reason.contains("deepseek-flash"), "{}", change.reason);

        let journal = state.attempt_snapshots(false);
        let skip = journal
            .iter()
            .find(|attempt| attempt.model == "deepseek-v4-pro" && attempt.status == "skipped")
            .expect("被时段挡下的模型必须有记录");
        let code = skip.failure_code.clone().unwrap_or_default();
        assert!(code.starts_with("outside_usage_window:"), "{code}");
        assert!(code.contains("next_available_ms") && code.contains("\"mode\":\"official_off_peak\""), "记录要带上所用规则与下一个可用时间：{code}");
        crate::agent::usage_windows::set_clock_override_ms(None);
    }

    /// CF-WINDOW-R5：所有候选模型都不在允许时段时进入等待，由注入时钟推进到
    /// 边界后自动继续；等待既不消耗技术恢复次数，也不是"恢复耗尽"。
    #[tokio::test]
    async fn a_blocked_fleet_waits_for_the_boundary_and_resumes_without_burning_recovery() {
        let _serial = crate::agent::usage_windows::clock_test_lock();
        let clock = |at: &str| {
            chrono::DateTime::parse_from_rfc3339(&format!("2026-10-13T{at}:00+08:00"))
                .unwrap()
                .timestamp_millis()
        };
        let state = ActiveRouteState::from_plan_with_health(RouteCandidatePlan::new(route("deepseek", "deepseek-v4-pro")), EndpointHealthRegistry::new(Duration::from_secs(120)));
        let mut settings = crate::config::settings::Settings::default();
        settings.model_usage_windows.insert(
            crate::agent::usage_windows::model_key("deepseek", "deepseek-v4-pro"),
            crate::agent::usage_windows::presets().remove("deepseek_off_peak").unwrap(),
        );
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(settings)));

        crate::agent::usage_windows::set_clock_override_ms(Some(clock("17:59")));
        let waiter = {
            let cloned = state.clone();
            tokio::spawn(async move { cloned.admit_usage_window(None, false, None).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "高峰时段必须等待，而不是失败");

        crate::agent::usage_windows::set_clock_override_ms(Some(clock("18:00")));
        let resumed = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("到 18:00 必须自动继续")
            .unwrap()
            .expect("等待不是错误");
        assert!(resumed.is_none(), "到点后同一个模型即可继续，不需要换模型");

        // 等待不是技术失败：健康状态不变、失败索引为空、记录里没有 failed。
        assert!(state.health.is_available("deepseek"));
        assert!(state.inner.lock().unwrap().failed_indices.is_empty());
        assert!(state.attempt_snapshots(false).iter().all(|attempt| attempt.status != "failed"), "等待不得写入技术失败");
        assert_eq!(
            crate::agent::usage_windows::wait_notice(&endpoint_label("deepseek"), Some(clock("18:00"))),
            "等 DeepSeek 闲时（10-13 18:00 开始）后继续；可在设置里放开时段，或换一个模型"
        );
        crate::agent::usage_windows::set_clock_override_ms(None);
    }

    /// CF-WINDOW-R2 / Compatibility Harness：没有配置任何时段时，准入是一次
    /// 纯直通——不看时钟、不换模型、不写记录。升级后未配置的用户行为与现在一致。
    #[tokio::test]
    async fn an_unconfigured_route_is_admitted_unchanged() {
        let _serial = crate::agent::usage_windows::clock_test_lock();
        // 把时钟顶到高峰时刻：如果未配置也会被拦，这条就会红。
        crate::agent::usage_windows::set_clock_override_ms(Some(
            chrono::DateTime::parse_from_rfc3339("2026-10-13T10:00:00+08:00").unwrap().timestamp_millis(),
        ));
        let state = ActiveRouteState::from_plan_with_health(RouteCandidatePlan::new(route("deepseek", "deepseek-v4-pro")), EndpointHealthRegistry::new(Duration::from_secs(120)));
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(crate::config::settings::Settings::default())));
        assert!(state.admit_usage_window(None, false, None).await.unwrap().is_none(), "未配置时段不得改变选路");
        assert_eq!(state.current().model_id, "deepseek-v4-pro");
        assert!(state.attempt_snapshots(false).is_empty(), "未配置时段不得写任何记录");
        assert!(state.inner.lock().unwrap().failed_indices.is_empty());
        crate::agent::usage_windows::set_clock_override_ms(None);
    }

    /// M86（真实场景 2026-10-14 11:08–11:17）：额度上限预过滤 + 时段跳过 +
    /// 无凭据三者同时发生。按 CF-WINDOW-R5 / CF-QUOTA 这必须是"可理解的等待"，
    /// 不是 failed / technical_recovery_exhausted；说明里要列出**全部**原因和
    /// 最早可用时间。
    #[test]
    fn quota_cap_plus_peak_plus_missing_credential_is_a_wait_not_a_failure() {
        let at = |stamp: &str| chrono::DateTime::parse_from_rfc3339(stamp).unwrap().timestamp_millis();
        let reset = at("2026-10-14T14:05:00+08:00");
        let resume = at("2026-10-14T12:00:00+08:00");
        let decision = crate::agent::usage_windows::availability_decision(vec![
            crate::agent::usage_windows::WaitCause::new("ChatGPT", crate::agent::usage_windows::WaitCauseKind::QuotaCap, Some(reset)),
            crate::agent::usage_windows::WaitCause::new("DeepSeek", crate::agent::usage_windows::WaitCauseKind::UsageWindow, Some(resume)),
            crate::agent::usage_windows::WaitCause::new("OpenRouter", crate::agent::usage_windows::WaitCauseKind::MissingCredential, None),
        ]);
        assert!(decision.waitable, "时间会让额度重置、让时段到点，所以这是等待而不是失败");
        assert_eq!(decision.earliest_ms, Some(resume), "最早可用时间取两个恢复时间里较早的那个");
        assert_eq!(decision.causes.len(), 3, "三条原因都要在案");
        for expected in [
            "ChatGPT", "已达订阅用量上限", "14:05",
            "DeepSeek", "高峰时段", "12:00",
            "OpenRouter", "尚未配置凭据",
            "不计入技术恢复次数",
        ] {
            assert!(decision.notice.contains(expected), "等待说明缺少「{expected}」：{}", decision.notice);
        }
    }

    /// M86：被预过滤掉的候选（额度超额、时段高峰、无凭据）过去不进任何记录，
    /// 失败说明于是只剩链上真正尝试过的那一条。现在预过滤本身就是记录在案的
    /// 一步，并且三者都不可用时是等待。
    #[tokio::test]
    async fn a_fleet_blocked_by_window_and_a_credential_less_endpoint_waits_and_records_every_prefilter_step() {
        let _serial = crate::agent::usage_windows::clock_test_lock();
        let clock = |at: &str| {
            chrono::DateTime::parse_from_rfc3339(&format!("2026-10-13T{at}:00+08:00"))
                .unwrap()
                .timestamp_millis()
        };
        let mut plan = RouteCandidatePlan::new(route("deepseek", "deepseek-v4-pro"));
        plan.push_fallback(RouteCandidate {
            endpoint_name: "openrouter".into(),
            model_id: "synthetic-model".into(),
            base_url: "https://openrouter.example".into(),
            credential_ref: None,
            legacy_inline_api_key: None,
            supports_vision: true,
            api_style: ApiStyle::Openai,
        });
        let state = ActiveRouteState::from_plan_with_health(plan, EndpointHealthRegistry::new(Duration::from_secs(120)));
        let mut settings = crate::config::settings::Settings::default();
        settings.model_usage_windows.insert(
            crate::agent::usage_windows::model_key("deepseek", "deepseek-v4-pro"),
            crate::agent::usage_windows::presets().remove("deepseek_off_peak").unwrap(),
        );
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(settings)));

        crate::agent::usage_windows::set_clock_override_ms(Some(clock("10:00")));
        let waiter = {
            let cloned = state.clone();
            tokio::spawn(async move { cloned.admit_usage_window(None, false, None).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "时段高峰 + 无凭据同时存在时，任务必须等待而不是失败");

        crate::agent::usage_windows::set_clock_override_ms(Some(clock("12:00")));
        let resumed = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("到 12:00 必须自动继续")
            .unwrap()
            .expect("等待不是错误");
        assert!(resumed.is_none(), "同一个模型到点即可继续");

        // 等待不是技术失败：健康不变、没有失败索引、记录里没有 failed。
        assert!(state.health.is_available("deepseek"));
        assert!(state.inner.lock().unwrap().failed_indices.is_empty(), "等待不得消耗技术恢复次数");
        let journal = state.attempt_snapshots(false);
        assert!(journal.iter().all(|attempt| attempt.status != "failed"), "等待不得写入技术失败");
        // M86：两步预过滤都要写进路由记录。
        assert!(
            journal.iter().any(|attempt| attempt.model == "deepseek-v4-pro" && attempt.failure_code.as_deref().unwrap_or_default().starts_with("outside_usage_window:")),
            "时段跳过必须有记录：{journal:?}"
        );
        assert!(
            journal.iter().any(|attempt| attempt.model == "synthetic-model" && attempt.failure_code.as_deref() == Some("route_prefilter:missing_credential")),
            "无凭据的候选作为预过滤的一步必须有记录：{journal:?}"
        );
        crate::agent::usage_windows::set_clock_override_ms(None);
    }

    /// M86 的反面：如果所有候选都只是"没配凭据"，等多久都不会变好——这时
    /// 不该空转等待，而是照原来的路径报出可执行的错误。
    #[tokio::test]
    async fn a_fleet_with_no_credential_at_all_is_not_waited_on() {
        let mut plan = RouteCandidatePlan::new(RouteCandidate {
            endpoint_name: "openrouter".into(),
            model_id: "synthetic-model".into(),
            base_url: "https://openrouter.example".into(),
            credential_ref: None,
            legacy_inline_api_key: None,
            supports_vision: true,
            api_style: ApiStyle::Openai,
        });
        let mut unconfigured = route("deepseek", "deepseek-v4-pro");
        unconfigured.credential_ref = None;
        unconfigured.legacy_inline_api_key = None;
        plan.push_fallback(unconfigured);
        let state = ActiveRouteState::from_plan_with_health(plan, EndpointHealthRegistry::new(Duration::from_secs(120)));
        let mut settings = crate::config::settings::Settings::default();
        // 让准入循环真的跑起来（配了时段就不再是纯直通），但这条时段不属于任何候选。
        settings.model_usage_windows.insert(
            crate::agent::usage_windows::model_key("elsewhere", "other-model"),
            crate::agent::usage_windows::presets().remove("deepseek_off_peak").unwrap(),
        );
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(settings)));
        let admitted = tokio::time::timeout(Duration::from_millis(500), state.admit_usage_window(None, false, None))
            .await
            .expect("没有可等待的原因时不得空转")
            .unwrap();
        assert!(admitted.is_none(), "缺凭据需要用户动作，不能靠等");
        assert!(state.inner.lock().unwrap().failed_indices.is_empty(), "预过滤不是技术失败");
    }

    #[tokio::test]
    async fn m123_missing_credentials_end_without_request_even_without_windows() {
        let mut candidate = route("openrouter", "synthetic");
        candidate.credential_ref = None;
        candidate.legacy_inline_api_key = None;
        let state = ActiveRouteState::from_plan_with_health(RouteCandidatePlan::new(candidate), EndpointHealthRegistry::new(Duration::from_secs(120)));
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(Default::default())));
        let error = state.admit_model_availability(None, false, None).await.unwrap_err();
        let unavailable = ModelUnavailable::parse(&error).unwrap();
        assert_eq!(unavailable.resume_at_ms, None);
        assert!(unavailable.notice.contains("凭据"));
        assert!(state.inner.lock().unwrap().failed_indices.is_empty());
    }

    #[tokio::test]
    async fn m123_waits_durably_then_admits_restored_route() {
        let state = ActiveRouteState::from_plan_with_health(RouteCandidatePlan::new(route("synthetic", "pro")), EndpointHealthRegistry::new(Duration::from_secs(120)));
        let mut settings = crate::config::settings::Settings::default();
        let key = super::super::usage_windows::model_key("synthetic", "pro");
        let mut blocked = super::super::usage_windows::presets().remove("deepseek_off_peak").unwrap();
        blocked.mode = "custom".into();
        blocked.weekdays.clear();
        blocked.periods = vec![];
        blocked.invert = false;
        settings.model_usage_windows.insert(key.clone(), blocked);
        let live = Arc::new(tokio::sync::RwLock::new(settings));
        state.attach_usage_settings(live.clone());
        let result = tokio::time::timeout(Duration::from_millis(100), state.admit_model_availability(None, false, None)).await.expect("must release runner, not sleep");
        let unavailable = ModelUnavailable::parse(&result.unwrap_err()).unwrap();
        assert!(unavailable.notice.contains("没有可用模型"));
        live.write().await.model_usage_windows.get_mut(&key).unwrap().mode = "anytime".into();
        assert!(state.admit_model_availability(None, false, None).await.unwrap().is_none());
        assert!(state.inner.lock().unwrap().failures.is_empty());
    }

    #[test]
    fn m123_earliest_recovery_across_window_quota_and_credentials() {
        use super::super::usage_windows::{WaitCause, WaitCauseKind, availability_decision};
        for (window, quota) in [(1800000000000, 1800001140000), (1900000000000, 1800000000000)] {
            let decision = availability_decision(vec![
                WaitCause::new("DeepSeek", WaitCauseKind::UsageWindow, Some(window)),
                WaitCause::new("ChatGPT", WaitCauseKind::QuotaCap, Some(quota)),
                WaitCause::new("OpenRouter", WaitCauseKind::MissingCredential, None),
            ]);
            let unavailable = ModelUnavailable::from_decision(decision);
            assert_eq!(unavailable.resume_at_ms, Some(window.min(quota) + 250));
            assert!(unavailable.notice.contains("DeepSeek") && unavailable.notice.contains("ChatGPT") && unavailable.notice.contains("OpenRouter"));
            assert_eq!(ModelUnavailable::parse(&unavailable.encode()).unwrap().resume_at_ms, unavailable.resume_at_ms);
        }
    }

    #[tokio::test]
    async fn m164_hard_cap_mixed_routes_wait_then_admit_at_boundary() {
        let _serial = super::super::usage_windows::clock_test_lock();
        let clock = |time: &str| chrono::DateTime::parse_from_rfc3339(&format!("2026-10-23T{time}:00+08:00")).unwrap().timestamp_millis();
        let mut gpt = route("m164-gpt", "gpt");
        gpt.api_style = ApiStyle::Chatgpt;
        gpt.base_url = "https://m164-gpt.synthetic.example".into();
        let mut plan = RouteCandidatePlan::new(gpt.clone());
        plan.push_fallback(route("deepseek", "pro"));
        let mut missing = route("openrouter", "synthetic");
        missing.credential_ref = None;
        missing.legacy_inline_api_key = None;
        plan.push_fallback(missing);
        let mut settings = crate::config::settings::Settings::default();
        settings.endpoints.insert(gpt.endpoint_name.clone(), crate::config::settings::Endpoint {
            base_url: gpt.base_url.clone(), key_ref: Some("synthetic-login".into()),
            api_style: ApiStyle::Chatgpt, custom_models: vec![], active_model: None,
        });
        settings.model_usage_windows.insert(super::super::usage_windows::model_key("deepseek", "pro"), super::super::usage_windows::presets().remove("deepseek_off_peak").unwrap());
        let state = ActiveRouteState::from_plan_with_health(plan, EndpointHealthRegistry::new(Duration::from_secs(120)));
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(settings)));
        let ledger = super::super::quota_cap::shared_quota_ledger();
        let quota = |five, week| super::super::quota_cap::EndpointQuota {
            source: super::super::quota_cap::UsageSource::ServerReported,
            five_hour: super::super::quota_cap::WindowUsage { used_ratio: five, resets_at_ms: Some(clock("19:00")) },
            weekly: super::super::quota_cap::WindowUsage { used_ratio: week, resets_at_ms: Some(clock("20:00")) },
            observed_at_ms: clock("17:59"),
        };
        super::super::usage_windows::set_clock_override_ms(Some(clock("17:59")));
        for (five, week) in [(0.80, 0.1), (0.81, 0.1), (0.79, 0.80)] {
            ledger.record_server_usage(&gpt.base_url, quota(five, week));
            let error = state.admit_model_availability(None, false, None).await.unwrap_err();
            assert_eq!(ModelUnavailable::parse(&error).unwrap().resume_at_ms, Some(clock("18:00") + 250));
            assert!(!error.contains("AUTH_MISSING"));
            assert!(state.inner.lock().unwrap().failures.is_empty());
        }
        ledger.record_server_usage(&gpt.base_url, quota(0.79, 0.79));
        assert!(state.admit_model_availability(None, false, None).await.unwrap().is_none());
        assert_eq!(state.current().endpoint_name, "m164-gpt");
        ledger.record_server_usage(&gpt.base_url, quota(0.81, 0.1));
        super::super::usage_windows::set_clock_override_ms(Some(clock("18:00")));
        let change = state.admit_model_availability(None, false, None).await.unwrap().unwrap();
        assert_eq!(change.to_endpoint, "deepseek");
        assert!(!change.notice().contains("OpenRouter"));
        ledger.clear(&gpt.base_url);
        super::super::usage_windows::set_clock_override_ms(None);
    }

    #[tokio::test]
    async fn m123_boundary_wake_and_mixed_available_route() {
        let _serial = super::super::usage_windows::clock_test_lock();
        let clock = |time: &str| chrono::DateTime::parse_from_rfc3339(&format!("2026-10-13T{time}:00+08:00")).unwrap().timestamp_millis();
        let mut plan = RouteCandidatePlan::new(route("deepseek", "pro"));
        let mut missing = route("openrouter", "synthetic");
        missing.credential_ref = None;
        missing.legacy_inline_api_key = None;
        plan.push_fallback(missing);
        let state = ActiveRouteState::from_plan_with_health(plan, EndpointHealthRegistry::new(Duration::from_secs(120)));
        let mut settings = crate::config::settings::Settings::default();
        settings.model_usage_windows.insert(super::super::usage_windows::model_key("deepseek", "pro"), super::super::usage_windows::presets().remove("deepseek_off_peak").unwrap());
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(settings)));
        super::super::usage_windows::set_clock_override_ms(Some(clock("17:59")));
        let error = state.admit_model_availability(None, false, None).await.unwrap_err();
        assert_eq!(ModelUnavailable::parse(&error).unwrap().resume_at_ms, Some(clock("18:00") + 250));
        super::super::usage_windows::set_clock_override_ms(Some(clock("18:00")));
        assert!(state.admit_model_availability(None, false, None).await.unwrap().is_none());
        assert_eq!(state.current().endpoint_name, "deepseek");
        super::super::usage_windows::set_clock_override_ms(Some(clock("17:59")));
        state.inner.lock().unwrap().candidates.push(route("available", "fallback"));
        let change = state.admit_model_availability(None, false, None).await.unwrap().unwrap();
        assert_eq!(change.to_endpoint, "available");
        assert!(state.inner.lock().unwrap().failed_indices.is_empty());
        super::super::usage_windows::set_clock_override_ms(None);
    }

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
        for transient in [
            "Our servers are currently overloaded. Please try again later.",
            "upstream 529 overloaded",
        ] {
            assert_eq!(
                classify_provider_failure(transient),
                ProviderFailureClass::EndpointUnavailable,
                "zero-output transient overload must enter the safe recovery policy"
            );
        }
    }

    /// U25. The classification table that decides whether an outage is waited
    /// out or taken as a verdict.
    ///
    /// `error sending request` is the exact text DeepSeek produced on
    /// 2026-10-08 for ~3 minutes, and it was classified as if the endpoint had
    /// refused: the objective gave up in about a minute while the network was
    /// already recovering on its own.
    #[test]
    fn transient_transport_failures_are_separated_from_explicit_refusals() {
        for transport in [
            "HTTP error: error sending request for url (https://api.deepseek.com/chat/completions)",
            "connection refused",
            "dns error: failed to lookup address information",
            "operation timed out",
            "connection reset by peer",
            "HTTP 502 Bad Gateway",
            "HTTP 503 Service Unavailable",
            "HTTP 504 Gateway Timeout",
            "tls handshake failure",
        ] {
            assert_eq!(
                classify_provider_failure(transport),
                ProviderFailureClass::TransportUnreachable,
                "{transport} is a network condition, not a verdict about the task"
            );
        }

        for refusal in [
            r#"HTTP 404 Not Found: {"error":{"message":"model not found"}}"#,
            r#"HTTP 503 Service Unavailable: {"code":"biscuit_baker_service_me_circuit_open"}"#,
            "The model gpt-4o has been deprecated and is unavailable",
        ] {
            assert_eq!(
                classify_provider_failure(refusal),
                ProviderFailureClass::EndpointUnavailable,
                "{refusal} is the endpoint explicitly refusing"
            );
        }

        // Both are still routable: the difference is how long the objective
        // waits, not whether another route may be tried.
        assert!(ProviderFailureClass::TransportUnreachable.permits_endpoint_failover());
        assert!(ProviderFailureClass::EndpointUnavailable.permits_endpoint_failover());
        assert_ne!(
            ProviderFailureClass::TransportUnreachable,
            ProviderFailureClass::EndpointUnavailable
        );
    }

    /// U25 requirement 3. A user who pinned one model must never be moved off it
    /// silently; automatic routing may use a second route it was given.
    #[test]
    fn an_outage_switches_routes_only_when_the_policy_offered_another_one() {
        let outage = "HTTP error: error sending request for url (https://api.deepseek.com/chat/completions)";

        // `fixed`: the plan holds exactly the model the user chose.
        let pinned = ActiveRouteState::from_plan_with_health(
            RouteCandidatePlan::new(route("deepseek", "deepseek-v4-pro")),
            EndpointHealthRegistry::new(Duration::from_secs(120)),
        );
        assert!(
            pinned.advance_after_failure(outage).is_none(),
            "a pinned model must not be swapped out silently"
        );
        assert_eq!(pinned.current().model_id, "deepseek-v4-pro");

        // `auto` / `prefer`: a usable candidate route exists, so it is used.
        let mut plan = RouteCandidatePlan::new_automatic(route("deepseek", "deepseek-v4-pro"));
        plan.push_fallback(route("chatgpt", "gpt-5.5"));
        let automatic = ActiveRouteState::from_plan_with_health(
            plan,
            EndpointHealthRegistry::new(Duration::from_secs(120)),
        );
        let change = automatic
            .advance_after_failure(outage)
            .expect("a usable candidate route exists");
        assert_eq!(change.to_endpoint, "chatgpt");
        assert_eq!(automatic.current().endpoint_name, "chatgpt");
    }

    #[test]
    fn endpoint_health_registries_are_isolated_between_tests() {
        let first = EndpointHealthRegistry::new(Duration::from_secs(120));
        let second = EndpointHealthRegistry::new(Duration::from_secs(120));

        first.mark_unavailable("shared-provider-name");

        assert!(!first.is_available("shared-provider-name"));
        assert!(second.is_available("shared-provider-name"));
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
    fn exhausted_routes_leave_recovery_owned_by_the_system() {
        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-5.5"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
        let state = ActiveRouteState::from_plan_with_health(plan, health);

        state
            .advance_after_failure("HTTP 503 Service Unavailable")
            .expect("fallback exists");
        let message = state.exhausted_error("HTTP 429 Too Many Requests");

        assert!(message.contains("目标与失败证据已保留"), "got: {message}");
        assert!(
            message.contains("系统将按退避策略重新观测可用路由"),
            "got: {message}"
        );
        for forbidden_user_handoff in ["后重试", "点击重试", "回复继续", "回到对话"]
        {
            assert!(
                !message.contains(forbidden_user_handoff),
                "route exhaustion must not hand technical recovery to the user: {message}"
            );
        }
    }

    #[test]
    fn mixed_exhaustion_keeps_primary_transport_cause_and_window_skip() {
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-synthetic"));
        plan.push_fallback(route("deepseek", "deepseek-synthetic"));
        plan.push_fallback(route("openrouter", "router-synthetic"));
        let state = ActiveRouteState::from_plan_with_health(
            plan, EndpointHealthRegistry::new(Duration::from_secs(120)),
        );
        state.advance_after_failure("error sending request: connection refused");
        {
            let mut inner = state.inner.lock().unwrap();
            inner.usage_skips.push(RouteAttemptSnapshot {
                endpoint: "deepseek".into(), model: "deepseek-synthetic".into(),
                status: "skipped".into(),
                failure_code: Some("outside_usage_window:{\"next_available_ms\":1791950400000}".into()),
            });
            inner.current_index = 2;
        }
        let message = state.exhausted_error("AUTH_MISSING: openrouter 尚未配置凭据");
        assert!(message.contains("DeepSeek"), "{message}");
        assert!(message.contains("允许使用时段"), "{message}");
        assert!(message.contains("可用"), "{message}");
        assert!(message.find("ChatGPT").unwrap() < message.find("DeepSeek").unwrap());
        assert!(message.find("DeepSeek").unwrap() < message.find("OpenRouter").unwrap());
        assert_eq!(classify_provider_failure(&message), ProviderFailureClass::TransportUnreachable);
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
            Some("TRANSPORT_UNREACHABLE")
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

    // ── U30 (2026-10-09) ───────────────────────────────────────────────────
    // Prefer mode: when the preferred endpoint is unavailable the same task
    // continues on the next one. Production session dd3779b3 showed six
    // chatgpt attempts, zero deepseek/openrouter attempts and a lost task.

    /// CF-PFB-R1. Every unavailability kind the spec lists still leaves the
    /// same turn a route out, in the user's order.
    #[test]
    fn every_unavailability_kind_still_offers_the_next_endpoint() {
        let cases = [
            (
                "cannot connect",
                "HTTP error: error sending request for url (https://chatgpt.com/backend-api/codex/responses)",
            ),
            ("timeout", "HTTP error: operation timed out"),
            ("502", "HTTP 502 Bad Gateway"),
            ("503", "HTTP 503 Service Unavailable"),
            ("504", "HTTP 504 Gateway Timeout"),
            ("sign-in expired", "HTTP 401 Unauthorized: auth_expired"),
            ("rate limited", "HTTP 429 Too Many Requests: rate limit"),
            ("quota used up", "insufficient_quota: quota exceeded"),
        ];
        for (label, reason) in cases {
            let health = EndpointHealthRegistry::new(Duration::from_secs(120));
            let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-6-luna"));
            plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
            plan.push_fallback(route("openrouter", "gpt-6-luna"));
            let state = ActiveRouteState::from_plan_with_health(plan, health);

            assert!(
                state.has_available_fallback(),
                "{label}: prefer mode must offer the next endpoint"
            );
            let change = state
                .advance_after_failure(reason)
                .unwrap_or_else(|| panic!("{label}: the task must continue on deepseek"));
            assert_eq!(change.from_endpoint, "chatgpt", "{label}");
            assert_eq!(change.to_endpoint, "deepseek", "{label}");
            assert_eq!(state.current().model_id, "deepseek-v4-pro", "{label}");
        }
    }

    /// CF-PFB-R2 + R3. Fall back only while something is actually available,
    /// and do not keep hitting an endpoint whose quota is spent.
    #[test]
    fn a_cooled_down_endpoint_is_not_offered_again_within_its_window() {
        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-6-luna"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
        plan.push_fallback(route("openrouter", "gpt-6-luna"));
        let state = ActiveRouteState::from_plan_with_health(plan.clone(), health.clone());

        state
            .advance_after_failure("insufficient_quota: quota exceeded")
            .expect("deepseek takes over");
        assert!(
            !health.is_available("chatgpt"),
            "a spent quota must cool the endpoint down instead of retrying it"
        );
        assert!(
            state.has_available_fallback(),
            "openrouter is still available"
        );
        state
            .advance_after_failure("HTTP 503 Service Unavailable")
            .expect("openrouter takes over");
        assert!(
            !state.has_available_fallback(),
            "every endpoint was tried and cooled down"
        );
        assert!(
            state.advance_after_failure("HTTP 503 Service Unavailable").is_none(),
            "an exhausted fleet waits for recovery instead of looping"
        );

        // R3: the user's preferred endpoint is tried again on a new turn, and
        // is usable again once its window has passed.
        let cooled = ActiveRouteState::from_plan_with_health(plan, health);
        assert_eq!(
            cooled.current().endpoint_name,
            "chatgpt",
            "a new turn goes back to the preferred endpoint"
        );
        let recovered = ActiveRouteState::from_plan_with_health(
            {
                let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-6-luna"));
                plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
                plan
            },
            EndpointHealthRegistry::new(Duration::from_secs(0)),
        );
        assert!(recovered.has_available_fallback());
    }

    #[tokio::test]
    async fn m125_overloaded_route_remains_eligible_for_scheduled_retry() {
        let state = ActiveRouteState::from_plan_with_health(
            RouteCandidatePlan::new(route("pinned", "pro")),
            EndpointHealthRegistry::new(Duration::from_secs(120)),
        );
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(Default::default())));
        state.record_current_failure("HTTP 529 overloaded");
        assert!(state.inner.lock().unwrap().failed_indices.is_empty(), "M125: overload waiting is not an exhausted route");
        assert!(state.admit_model_availability(None, false, None).await.unwrap().is_none());
        assert_eq!(state.current().endpoint_name, "pinned");
    }

    #[tokio::test]
    async fn m125_unconfigured_fallback_is_not_available() {
        let mut plan = RouteCandidatePlan::new(route("primary", "pro"));
        let mut missing = route("backup", "pro");
        missing.credential_ref = None;
        missing.legacy_inline_api_key = None;
        plan.push_fallback(missing);
        let state = ActiveRouteState::from_plan_with_health(plan, EndpointHealthRegistry::new(Duration::from_secs(120)));
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(Default::default())));
        assert!(!state.has_available_fallback(), "M125: credentials are required for a usable fallback");
    }

    /// M152/R1：留余量是软偏好。备选端点没有凭据就不算"可用备选"，被跳过的
    /// 首选端点必须原地留下，而不是把这一轮交给一个注定 AUTH_MISSING 的路由。
    #[test]
    fn m152_capped_primary_stays_when_the_fallback_has_no_credential() {
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-5.5"));
        let mut fallback = route("openrouter", "gpt-5.5");
        fallback.credential_ref = None;
        fallback.legacy_inline_api_key = None;
        plan.push_fallback(fallback);
        plan.skip_for_quota(
            "chatgpt",
            "GPT 本周窗口已用 80%，为给你留余量已切到 OpenRouter，约 18:20 恢复。".into(),
        );
        let state = ActiveRouteState::from_plan_with_health(
            plan,
            EndpointHealthRegistry::new(Duration::from_secs(120)),
        );
        assert_eq!(
            state.current().endpoint_name, "chatgpt",
            "M152: 缺凭据的备选不能被当成留余量的落点"
        );
        assert!(state.take_initial_route_change().is_none());
    }

    /// M164/R1：用户上限是硬门禁，没有备选也不得继续开新请求。
    #[tokio::test]
    async fn m164_reserve_capped_primary_waits_when_every_alternative_is_blocked() {
        let mut primary = route("chatgpt", "gpt-5.5");
        primary.base_url = "https://m152-primary.example".into();
        let mut fallback = route("chatgpt-backup", "gpt-5.5");
        fallback.base_url = "https://m152-backup.example".into();

        let mut settings = crate::config::settings::Settings::default();
        for (name, url) in [
            ("chatgpt", primary.base_url.clone()),
            ("chatgpt-backup", fallback.base_url.clone()),
        ] {
            settings.endpoints.insert(
                name.into(),
                crate::config::settings::Endpoint {
                    base_url: url,
                    key_ref: Some(format!("{name}-key-ref")),
                    api_style: crate::config::settings::ApiStyle::Chatgpt,
                    custom_models: vec![],
                    active_model: None,
                },
            );
        }

        let ledger = crate::agent::quota_cap::shared_quota_ledger();
        let now = crate::agent::usage_windows::now_ms();
        let quota = |ratio: f64| crate::agent::quota_cap::EndpointQuota {
            source: crate::agent::quota_cap::UsageSource::ServerReported,
            five_hour: crate::agent::quota_cap::WindowUsage {
                used_ratio: ratio,
                resets_at_ms: Some(now + 1_800_000),
            },
            weekly: crate::agent::quota_cap::WindowUsage {
                used_ratio: 0.1,
                resets_at_ms: Some(now + 86_400_000),
            },
            observed_at_ms: now,
        };
        ledger.record_server_usage(&primary.base_url, quota(0.85));
        ledger.record_server_usage(&fallback.base_url, quota(1.0));

        let mut plan = RouteCandidatePlan::new(primary.clone());
        plan.push_fallback(fallback.clone());
        plan.skip_for_quota(
            "chatgpt",
            "GPT 本五小时窗口已用 85%，为给你留余量已切到 ChatGPT，约 18:20 恢复。".into(),
        );
        let state = ActiveRouteState::from_plan_with_health(
            plan,
            EndpointHealthRegistry::new(Duration::from_secs(120)),
        );
        state.attach_usage_settings(Arc::new(tokio::sync::RwLock::new(settings)));

        let error = state
            .admit_model_availability(None, false, None)
            .await
            .expect_err("M164: 达到用户上限必须等待，不能继续 GPT");
        let unavailable = ModelUnavailable::parse(&error).expect("durable availability wait");
        assert_eq!(unavailable.resume_at_ms, Some(now + 1_800_000 + 250));
        assert!(!error.contains("AUTH_MISSING"));

        ledger.clear(&primary.base_url);
        ledger.clear(&fallback.base_url);
    }

    /// CF-PFB-R6. Fixed mode is a pin: one candidate, no fallback, ever.
    #[test]
    fn a_pinned_plan_never_offers_a_fallback() {
        let state = ActiveRouteState::from_plan_with_health(
            RouteCandidatePlan::new(route("deepseek", "deepseek-v4-pro")),
            EndpointHealthRegistry::new(Duration::from_secs(120)),
        );

        assert!(!state.has_available_fallback());
        assert!(state
            .advance_after_failure("HTTP 503 Service Unavailable")
            .is_none());
        assert_eq!(state.current().endpoint_name, "deepseek");
    }

    /// CF-QUOTA-R3. A subscription endpoint that reached its window cap hands
    /// the turn to the next candidate, and the session is told why in plain
    /// language.
    #[test]
    fn quota_capped_primary_hands_the_turn_to_the_next_candidate() {
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-5.5"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
        plan.skip_for_quota(
            "chatgpt",
            "GPT 本五小时窗口已用 80%，为给你留余量已切到 DeepSeek，约 18:20 恢复。".into(),
        );

        let health = EndpointHealthRegistry::new(Duration::from_secs(120));
        let state = ActiveRouteState::from_plan_with_health(plan, health.clone());
        assert_eq!(state.current().endpoint_name, "deepseek");

        let change = state
            .take_initial_route_change()
            .expect("handover is announced");
        assert_eq!(change.kind, RouteChangeKind::QuotaCap);
        assert_eq!(
            change.notice(),
            "GPT 本五小时窗口已用 80%，为给你留余量已切到 DeepSeek，约 18:20 恢复。"
        );
        // A quota handover is a preference change, not a health failure: the
        // endpoint stays healthy and the next turn returns to it.
        assert!(health.is_available("chatgpt"));
    }

    /// CF-QUOTA-R5. With nowhere to hand over to, the plan keeps the capped
    /// endpoint so the caller can report the honest reason instead of the
    /// constructor silently inventing an endpoint.
    #[test]
    fn quota_capped_primary_without_an_alternative_stays_put() {
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-5.5"));
        plan.skip_for_quota("chatgpt", "GPT 本五小时窗口已用 80%。".into());
        let state = ActiveRouteState::from_plan_with_health(
            plan,
            EndpointHealthRegistry::new(Duration::from_secs(120)),
        );
        assert_eq!(state.current().endpoint_name, "chatgpt");
        assert!(state.take_initial_route_change().is_none());
    }

    /// CF-QUOTA-R3. A handover never lands on another endpoint that is itself
    /// over its cap.
    #[test]
    fn a_quota_handover_never_lands_on_another_capped_endpoint() {
        let mut plan = RouteCandidatePlan::new(route("chatgpt", "gpt-5.5"));
        plan.push_fallback(route("chatgpt-alt", "gpt-5.5"));
        plan.push_fallback(route("deepseek", "deepseek-v4-pro"));
        plan.skip_for_quota("chatgpt", "GPT 本五小时窗口已用 80%。".into());
        plan.skip_for_quota("chatgpt-alt", "GPT 本每周窗口已用 80%。".into());
        let state = ActiveRouteState::from_plan_with_health(
            plan,
            EndpointHealthRegistry::new(Duration::from_secs(120)),
        );
        assert_eq!(state.current().endpoint_name, "deepseek");
    }
}
