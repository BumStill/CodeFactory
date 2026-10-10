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
            Self::ServerReported => "服务端读数",
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
            let used_percent = window.used_percent(now_ms);
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
                if still_valid {
                    return Some(server);
                }
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

    // CF-QUOTA-R3: after the window resets, usage is zero and the endpoint is
    // eligible again.
    #[test]
    fn a_reset_window_reports_zero_usage_and_clears_the_breach() {
        let mut quota = server_quota(1.0, 0.5);
        quota.five_hour.resets_at_ms = Some(NOW - 1);
        assert!(evaluate_cap(&quota, QuotaCap::default(), NOW).is_none());
        assert_eq!(quota.five_hour.used_percent(NOW), 0);
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

    // CF-QUOTA-R1: once the server snapshot's windows have both reset, the
    // ledger stops presenting the stale reading as current.
    #[test]
    fn a_fully_reset_server_snapshot_falls_back_to_the_estimate() {
        let ledger = QuotaLedger::new();
        let mut stale = server_quota(0.9, 0.9);
        stale.five_hour.resets_at_ms = Some(NOW - 10);
        stale.weekly.resets_at_ms = Some(NOW - 10);
        ledger.record_server_usage("chatgpt", stale);
        let quota = ledger
            .snapshot("chatgpt", SubscriptionBudget::default(), NOW)
            .unwrap();
        assert_eq!(quota.source, UsageSource::Estimated);
        assert!(evaluate_cap(&quota, QuotaCap::default(), NOW).is_none());
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
