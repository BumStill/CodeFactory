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

/// Pairing details the user copies into the browser extension.
///
/// Starting the bridge is idempotent, so opening Settings repeatedly shows the
/// same port and token instead of invalidating an extension that is already
/// paired.
#[tauri::command]
pub async fn browser_bridge_pairing() -> Result<serde_json::Value, AppError> {
    let bridge = std::sync::Arc::clone(&crate::tools::browser_session::BRIDGE);
    let pairing = bridge.start().await.map_err(|error| AppError::Other(error.to_string()))?;
    Ok(serde_json::json!({
        "port": pairing.port,
        "token": pairing.token,
        "connected": bridge.connected().await,
    }))
}

/// Whether the app-managed Chromium is already downloaded.
#[tauri::command]
pub async fn browser_chromium_status() -> Result<serde_json::Value, AppError> {
    use crate::browser::install;
    let Some(platform) = install::Platform::current() else {
        return Ok(serde_json::json!({"supported": false}));
    };
    let Some(root) = install::install_root() else {
        return Ok(serde_json::json!({"supported": true, "installed": false}));
    };
    Ok(match install::detect(&root, platform) {
        install::InstallState::Ready(found) => serde_json::json!({
            "supported": true, "installed": true, "version": found.version,
        }),
        install::InstallState::Missing { previous } => serde_json::json!({
            "supported": true, "installed": false, "needs_repair": previous.is_some(),
        }),
    })
}

/// Download the app-managed Chromium, emitting progress to the frontend.
#[tauri::command]
pub async fn browser_download_chromium(app: tauri::AppHandle) -> Result<serde_json::Value, AppError> {
    use tauri::Emitter;
    let install = crate::browser::download::ensure_installed(&move |progress| {
        // Progress is emitted rather than returned so a 150 MB download can show
        // a bar instead of a frozen dialog.
        let _ = app.emit("browser:chromium-progress", &progress);
    })
    .await
    .map_err(|error| AppError::Other(error.to_string()))?;
    Ok(serde_json::json!({"version": install.version}))
}
