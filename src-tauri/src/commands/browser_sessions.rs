// SPDX-License-Identifier: Apache-2.0
use crate::errors::AppError;
use crate::tools::browser_session::{self, BrowserSessionView};
use tauri::{Manager, WebviewUrl};

#[derive(Debug, serde::Deserialize)]
pub struct EmbeddedBrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

fn embedded_label(session_id: &str) -> String {
    let safe: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("embedded-browser-{safe}")
}

fn parse_https_url(url: &str) -> Result<url::Url, AppError> {
    let parsed = url::Url::parse(url).map_err(|e| AppError::Other(format!("Invalid browser URL: {e}")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        scheme => Err(AppError::Other(format!(
            "Embedded browser only supports HTTP(S) URLs, got {scheme}"
        ))),
    }
}

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

#[tauri::command]
pub async fn embedded_browser_mount(
    app: tauri::AppHandle,
    session_id: String,
    url: String,
    bounds: EmbeddedBrowserBounds,
) -> Result<(), AppError> {
    let url = parse_https_url(&url)?;
    let window = app
        .get_window("main")
        .ok_or_else(|| AppError::Other("Main window is not available".into()))?;
    let label = embedded_label(&session_id);
    if let Some(webview) = window.webviews().into_iter().find(|w| w.label() == label) {
        webview
            .set_position(tauri::LogicalPosition::new(bounds.x, bounds.y))
            .map_err(|e| AppError::Other(format!("Failed to move embedded browser: {e}")))?;
        webview
            .set_size(tauri::LogicalSize::new(bounds.width, bounds.height))
            .map_err(|e| AppError::Other(format!("Failed to resize embedded browser: {e}")))?;
        return Ok(());
    }

    let builder = tauri::webview::WebviewBuilder::new(label, WebviewUrl::External(url))
        .on_navigation(|url| matches!(url.scheme(), "http" | "https"));
    window
        .add_child(
            builder,
            tauri::LogicalPosition::new(bounds.x, bounds.y),
            tauri::LogicalSize::new(bounds.width, bounds.height),
        )
        .map_err(|e| AppError::Other(format!("Failed to mount embedded browser: {e}")))?;
    Ok(())
}

#[tauri::command]
pub async fn embedded_browser_resize(
    app: tauri::AppHandle,
    session_id: String,
    bounds: EmbeddedBrowserBounds,
) -> Result<(), AppError> {
    let window = app
        .get_window("main")
        .ok_or_else(|| AppError::Other("Main window is not available".into()))?;
    let label = embedded_label(&session_id);
    let webview = window
        .webviews()
        .into_iter()
        .find(|w| w.label() == label)
        .ok_or_else(|| AppError::Other("Embedded browser is not mounted".into()))?;
    webview
        .set_position(tauri::LogicalPosition::new(bounds.x, bounds.y))
        .map_err(|e| AppError::Other(format!("Failed to move embedded browser: {e}")))?;
    webview
        .set_size(tauri::LogicalSize::new(bounds.width, bounds.height))
        .map_err(|e| AppError::Other(format!("Failed to resize embedded browser: {e}")))?;
    Ok(())
}

#[tauri::command]
pub async fn embedded_browser_unmount(app: tauri::AppHandle, session_id: String) -> Result<(), AppError> {
    if let Some(window) = app.get_window("main") {
        let label = embedded_label(&session_id);
        if let Some(webview) = window.webviews().into_iter().find(|w| w.label() == label) {
            webview
                .close()
                .map_err(|e| AppError::Other(format!("Failed to close embedded browser: {e}")))?;
        }
    }
    Ok(())
}
