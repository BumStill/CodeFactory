// SPDX-License-Identifier: Apache-2.0
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use tauri::State;

use crate::errors::AppError;
use crate::openrouter::types::ModelInfo;
use crate::AppState;

const MODEL_CATALOG_TTL_MS: i64 = 24 * 60 * 60 * 1000;
static MODELS_CACHE: Lazy<Mutex<HashMap<String, (i64, Vec<ModelInfo>)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn cached_models(endpoint_name: &str, allow_stale: bool) -> Option<Vec<ModelInfo>> {
    let guard = MODELS_CACHE.lock().ok()?;
    let (stored_at, models) = guard.get(endpoint_name)?;
    (allow_stale || now_ms().saturating_sub(*stored_at) <= MODEL_CATALOG_TTL_MS)
        .then(|| models.clone())
}

fn cache_put(endpoint_name: &str, models: Vec<ModelInfo>) {
    if let Ok(mut guard) = MODELS_CACHE.lock() {
        guard.insert(endpoint_name.to_string(), (now_ms(), models));
    }
}

fn merge_models(mut custom: Vec<ModelInfo>, remote: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let custom_ids: HashSet<String> = custom.iter().map(|model| model.id.clone()).collect();
    let mut remote: Vec<ModelInfo> = remote
        .into_iter()
        .filter(|model| !custom_ids.contains(&model.id))
        .map(|model| ModelInfo { is_custom: false, ..model })
        .collect();
    remote.sort_by(|left, right| left.id.cmp(&right.id));
    custom.extend(remote);
    custom
}
pub(crate) fn preferred_openai_model(models: &[ModelInfo], family: &str) -> Option<(String, bool)> {
    let family = family.trim().to_ascii_lowercase();
    let preferred = if family.is_empty() { "terra" } else { family.as_str() };
    let mut candidates = models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>();
    candidates.sort_by(|a, b| natural_model_cmp(b, a));
    if let Some(model) = candidates.iter().find(|id| id.to_ascii_lowercase().contains(preferred)) {
        return Some(((*model).to_string(), false));
    }
    for fallback in ["terra", "sol", "gpt-6", "gpt-5.6", "gpt-5"] {
        if let Some(model) = candidates.iter().find(|id| id.to_ascii_lowercase().contains(fallback)) {
            return Some(((*model).to_string(), true));
        }
    }
    candidates.first().map(|id| ((*id).to_string(), true))
}

fn natural_model_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let numeric = |value: &str| value.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse::<u64>().ok()).collect::<Vec<_>>();
    numeric(left).cmp(&numeric(right)).then_with(|| left.cmp(right))
}





#[tauri::command]
pub async fn list_models(
    endpoint_name: String,
    state: State<'_, AppState>,
) -> Result<Vec<ModelInfo>, AppError> {
    let settings = state.settings.read().await;
    let endpoint = settings
        .endpoints
        .get(&endpoint_name)
        .ok_or_else(|| AppError::Other(format!("Unknown endpoint: {endpoint_name}")))?;

    // ── 1. Build user-defined entries (always present, never fails) ──────────
    let mut merged: Vec<ModelInfo> = endpoint
        .custom_models
        .iter()
        .map(|m| ModelInfo {
            id: m.id.clone(),
            name: m.name.clone().unwrap_or_else(|| m.id.clone()),
            context_length: m.context_length.unwrap_or(0),
            pricing: None,
            supported_parameters: None,
            is_custom: true,
        })
        .collect();

    let custom_ids: HashSet<String> = merged.iter().map(|m| m.id.clone()).collect();

    // ChatGPT (OAuth) endpoints: /models needs the OAuth token, not an API key,
    // and custom_models above already list the available codex models — so skip
    // the remote fetch entirely (with the empty key it just 401s on every poll).
    if endpoint.api_style == crate::config::settings::ApiStyle::Chatgpt {
        return Ok(merged);
    }

    // ── 2. Try to fetch remote /models — failures are non-fatal, but
    // cache is authoritative when available (R1: not silently empty).
    if let Some(cached) = cached_models(&endpoint_name, false) {
        return Ok(merge_models(merged, cached));
    }

    let key_ref = endpoint
        .key_ref
        .clone()
        .unwrap_or_else(|| format!("codefactory.endpoint.{endpoint_name}"));
    let api_key = crate::credential_broker::CredentialBroker::global()
        .get(&key_ref)
        .await
        .map_err(|error| AppError::Other(error.message))?
        .unwrap_or_default();

    let client = crate::openrouter::OpenRouterClient::new(&endpoint.base_url, api_key);
    match client.list_models().await {
        Ok(remote) => {
            cache_put(&endpoint_name, remote.clone());
            Ok(merge_models(merged, remote))
        }
        Err(error) => {
            if let Some(cached) = cached_models(&endpoint_name, true) {
                tracing::warn!("list_models: remote refresh failed for endpoint '{endpoint_name}', serving last successful catalog: {error}");
                Ok(merge_models(merged, cached))
            } else {
                Err(AppError::Other(format!(
                    "MODEL_CATALOG_UNAVAILABLE: endpoint '{endpoint_name}' model catalog refresh failed and no cached list is available: {error}"
                )))
            }
        }
    }
}
#[cfg(test)]
mod m286_tests {
    use super::*;
    fn model(id: &str, is_custom: bool) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            name: id.into(),
            context_length: 0,
            pricing: None,
            supported_parameters: None,
            is_custom,
        }
    }




    #[test]
    fn m286_balanced_prefers_latest_terra_and_falls_back_explicitly() {
        let catalog = vec![model("gpt-6.0-terra", false), model("gpt-6.1-terra", false), model("gpt-6.2-sol", false)];
        assert_eq!(preferred_openai_model(&catalog, "terra"), Some(("gpt-6.1-terra".into(), false)));
        let no_terra = vec![model("gpt-6.1-sol", false), model("gpt-6.2-sol", false)];
        assert_eq!(preferred_openai_model(&no_terra, "terra"), Some(("gpt-6.2-sol".into(), true)));
    }

    #[test]
    fn m286_missing_current_model_repairs_to_catalog_choice() {
        let catalog = vec![model("gpt-6.1-terra", false), model("gpt-6.2-sol", false)];
        let current = "gpt-5";
        let repaired = if catalog.iter().any(|model| model.id == current) { (current.to_string(), false) } else { preferred_openai_model(&catalog, "terra").unwrap() };
        assert_eq!(repaired, ("gpt-6.1-terra".into(), false));
    }
}
