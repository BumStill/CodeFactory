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
    new_settings: Settings,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    settings::save(&new_settings)?;
    *state.settings.write().await = new_settings;
    Ok(())
}

#[tauri::command]
pub async fn get_api_key(key_ref: String) -> Result<Option<String>, AppError> {
    crate::secrets::get_key(&key_ref)
}

#[tauri::command]
pub async fn save_api_key(key_ref: String, value: String) -> Result<(), AppError> {
    crate::secrets::set_key(&key_ref, &value)
}

#[tauri::command]
pub async fn delete_api_key(key_ref: String) -> Result<(), AppError> {
    crate::secrets::delete_key(&key_ref)
}
