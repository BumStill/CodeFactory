// SPDX-License-Identifier: Apache-2.0
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

    let key_ref = endpoint
        .key_ref
        .clone()
        .unwrap_or_else(|| format!("codefactory.endpoint.{endpoint_name}"));
    let api_key = crate::secrets::get_key(&key_ref)?.unwrap_or_default();

    let client = crate::openrouter::OpenRouterClient::new(&endpoint.base_url, api_key);
    client.list_models().await
}
