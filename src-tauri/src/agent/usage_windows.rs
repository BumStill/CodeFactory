// SPDX-License-Identifier: Apache-2.0
//! CF-WINDOW: data-driven, per-endpoint/model request admission.
use chrono::{Datelike, TimeZone, Timelike, Utc};
use std::sync::{Mutex, OnceLock};
static SESSION_OVERRIDES: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
pub fn session_ignores_windows(session: &str) -> bool {
    SESSION_OVERRIDES.get_or_init(Default::default).lock().expect("usage override lock").contains(session)
}
#[tauri::command]
pub fn set_session_usage_window_override(session_id: String, ignore: bool) -> Result<(), String> {
    if session_id.trim().is_empty() { return Err("session_id must be nonempty".into()); }
    let mut overrides = SESSION_OVERRIDES.get_or_init(Default::default).lock().map_err(|e| e.to_string())?;
    if ignore { overrides.insert(session_id); } else { overrides.remove(&session_id); }
    Ok(())
}
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Calendar {
    #[serde(default)] pub years: Vec<i32>,
    #[serde(default)] pub holidays: Vec<String>,
    #[serde(default)] pub workdays: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Period { pub start: String, pub end: String }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageWindow {
    pub mode: String,
    #[serde(default)] pub timezone: Option<String>,
    #[serde(default)] pub weekdays: Vec<u32>,
    #[serde(default)] pub periods: Vec<Period>,
    #[serde(default)] pub exclude_holidays: bool,
    #[serde(default)] pub invert: bool,
    #[serde(default)] pub calendar: Calendar,
}
#[derive(Clone, Debug, Serialize)]
pub struct WindowStatus { pub available: bool, pub next_transition_ms: Option<i64> }
pub fn model_key(endpoint: &str, model: &str) -> String {
    serde_json::to_string(&(endpoint, model)).expect("string tuple")
}

/// 未固定时的哨兵值：沿用系统时钟。
const WALL_CLOCK: i64 = i64::MIN;
static CLOCK_OVERRIDE_MS: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(WALL_CLOCK);

/// 时段判定读的唯一时钟。CF-WINDOW-R4/R5 的验收要把"跨过边界"做成
/// 可注入的积分测试（不真的等到 18:00），所以时钟在这里留一个接缝；
/// 生产路径不设置它，读到的就是真实时间。
pub fn now_ms() -> i64 {
    match CLOCK_OVERRIDE_MS.load(std::sync::atomic::Ordering::SeqCst) {
        WALL_CLOCK => chrono::Utc::now().timestamp_millis(),
        pinned => pinned,
    }
}

/// 测试接缝：`None` 恢复真实时钟。
#[doc(hidden)]
pub fn set_clock_override_ms(pinned: Option<i64>) {
    CLOCK_OVERRIDE_MS.store(pinned.unwrap_or(WALL_CLOCK), std::sync::atomic::Ordering::SeqCst);
}

/// 固定时钟的测试之间串行进行——时钟是进程级的，不能让两个测试互相插队。
#[doc(hidden)]
pub fn clock_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default).lock().unwrap_or_else(|poison| poison.into_inner())
}

/// CF-WINDOW-R5：等待提示必须写明"在等谁、几点开始、用户能做什么"，
/// 且明确它不是技术失败。抽成纯函数，好让验收直接断言这句话。
pub fn wait_notice(label: &str, next_available_ms: Option<i64>) -> String {
    let clock = next_available_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| at.with_timezone(&chrono_tz::Asia::Shanghai).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "允许时段恢复".into());
    format!("等 {label} 闲时（{clock} 开始）后继续；可在设置里放开时段，或换一个模型")
}
pub fn presets() -> std::collections::HashMap<String, UsageWindow> {
    serde_json::from_str(include_str!("../../../src/lib/usageWindowPresets.json")).expect("validated preset data")
}
impl UsageWindow {
    pub fn validate(&self) -> Result<(), String> {
        if !["anytime", "official_off_peak", "custom"].contains(&self.mode.as_str()) { return Err("unknown usage window mode".into()); }
        self.timezone.as_deref().unwrap_or("UTC").parse::<chrono_tz::Tz>().map_err(|_| "invalid IANA timezone")?;
        if self.weekdays.iter().any(|d| *d > 6) { return Err("weekday must be 0..6".into()); }
        for period in &self.periods {
            for clock in [&period.start, &period.end] {
                chrono::NaiveTime::parse_from_str(clock, "%H:%M").map_err(|_| "invalid HH:MM period")?;
            }
        }
        Ok(())
    }
    fn available(&self, now_ms: i64) -> bool {
        if self.mode == "anytime" { return true; }
        let Ok(tz) = self.timezone.as_deref().unwrap_or("UTC").parse::<chrono_tz::Tz>() else { return false; };
        let Some(now) = Utc.timestamp_millis_opt(now_ms).single() else { return false; };
        let local = now.with_timezone(&tz);
        let date = local.date_naive();
        // CF-WINDOW-R3: a year outside the bundled notice has no holiday data,
        // so holidays/workdays are unknown and the conservative weekday-is-peak
        // rule stands. Settings states this explicitly.
        let covered = self.calendar.years.contains(&date.year());
        let workday = |date: chrono::NaiveDate| {
            let key = date.to_string();
            if covered && self.calendar.workdays.contains(&key) { return true; }
            if self.exclude_holidays && covered && self.calendar.holidays.contains(&key) { return false; }
            self.weekdays.contains(&date.weekday().num_days_from_sunday())
        };
        let minute = local.hour() * 60 + local.minute();
        let matches = self.periods.iter().any(|p| {
            let parse = |s: &str| chrono::NaiveTime::parse_from_str(s, "%H:%M").ok().map(|t| t.hour() * 60 + t.minute());
            let (Some(start), Some(end)) = (parse(&p.start), parse(&p.end)) else { return false; };
            if start < end { workday(date) && minute >= start && minute < end }
            else { (workday(date) && minute >= start) || (date.pred_opt().is_some_and(workday) && minute < end) }
        });
        if self.invert { !matches } else { matches }
    }
    pub fn evaluate(&self, now_ms: i64) -> WindowStatus {
        let available = self.available(now_ms);
        if self.mode == "anytime" { return WindowStatus { available, next_transition_ms: None }; }
        let first = now_ms.div_euclid(60000) * 60000 + 60000;
        let next = (0..=8*24*60).map(|i| first + i * 60000).find(|ms| self.available(*ms) != available);
        WindowStatus { available, next_transition_ms: next }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    // CF-WINDOW-R3: the bundled notice covers 2026 only. A year with no notice
    // must fall back conservatively (weekday = peak) instead of guessing.
    #[test]
    fn years_without_a_bundled_notice_stay_conservative() {
        let rule = presets().remove("deepseek_off_peak").unwrap();
        assert_eq!(rule.calendar.years, vec![2026]);
        let at = |stamp: &str| chrono::DateTime::parse_from_rfc3339(stamp).unwrap().timestamp_millis();
        // 2026-10-01 is a holiday → off-peak all day.
        assert!(rule.evaluate(at("2026-10-01T10:00:00+08:00")).available);
        // 2026-10-10 is an adjusted working Saturday → peak.
        assert!(!rule.evaluate(at("2026-10-10T10:00:00+08:00")).available);
        // 2027 has no published notice → weekday stays peak, weekend stays off-peak.
        assert!(!rule.evaluate(at("2027-10-01T10:00:00+08:00")).available);
        assert!(rule.evaluate(at("2027-10-03T10:00:00+08:00")).available);
        assert!(!rule.evaluate(at("2027-10-01T10:00:00+08:00")).available, "no holiday may be assumed for an uncovered year");
    }

    #[test]
    fn model_keys_and_official_boundaries_match_frontend() {
        assert_ne!(model_key("a", "pro"), model_key("a", "flash"));
        assert_ne!(model_key("a", "pro"), model_key("b", "pro"));
        let rule = presets().remove("deepseek_off_peak").unwrap();
        rule.validate().unwrap();
        for (clock, expected) in [("08:59",true),("09:00",false),("11:59",false),("12:00",true),("13:59",true),("14:00",false),("17:59",false),("18:00",true)] {
            let now = chrono::DateTime::parse_from_rfc3339(&format!("2026-10-13T{clock}:00+08:00")).unwrap().timestamp_millis();
            assert_eq!(rule.evaluate(now).available, expected, "{clock}");
        }
    }
}
