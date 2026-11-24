// SPDX-License-Identifier: Apache-2.0
//! M274: OpenAI **API** (pay-per-token, funded by the Pro membership credit) as
//! a first-class route.
//!
//! The ChatGPT subscription route (`chatgpt`) bills nothing per token; the API
//! route (`openai`) bills in **USD** per million tokens. So this module is the
//! USD counterpart of `quota_cap::daily_spend`:
//!
//! * R2 — every call is priced from its model + token counts (cached input is
//!   charged at the cached rate), never treated as free.
//! * R3 — a configurable USD daily cap; reaching it makes this endpoint yield to
//!   another one, and only a turn with no alternative waits.
//! * R5 — today's spend / cap / reset time is reported with an explicit
//!   estimate marker and the rate-card date.
//!
//! Rate card values are USD per million tokens and are recorded with their
//! source date so the UI can say "as of 2025-09".

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The API endpoint name used by `Settings::default_endpoints()`.
pub const ENDPOINT_NAME: &str = "openai";
pub const CURRENCY: &str = "USD";
/// R3: a spent-per-day ceiling that is visible but not punishing.
pub const DEFAULT_DAILY_CAP_USD: f64 = 3.0;
/// R5: the UI shows which price list the number came from.
pub const RATE_CARD_DATE: &str = "2025-09";
pub const RATE_CARD_SOURCE: &str = "OpenAI API pricing (USD / 1M tokens)";

/// Per-million-token USD prices for one model.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelRate {
    pub input_usd_per_mtok: f64,
    pub cached_input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
}

impl ModelRate {
    /// Cached input is a *share* of input, so the miss share is the remainder —
    /// double counting it was what made the CNY watchdog overestimate ~3x.
    pub fn estimate_usd(&self, input_tokens: i64, cached_tokens: i64, output_tokens: i64) -> f64 {
        let input = input_tokens.max(0) as f64;
        let cached = (cached_tokens.max(0) as f64).min(input);
        let miss = input - cached;
        let output = output_tokens.max(0) as f64;
        (miss * self.input_usd_per_mtok
            + cached * self.cached_input_usd_per_mtok
            + output * self.output_usd_per_mtok)
            / 1_000_000.0
    }
}

/// Longest-prefix-wins table, ordered most specific first.
const RATE_TABLE: &[(&str, ModelRate)] = &[
    (
        "gpt-5-mini",
        ModelRate { input_usd_per_mtok: 0.25, cached_input_usd_per_mtok: 0.025, output_usd_per_mtok: 2.0 },
    ),
    (
        "gpt-5-nano",
        ModelRate { input_usd_per_mtok: 0.05, cached_input_usd_per_mtok: 0.005, output_usd_per_mtok: 0.4 },
    ),
    (
        "gpt-5",
        ModelRate { input_usd_per_mtok: 1.25, cached_input_usd_per_mtok: 0.125, output_usd_per_mtok: 10.0 },
    ),
    (
        "gpt-4.1-mini",
        ModelRate { input_usd_per_mtok: 0.4, cached_input_usd_per_mtok: 0.1, output_usd_per_mtok: 1.6 },
    ),
    (
        "gpt-4.1",
        ModelRate { input_usd_per_mtok: 2.0, cached_input_usd_per_mtok: 0.5, output_usd_per_mtok: 8.0 },
    ),
    (
        "gpt-4o-mini",
        ModelRate { input_usd_per_mtok: 0.15, cached_input_usd_per_mtok: 0.075, output_usd_per_mtok: 0.6 },
    ),
    (
        "gpt-4o",
        ModelRate { input_usd_per_mtok: 2.5, cached_input_usd_per_mtok: 1.25, output_usd_per_mtok: 10.0 },
    ),
    (
        "o4-mini",
        ModelRate { input_usd_per_mtok: 1.1, cached_input_usd_per_mtok: 0.275, output_usd_per_mtok: 4.4 },
    ),
    (
        "o3",
        ModelRate { input_usd_per_mtok: 2.0, cached_input_usd_per_mtok: 0.5, output_usd_per_mtok: 8.0 },
    ),
];

/// R2: an unknown model must never look free. Fall back to the most expensive
/// published rate in the table so the cap stays conservative.
pub const CONSERVATIVE_RATE: ModelRate = ModelRate {
    input_usd_per_mtok: 2.5,
    cached_input_usd_per_mtok: 1.25,
    output_usd_per_mtok: 10.0,
};

/// R2: model → rate, by longest case-insensitive prefix match.
pub fn rate_for_model(model: &str) -> ModelRate {
    let lower = model.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return CONSERVATIVE_RATE;
    }
    // Longest prefix wins; the table is ordered that way, but do not rely on it.
    let mut best: Option<(&str, ModelRate)> = None;
    for (prefix, rate) in RATE_TABLE {
        if lower.starts_with(prefix) && best.map(|(b, _)| prefix.len() > b.len()).unwrap_or(true) {
            best = Some((prefix, *rate));
        }
    }
    best.map(|(_, rate)| rate).unwrap_or(CONSERVATIVE_RATE)
}

/// R2: cost of one call in USD.
pub fn estimate_cost_usd(model: &str, input_tokens: i64, cached_tokens: i64, output_tokens: i64) -> f64 {
    rate_for_model(model).estimate_usd(input_tokens, cached_tokens, output_tokens)
}

/// R3/R4: what the user can configure for the API route.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UsdDailyCap {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_cap_usd")]
    pub daily_cap_usd: f64,
}

fn default_enabled() -> bool {
    true
}

fn default_cap_usd() -> f64 {
    DEFAULT_DAILY_CAP_USD
}

impl Default for UsdDailyCap {
    fn default() -> Self {
        Self { enabled: true, daily_cap_usd: DEFAULT_DAILY_CAP_USD }
    }
}

impl UsdDailyCap {
    /// A disabled or non-positive cap means "record, never block".
    pub fn is_enforcing(&self) -> bool {
        self.enabled && self.daily_cap_usd > 0.0
    }
}

/// Whether this endpoint is billed in USD by the OpenAI API.
///
/// `openai` is the preset; `openai-*` are user variants of the same API route
/// (e.g. a backup). Anything else (`chatgpt`, `deepseek`, `openrouter`, a local
/// OpenAI-compatible server) is **not** accounted here.
pub fn is_usd_endpoint(endpoint_name: &str) -> bool {
    let lower = endpoint_name.trim().to_ascii_lowercase();
    lower == ENDPOINT_NAME || lower.starts_with("openai-")
}

fn day_start_ms(now_ms: i64) -> i64 {
    crate::agent::quota_cap::daily_spend::day_start_ms(now_ms).unwrap_or(now_ms)
}

fn next_reset_ms(now_ms: i64) -> Option<i64> {
    crate::agent::quota_cap::daily_spend::next_reset_ms(now_ms)
}

// ------------------------------------------------------------------- ledger

#[derive(Debug, Clone, Default)]
struct Entry {
    day_start_ms: i64,
    estimated_usd: f64,
    /// M279: pre-request holds that have not settled yet.
    reserved_usd: f64,
    reservations: HashMap<u64, (f64, i64)>,
}

/// In-memory per-endpoint, per-local-day USD spend. Same shape as the CNY
/// ledger: usage is recorded as events arrive, so the cap can block the *next*
/// request without a database round trip.
#[derive(Debug, Default)]
pub struct UsdSpendLedger {
    entries: Mutex<HashMap<String, Entry>>,
    next_reservation: std::sync::atomic::AtomicU64,
}

impl UsdSpendLedger {
    /// M279 / R1: replace today's totals with values rebuilt from
    /// `model_usage_events`, mirroring the CNY ledger's startup restore.
    pub fn restore_estimates(&self, totals: HashMap<String, f64>, now_ms: i64) {
        let day = day_start_ms(now_ms);
        let mut guard = self.entries.lock().expect("usd spend ledger poisoned");
        guard.retain(|_, entry| entry.day_start_ms == day);
        for entry in guard.values_mut() {
            entry.estimated_usd = 0.0;
        }
        for (endpoint, cost) in totals {
            guard
                .entry(endpoint)
                .or_insert_with(|| Entry { day_start_ms: day, ..Default::default() })
                .estimated_usd = cost;
        }
    }

    pub fn record_estimated(&self, endpoint: &str, cost_usd: f64, now_ms: i64) {
        if !(cost_usd > 0.0) || !is_usd_endpoint(endpoint) {
            return;
        }
        let day = day_start_ms(now_ms);
        let mut guard = self.entries.lock().expect("usd spend ledger poisoned");
        let entry = guard.entry(endpoint.to_string()).or_insert(Entry { day_start_ms: day, ..Default::default() });
        if entry.day_start_ms < day {
            *entry = Entry { day_start_ms: day, ..Default::default() };
        }
        entry.estimated_usd += cost_usd;
    }

    fn prune_reservations(entry: &mut Entry, now_ms: i64) {
        let expired = entry
            .reservations
            .iter()
            .filter(|(_, (_, at))| now_ms.saturating_sub(*at) >= crate::agent::quota_cap::daily_spend::RESERVATION_TTL_MS)
            .map(|(token, _)| *token)
            .collect::<Vec<_>>();
        for token in expired {
            if let Some((amount, _)) = entry.reservations.remove(&token) {
                entry.reserved_usd = (entry.reserved_usd - amount).max(0.0);
            }
        }
    }

    /// M279 / R2: hold this request's estimated cost before it is sent. The
    /// check and the hold share one lock, so concurrent requests cannot both
    /// spend the same remaining headroom.
    pub fn try_reserve(
        &self,
        endpoint: &str,
        amount_usd: f64,
        cap_usd: f64,
        now_ms: i64,
    ) -> Option<u64> {
        if !is_usd_endpoint(endpoint)
            || !amount_usd.is_finite()
            || amount_usd < 0.0
            || !cap_usd.is_finite()
        {
            return None;
        }
        let day = day_start_ms(now_ms);
        let mut guard = self.entries.lock().expect("usd spend ledger poisoned");
        let entry = guard
            .entry(endpoint.to_string())
            .or_insert(Entry { day_start_ms: day, ..Default::default() });
        if entry.day_start_ms < day {
            *entry = Entry { day_start_ms: day, ..Default::default() };
        }
        Self::prune_reservations(entry, now_ms);
        if entry.estimated_usd + entry.reserved_usd + amount_usd > cap_usd {
            return None;
        }
        let token = self
            .next_reservation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        entry.reserved_usd += amount_usd;
        entry.reservations.insert(token, (amount_usd, now_ms));
        Some(token)
    }

    pub fn settle_reservation(&self, token: u64, actual_usd: f64) {
        let mut guard = self.entries.lock().expect("usd spend ledger poisoned");
        for entry in guard.values_mut() {
            if let Some((reserved, _)) = entry.reservations.remove(&token) {
                entry.reserved_usd = (entry.reserved_usd - reserved).max(0.0);
                if actual_usd.is_finite() && actual_usd > 0.0 {
                    entry.estimated_usd += actual_usd;
                }
                return;
            }
        }
    }

    pub fn release_reservation(&self, token: u64) {
        let mut guard = self.entries.lock().expect("usd spend ledger poisoned");
        for entry in guard.values_mut() {
            if let Some((reserved, _)) = entry.reservations.remove(&token) {
                entry.reserved_usd = (entry.reserved_usd - reserved).max(0.0);
                return;
            }
        }
    }

    pub fn reserved_usd(&self, endpoint: &str, now_ms: i64) -> f64 {
        if !is_usd_endpoint(endpoint) {
            return 0.0;
        }
        let day = day_start_ms(now_ms);
        let guard = self.entries.lock().expect("usd spend ledger poisoned");
        guard
            .get(endpoint)
            .filter(|entry| entry.day_start_ms == day)
            .map(|entry| entry.reserved_usd)
            .unwrap_or(0.0)
    }

    /// M279: today's settled USD spend for one endpoint (used by the restart
    /// rebuild test and by status surfaces).
    pub fn spent_usd(&self, endpoint: &str, now_ms: i64) -> f64 {
        if !is_usd_endpoint(endpoint) {
            return 0.0;
        }
        let day = day_start_ms(now_ms);
        let guard = self.entries.lock().expect("usd spend ledger poisoned");
        guard
            .get(endpoint)
            .filter(|entry| entry.day_start_ms == day)
            .map(|entry| entry.estimated_usd)
            .unwrap_or(0.0)
    }

    pub fn estimated_usd(&self, endpoint: &str, now_ms: i64) -> f64 {
        if !is_usd_endpoint(endpoint) {
            return 0.0;
        }
        let day = day_start_ms(now_ms);
        let guard = self.entries.lock().expect("usd spend ledger poisoned");
        guard
            .get(endpoint)
            .filter(|entry| entry.day_start_ms == day)
            .map(|entry| entry.estimated_usd)
            .unwrap_or(0.0)
    }

    pub fn status(&self, endpoint: &str, cap: &UsdDailyCap, now_ms: i64) -> UsdSpendStatus {
        let spent = self.estimated_usd(endpoint, now_ms);
        UsdSpendStatus {
            endpoint: endpoint.to_string(),
            currency: CURRENCY.to_string(),
            spent_usd: round4(spent),
            cap_usd: round4(cap.daily_cap_usd),
            remaining_usd: round4(cap.daily_cap_usd - spent),
            reset_at_ms: next_reset_ms(now_ms),
            cap_reached: cap.is_enforcing() && spent >= cap.daily_cap_usd,
            enforcing: cap.is_enforcing(),
            estimated: true,
            rate_card_date: RATE_CARD_DATE.to_string(),
        }
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

static LEDGER: OnceLock<UsdSpendLedger> = OnceLock::new();

pub fn ledger() -> &'static UsdSpendLedger {
    LEDGER.get_or_init(UsdSpendLedger::default)
}

/// R5: what the usage surface shows for the USD route.
#[derive(Debug, Clone, Serialize)]
pub struct UsdSpendStatus {
    pub endpoint: String,
    pub currency: String,
    pub spent_usd: f64,
    pub cap_usd: f64,
    pub remaining_usd: f64,
    pub reset_at_ms: Option<i64>,
    pub cap_reached: bool,
    pub enforcing: bool,
    /// The number is our token math, not the provider's invoice.
    pub estimated: bool,
    pub rate_card_date: String,
}

impl UsdSpendStatus {
    /// 「OpenAI API 今日 $0.42 / $3.00（估算）」
    pub fn headline(&self) -> String {
        format!(
            "{} 今日 ${:.2} / ${:.2}（估算）",
            display_name(),
            self.spent_usd,
            self.cap_usd
        )
    }

    /// R3: the sentence the user reads when the API budget is gone.
    pub fn cap_notice(&self) -> Option<String> {
        if !self.cap_reached {
            return None;
        }
        Some(format!("{} 今日预算已用完，明天 0 点后继续", display_name()))
    }
}

fn display_name() -> &'static str {
    "OpenAI API"
}

/// R3: `None` means "this endpoint may be used".
pub fn cap_wait_cause(
    cap: &UsdDailyCap,
    endpoint: &str,
    now_ms: i64,
) -> Option<crate::agent::usage_windows::WaitCause> {
    if !is_usd_endpoint(endpoint) || !cap.is_enforcing() {
        return None;
    }
    let status = ledger().status(endpoint, cap, now_ms);
    if !status.cap_reached {
        return None;
    }
    Some(crate::agent::usage_windows::WaitCause::new(
        display_name(),
        crate::agent::usage_windows::WaitCauseKind::DailySpendCap,
        status.reset_at_ms,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn at(iso: &str) -> i64 {
        DateTime::parse_from_rfc3339(iso).expect("valid iso").timestamp_millis()
    }

    /// R2 / R9: gpt-4o-mini priced from input + cached + output, cache hits at
    /// the cached rate.
    #[test]
    fn prices_a_call_from_its_model_and_token_counts() {
        // 1M input of which 400k cached, 200k output:
        // 600k * 0.15 + 400k * 0.075 + 200k * 0.6, all per million => 0.24 USD.
        let cost = estimate_cost_usd("gpt-4o-mini", 1_000_000, 400_000, 200_000);
        let expected = (600_000.0 * 0.15 + 400_000.0 * 0.075 + 200_000.0 * 0.6) / 1_000_000.0;
        assert!((cost - expected).abs() < 1e-12, "got {cost}, expected {expected}");
        assert_eq!(expected, 0.24);
        // Cached can never exceed input.
        assert_eq!(estimate_cost_usd("gpt-4o-mini", 1_000, 5_000, 0), 0.075 / 1_000_000.0 * 1_000.0);
    }

    /// R2: an unknown model is never free — it falls back to the conservative
    /// (highest published) rate.
    #[test]
    fn unknown_model_is_not_free() {
        let cost = estimate_cost_usd("some-future-model", 1_000_000, 0, 0);
        assert_eq!(cost, CONSERVATIVE_RATE.input_usd_per_mtok);
        assert!(cost > 0.0);
        assert_eq!(rate_for_model(""), CONSERVATIVE_RATE);
        // Model ids that only differ by vendor suffix still match by prefix.
        assert_eq!(rate_for_model("GPT-4o-mini-2024-07-18").output_usd_per_mtok, 0.6);
        // Longer prefix wins over a shorter one that also matches.
        assert_eq!(rate_for_model("gpt-5-mini-2025-08-07").output_usd_per_mtok, 2.0);
    }

    /// R3 / R4: crossing the cap blocks the next request and resets at local
    /// midnight; a day later starts from zero.
    ///
    /// Each test uses its own `openai-*` endpoint: the ledger is global and the
    /// suite runs in parallel, so a shared name would race.
    /// M279 / R1 + R2: a restored total is no longer free headroom, and holds
    /// still cannot overspend the restored value.
    #[test]
    fn m279_restored_usd_total_blocks_new_reservations() {
        let now = at("2026-10-30T02:00:00Z");
        let endpoint = "openai-m279-restore";
        let ledger = UsdSpendLedger::default();
        let totals = HashMap::from([(endpoint.to_string(), 0.24)]);
        ledger.restore_estimates(totals, now);
        assert!((ledger.spent_usd(endpoint, now) - 0.24).abs() < 1e-9);
        assert!(
            ledger.try_reserve(endpoint, 0.01, 0.24, now).is_none(),
            "0.24 already spent leaves no room for another 0.01 under a 0.24 cap"
        );
        assert!(ledger.try_reserve(endpoint, 0.1, 1.0, now).is_some());
        assert!((ledger.reserved_usd(endpoint, now) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn usd_cap_blocks_then_resets_at_local_midnight() {
        let now = at("2026-10-30T02:00:00Z"); // 10:00 +08:00
        let endpoint = "openai-m274-cap";
        let cap = UsdDailyCap { enabled: true, daily_cap_usd: 0.5 };
        // 0.24 twice = 0.48 (under the line), a third call = 0.72 (over).
        ledger().record_estimated(endpoint, 0.24, now);
        assert!(cap_wait_cause(&cap, endpoint, now).is_none());
        ledger().record_estimated(endpoint, 0.24, now);
        assert!(cap_wait_cause(&cap, endpoint, now).is_none());
        ledger().record_estimated(endpoint, 0.24, now);
        let cause = cap_wait_cause(&cap, endpoint, now).expect("cap reached");
        assert_eq!(cause.kind, crate::agent::usage_windows::WaitCauseKind::DailySpendCap);
        assert_eq!(cause.until_ms, next_reset_ms(now));
        let status = ledger().status(endpoint, &cap, now);
        assert!(status.cap_reached);
        assert!(status.estimated);
        assert_eq!(status.currency, "USD");
        assert_eq!(status.cap_notice().as_deref(), Some("OpenAI API 今日预算已用完，明天 0 点后继续"));
        assert_eq!(status.headline(), "OpenAI API 今日 $0.72 / $0.50（估算）");
        // Next local day: fresh bucket.
        let tomorrow = at("2026-10-30T16:00:01Z");
        assert!(cap_wait_cause(&cap, endpoint, tomorrow).is_none());
        assert_eq!(ledger().estimated_usd(endpoint, tomorrow), 0.0);
    }

    /// R3: a disabled cap records but never blocks.
    #[test]
    fn disabled_cap_never_blocks() {
        let now = at("2026-11-01T02:00:00Z");
        let endpoint = "openai-m274-off";
        ledger().record_estimated(endpoint, 99.0, now);
        let cap = UsdDailyCap { enabled: false, daily_cap_usd: 0.01 };
        assert!(cap_wait_cause(&cap, endpoint, now).is_none());
        let mut zero = cap;
        zero.enabled = true;
        zero.daily_cap_usd = 0.0;
        assert!(cap_wait_cause(&zero, endpoint, now).is_none());
    }

    /// Other endpoints never get USD accounting.
    #[test]
    fn non_usd_endpoints_are_not_accounted() {
        let now = at("2026-11-02T02:00:00Z");
        ledger().record_estimated("deepseek", 5.0, now);
        assert_eq!(ledger().estimated_usd("deepseek", now), 0.0);
        assert!(!is_usd_endpoint("chatgpt"));
        assert!(is_usd_endpoint("OpenAI"));
        assert!(is_usd_endpoint("openai-backup"));
    }

    /// R1 / R4: the preset exists for fresh and existing settings, never
    /// overwrites a user edit, carries the USD cap, and sits ahead of the other
    /// pay-per-token providers in the reserve order.
    #[test]
    fn settings_expose_the_preset_cap_and_reserve_order() {
        use crate::config::settings::Settings;

        let settings = Settings::default();
        let preset = settings.endpoints.get(ENDPOINT_NAME).expect("openai preset");
        assert_eq!(preset.base_url, "https://api.openai.com/v1");
        assert!(preset.key_ref.is_some());
        assert_eq!(settings.endpoint_openai_api_cap(ENDPOINT_NAME), Some(UsdDailyCap::default()));
        assert_eq!(settings.endpoint_openai_api_cap("deepseek"), None);

        // Existing settings file (no preset yet) gets it forward-added.
        let mut legacy = Settings::default();
        legacy.endpoints.remove(ENDPOINT_NAME);
        legacy.ensure_openai_api_preset();
        assert!(legacy.endpoints.contains_key(ENDPOINT_NAME));

        // A hand-edited preset is left alone (e.g. routed through a proxy).
        let mut custom = Settings::default();
        custom.endpoints.get_mut(ENDPOINT_NAME).unwrap().base_url = "https://proxy.local/v1".into();
        custom.ensure_openai_api_preset();
        assert_eq!(custom.endpoints[ENDPOINT_NAME].base_url, "https://proxy.local/v1");

        // R4: subscription first, then the Pro-funded API route, then the
        // other providers; unknown names never become candidates.
        assert_eq!(
            settings.endpoint_route_order,
            vec!["chatgpt".to_string(), "openai".to_string(), "deepseek".to_string(), "openrouter".to_string()]
        );
        let ordered = settings.ordered_endpoint_names();
        assert_eq!(ordered, vec!["openai".to_string(), "openrouter".to_string()]);
        let mut with_deepseek = settings.clone();
        with_deepseek.endpoints.insert(
            "deepseek".into(),
            with_deepseek.endpoints[ENDPOINT_NAME].clone(),
        );
        assert_eq!(
            with_deepseek.ordered_endpoint_names(),
            vec!["openai".to_string(), "deepseek".to_string(), "openrouter".to_string()]
        );
    }
}
