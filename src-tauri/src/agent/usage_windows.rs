// SPDX-License-Identifier: Apache-2.0
//! CF-WINDOW: data-driven, per-endpoint/model request admission.
use chrono::{Datelike, TimeZone, Timelike, Utc};
use std::sync::{Mutex, OnceLock};
static SESSION_OVERRIDES: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
fn overrides() -> &'static Mutex<std::collections::HashSet<String>> {
    SESSION_OVERRIDES.get_or_init(Default::default)
}
pub fn session_ignores_windows(session: &str) -> bool {
    overrides()
        .lock()
        .expect("usage override lock")
        .contains(session)
}

/// CF-WINDOW-R6（M86 返修）：豁免不再只活在内存里。用户按下「本会话忽略时段
/// 限制」后写进 Settings，随应用重启一起回来——「一重启豁免就没了、会话被
/// 悄悄拦下」正是这条要修的问题。返回是否真的改了配置。
pub fn persist_session_override(
    settings: &mut crate::config::settings::Settings,
    session: &str,
    ignore: bool,
) -> bool {
    if ignore {
        if settings
            .session_usage_window_overrides
            .iter()
            .any(|existing| existing == session)
        {
            return false;
        }
        settings
            .session_usage_window_overrides
            .push(session.to_string());
        true
    } else {
        let before = settings.session_usage_window_overrides.len();
        settings
            .session_usage_window_overrides
            .retain(|existing| existing != session);
        settings.session_usage_window_overrides.len() != before
    }
}

/// 进程启动时把持久化的会话豁免读回内存缓存——这就是"重启后仍然有效"的
/// 那一步。
pub fn apply_persisted_overrides(sessions: &[String]) {
    let mut cache = overrides().lock().expect("usage override lock");
    for session in sessions {
        cache.insert(session.clone());
    }
}

/// 启动路径调用：从磁盘上的设置读回豁免。
pub fn hydrate_session_overrides() {
    apply_persisted_overrides(&crate::config::settings::load().session_usage_window_overrides);
}

#[doc(hidden)]
pub fn clear_session_overrides_for_test() {
    overrides().lock().expect("usage override lock").clear();
}

#[tauri::command]
pub async fn set_session_usage_window_override(
    session_id: String,
    ignore: bool,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Err("session_id must be nonempty".into());
    }
    let mut current = state.settings.write().await;
    let mut candidate = current.clone();
    if persist_session_override(&mut candidate, &session_id, ignore) {
        crate::config::settings::save(&candidate).map_err(|error| error.to_string())?;
        *current = candidate;
    }
    cache_session_override(session_id, ignore)
}

pub(crate) fn cache_session_override(session_id: String, ignore: bool) -> Result<(), String> {
    let mut cache = overrides().lock().map_err(|e| e.to_string())?;
    if ignore {
        cache.insert(session_id);
    } else {
        cache.remove(&session_id);
    }
    Ok(())
}

/// CF-WINDOW-R6：让界面能显示"本会话已忽略时段限制"，并提供撤销入口。
#[tauri::command]
pub fn get_session_usage_window_overrides() -> Vec<String> {
    let mut sessions = overrides()
        .lock()
        .expect("usage override lock")
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    sessions.sort();
    sessions
}
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Calendar {
    #[serde(default)]
    pub years: Vec<i32>,
    #[serde(default)]
    pub holidays: Vec<String>,
    #[serde(default)]
    pub workdays: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Period {
    pub start: String,
    pub end: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageWindow {
    pub mode: String,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub weekdays: Vec<u32>,
    #[serde(default)]
    pub periods: Vec<Period>,
    #[serde(default)]
    pub exclude_holidays: bool,
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub calendar: Calendar,
}
#[derive(Clone, Debug, Serialize)]
pub struct WindowStatus {
    pub available: bool,
    pub next_transition_ms: Option<i64>,
}
pub fn model_key(endpoint: &str, model: &str) -> String {
    serde_json::to_string(&(endpoint, model)).expect("string tuple")
}

pub fn effective_rule<'a>(
    settings: &'a crate::config::settings::Settings,
    endpoint: &str,
    model: &str,
) -> Option<&'a UsageWindow> {
    settings
        .model_usage_windows
        .get(&model_key(endpoint, model))
        .or_else(|| settings.endpoint_usage_windows.get(endpoint))
}

/// Only identical legacy rules are promoted; mixed groups retain their exact rules.
pub fn migrate_endpoint_defaults(settings: &mut crate::config::settings::Settings) -> bool {
    let mut groups: std::collections::HashMap<String, Vec<(String, UsageWindow)>> =
        Default::default();
    for (key, rule) in &settings.model_usage_windows {
        if let Ok((endpoint, _)) = serde_json::from_str::<(String, String)>(key) {
            groups
                .entry(endpoint)
                .or_default()
                .push((key.clone(), rule.clone()));
        }
    }
    let mut changed = false;
    for (endpoint, rules) in groups {
        if settings.endpoint_usage_windows.contains_key(&endpoint)
            || !rules.iter().all(|(_, rule)| rule == &rules[0].1)
        {
            continue;
        }
        if let Some(config) = settings.endpoints.get(&endpoint) {
            for model in config
                .custom_models
                .iter()
                .map(|m| m.id.as_str())
                .chain(config.active_model.as_deref())
            {
                let key = model_key(&endpoint, model);
                if !settings.model_usage_windows.contains_key(&key) {
                    settings.model_usage_windows.insert(
                        key,
                        serde_json::from_value(serde_json::json!({"mode":"anytime"})).unwrap(),
                    );
                }
            }
        }
        settings
            .endpoint_usage_windows
            .insert(endpoint, rules[0].1.clone());
        for (key, _) in rules {
            settings.model_usage_windows.remove(&key);
        }
        changed = true;
    }
    changed
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
    CLOCK_OVERRIDE_MS.store(
        pinned.unwrap_or(WALL_CLOCK),
        std::sync::atomic::Ordering::SeqCst,
    );
}

/// 固定时钟的测试之间串行进行——时钟是进程级的，不能让两个测试互相插队。
#[doc(hidden)]
pub fn clock_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// CF-WINDOW-R5：等待提示必须写明"在等谁、几点开始、用户能做什么"，
/// 且明确它不是技术失败。抽成纯函数，好让验收直接断言这句话。
pub fn wait_notice(label: &str, next_available_ms: Option<i64>) -> String {
    let clock = next_available_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| {
            at.with_timezone(&chrono_tz::Asia::Shanghai)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "允许时段恢复".into());
    format!("等 {label} 闲时（{clock} 开始）后继续；可在设置里放开时段，或换一个模型")
}

/// CF-WINDOW-R5 + CF-QUOTA + M86：一个候选为什么不可用。
///
/// 等待只有在"时间会让它变好"时才成立——时段会到点、额度会重置；缺凭据不会
/// 自己好，它需要用户动作，所以它是一条**原因**，不是一条等待理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitCauseKind {
    UsageWindow,
    QuotaCap,
    MissingCredential,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitCause {
    pub label: String,
    pub kind: WaitCauseKind,
    pub until_ms: Option<i64>,
}

impl WaitCause {
    pub fn new(label: impl Into<String>, kind: WaitCauseKind, until_ms: Option<i64>) -> Self {
        Self {
            label: label.into(),
            kind,
            until_ms,
        }
    }
    fn clock(&self) -> Option<String> {
        self.until_ms
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|at| {
                at.with_timezone(&chrono_tz::Asia::Shanghai)
                    .format("%m-%d %H:%M")
                    .to_string()
            })
    }
    /// 一句话，写明原因和（若有）恢复时间。
    pub fn sentence(&self) -> String {
        match (self.kind, self.clock()) {
            (WaitCauseKind::UsageWindow, Some(clock)) => {
                format!("{} 高峰时段，{clock} 可用", self.label)
            }
            (WaitCauseKind::UsageWindow, None) => format!("{} 不在允许使用时段", self.label),
            (WaitCauseKind::QuotaCap, Some(clock)) => {
                format!("{} 已达订阅用量上限，{clock} 重置", self.label)
            }
            (WaitCauseKind::QuotaCap, None) => format!("{} 已达订阅用量上限", self.label),
            (WaitCauseKind::MissingCredential, _) => format!("{} 尚未配置凭据", self.label),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailabilityDecision {
    /// 有没有"等一等就会好"的原因（时段到点 / 额度重置）。
    pub waitable: bool,
    /// 最早可以再试的时间（时段边界或额度重置里较早的那个）。
    pub earliest_ms: Option<i64>,
    pub causes: Vec<WaitCause>,
    /// 会话里如实显示的那句话：列出全部原因与最早可用时间。
    pub notice: String,
}

/// CF-WINDOW-R5 / M86：所有候选都不可用时，判定该等还是该报错。
///
/// M86 的真实场景：GPT 已达订阅上限（14:05 重置）、DeepSeek 处于高峰
/// （12:00 可用）、OpenRouter 未配置凭据。三者都不可用，但**时间会让前两个
/// 变好**，所以任务是"可理解的等待"，不是失败。只有"等也不会变好"的原因
/// （例如全部候选都没凭据）才交给原来的错误路径，不在这里空转。
pub fn availability_decision(causes: Vec<WaitCause>) -> AvailabilityDecision {
    let earliest_ms = causes.iter().filter_map(|cause| cause.until_ms).min();
    let notice = wait_notice_causes(&causes, earliest_ms);
    AvailabilityDecision {
        waitable: earliest_ms.is_some(),
        earliest_ms,
        causes,
        notice,
    }
}

/// 等待说明：全部原因 + 最早可用时间 + 用户能做什么，并明确它不是技术失败。
pub fn wait_notice_causes(causes: &[WaitCause], earliest_ms: Option<i64>) -> String {
    let clock = earliest_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| {
            at.with_timezone(&chrono_tz::Asia::Shanghai)
                .format("%m-%d %H:%M")
                .to_string()
        });
    let reasons = causes
        .iter()
        .map(WaitCause::sentence)
        .collect::<Vec<_>>()
        .join("；");
    match clock {
        Some(clock) => format!("等待模型可用：{reasons}；最早 {clock} 自动继续。等待不计入技术恢复次数；可在设置里放开时段，或换一个模型"),
        None => format!("{reasons}；等待不计入技术恢复次数；可在设置里放开时段、换一个模型，或先配好凭据"),
    }
}
pub fn presets() -> std::collections::HashMap<String, UsageWindow> {
    serde_json::from_str(include_str!("../../../src/lib/usageWindowPresets.json"))
        .expect("validated preset data")
}
impl UsageWindow {
    pub fn validate(&self) -> Result<(), String> {
        if !["anytime", "official_off_peak", "custom"].contains(&self.mode.as_str()) {
            return Err("unknown usage window mode".into());
        }
        self.timezone
            .as_deref()
            .unwrap_or("UTC")
            .parse::<chrono_tz::Tz>()
            .map_err(|_| "invalid IANA timezone")?;
        if self.weekdays.iter().any(|d| *d > 6) {
            return Err("weekday must be 0..6".into());
        }
        for period in &self.periods {
            for clock in [&period.start, &period.end] {
                chrono::NaiveTime::parse_from_str(clock, "%H:%M")
                    .map_err(|_| "invalid HH:MM period")?;
            }
        }
        Ok(())
    }
    fn available(&self, now_ms: i64) -> bool {
        if self.mode == "anytime" {
            return true;
        }
        let Ok(tz) = self
            .timezone
            .as_deref()
            .unwrap_or("UTC")
            .parse::<chrono_tz::Tz>()
        else {
            return false;
        };
        let Some(now) = Utc.timestamp_millis_opt(now_ms).single() else {
            return false;
        };
        let local = now.with_timezone(&tz);
        let date = local.date_naive();
        // CF-WINDOW-R3: a year outside the bundled notice has no holiday data,
        // so holidays/workdays are unknown and the conservative weekday-is-peak
        // rule stands. Settings states this explicitly.
        let covered = self.calendar.years.contains(&date.year());
        let workday = |date: chrono::NaiveDate| {
            let key = date.to_string();
            if covered && self.calendar.workdays.contains(&key) {
                return true;
            }
            if self.exclude_holidays && covered && self.calendar.holidays.contains(&key) {
                return false;
            }
            self.weekdays
                .contains(&date.weekday().num_days_from_sunday())
        };
        let minute = local.hour() * 60 + local.minute();
        let matches = self.periods.iter().any(|p| {
            let parse = |s: &str| {
                chrono::NaiveTime::parse_from_str(s, "%H:%M")
                    .ok()
                    .map(|t| t.hour() * 60 + t.minute())
            };
            let (Some(start), Some(end)) = (parse(&p.start), parse(&p.end)) else {
                return false;
            };
            if start < end {
                workday(date) && minute >= start && minute < end
            } else {
                (workday(date) && minute >= start)
                    || (date.pred_opt().is_some_and(workday) && minute < end)
            }
        });
        if self.invert {
            !matches
        } else {
            matches
        }
    }
    pub fn evaluate(&self, now_ms: i64) -> WindowStatus {
        let available = self.available(now_ms);
        if self.mode == "anytime" {
            return WindowStatus {
                available,
                next_transition_ms: None,
            };
        }
        let first = now_ms.div_euclid(60000) * 60000 + 60000;
        let next = (0..=8 * 24 * 60)
            .map(|i| first + i * 60000)
            .find(|ms| self.available(*ms) != available);
        WindowStatus {
            available,
            next_transition_ms: next,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    // CF-WINDOW-R3: the bundled notice covers 2026 only. A year with no notice
    // must fall back conservatively (weekday = peak) instead of guessing.
    #[test]
    fn endpoint_default_migration_preserves_existing_admission() {
        let mut settings = crate::config::settings::Settings::default();
        let rule = presets().remove("deepseek_off_peak").unwrap();
        settings
            .model_usage_windows
            .insert(model_key("synthetic", "pro"), rule.clone());
        settings
            .model_usage_windows
            .insert(model_key("synthetic", "flash"), rule);
        let before = settings.clone();
        assert!(migrate_endpoint_defaults(&mut settings));
        assert!(settings.model_usage_windows.is_empty());
        assert!(!migrate_endpoint_defaults(&mut settings));
        for model in ["pro", "flash"] {
            for hour in 0..24 {
                let at = chrono::DateTime::parse_from_rfc3339(&format!(
                    "2026-10-13T{hour:02}:00:00+08:00"
                ))
                .unwrap()
                .timestamp_millis();
                assert_eq!(
                    effective_rule(&before, "synthetic", model)
                        .unwrap()
                        .evaluate(at)
                        .available,
                    effective_rule(&settings, "synthetic", model)
                        .unwrap()
                        .evaluate(at)
                        .available
                );
            }
        }
        assert!(effective_rule(&settings, "synthetic", "new-model").is_some());
        let anytime: UsageWindow =
            serde_json::from_value(serde_json::json!({"mode":"anytime"})).unwrap();
        settings
            .model_usage_windows
            .insert(model_key("synthetic", "pro"), anytime);
        assert_eq!(
            effective_rule(&settings, "synthetic", "pro").unwrap().mode,
            "anytime"
        );
        let mut mixed = before;
        mixed
            .model_usage_windows
            .get_mut(&model_key("synthetic", "pro"))
            .unwrap()
            .mode = "anytime".into();
        assert!(!migrate_endpoint_defaults(&mut mixed));
        assert!(mixed.endpoint_usage_windows.is_empty());
    }

    #[test]
    fn years_without_a_bundled_notice_stay_conservative() {
        let rule = presets().remove("deepseek_off_peak").unwrap();
        assert_eq!(rule.calendar.years, vec![2026]);
        let at = |stamp: &str| {
            chrono::DateTime::parse_from_rfc3339(stamp)
                .unwrap()
                .timestamp_millis()
        };
        // 2026-10-01 is a holiday → off-peak all day.
        assert!(rule.evaluate(at("2026-10-01T10:00:00+08:00")).available);
        // 2026-10-10 is an adjusted working Saturday → peak.
        assert!(!rule.evaluate(at("2026-10-10T10:00:00+08:00")).available);
        // 2027 has no published notice → weekday stays peak, weekend stays off-peak.
        assert!(!rule.evaluate(at("2027-10-01T10:00:00+08:00")).available);
        assert!(rule.evaluate(at("2027-10-03T10:00:00+08:00")).available);
        assert!(
            !rule.evaluate(at("2027-10-01T10:00:00+08:00")).available,
            "no holiday may be assumed for an uncovered year"
        );
    }

    #[test]
    fn model_keys_and_official_boundaries_match_frontend() {
        assert_ne!(model_key("a", "pro"), model_key("a", "flash"));
        assert_ne!(model_key("a", "pro"), model_key("b", "pro"));
        let rule = presets().remove("deepseek_off_peak").unwrap();
        rule.validate().unwrap();
        for (clock, expected) in [
            ("08:59", true),
            ("09:00", false),
            ("11:59", false),
            ("12:00", true),
            ("13:59", true),
            ("14:00", false),
            ("17:59", false),
            ("18:00", true),
        ] {
            let now = chrono::DateTime::parse_from_rfc3339(&format!("2026-10-13T{clock}:00+08:00"))
                .unwrap()
                .timestamp_millis();
            assert_eq!(rule.evaluate(now).available, expected, "{clock}");
        }
    }

    /// CF-WINDOW-R6（M86 返修）：豁免写进设置，所以**重启之后仍然有效**；
    /// 用户撤销之后，重启也不会把它带回来。这里用一个真实的 Settings JSON
    /// 往返来代表"重启"：内存缓存清空 → 从设置读回。
    #[test]
    fn a_session_override_survives_a_restart_and_can_be_revoked() {
        let _serial = clock_test_lock();
        clear_session_overrides_for_test();
        let mut settings = crate::config::settings::Settings::default();
        assert!(
            persist_session_override(&mut settings, "session-synthetic-a", true),
            "第一次授予应当改动配置"
        );
        assert!(
            !persist_session_override(&mut settings, "session-synthetic-a", true),
            "重复授予不必再写一次"
        );

        // 重启：新的 Settings 实例来自磁盘上的 JSON，进程内缓存是空的。
        let persisted = serde_json::to_string(&settings).expect("settings serialize");
        let mut restarted: crate::config::settings::Settings =
            serde_json::from_str(&persisted).expect("settings deserialize");
        clear_session_overrides_for_test();
        assert!(
            !session_ignores_windows("session-synthetic-a"),
            "重启瞬间缓存是空的"
        );
        apply_persisted_overrides(&restarted.session_usage_window_overrides);
        assert!(
            session_ignores_windows("session-synthetic-a"),
            "重启后豁免必须还在"
        );
        assert!(
            !session_ignores_windows("session-synthetic-b"),
            "豁免仍然只属于那一个会话"
        );

        // 撤销：同样要挺过一次重启。
        assert!(persist_session_override(
            &mut restarted,
            "session-synthetic-a",
            false
        ));
        let revoked = serde_json::to_string(&restarted).expect("settings serialize");
        let after_revoke: crate::config::settings::Settings =
            serde_json::from_str(&revoked).expect("settings deserialize");
        clear_session_overrides_for_test();
        apply_persisted_overrides(&after_revoke.session_usage_window_overrides);
        assert!(
            !session_ignores_windows("session-synthetic-a"),
            "撤销之后不得复活"
        );
        clear_session_overrides_for_test();
    }
}
