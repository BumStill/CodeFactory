// SPDX-License-Identifier: Apache-2.0
//! M212: per-endpoint daily spend cap in CNY.
//!
//! A pay-per-token endpoint can burn a whole prepaid balance in a day without
//! any technical failure: DeepSeek's 10-29 usage was ~43M input tokens, the
//! balance ran out, and every request then failed as `PROVIDER_FATAL` for the
//! rest of the day. This module makes "how much did this endpoint cost me
//! today" a first-class number and turns the configured cap into a routing
//! decision, using the same wait mechanism as the subscription quota cap
//! (`subscription_quota_cap`) so the turn enters `waiting_wake` instead of
//! looking like a retry that failed.
//!
//! Accounting truth, in priority order:
//!
//! 1. the provider's own balance (`GET /user/balance` snapshots) — daily spend
//!    is `first snapshot of the day + top-ups - current balance`;
//! 2. our own token accounting priced with the configured per-million table
//!    (`cost_source = configured_price`), marked "估算" in the UI.
//!
//! Everything here is local-day scoped in Asia/Shanghai, matching every other
//! usage surface in the app.

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
) -> Option<super::usage_windows::WaitCause> {
    if !config.is_enforcing() {
        return None;
    }
    let status = ledger().status(endpoint, config, now_ms);
    if !status.cap_reached {
        return None;
    }
    Some(super::usage_windows::WaitCause::new(
        display_name(endpoint),
        super::usage_windows::WaitCauseKind::DailySpendCap,
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
        assert_eq!(cause.kind, super::super::usage_windows::WaitCauseKind::DailySpendCap);
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
