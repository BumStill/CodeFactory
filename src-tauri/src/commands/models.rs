// SPDX-License-Identifier: Apache-2.0
use std::collections::HashSet;

use tauri::State;

use crate::errors::AppError;
use crate::openrouter::types::ModelInfo;
use crate::AppState;

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

    // ── 2. Try to fetch remote /models — failures are non-fatal ──────────────
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
            // Custom entries take precedence — skip any remote id already covered.
            for m in remote {
                if !custom_ids.contains(&m.id) {
                    merged.push(ModelInfo {
                        is_custom: false,
                        ..m
                    });
                }
            }
        }
        Err(err) => {
            // Endpoint may not implement /models (LMStudio, Ollama, private gateways).
            // Log and return whatever custom models the user configured.
            tracing::warn!(
                "list_models: remote fetch failed for endpoint '{endpoint_name}': {err}"
            );
        }
    }

    Ok(merged)
}
