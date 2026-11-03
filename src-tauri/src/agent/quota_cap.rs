// SPDX-License-Identifier: Apache-2.0
//! CF-QUOTA: keep a subscription endpoint from eating the user's whole quota.
//!
//! ChatGPT Plus is metered by a rolling 5-hour window plus a weekly ceiling,
//! denominated in tokens. On 2026-10-10 CodeFactory burned two whole windows
//! (≈27–32M input tokens each) and left the user with nothing on their own
//! ChatGPT/Codex. U33 already hands over *after* the quota is gone; this
//! module hands over *before* — at a configurable share (default 80%) of each
//! window — and hands back once the window resets.
//!
//! Two things are deliberately kept apart here:
//!
//! * what the *service* says (the `x-codex-*` response headers Codex CLI's
//!   `/status` also reads), and
//! * what we can only *estimate* locally from token kinds and the official
//!   rate card when the service told us nothing.
//!
//! The source travels with every snapshot so the UI never presents an estimate
//! as a server reading. This module is credential-free by construction: it
//! only ever sees endpoint *names*, percentages and token counts.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Default share of the 5-hour window CodeFactory may consume.
pub const DEFAULT_FIVE_HOUR_CAP_PERCENT: u8 = 80;
/// Default share of the weekly window CodeFactory may consume.
pub const DEFAULT_WEEKLY_CAP_PERCENT: u8 = 80;

/// Production evidence (2026-10-10): one exhausted 5-hour window absorbed
/// ≈27–32M input tokens, so a 30M weighted-unit budget is the estimate anchor.
const DEFAULT_FIVE_HOUR_UNITS: f64 = 30_000_000.0;
/// A 5-hour window is metered ~8× per week; a weekly ceiling ≈8× one window is
/// the conservative anchor when the service gives us no weekly reading.
const DEFAULT_WEEKLY_UNITS: f64 = 250_000_000.0;

/// Official rate-card weights, relative to one uncached input token.
const WEIGHT_CACHED_INPUT: f64 = 0.25;
const WEIGHT_OUTPUT: f64 = 4.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum QuotaWindowKind {
    FiveHour,
    Weekly,
}

impl QuotaWindowKind {
    /// Plain-language window name for user-facing copy and route reasons.
    pub fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5 小时窗口",
            Self::Weekly => "每周窗口",
        }
    }

    /// Nominal length of the window, used to anchor a local estimate.
    pub fn period_ms(self) -> i64 {
        match self {
            Self::FiveHour => 5 * 60 * 60 * 1000,
            Self::Weekly => 7 * 24 * 60 * 60 * 1000,
        }
    }
}

/// Where a usage snapshot came from — never blur the two.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageSource {
    /// Read from the service's own usage headers.
    LiveQuery,
    ServerReported,
    /// Locally derived from token counts and the official rate card.
    Estimated,
}

impl UsageSource {
    pub fn is_estimate(self) -> bool {
        matches!(self, Self::Estimated)
    }

    /// Marker the UI appends so an estimate is never mistaken for a reading.
    pub fn label(self) -> &'static str {
        match self {
            Self::LiveQuery => "实时查询",
            Self::ServerReported => "回复头读数",
            Self::Estimated => "本地估算",
        }
    }
}

/// One metering window's consumption.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowUsage {
    /// 0.0..=1.0 share of the window already consumed.
    pub used_ratio: f64,
    /// Absolute epoch-ms when this window resets, when known.
    pub resets_at_ms: Option<i64>,
}

impl WindowUsage {
    pub fn empty() -> Self {
        Self {
            used_ratio: 0.0,
            resets_at_ms: None,
        }
    }

    /// Usage as of `now_ms`, accounting for a window that has already reset.
    pub fn effective_ratio(&self, now_ms: i64) -> f64 {
        if self.has_reset(now_ms) {
            return 0.0;
        }
        self.used_ratio.clamp(0.0, 1.0)
    }

    pub fn has_reset(&self, now_ms: i64) -> bool {
        self.resets_at_ms.map(|reset| reset <= now_ms).unwrap_or(false)
    }

    pub fn used_percent(&self, now_ms: i64) -> u8 {
        (self.effective_ratio(now_ms) * 100.0).round().clamp(0.0, 100.0) as u8
    }
}

/// A subscription endpoint's usage across both metering windows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EndpointQuota {
    pub source: UsageSource,
    pub five_hour: WindowUsage,
    pub weekly: WindowUsage,
    /// Epoch-ms when this snapshot was observed.
    pub observed_at_ms: i64,
}

impl EndpointQuota {
    pub fn source_label(&self, now_ms: i64) -> &'static str {
        if self.source != UsageSource::Estimated
            && (self.five_hour.has_reset(now_ms) || self.weekly.has_reset(now_ms))
        {
            "推算（已过重置点，待刷新）"
        } else {
            self.source.label()
        }
    }

    pub fn window(&self, kind: QuotaWindowKind) -> WindowUsage {
        match kind {
            QuotaWindowKind::FiveHour => self.five_hour,
            QuotaWindowKind::Weekly => self.weekly,
        }
    }
}

/// Per-endpoint caps, in percent of each window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaCap {
    pub five_hour_percent: u8,
    pub weekly_percent: u8,
}

impl Default for QuotaCap {
    fn default() -> Self {
        Self {
            five_hour_percent: DEFAULT_FIVE_HOUR_CAP_PERCENT,
            weekly_percent: DEFAULT_WEEKLY_CAP_PERCENT,
        }
    }
}

impl QuotaCap {
    pub fn new(five_hour_percent: u8, weekly_percent: u8) -> Self {
        Self {
            five_hour_percent,
            weekly_percent,
        }
        .clamped()
    }

    /// Caps above 100% make no sense; clamp rather than silently accepting a
    /// value that would let the app exhaust the window.
    pub fn clamped(self) -> Self {
        Self {
            five_hour_percent: self.five_hour_percent.min(100),
            weekly_percent: self.weekly_percent.min(100),
        }
    }

    pub fn for_window(&self, kind: QuotaWindowKind) -> u8 {
        match kind {
            QuotaWindowKind::FiveHour => self.five_hour_percent,
            QuotaWindowKind::Weekly => self.weekly_percent,
        }
    }
}

/// The binding window that has reached its cap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapBreach {
    pub window: QuotaWindowKind,
    pub cap_percent: u8,
    pub used_percent: u8,
    pub source: UsageSource,
    /// When the breached window resets, when known.
    pub resets_at_ms: Option<i64>,
}

impl CapBreach {
    /// M164：用户设定的上限就是硬上限，不再把 100% 以下视为软偏好。
    pub fn is_hard_exhausted(&self) -> bool {
        self.used_percent >= self.cap_percent
    }

    /// "约 18:20 恢复" — plain-language reset hint, or `None` when the service
    /// did not tell us when the window turns over.
    pub fn reset_hint(&self, now_ms: i64) -> Option<String> {
        let resets_at = self.resets_at_ms?;
        if resets_at <= now_ms {
            return None;
        }
        format_reset_hint(resets_at)
    }

    /// The exact sentence the session shows when the cap moves the turn to the
    /// next endpoint.
    pub fn handover_notice(&self, to_endpoint_label: &str, now_ms: i64) -> String {
        let reset = self
            .reset_hint(now_ms)
            .map(|hint| format!("，{hint}"))
            .unwrap_or_default();
        let estimate = if self.source.is_estimate() {
            "（本地估算）"
        } else {
            ""
        };
        format!(
            "GPT 本{}已用 {}%{}，为给你留余量已切到 {}{}。",
            short_window_label(self.window),
            self.used_percent,
            estimate,
            to_endpoint_label,
            reset,
        )
    }
}

fn short_window_label(kind: QuotaWindowKind) -> &'static str {
    match kind {
        QuotaWindowKind::FiveHour => "五小时窗口",
        QuotaWindowKind::Weekly => "周窗口",
    }
}

/// Render an epoch-ms instant as a local `HH:MM` clock hint.
pub fn format_reset_hint(resets_at_ms: i64) -> Option<String> {
    use chrono::{Local, TimeZone};
    let dt = Local.timestamp_millis_opt(resets_at_ms).single()?;
    Some(format!("约 {} 恢复", dt.format("%H:%M")))
}

/// Decide whether `quota` has reached `cap` as of `now_ms`.
///
/// Returns the *binding* breach: when both windows are at their cap, the one
/// that frees up soonest wins, because that is the one that tells the user when
/// CodeFactory will hand the work back to the subscription.
pub fn evaluate_cap(quota: &EndpointQuota, cap: QuotaCap, now_ms: i64) -> Option<CapBreach> {
    let cap = cap.clamped();
    let mut breaches: Vec<CapBreach> = [QuotaWindowKind::FiveHour, QuotaWindowKind::Weekly]
        .into_iter()
        .filter_map(|kind| {
            let window = quota.window(kind);
            let cap_percent = cap.for_window(kind);
            let used_percent = if window.has_reset(now_ms) && quota.source != UsageSource::Estimated {
                // A reset timestamp is not evidence of available subscription quota.
                100
            } else { window.used_percent(now_ms) };
            if used_percent >= cap_percent {
                Some(CapBreach {
                    window: kind,
                    cap_percent,
                    used_percent,
                    source: quota.source,
                    resets_at_ms: window.resets_at_ms.filter(|reset| *reset > now_ms),
                })
            } else {
                None
            }
        })
        .collect();
    breaches.sort_by_key(|breach| {
        (
            breach.resets_at_ms.unwrap_or(i64::MAX),
            matches!(breach.window, QuotaWindowKind::FiveHour),
        )
    });
    breaches.into_iter().next()
}

/// Parse the service-reported usage out of ChatGPT/Codex response headers.
///
/// Codex CLI's `/status` reads the same family:
/// `x-codex-primary-used-percent`, `x-codex-primary-window-minutes`,
/// `x-codex-primary-reset-after-seconds`, and the matching `secondary` set.
/// Primary/secondary are mapped by their advertised window length so a future
/// service-side reordering cannot silently swap the 5-hour and weekly windows.
pub fn parse_codex_usage_headers<'a, I>(headers: I, now_ms: i64) -> Option<EndpointQuota>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut by_name: HashMap<String, String> = HashMap::new();
    for (name, value) in headers {
        by_name.insert(name.to_ascii_lowercase(), value.trim().to_string());
    }

    let read_percent = |prefix: &str, by_name: &HashMap<String, String>| -> Option<f64> {
        let raw = by_name.get(&format!("x-codex-{prefix}-used-percent"))?;
        let value: f64 = raw.parse().ok()?;
        if !value.is_finite() {
            return None;
        }
        // The header is a percentage (Codex CLI renders it as-is). Accept a
        // 0..1 ratio too, since some gateways normalise, without ever letting
        // a real 100% reading collapse to 1%.
        let percent = if value <= 1.0 && raw.contains('.') {
            value * 100.0
        } else {
            value
        };
        Some(percent.clamp(0.0, 100.0))
    };

    let mut five_hour: Option<WindowUsage> = None;
    let mut weekly: Option<WindowUsage> = None;

    for prefix in ["primary", "secondary"] {
        let Some(percent) = read_percent(prefix, &by_name) else {
            continue;
        };
        let window_minutes = by_name
            .get(&format!("x-codex-{prefix}-window-minutes"))
            .and_then(|raw| raw.parse::<i64>().ok());
        let reset_after_secs = by_name
            .get(&format!("x-codex-{prefix}-reset-after-seconds"))
            .and_then(|raw| raw.parse::<i64>().ok());
        let kind = match window_minutes {
            Some(minutes) if minutes > 24 * 60 => QuotaWindowKind::Weekly,
            Some(_) => QuotaWindowKind::FiveHour,
            // No window length advertised: primary is the 5-hour window and
            // secondary is the weekly one, matching the service's own order.
            None if prefix == "primary" => QuotaWindowKind::FiveHour,
            None => QuotaWindowKind::Weekly,
        };
        let usage = WindowUsage {
            used_ratio: (percent / 100.0).clamp(0.0, 1.0),
            resets_at_ms: reset_after_secs.map(|secs| now_ms + secs.max(0) * 1000),
        };
        match kind {
            QuotaWindowKind::FiveHour => five_hour = Some(usage),
            QuotaWindowKind::Weekly => weekly = Some(usage),
        }
    }

    if five_hour.is_none() && weekly.is_none() {
        return None;
    }
    Some(EndpointQuota {
        source: UsageSource::ServerReported,
        five_hour: five_hour.unwrap_or_else(WindowUsage::empty),
        weekly: weekly.unwrap_or_else(WindowUsage::empty),
        observed_at_ms: now_ms,
    })
}

/// The response-header prefixes this module consumes, so the transport edge and
/// the parser can never drift apart.
pub const CODEX_USAGE_HEADER_PREFIXES: [&str; 2] = ["x-codex-primary", "x-codex-secondary"];

/// Track whether a header name is one of the ones the parser understands.
pub fn is_codex_usage_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    CODEX_USAGE_HEADER_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// Token counts for one request/response, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

impl TokenUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.cached_input_tokens + self.output_tokens
    }

    /// Weighted units the official rate card would charge for this usage.
    pub fn weighted_units(&self) -> f64 {
        self.input_tokens as f64
            + self.cached_input_tokens as f64 * WEIGHT_CACHED_INPUT
            + self.output_tokens as f64 * WEIGHT_OUTPUT
    }
}

/// Nominal weighted-unit budget of each window for a subscription endpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubscriptionBudget {
    pub five_hour_units: f64,
    pub weekly_units: f64,
}

impl Default for SubscriptionBudget {
    fn default() -> Self {
        Self {
            five_hour_units: DEFAULT_FIVE_HOUR_UNITS,
            weekly_units: DEFAULT_WEEKLY_UNITS,
        }
    }
}

impl SubscriptionBudget {
    pub fn units_for(&self, kind: QuotaWindowKind) -> f64 {
        match kind {
            QuotaWindowKind::FiveHour => self.five_hour_units,
            QuotaWindowKind::Weekly => self.weekly_units,
        }
        .max(1.0)
    }
}

/// One locally-tracked window: consumed weighted units plus the window anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LocalWindow {
    consumed_units: f64,
    resets_at_ms: i64,
}

impl LocalWindow {
    fn fresh(now_ms: i64, kind: QuotaWindowKind) -> Self {
        Self {
            consumed_units: 0.0,
            resets_at_ms: now_ms + kind.period_ms(),
        }
    }

    fn observe(&mut self, kind: QuotaWindowKind, delta_units: f64, now_ms: i64) {
        if now_ms >= self.resets_at_ms {
            *self = Self::fresh(now_ms, kind);
        }
        self.consumed_units += delta_units.max(0.0);
    }

    fn usage(&self, budget_units: f64, now_ms: i64) -> WindowUsage {
        if now_ms >= self.resets_at_ms {
            return WindowUsage {
                used_ratio: 0.0,
                resets_at_ms: Some(self.resets_at_ms),
            };
        }
        WindowUsage {
            used_ratio: (self.consumed_units / budget_units.max(1.0)).clamp(0.0, 1.0),
            resets_at_ms: Some(self.resets_at_ms),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct EndpointLedger {
    server: Option<EndpointQuota>,
    five_hour: LocalWindow,
    weekly: LocalWindow,
}

/// Process-wide ledger of subscription usage, keyed by endpoint name.
///
/// Mirrors [`super::failover::shared_endpoint_health`]: route planning reads it
/// fresh on every new turn, so a window reset automatically lets the
/// subscription endpoint back without any background timer.
pub struct QuotaLedger {
    inner: Mutex<HashMap<String, EndpointLedger>>,
}

impl Default for QuotaLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl QuotaLedger {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn with_entry<R>(
        &self,
        endpoint: &str,
        now_ms: i64,
        f: impl FnOnce(&mut EndpointLedger) -> R,
    ) -> R {
        let mut guard = self.inner.lock().expect("quota ledger mutex poisoned");
        let entry = guard
            .entry(endpoint.to_string())
            .or_insert_with(|| EndpointLedger {
                server: None,
                five_hour: LocalWindow::fresh(now_ms, QuotaWindowKind::FiveHour),
                weekly: LocalWindow::fresh(now_ms, QuotaWindowKind::Weekly),
            });
        f(entry)
    }

    /// Record a snapshot read from the service's own usage headers.
    pub fn record_server_usage(&self, endpoint: &str, quota: EndpointQuota) {
        let now_ms = quota.observed_at_ms;
        self.with_entry(endpoint, now_ms, |entry| entry.server = Some(quota));
    }

    /// Add locally-observed token usage to the running estimate.
    pub fn record_local_usage(&self, endpoint: &str, usage: TokenUsage, now_ms: i64) {
        let units = usage.weighted_units();
        self.with_entry(endpoint, now_ms, |entry| {
            entry
                .five_hour
                .observe(QuotaWindowKind::FiveHour, units, now_ms);
            entry.weekly.observe(QuotaWindowKind::Weekly, units, now_ms);
        });
    }

    /// The best snapshot we have: the service reading when it is still inside
    /// its own window, otherwise the local estimate. Never fabricates a
    /// server reading from an estimate.
    pub fn snapshot(
        &self,
        endpoint: &str,
        budget: SubscriptionBudget,
        now_ms: i64,
    ) -> Option<EndpointQuota> {
        self.with_entry(endpoint, now_ms, |entry| {
            let had_reading = entry.server.is_some();
            if let Some(server) = entry.server {
                let still_valid = server
                    .five_hour
                    .resets_at_ms
                    .map(|reset| reset > now_ms)
                    .unwrap_or(false)
                    || server
                        .weekly
                        .resets_at_ms
                        .map(|reset| reset > now_ms)
                        .unwrap_or(false);
                // Keep stale server evidence for conservative routing; display labels
                // distinguish expired windows until a successful refresh arrives.
                let _ = still_valid;
                return Some(server);
            }
            // No reading and no locally-observed usage: we know nothing, which
            // is not the same as "zero used". Saying so lets the caller try its
            // next key instead of treating silence as a 0% reading. A stale
            // reading whose windows have all reset is different — those windows
            // are genuinely back to zero, and that is what we report.
            if !had_reading && entry.five_hour.consumed_units <= 0.0 && entry.weekly.consumed_units <= 0.0
            {
                return None;
            }
            Some(EndpointQuota {
                source: UsageSource::Estimated,
                five_hour: entry
                    .five_hour
                    .usage(budget.units_for(QuotaWindowKind::FiveHour), now_ms),
                weekly: entry
                    .weekly
                    .usage(budget.units_for(QuotaWindowKind::Weekly), now_ms),
                observed_at_ms: now_ms,
            })
        })
    }

    /// Evaluate the cap for one endpoint as of `now_ms`.
    pub fn evaluate(
        &self,
        endpoint: &str,
        cap: QuotaCap,
        budget: SubscriptionBudget,
        now_ms: i64,
    ) -> Option<CapBreach> {
        let snapshot = self.snapshot(endpoint, budget, now_ms)?;
        evaluate_cap(&snapshot, cap, now_ms)
    }

    /// Forget everything we know about `endpoint` (test seam).
    pub fn clear(&self, endpoint: &str) {
        self.inner
            .lock()
            .expect("quota ledger mutex poisoned")
            .remove(endpoint);
    }
}

static SHARED_QUOTA_LEDGER: OnceLock<QuotaLedger> = OnceLock::new();

/// The process-wide ledger route planning consults.
pub fn shared_quota_ledger() -> &'static QuotaLedger {
    SHARED_QUOTA_LEDGER.get_or_init(QuotaLedger::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_760_000_000_000; // fixed instant for deterministic tests

    fn server_quota(five_hour: f64, weekly: f64) -> EndpointQuota {
        EndpointQuota {
            source: UsageSource::ServerReported,
            five_hour: WindowUsage {
                used_ratio: five_hour,
                resets_at_ms: Some(NOW + 60_000),
            },
            weekly: WindowUsage {
                used_ratio: weekly,
                resets_at_ms: Some(NOW + 3_600_000),
            },
            observed_at_ms: NOW,
        }
    }

    // CF-QUOTA-R1: synthetic service data parses correctly.
    #[test]
    fn parses_service_reported_usage_from_codex_headers() {
        let headers = [
            ("x-codex-primary-used-percent", "42.5"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-after-seconds", "1800"),
            ("x-codex-secondary-used-percent", "80"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-after-seconds", "3600"),
        ];
        let quota = parse_codex_usage_headers(headers, NOW).expect("server usage parses");
        assert_eq!(quota.source, UsageSource::ServerReported);
        assert!((quota.five_hour.used_ratio - 0.425).abs() < 1e-9);
        assert_eq!(quota.five_hour.resets_at_ms, Some(NOW + 1_800_000));
        assert!((quota.weekly.used_ratio - 0.80).abs() < 1e-9);
        assert_eq!(quota.weekly.resets_at_ms, Some(NOW + 3_600_000));
    }

    // CF-QUOTA-R1: window length decides which window a header set describes.
    #[test]
    fn primary_header_with_a_weekly_length_lands_in_the_weekly_window() {
        let headers = [
            ("x-codex-primary-used-percent", "20"),
            ("x-codex-primary-window-minutes", "10080"),
        ];
        let quota = parse_codex_usage_headers(headers, NOW).unwrap();
        assert!((quota.weekly.used_ratio - 0.20).abs() < 1e-9);
        assert_eq!(quota.five_hour.used_ratio, 0.0);
    }

    // CF-QUOTA-R1: no usage header at all → the parser says "unknown" instead of
    // inventing a reading.
    #[test]
    fn absent_usage_headers_produce_no_snapshot() {
        let headers = [
            ("content-type", "application/json"),
            ("x-request-id", "abc"),
        ];
        assert!(parse_codex_usage_headers(headers, NOW).is_none());
    }

    #[test]
    fn header_name_detection_matches_the_parser() {
        assert!(is_codex_usage_header("x-codex-primary-used-percent"));
        assert!(is_codex_usage_header(
            "X-Codex-Secondary-Reset-After-Seconds"
        ));
        assert!(!is_codex_usage_header("content-type"));
    }

    // CF-QUOTA-R3 / test matrix: 79% / 80% / 95% / 100%.
    #[test]
    fn cap_boundary_is_inclusive_at_the_configured_share() {
        let cap = QuotaCap::default();
        assert!(evaluate_cap(&server_quota(0.79, 0.10), cap, NOW).is_none());
        let at_cap = evaluate_cap(&server_quota(0.80, 0.10), cap, NOW).expect("80% is at the cap");
        assert_eq!(at_cap.window, QuotaWindowKind::FiveHour);
        assert_eq!(at_cap.used_percent, 80);
        assert_eq!(at_cap.cap_percent, 80);
        assert!(evaluate_cap(&server_quota(0.95, 0.10), cap, NOW).is_some());
        let full = evaluate_cap(&server_quota(1.0, 0.10), cap, NOW).unwrap();
        assert_eq!(full.used_percent, 100);
    }

    // CF-QUOTA-R3: the weekly window binds independently of the 5-hour window.
    #[test]
    fn weekly_cap_binds_even_when_the_five_hour_window_is_low() {
        let breach = evaluate_cap(&server_quota(0.05, 0.85), QuotaCap::default(), NOW)
            .expect("weekly cap binds");
        assert_eq!(breach.window, QuotaWindowKind::Weekly);
        assert_eq!(breach.used_percent, 85);
    }

    // CF-QUOTA-R3 live repair: reset time alone cannot authorize requests.
    #[test]
    fn a_reset_window_requires_a_fresh_reading_before_clearing_the_breach() {
        let mut quota = server_quota(1.0, 0.5);
        quota.five_hour.resets_at_ms = Some(NOW - 1);
        let breach = evaluate_cap(&quota, QuotaCap::default(), NOW)
            .expect("expired reading must not silently authorize subscription use");
        assert_eq!(breach.window, QuotaWindowKind::FiveHour);
        let mut refreshed = server_quota(0.05, 0.5);
        refreshed.source = UsageSource::LiveQuery;
        assert!(evaluate_cap(&refreshed, QuotaCap::default(), NOW).is_none());
    }

    // CF-QUOTA-R2: caps are configurable and clamped.
    #[test]
    fn caps_are_configurable_and_clamped_to_100() {
        let cap = QuotaCap::new(50, 120);
        assert_eq!(cap.five_hour_percent, 50);
        assert_eq!(cap.weekly_percent, 100);
        // A lower cap makes 79% already a breach.
        assert!(evaluate_cap(&server_quota(0.79, 0.0), cap, NOW).is_some());
    }

    // CF-QUOTA-R1: the estimate weights token kinds per the official rate card.
    #[test]
    fn local_estimate_weights_token_kinds_per_the_rate_card() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            cached_input_tokens: 4_000_000,
            output_tokens: 250_000,
        };
        assert_eq!(usage.total_tokens(), 5_250_000);
        // 1M*1 + 4M*0.25 + 0.25M*4 = 1M + 1M + 1M
        assert!((usage.weighted_units() - 3_000_000.0).abs() < 1e-6);
    }

    // CF-QUOTA-R1: with no service reading, the ledger falls back to the local
    // estimate and says so.
    #[test]
    fn no_service_reading_falls_back_to_a_labelled_estimate() {
        let ledger = QuotaLedger::new();
        ledger.record_local_usage(
            "chatgpt",
            TokenUsage {
                input_tokens: 15_000_000,
                cached_input_tokens: 0,
                output_tokens: 0,
            },
            NOW,
        );
        let quota = ledger
            .snapshot("chatgpt", SubscriptionBudget::default(), NOW)
            .expect("estimate is available");
        assert_eq!(quota.source, UsageSource::Estimated);
        assert!(quota.source.is_estimate());
        assert!((quota.five_hour.used_ratio - 0.5).abs() < 1e-9);
        assert!(evaluate_cap(&quota, QuotaCap::default(), NOW).is_none());

        // Cross 80% of the 5-hour budget → the ledger reports a breach.
        ledger.record_local_usage(
            "chatgpt",
            TokenUsage {
                input_tokens: 10_000_000,
                cached_input_tokens: 0,
                output_tokens: 0,
            },
            NOW,
        );
        let breach = ledger
            .evaluate(
                "chatgpt",
                QuotaCap::default(),
                SubscriptionBudget::default(),
                NOW,
            )
            .expect("estimated usage crosses the cap");
        assert_eq!(breach.used_percent, 83);
        assert!(breach.source.is_estimate());
    }

    // CF-QUOTA-R1: a server reading wins while it is still inside its window.
    #[test]
    fn a_service_reading_overrides_the_local_estimate_inside_its_window() {
        let ledger = QuotaLedger::new();
        ledger.record_local_usage(
            "chatgpt",
            TokenUsage {
                input_tokens: 1_000_000,
                ..Default::default()
            },
            NOW,
        );
        ledger.record_server_usage("chatgpt", server_quota(0.85, 0.10));
        let quota = ledger
            .snapshot("chatgpt", SubscriptionBudget::default(), NOW)
            .unwrap();
        assert_eq!(quota.source, UsageSource::ServerReported);
        assert!((quota.five_hour.used_ratio - 0.85).abs() < 1e-9);
    }

    // CF-QUOTA-R9: retain expired evidence and block until refreshed.
    #[test]
    fn a_fully_reset_server_snapshot_remains_blocked_until_refreshed() {
        let ledger = QuotaLedger::new();
        let mut stale = server_quota(0.9, 0.9);
        stale.five_hour.resets_at_ms = Some(NOW - 10);
        stale.weekly.resets_at_ms = Some(NOW - 10);
        ledger.record_server_usage("chatgpt", stale);
        let quota = ledger
            .snapshot("chatgpt", SubscriptionBudget::default(), NOW)
            .unwrap();
        assert_eq!(quota.source, UsageSource::ServerReported);
        assert!(evaluate_cap(&quota, QuotaCap::default(), NOW).is_some());
        let mut fresh = server_quota(0.12, 0.24);
        fresh.source = UsageSource::LiveQuery;
        ledger.record_server_usage("chatgpt", fresh);
        assert!(ledger.evaluate("chatgpt", QuotaCap::default(), SubscriptionBudget::default(), NOW).is_none());
    }

    // CF-QUOTA-R5: only the subscription endpoint carries quota; a pay-per-token
    // endpoint has no ledger entry and therefore no breach.
    #[test]
    fn an_endpoint_with_no_usage_is_never_over_cap() {
        let ledger = QuotaLedger::new();
        assert!(ledger
            .evaluate(
                "deepseek",
                QuotaCap::default(),
                SubscriptionBudget::default(),
                NOW
            )
            .is_none());
    }

    // CF-QUOTA-R4: the handover sentence is plain language, carries the reset
    // time, and labels an estimate.
    #[test]
    fn handover_notice_names_the_window_the_share_and_the_reset_time() {
        let breach = evaluate_cap(&server_quota(0.80, 0.10), QuotaCap::default(), NOW).unwrap();
        let notice = breach.handover_notice("DeepSeek", NOW);
        assert!(notice.contains("已用 80%"), "{notice}");
        assert!(notice.contains("已切到 DeepSeek"), "{notice}");
        assert!(notice.contains("约 ") && notice.contains(" 恢复"), "{notice}");
        assert!(!notice.contains("（本地估算）"), "{notice}");

        let mut estimated = server_quota(0.90, 0.10);
        estimated.source = UsageSource::Estimated;
        let estimate_breach = evaluate_cap(&estimated, QuotaCap::default(), NOW).unwrap();
        assert!(estimate_breach
            .handover_notice("DeepSeek", NOW)
            .contains("（本地估算）"));
    }

    #[test]
    fn reset_hint_is_absent_once_the_window_has_passed() {
        let mut quota = server_quota(0.9, 0.9);
        quota.five_hour.resets_at_ms = Some(NOW - 5);
        // The five-hour window has reset (and would fault), so the still-open
        // weekly window is the one that binds.
        assert_eq!(
            evaluate_cap(&quota, QuotaCap::default(), NOW).unwrap().window,
            QuotaWindowKind::Weekly
        );
        let breach = CapBreach {
            window: QuotaWindowKind::FiveHour,
            cap_percent: 80,
            used_percent: 90,
            source: UsageSource::ServerReported,
            resets_at_ms: Some(NOW - 5),
        };
        assert!(breach.reset_hint(NOW).is_none());
    }
}

// ===================================================================== M212
// 按端点的每日人民币预算（例如 DeepSeek 每天 ¥10）。
// 实现放在 quota_cap.rs 内：它与订阅用量上限共用同一套 WaitCause 等待语义。
pub mod daily_spend {
    // SPDX-License-Identifier: Apache-2.0
    //  M212: per-endpoint daily spend cap in CNY.
    // 
    //  A pay-per-token endpoint can burn a whole prepaid balance in a day without
    //  any technical failure: DeepSeek's 10-29 usage was ~43M input tokens, the
    //  balance ran out, and every request then failed as `PROVIDER_FATAL` for the
    //  rest of the day. This module makes "how much did this endpoint cost me
    //  today" a first-class number and turns the configured cap into a routing
    //  decision, using the same wait mechanism as the subscription quota cap
    //  (`subscription_quota_cap`) so the turn enters `waiting_wake` instead of
    //  looking like a retry that failed.
    // 
    //  Accounting truth, in priority order:
    // 
    //  1. the provider's own balance (`GET /user/balance` snapshots) — daily spend
    //     is `first snapshot of the day + top-ups - current balance`;
    //  2. our own token accounting priced with the configured per-million table
    //     (`cost_source = configured_price`), marked "估算" in the UI.
    // 
    //  Everything here is local-day scoped in Asia/Shanghai, matching every other
    //  usage surface in the app.

    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, RwLock};

    use chrono::{DateTime, TimeZone, Utc};
    use serde::{Deserialize, Serialize};

    /// The cap currency is CNY because the user's provider bills in CNY.
    pub const CURRENCY: &str = "CNY";
    pub const DEFAULT_DAILY_CAP_CNY: f64 = 10.0;
    /// M212 补充：单价按用户 10-30 的真实账单反算，元每百万 token。
    /// 命中缓存 ¥0.1 / 未命中输入 ¥0.8 / 输出 ¥2。
    pub const DEFAULT_CACHE_HIT_INPUT_CNY_PER_MTOK: f64 = 0.1;
    pub const DEFAULT_INPUT_CNY_PER_MTOK: f64 = 0.8;
    pub const DEFAULT_OUTPUT_CNY_PER_MTOK: f64 = 2.0;
    /// R8: warn before the balance is gone, not after.
    pub const DEFAULT_LOW_BALANCE_WARN_CNY: f64 = 20.0;
    /// `cost_source` for our own priced estimate (never provider-actual dollars).
    pub const CONFIGURED_PRICE_COST_SOURCE: &str = "configured_price";

    /// Per-million-token prices for one endpoint, in CNY.
    #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
    pub struct PriceTable {
        #[serde(default = "default_cache_hit_input_price")]
        pub cache_hit_input_cny_per_mtok: f64,
        #[serde(default = "default_input_price")]
        pub input_cny_per_mtok: f64,
        #[serde(default = "default_output_price")]
        pub output_cny_per_mtok: f64,
    }

    fn default_cache_hit_input_price() -> f64 {
        DEFAULT_CACHE_HIT_INPUT_CNY_PER_MTOK
    }
    fn default_input_price() -> f64 {
        DEFAULT_INPUT_CNY_PER_MTOK
    }
    fn default_output_price() -> f64 {
        DEFAULT_OUTPUT_CNY_PER_MTOK
    }

    impl Default for PriceTable {
        fn default() -> Self {
            Self {
                cache_hit_input_cny_per_mtok: DEFAULT_CACHE_HIT_INPUT_CNY_PER_MTOK,
                input_cny_per_mtok: DEFAULT_INPUT_CNY_PER_MTOK,
                output_cny_per_mtok: DEFAULT_OUTPUT_CNY_PER_MTOK,
            }
        }
    }

    impl PriceTable {
        /// A price table is usable only when at least one rate is positive;
        /// otherwise the endpoint has no money number and the cap cannot judge.
        pub fn is_usable(&self) -> bool {
            self.input_cny_per_mtok > 0.0
                || self.cache_hit_input_cny_per_mtok > 0.0
                || self.output_cny_per_mtok > 0.0
        }

        /// Estimated cost in CNY. `cached_tokens` is the cache-hit share of
        /// `input_tokens`, so the miss share is the remainder — double counting
        /// the cached part was what made the earlier watchdog overestimate ~3x.
        pub fn estimate_cny(&self, input_tokens: i64, cached_tokens: i64, output_tokens: i64) -> f64 {
            let input = input_tokens.max(0) as f64;
            let cached = (cached_tokens.max(0) as f64).min(input);
            let miss = input - cached;
            let output = output_tokens.max(0) as f64;
            let per_mtok = 1_000_000.0;
            (miss * self.input_cny_per_mtok
                + cached * self.cache_hit_input_cny_per_mtok
                + output * self.output_cny_per_mtok)
                / per_mtok
        }
    }

    /// R1: what the user can configure per endpoint.
    #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
    pub struct DailySpendConfig {
        #[serde(default = "default_enabled")]
        pub enabled: bool,
        #[serde(default = "default_daily_cap_cny")]
        pub daily_cap_cny: f64,
        #[serde(default = "default_low_balance_warn_cny")]
        pub low_balance_warn_cny: f64,
        #[serde(default)]
        pub prices: PriceTable,
    }

    fn default_enabled() -> bool {
        true
    }
    fn default_daily_cap_cny() -> f64 {
        DEFAULT_DAILY_CAP_CNY
    }
    fn default_low_balance_warn_cny() -> f64 {
        DEFAULT_LOW_BALANCE_WARN_CNY
    }

    impl Default for DailySpendConfig {
        fn default() -> Self {
            Self {
                enabled: true,
                daily_cap_cny: DEFAULT_DAILY_CAP_CNY,
                low_balance_warn_cny: DEFAULT_LOW_BALANCE_WARN_CNY,
                prices: PriceTable::default(),
            }
        }
    }

    impl DailySpendConfig {
        /// A disabled cap or a non-positive cap means "no hard limit": the number
        /// still gets recorded, it just never blocks routing.
        pub fn is_enforcing(&self) -> bool {
            self.enabled && self.daily_cap_cny > 0.0
        }
    }

    /// R6: DeepSeek ships with ¥10/day and the bill-derived price table, so the
    /// feature works before the user opens settings.
    pub fn default_config() -> DailySpendConfig {
        DailySpendConfig::default()
    }

    /// Endpoint names that get the built-in default cap without any user config.
    fn has_builtin_default(endpoint_name: &str) -> bool {
        endpoint_name.to_ascii_lowercase().contains("deepseek")
    }

    // ---------------------------------------------------------------- local day

    fn shanghai() -> chrono_tz::Tz {
        chrono_tz::Asia::Shanghai
    }

    /// Start of the local (Asia/Shanghai) day containing `now_ms`, as epoch ms.
    pub fn day_start_ms(now_ms: i64) -> Option<i64> {
        let now = Utc.timestamp_millis_opt(now_ms).single()?;
        let local = now.with_timezone(&shanghai());
        let date = local.date_naive();
        let start = date.and_hms_opt(0, 0, 0)?;
        shanghai()
            .from_local_datetime(&start)
            .single()
            .map(|at| at.timestamp_millis())
    }

    /// R3: the next local midnight — the second part of the wake time.
    pub fn next_reset_ms(now_ms: i64) -> Option<i64> {
        let now = Utc.timestamp_millis_opt(now_ms).single()?;
        let local = now.with_timezone(&shanghai());
        let tomorrow = local.date_naive().succ_opt()?;
        let start = tomorrow.and_hms_opt(0, 0, 0)?;
        shanghai()
            .from_local_datetime(&start)
            .single()
            .map(|at| at.timestamp_millis())
    }

    // ------------------------------------------------------------- balance math

    /// One `GET /user/balance` observation. `topped_up_cny` is the credit that
    /// arrived since the previous observation (a rising balance is a top-up, not
    /// negative spend).
    #[derive(Debug, Clone, Copy, PartialEq, Serialize)]
    pub struct BalanceSnapshot {
        pub at_ms: i64,
        pub balance_cny: f64,
        pub topped_up_cny: f64,
    }

    /// R7: real spend for the day = opening balance + top-ups - current balance.
    pub fn real_spend_cny(opening_balance_cny: f64, topped_up_cny: f64, current_balance_cny: f64) -> f64 {
        (opening_balance_cny + topped_up_cny - current_balance_cny).max(0.0)
    }

    /// Parses DeepSeek's `GET /user/balance` body: CNY entries under
    /// `balance_infos`. Returns `None` when the payload carries no CNY number.
    pub fn parse_balance_cny(body: &str) -> Option<f64> {
        let value: serde_json::Value = serde_json::from_str(body).ok()?;
        let infos = value.get("balance_infos")?.as_array()?;
        infos
            .iter()
            .filter(|info| {
                info.get("currency")
                    .and_then(|currency| currency.as_str())
                    .map(|currency| currency.eq_ignore_ascii_case(CURRENCY))
                    .unwrap_or(true)
            })
            .find_map(|info| info.get("total_balance")?.as_str()?.parse::<f64>().ok())
    }

    /// Whether a request failure means "the prepaid balance is gone", so the user
    /// sees `DeepSeek 余额不足` instead of a bare `PROVIDER_FATAL` (R8).
    pub fn is_insufficient_balance(status: u16, body: &str) -> bool {
        let text = body.to_ascii_lowercase();
        status == 402
            || (status == 400 && text.contains("insufficient balance"))
            || text.contains("insufficient_balance")
            || text.contains("balance is not enough")
    }

    // ------------------------------------------------------------------- ledger

    #[derive(Debug, Clone, Default)]
    struct EndpointLedger {
        day_start_ms: i64,
        estimated_cny: f64,
        opening_balance_cny: Option<f64>,
        topped_up_cny: f64,
        current_balance_cny: Option<f64>,
    }

    /// In-memory, per-endpoint, per-local-day spend. Recorded as usage events
    /// arrive so a turn that crosses the line stops at the end of its current
    /// request (R4) without a database round trip on every routing decision.
    #[derive(Debug, Default)]
    pub struct SpendLedger {
        entries: Mutex<HashMap<String, EndpointLedger>>,
    }

    impl SpendLedger {
        fn entry(&self, endpoint: &str, now_ms: i64) -> std::sync::MutexGuard<'_, HashMap<String, EndpointLedger>> {
            let mut guard = self.entries.lock().expect("spend ledger poisoned");
            let day = day_start_ms(now_ms).unwrap_or(now_ms);
            let stored_day = guard.get(endpoint).map(|entry| entry.day_start_ms).unwrap_or(day);
            if stored_day < day {
                // A new local day: yesterday's numbers never carry over.
                guard.insert(endpoint.to_string(), EndpointLedger { day_start_ms: day, ..Default::default() });
            }
            guard
                .entry(endpoint.to_string())
                .or_insert_with(|| EndpointLedger { day_start_ms: day, ..Default::default() });
            guard
        }

        /// Add priced usage to today's estimate.
        pub fn record_estimated(&self, endpoint: &str, cost_cny: f64, now_ms: i64) {
            if !(cost_cny > 0.0) {
                return;
            }
            let mut guard = self.entry(endpoint, now_ms);
            if let Some(entry) = guard.get_mut(endpoint) {
                entry.estimated_cny += cost_cny;
            }
        }

        /// Record a balance observation. The first observation of the day is the
        /// opening balance; a later rise is credit (a top-up), never negative spend.
        pub fn observe_balance(&self, endpoint: &str, balance_cny: f64, now_ms: i64) {
            let mut guard = self.entry(endpoint, now_ms);
            let Some(entry) = guard.get_mut(endpoint) else { return };
            match (entry.current_balance_cny, entry.opening_balance_cny) {
                (None, _) => {
                    entry.opening_balance_cny = Some(balance_cny);
                }
                (Some(previous), _) if balance_cny > previous => {
                    entry.topped_up_cny += balance_cny - previous;
                }
                _ => {}
            }
            entry.current_balance_cny = Some(balance_cny);
        }

        pub fn estimated_cny(&self, endpoint: &str, now_ms: i64) -> f64 {
            let guard = self.entry(endpoint, now_ms);
            guard.get(endpoint).map(|entry| entry.estimated_cny).unwrap_or(0.0)
        }

        fn real_cny(&self, endpoint: &str, now_ms: i64) -> Option<f64> {
            let guard = self.entry(endpoint, now_ms);
            let entry = guard.get(endpoint)?;
            let opening = entry.opening_balance_cny?;
            let current = entry.current_balance_cny?;
            Some(real_spend_cny(opening, entry.topped_up_cny, current))
        }

        fn balance_cny(&self, endpoint: &str, now_ms: i64) -> Option<f64> {
            let guard = self.entry(endpoint, now_ms);
            guard.get(endpoint).and_then(|entry| entry.current_balance_cny)
        }

        /// R4: the cap blocks the *next* request, never the one already in flight.
        pub fn status(&self, endpoint: &str, config: &DailySpendConfig, now_ms: i64) -> EndpointSpendStatus {
            let estimated = self.estimated_cny(endpoint, now_ms);
            let real = self.real_cny(endpoint, now_ms);
            let balance = self.balance_cny(endpoint, now_ms);
            let source = if real.is_some() { SpendSource::Balance } else { SpendSource::Estimated };
            let spent = real.unwrap_or(estimated);
            let cap = config.daily_cap_cny;
            let remaining = cap - spent;
            let low_balance = balance.map(|balance| balance <= config.low_balance_warn_cny);
            EndpointSpendStatus {
                endpoint: endpoint.to_string(),
                currency: CURRENCY.to_string(),
                spent_cny: round4(spent),
                estimated_cny: round4(estimated),
                cap_cny: round4(cap),
                remaining_cny: round4(remaining),
                source,
                reset_at_ms: next_reset_ms(now_ms),
                cap_reached: config.is_enforcing() && spent >= cap,
                enforcing: config.is_enforcing(),
                balance_cny: balance.map(round4),
                low_balance,
                low_balance_warn_cny: round4(config.low_balance_warn_cny),
            }
        }
    }

    fn round4(value: f64) -> f64 {
        (value * 10_000.0).round() / 10_000.0
    }

    /// R7: `balance` = real money, `estimated` = our token math. The UI must say
    /// which one it is showing.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum SpendSource {
        Balance,
        Estimated,
    }

    impl SpendSource {
        pub fn is_estimate(self) -> bool {
            matches!(self, SpendSource::Estimated)
        }
    }

    /// R5/R9: what the usage page and the read-only local entry report.
    #[derive(Debug, Clone, Serialize)]
    pub struct EndpointSpendStatus {
        pub endpoint: String,
        pub currency: String,
        pub spent_cny: f64,
        pub estimated_cny: f64,
        pub cap_cny: f64,
        pub remaining_cny: f64,
        pub source: SpendSource,
        pub reset_at_ms: Option<i64>,
        pub cap_reached: bool,
        pub enforcing: bool,
        pub balance_cny: Option<f64>,
        pub low_balance: Option<bool>,
        pub low_balance_warn_cny: f64,
    }

    impl EndpointSpendStatus {
        /// 「DeepSeek 今日 ¥x / ¥10」— with the estimate marker when the number is
        /// ours rather than the provider's bill.
        pub fn headline(&self) -> String {
            let estimate = if self.source.is_estimate() { "（估算）" } else { "" };
            format!(
                "{} 今日 ¥{:.2} / ¥{:.2}{}",
                display_name(&self.endpoint),
                self.spent_cny,
                self.cap_cny,
                estimate
            )
        }

        /// R5/R3: one readable sentence when the budget is gone.
        pub fn cap_notice(&self) -> Option<String> {
            if !self.cap_reached {
                return None;
            }
            Some(format!(
                "{} 今日预算已用完，明天 0 点后继续",
                display_name(&self.endpoint)
            ))
        }

        /// R8: warn before the balance is unspendable.
        pub fn low_balance_notice(&self) -> Option<String> {
            let balance = self.balance_cny?;
            if self.low_balance != Some(true) {
                return None;
            }
            Some(format!(
                "{} 余额 ¥{:.2}，低于 ¥{:.2}，请及时充值",
                display_name(&self.endpoint),
                balance,
                self.low_balance_warn_cny
            ))
        }
    }

    /// Endpoint names are stable ids (`deepseek`, `deepseek-backup`); show a
    /// human label in the sentence the user reads.
    pub fn display_name(endpoint: &str) -> String {
        let lower = endpoint.to_ascii_lowercase();
        if lower.contains("deepseek") {
            "DeepSeek".to_string()
        } else if endpoint.is_empty() {
            "该模型".to_string()
        } else {
            endpoint.to_string()
        }
    }

    static LEDGER: OnceLock<SpendLedger> = OnceLock::new();

    pub fn ledger() -> &'static SpendLedger {
        LEDGER.get_or_init(SpendLedger::default)
    }

    // -------------------------------------------------------- config registry
    //
    // The accounting funnel (`record_usage_event`) has no settings handle, so the
    // resolved per-endpoint configs are mirrored here whenever settings load or
    // save. Missing entries fall back to the built-in DeepSeek default.

    static CONFIGS: OnceLock<RwLock<HashMap<String, DailySpendConfig>>> = OnceLock::new();

    fn configs() -> &'static RwLock<HashMap<String, DailySpendConfig>> {
        CONFIGS.get_or_init(|| RwLock::new(HashMap::new()))
    }

    pub fn install_configs(map: HashMap<String, DailySpendConfig>) {
        if let Ok(mut guard) = configs().write() {
            *guard = map;
        }
    }

    pub fn config_for(endpoint: &str) -> Option<DailySpendConfig> {
        if let Ok(guard) = configs().read() {
            if let Some(config) = guard.get(endpoint) {
                return Some(*config);
            }
        }
        if has_builtin_default(endpoint) {
            return Some(default_config());
        }
        None
    }

    /// R2: price an event for endpoints that have a price table.
    pub fn price_usage(endpoint: &str, input_tokens: i64, cached_tokens: i64, output_tokens: i64) -> Option<f64> {
        let config = config_for(endpoint)?;
        if !config.prices.is_usable() {
            return None;
        }
        Some(config.prices.estimate_cny(input_tokens, cached_tokens, output_tokens))
    }

    /// R3: the routing decision. `None` means "this endpoint may be used".
    ///
    /// The `until_ms` is the next local midnight, so a waiting turn resumes when
    /// the daily bucket refills; callers may push it later (e.g. an endpoint that
    /// is also outside its usage window).
    pub fn cap_wait_cause(
        config: &DailySpendConfig,
        endpoint: &str,
        now_ms: i64,
    ) -> Option<crate::agent::usage_windows::WaitCause> {
        if !config.is_enforcing() {
            return None;
        }
        let status = ledger().status(endpoint, config, now_ms);
        if !status.cap_reached {
            return None;
        }
        Some(crate::agent::usage_windows::WaitCause::new(
            display_name(endpoint),
            crate::agent::usage_windows::WaitCauseKind::DailySpendCap,
            status.reset_at_ms,
        ))
    }

    /// R9 payload for one endpoint, without credentials or base URLs.
    pub fn status_for(endpoint: &str, config: &DailySpendConfig, now_ms: i64) -> EndpointSpendStatus {
        ledger().status(endpoint, config, now_ms)
    }

    /// Tests use distinct endpoint names instead of a global reset: the ledger is
    /// day-scoped per endpoint, and clearing the shared map would race with the
    /// other tests running in parallel in the same process.
    pub(crate) fn reset_for_tests() {}

    #[cfg(test)]
    mod tests {
        use super::*;

        fn at(iso: &str) -> i64 {
            DateTime::parse_from_rfc3339(iso).expect("valid iso").timestamp_millis()
        }

        #[test]
        fn price_table_charges_cache_hits_separately() {
            let prices = PriceTable::default();
            // 1M input with 1M cache hits: only the hit rate applies.
            assert_eq!(prices.estimate_cny(1_000_000, 1_000_000, 0), 0.1);
            // 1M input, 400k cached => 600k miss at 0.8 + 400k hit at 0.1.
            let expected = 600_000.0 * 0.8 / 1_000_000.0 + 400_000.0 * 0.1 / 1_000_000.0;
            assert!((prices.estimate_cny(1_000_000, 400_000, 0) - expected).abs() < 1e-9);
            // Output tokens use the output rate: 1M out = ¥2.
            assert_eq!(prices.estimate_cny(0, 0, 1_000_000), 2.0);
            // Cached tokens can never exceed the input count.
            assert_eq!(prices.estimate_cny(1_000, 5_000, 0), 0.0001);
        }

        #[test]
        fn local_day_boundary_is_shanghai_midnight() {
            // 2026-10-30 00:00 +08:00 == 2026-10-29T16:00:00Z
            let midnight = at("2026-10-29T16:00:00Z");
            assert_eq!(day_start_ms(midnight), Some(midnight));
            // One millisecond before that belongs to 10-29 local.
            assert_eq!(
                day_start_ms(midnight - 1),
                Some(at("2026-10-28T16:00:00Z"))
            );
            // 23:59 local (15:59Z) still resets at the coming midnight.
            assert_eq!(next_reset_ms(at("2026-10-30T15:59:59Z")), Some(midnight + 86_400_000));
            assert_eq!(next_reset_ms(midnight), Some(at("2026-10-30T16:00:00Z")));
        }

        #[test]
        fn crossing_the_cap_blocks_and_resets_next_midnight() {
            reset_for_tests();
            let now = at("2026-10-30T02:00:00Z"); // 10:00 +08:00
            let config = DailySpendConfig::default();
            let endpoint = "deepseek-cap";
            // 6M uncached input tokens at ¥0.8/Mtok = ¥4.80, twice = ¥9.60.
            ledger().record_estimated(endpoint, 4.8, now);
            assert!(cap_wait_cause(&config, endpoint, now).is_none());
            ledger().record_estimated(endpoint, 4.8, now); // ¥9.60 still under
            assert!(cap_wait_cause(&config, endpoint, now).is_none());
            ledger().record_estimated(endpoint, 0.5, now); // ¥10.10 over the line
            let cause = cap_wait_cause(&config, endpoint, now).expect("cap reached");
            assert_eq!(cause.kind, crate::agent::usage_windows::WaitCauseKind::DailySpendCap);
            assert_eq!(cause.until_ms, next_reset_ms(now));
            assert_eq!(cause.until_ms, Some(at("2026-10-30T16:00:00Z")));
            let status = ledger().status(endpoint, &config, now);
            assert!(status.cap_reached);
            assert_eq!(status.source, SpendSource::Estimated);
            assert_eq!(status.cap_notice().as_deref(), Some("DeepSeek 今日预算已用完，明天 0 点后继续"));
            // R4: recorded usage never mutates the cap itself.
            assert!((status.spent_cny - 10.1).abs() < 1e-6);

            // Next local day starts from zero.
            let tomorrow = at("2026-10-30T16:00:01Z");
            assert!(cap_wait_cause(&config, endpoint, tomorrow).is_none());
            assert_eq!(ledger().estimated_cny(endpoint, tomorrow), 0.0);
        }

        #[test]
        fn balance_snapshots_are_truth_and_topups_are_credit() {
            reset_for_tests();
            let now = at("2026-10-30T02:00:00Z");
            let config = DailySpendConfig::default();
            let endpoint = "deepseek-bal";
            ledger().record_estimated(endpoint, 9.0, now); // our guess: over the line
            ledger().observe_balance(endpoint, 13.17, now);
            // Real bill says ¥3.17 spent: the cap must follow the money, not the guess.
            ledger().observe_balance(endpoint, 10.0, now);
            let status = ledger().status(endpoint, &config, now);
            assert_eq!(status.source, SpendSource::Balance);
            assert!(!status.cap_reached, "real spend ¥3.17 is under ¥10");
            assert!((status.spent_cny - 3.17).abs() < 1e-6);
            // A top-up is credit, not negative spend: ¥20 arrived, then ¥5 was used.
            ledger().observe_balance(endpoint, 30.0, now);
            ledger().observe_balance(endpoint, 25.0, now);
            let status = ledger().status(endpoint, &config, now);
            assert!(
                (status.spent_cny - 8.17).abs() < 1e-6,
                "opening 13.17 + topup 20 - current 25 = 8.17, got {}",
                status.spent_cny
            );
        }

        #[test]
        fn low_balance_and_insufficient_balance_are_readable() {
            reset_for_tests();
            let now = at("2026-10-30T02:00:00Z");
            let config = DailySpendConfig::default();
            ledger().observe_balance("deepseek-low", 5.0, now);
            let status = ledger().status("deepseek-low", &config, now);
            assert_eq!(status.low_balance, Some(true));
            let notice = status.low_balance_notice().expect("low balance notice");
            assert!(notice.contains("余额 ¥5.00"), "{notice}");
            assert!(is_insufficient_balance(402, ""));
            assert!(is_insufficient_balance(400, "{\"error\":{\"message\":\"Insufficient Balance\"}}"));
            assert!(!is_insufficient_balance(500, "internal error"));
        }

        #[test]
        fn parsed_balance_uses_cny_entries_only() {
            let body = r#"{"is_available":true,"balance_infos":[
                {"currency":"USD","total_balance":"0.00"},
                {"currency":"CNY","total_balance":"13.17"}]}"#;
            assert_eq!(parse_balance_cny(body), Some(13.17));
            assert_eq!(parse_balance_cny("{}"), None);
            assert_eq!(parse_balance_cny("not json"), None);
        }

        #[test]
        fn disabled_cap_never_blocks_and_unpriced_endpoint_prices_nothing() {
            reset_for_tests();
            let now = at("2026-10-30T02:00:00Z");
            ledger().record_estimated("deepseek-off", 50.0, now);
            let mut config = DailySpendConfig::default();
            config.daily_cap_cny = 0.0;
            assert!(cap_wait_cause(&config, "deepseek-off", now).is_none());
            assert_eq!(price_usage("unknown-endpoint", 1_000, 0, 0), None);
            // Built-in default applies to deepseek without any saved settings.
            let priced = price_usage("deepseek", 1_000_000, 0, 0).expect("builtin price");
            assert!((priced - 0.8).abs() < 1e-9);
        }

        #[test]
        fn installed_config_overrides_builtin_default() {
            reset_for_tests();
            let mut config = DailySpendConfig::default();
            config.daily_cap_cny = 0.01;
            install_configs(HashMap::from([("deepseek-override".to_string(), config)]));
            let now = at("2026-10-30T02:00:00Z");
            let resolved = config_for("deepseek-override").expect("configured");
            assert_eq!(resolved.daily_cap_cny, 0.01);
            ledger().record_estimated("deepseek-override", 0.02, now);
            assert!(cap_wait_cause(&resolved, "deepseek-override", now).is_some());
            assert!(cap_wait_cause(&resolved, "deepseek-override", next_reset_ms(now).unwrap()).is_none());
            reset_for_tests();
        }

        #[test]
        fn headline_carries_the_estimate_marker() {
            reset_for_tests();
            let now = at("2026-10-30T02:00:00Z");
            let config = DailySpendConfig::default();
            ledger().record_estimated("deepseek-head", 1.5, now);
            let status = ledger().status("deepseek-head", &config, now);
            assert_eq!(status.headline(), "DeepSeek 今日 ¥1.50 / ¥10.00（估算）");
            ledger().observe_balance("deepseek-head", 8.5, now);
            let real = ledger().status("deepseek-head", &config, now);
            assert_eq!(real.headline(), "DeepSeek 今日 ¥0.00 / ¥10.00");
        }

        /// R3 验收：有其它可用模型时改走其它模型 —— 路由候选选择用的是同一个
        /// `cap_wait_cause`，所以这里断言"被挡的端点让位、未挡的端点可选"。
        #[test]
        fn capped_endpoint_yields_to_an_available_one() {
            reset_for_tests();
            let now = at("2026-10-30T02:00:00Z");
            let config = DailySpendConfig::default();
            ledger().record_estimated("deepseek-capped", 12.0, now);
            let candidates = ["deepseek-capped", "openrouter"];
            let selected = candidates
                .iter()
                .find(|endpoint| cap_wait_cause(&config, endpoint, now).is_none())
                .copied();
            assert_eq!(selected, Some("openrouter"));
            // With only the capped endpoint left there is nothing to fall back to,
            // which is exactly when the turn waits instead of failing.
            assert!(cap_wait_cause(&config, "deepseek-capped", now).is_some());
        }
    }
}

// ---------------------------------------------------------------- M212
// 按端点的每日人民币预算（例如 DeepSeek 每天 ¥10）。

/// 按端点今日预算的只读视图。不含凭据、base_url 或密钥引用。
#[derive(Debug, Clone, serde::Serialize)]
pub struct EndpointSpendReport {
    pub currency: String,
    pub statuses: Vec<crate::agent::quota_cap::daily_spend::EndpointSpendStatus>,
}

/// 只报告**有预算概念**的端点：显式配置过，或命中内置默认值（DeepSeek）。
pub fn endpoint_spend_report(
    settings: &crate::config::settings::Settings,
    now_ms: i64,
) -> EndpointSpendReport {
    let mut names: Vec<String> = settings.endpoint_daily_spend_caps.keys().cloned().collect();
    names.extend(
        settings
            .endpoints
            .keys()
            .filter(|name| name.to_ascii_lowercase().contains("deepseek"))
            .cloned(),
    );
    names.sort();
    names.dedup();
    let statuses = names
        .into_iter()
        .filter_map(|name| {
            settings
                .endpoint_daily_spend_cap(&name)
                .map(|config| crate::agent::quota_cap::daily_spend::status_for(&name, &config, now_ms))
        })
        .collect();
    EndpointSpendReport {
        currency: crate::agent::quota_cap::daily_spend::CURRENCY.to_string(),
        statuses,
    }
}

/// 用法页面与只读入口共用：今日花了多少、上限多少、什么时候重置。
#[tauri::command]
pub fn get_endpoint_spend_status() -> EndpointSpendReport {
    let settings = crate::config::settings::load();
    endpoint_spend_report(&settings, crate::agent::usage_windows::now_ms())
}

#[cfg(test)]
mod m212_daily_spend_tests {
    use super::*;
    use chrono::DateTime;

    fn at(iso: &str) -> i64 {
        DateTime::parse_from_rfc3339(iso).expect("valid").timestamp_millis()
    }

    #[test]
    fn report_lists_deepseek_with_the_builtin_daily_cap() {
        let mut settings = crate::config::settings::Settings::default();
        settings.endpoint_daily_spend_caps.insert(
            "deepseek".into(),
            crate::agent::quota_cap::daily_spend::default_config(),
        );
        let now = at("2026-10-30T02:00:00Z"); // 10:00 +08:00
        let report = endpoint_spend_report(&settings, now);
        assert_eq!(report.currency, "CNY");
        let status = report
            .statuses
            .iter()
            .find(|status| status.endpoint == "deepseek")
            .expect("deepseek reported");
        assert_eq!(status.cap_cny, 10.0);
        assert_eq!(status.reset_at_ms, Some(at("2026-10-30T16:00:00Z")));
        assert!(!status.cap_reached);
        // The display sentence the user reads in the usage page.
        let headline = status.headline();
        assert!(headline.starts_with("DeepSeek 今日 ¥"), "{headline}");
        assert!(headline.ends_with(" / ¥10.00（估算）"), "{headline}");
        assert_eq!(status.cap_notice(), None);
    }
}
