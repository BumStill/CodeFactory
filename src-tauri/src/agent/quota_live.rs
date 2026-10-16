// SPDX-License-Identifier: Apache-2.0
//! CF-QUOTA live-query parser; transport wiring follows the task specification.
use super::quota_cap::{EndpointQuota, UsageSource, WindowUsage};

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Credits {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<f64>,
}

pub fn parse_credits(value: &serde_json::Value) -> Option<Credits> {
    let credits = value.get("credits")?.as_object()?;
    let balance = credits.get("balance").and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|n: &f64| n.is_finite() && *n >= 0.0);
    Some(Credits {
        has_credits: credits.get("has_credits").and_then(|v| v.as_bool()).unwrap_or(false),
        unlimited: credits.get("unlimited").and_then(|v| v.as_bool()).unwrap_or(false),
        balance,
    })
}

static CREDITS: std::sync::OnceLock<std::sync::Mutex<Option<Credits>>> = std::sync::OnceLock::new();

pub fn credits_snapshot() -> Option<Credits> {
    *CREDITS.get_or_init(|| std::sync::Mutex::new(None)).lock().expect("credits mutex poisoned")
}

pub const USAGE_URLS: [&str; 2] = [
    "https://chatgpt.com/backend-api/wham/usage",
    "https://chatgpt.com/backend-api/codex/usage",
];

pub fn parse_usage(value: &serde_json::Value, now_ms: i64) -> Option<EndpointQuota> {
    let limits = value.get("rate_limit")?;
    let window = |name: &str| -> Option<WindowUsage> {
        let v = limits.get(name)?;
        let percent = v.get("used_percent")?.as_f64()?;
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) { return None; }
        let reset = v.get("reset_at").and_then(|v| v.as_i64())
            .and_then(|secs| secs.checked_mul(1000))
            .or_else(|| v.get("reset_after_seconds")?.as_i64()?.checked_mul(1000)?.checked_add(now_ms));
        Some(WindowUsage { used_ratio: percent / 100.0, resets_at_ms: reset })
    };
    Some(EndpointQuota {
        source: UsageSource::LiveQuery,
        five_hour: window("primary_window")?,
        weekly: window("secondary_window")?,
        observed_at_ms: now_ms,
    })
}

/// No configurable URLs, redirects, response bodies or credential-bearing errors.
pub async fn refresh(settings: &crate::config::settings::Settings) -> Option<&'static str> {
    refresh_if_needed(settings, true).await
}

static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn refresh_if_needed(settings: &crate::config::settings::Settings, force: bool) -> Option<&'static str> {
    use crate::config::settings::ApiStyle;
    if !settings.endpoints.values().any(|e| e.api_style == ApiStyle::Chatgpt) { return None; }
    let _guard = REFRESH_LOCK.lock().await;
    let now = chrono::Utc::now().timestamp_millis();
    let ledger = super::quota_cap::shared_quota_ledger();
    if !force && settings.endpoints.iter().filter(|(_, e)| e.api_style == ApiStyle::Chatgpt).all(|(name, _)| {
        ledger.snapshot(name, super::quota_cap::SubscriptionBudget::default(), now)
            .map(|q| q.source == UsageSource::LiveQuery && now - q.observed_at_ms < 120_000 && !q.five_hour.has_reset(now) && !q.weekly.has_reset(now)).unwrap_or(false)
    }) { return None; }
    let (token, account) = match crate::codex_auth::valid_access_token().await {
        Ok(auth) => auth, Err(_) => return Some("未登录或登录已过期"),
    };
    let client = match reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10)).build() {
        Ok(client) => client, Err(_) => return Some("网络连接失败"),
    };
    let mut error = "用量接口已变更";
    for url in USAGE_URLS {
        let mut request = client.get(url).bearer_auth(&token);
        if let Some(id) = &account { request = request.header("chatgpt-account-id", id); }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                if let Ok(value) = response.json::<serde_json::Value>().await {
                    if let Some(quota) = parse_usage(&value, chrono::Utc::now().timestamp_millis()) {
                        *CREDITS.get_or_init(|| std::sync::Mutex::new(None)).lock().expect("credits mutex poisoned") = parse_credits(&value);
                        let ledger = super::quota_cap::shared_quota_ledger();
                        for (name, endpoint) in &settings.endpoints {
                            if endpoint.api_style == ApiStyle::Chatgpt {
                                ledger.record_server_usage(name, quota);
                                ledger.record_server_usage(&endpoint.base_url, quota);
                            }
                        }
                        return None;
                    }
                }
                error = "用量接口已变更";
            }
            Ok(response) if response.status().as_u16() == 401 => error = "登录已过期",
            Ok(_) => error = "用量接口不可用",
            Err(_) => error = "网络连接失败",
        }
    }
    Some(error)
}

pub fn spawn_refresh(app: tauri::AppHandle) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        loop {
            let settings = app.state::<crate::AppState>().settings.read().await.clone();
            refresh_if_needed(&settings, false).await;
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credits_are_optional_typed_and_never_replace_window_caps() {
        let credits = parse_credits(&serde_json::json!({"credits": {
            "has_credits": true, "unlimited": false, "balance": "12.50", "private": "ignored"
        }})).unwrap();
        assert_eq!(credits.balance, Some(12.5));
        assert!(credits.has_credits);
        assert!(!credits.unlimited);
        assert!(parse_credits(&serde_json::json!({})).is_none());
        assert_eq!(parse_credits(&serde_json::json!({"credits":{"balance":"secret"}})).unwrap().balance, None);
    }

    #[test]
    fn synthetic_usage_preserves_percent_and_reset_units() {
        let value = serde_json::json!({"rate_limit": {
            "primary_window": {"used_percent": 0.5, "reset_at": 1800000000},
            "secondary_window": {"used_percent": 87, "reset_after_seconds": 600}},
            "credits": {"balance": "12.5"}, "unknown": true});
        let q = parse_usage(&value, 1000).unwrap();
        assert_eq!(q.source, UsageSource::LiveQuery);
        assert_eq!(q.five_hour.used_ratio, 0.005);
        assert_eq!(q.weekly.used_ratio, 0.87);
        assert_eq!(q.five_hour.resets_at_ms, Some(1800000000000));
        assert_eq!(q.weekly.resets_at_ms, Some(601000));
        assert!(parse_usage(&serde_json::json!({"unexpected": 42}), 1000).is_none());
    }
}
