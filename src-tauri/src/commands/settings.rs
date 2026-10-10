// SPDX-License-Identifier: Apache-2.0
use tauri::State;

use crate::config::settings::{self, ApiStyle, Settings};
use crate::errors::AppError;
use crate::AppState;

/// CF-QUOTA-R4: one subscription endpoint's usage, caps and reset times for the
/// settings/usage surfaces. Only routing and display fields are exposed — never
/// a credential, and never a raw provider response body.
#[derive(serde::Serialize)]
pub struct SubscriptionQuotaStatus {
    pub endpoint: String,
    pub label: String,
    /// "服务端读数" / "本地估算" / "尚无用量记录".
    pub source: &'static str,
    pub five_hour_percent: u8,
    pub weekly_percent: u8,
    pub five_hour_cap_percent: u8,
    pub weekly_cap_percent: u8,
    pub five_hour_resets_at_ms: Option<i64>,
    pub weekly_resets_at_ms: Option<i64>,
    /// True while CodeFactory is handing this endpoint's turns to the next one.
    pub over_cap: bool,
}

/// CF-QUOTA-R4. Read the current subscription metering state. Used by the
/// settings/usage surface so the user sees which endpoints are capped, at what
/// share, and when each window resets.
#[tauri::command]
pub async fn subscription_quota_status(
    state: State<'_, AppState>,
) -> Result<Vec<SubscriptionQuotaStatus>, AppError> {
    let settings = state.settings.read().await.clone();
    let ledger = crate::agent::quota_cap::shared_quota_ledger();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let budget = crate::agent::quota_cap::SubscriptionBudget::default();
    let mut statuses = Vec::new();
    for (name, endpoint) in settings.endpoints.iter() {
        if endpoint.api_style != ApiStyle::Chatgpt {
            continue;
        }
        let cap = settings.quota_cap_for(name);
        // The transport records usage under the route's base URL; a settings- or
        // test-driven writer may use the endpoint name. Try both keys.
        let snapshot = ledger
            .snapshot(name, budget, now_ms)
            .or_else(|| ledger.snapshot(&endpoint.base_url, budget, now_ms));
        let over_cap = ledger
            .evaluate(name, cap, budget, now_ms)
            .or_else(|| ledger.evaluate(&endpoint.base_url, cap, budget, now_ms))
            .is_some();
        statuses.push(SubscriptionQuotaStatus {
            endpoint: name.clone(),
            label: crate::agent::failover::endpoint_label(name),
            source: snapshot
                .map(|quota| quota.source.label())
                .unwrap_or("尚无用量记录"),
            five_hour_percent: snapshot
                .map(|quota| quota.five_hour.used_percent(now_ms))
                .unwrap_or(0),
            weekly_percent: snapshot
                .map(|quota| quota.weekly.used_percent(now_ms))
                .unwrap_or(0),
            five_hour_cap_percent: cap.five_hour_percent,
            weekly_cap_percent: cap.weekly_percent,
            five_hour_resets_at_ms: snapshot.and_then(|quota| quota.five_hour.resets_at_ms),
            weekly_resets_at_ms: snapshot.and_then(|quota| quota.weekly.resets_at_ms),
            over_cap,
        });
    }
    statuses.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
    Ok(statuses)
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, AppError> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
pub async fn save_settings(
    mut new_settings: Settings,
    state: State<'_, AppState>,
) -> Result<Settings, AppError> {
    let mut current = state.settings.write().await;
    crate::codex_auth::reconcile_chatgpt_settings(&current, &mut new_settings);
    if new_settings.delivery_ceiling != current.delivery_ceiling
        || !current.delivery_ceiling_explicit
    {
        new_settings.delivery_ceiling_explicit = true;
    }
    settings::persist_git_remote_inline_tokens(&mut new_settings)?;
    settings::save(&new_settings)?;
    *current = new_settings.clone();
    Ok(new_settings)
}

#[tauri::command]
pub async fn save_api_key(key_ref: String, value: String) -> Result<(), AppError> {
    crate::credential_broker::CredentialBroker::global()
        .put(&key_ref, &value)
        .await
        .map(|_| ())
        .map_err(|error| AppError::Other(error.message))
}

#[tauri::command]
pub async fn delete_api_key(key_ref: String) -> Result<(), AppError> {
    crate::credential_broker::CredentialBroker::global()
        .delete(&key_ref)
        .await
        .map_err(|error| AppError::Other(error.message))
}
