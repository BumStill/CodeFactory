// SPDX-License-Identifier: Apache-2.0
use tauri::State;

use crate::config::settings::{self, Settings};
use crate::errors::AppError;
use crate::AppState;

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
