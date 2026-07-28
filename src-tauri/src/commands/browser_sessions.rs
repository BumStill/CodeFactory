// SPDX-License-Identifier: Apache-2.0
use crate::errors::AppError;
use crate::tools::browser_session::{self, BrowserSessionView};

#[tauri::command]
pub async fn list_browser_sessions() -> Result<Vec<BrowserSessionView>, AppError> {
    Ok(browser_session::list_managed_sessions())
}

#[tauri::command]
pub async fn close_browser_session(session_id: String) -> Result<(), AppError> {
    browser_session::close_managed_session(&session_id)
        .await
        .map_err(AppError::Other)
}
