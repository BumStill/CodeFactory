// SPDX-License-Identifier: Apache-2.0
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use tauri::State;

use crate::errors::AppError;
use crate::openrouter::types::ModelInfo;
use crate::AppState;

static MODELS_CACHE: Lazy<Mutex<HashMap<String, (i64, Vec<ModelInfo>)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn cache_get(endpoint_name: &str) -> Option<Vec<ModelInfo>> {
    const TTL_MS: i64 = 24 * 60 * 60 * 1000;
    let guard = MODELS_CACHE.lock().ok()?;
    let (stored_at, models) = guard.get(endpoint_name)?;
    (now_ms() - *stored_at <= TTL_MS).then(|| models.clone())
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

    let custom: Vec<ModelInfo> = endpoint
        .custom_models
        .iter()
        .map(|model| ModelInfo {
            id: model.id.clone(),
            name: model.name.clone().unwrap_or_else(|| model.id.clone()),
            context_length: model.context_length.unwrap_or(0),
            pricing: None,
            supported_parameters: None,
            is_custom: true,
        })
        .collect();

    if endpoint.api_style == crate::config::settings::ApiStyle::Chatgpt {
        return Ok(custom);
    }

    if let Some(cached) = cache_get(&endpoint_name) {
        return Ok(merge_models(custom, cached));
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
            Ok(merge_models(custom, remote))
        }
        Err(error) => {
            tracing::warn!("list_models: remote fetch failed for endpoint '{endpoint_name}': {error}");
            Ok(custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, is_custom: bool) -> ModelInfo {
        ModelInfo { id: id.into(), name: id.into(), context_length: 0, pricing: None, supported_parameters: None, is_custom }
    }

    #[test]
    fn merge_keeps_custom_precedence_and_sorts_remote_tail() {
        let merged = merge_models(
            vec![model("z", true), model("a", true)],
            vec![model("b", false), model("a", false), model("m", false)],
        );
        let ids: Vec<_> = merged.into_iter().map(|model| (model.id, model.is_custom)).collect();
        assert_eq!(ids, vec![
            ("z".to_string(), true),
            ("a".to_string(), true),
            ("b".to_string(), false),
            ("m".to_string(), false),
        ]);
    }

    #[test]
    fn cache_is_scoped_by_endpoint() {
        cache_put("openai-a", vec![model("gpt-a", false)]);
        cache_put("openai-b", vec![model("gpt-b", false)]);
        assert_eq!(cache_get("openai-a").unwrap()[0].id, "gpt-a");
        assert_eq!(cache_get("openai-b").unwrap()[0].id, "gpt-b");
    }
}
