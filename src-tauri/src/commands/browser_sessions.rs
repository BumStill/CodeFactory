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

/// Live state of the extension bridge, for the Settings panel.
///
/// Starting the bridge is idempotent, so opening Settings repeatedly shows the
/// same port and token instead of invalidating an extension that is already
/// paired. The port and token are still reported because manual pairing remains
/// supported, but the normal path needs neither: `extension_dir` is a folder
/// CodeFactory has already stamped the pairing into.
#[tauri::command]
pub async fn browser_bridge_pairing() -> Result<serde_json::Value, AppError> {
    let bridge = std::sync::Arc::clone(&crate::tools::browser_session::BRIDGE);
    let pairing = bridge.start().await.map_err(|error| AppError::Other(error.to_string()))?;
    Ok(serde_json::json!({
        "port": pairing.port,
        "token": pairing.token,
        "connected": bridge.connected().await,
        "extension_dir": crate::browser::extension_package::existing_dir()
            .map(|dir| dir.display().to_string()),
    }))
}

/// Write the extension out, ready to load, with pairing already filled in.
///
/// This is the whole of setup that CodeFactory can do on the user's behalf. What
/// is left — loading an unpacked extension — is a decision Chrome deliberately
/// reserves for a human, and no API or registry key can stand in for it without
/// administrator rights, so the goal here is that it be the *only* remaining
/// step: no repository checkout, no build command, and nothing to copy.
#[tauri::command]
pub async fn browser_extension_prepare() -> Result<serde_json::Value, AppError> {
    let bridge = std::sync::Arc::clone(&crate::tools::browser_session::BRIDGE);
    let pairing = bridge
        .start()
        .await
        .map_err(|error| AppError::Other(error.to_string()))?;
    let dir = crate::browser::extension_package::prepare(pairing.port, &pairing.token)
        .map_err(AppError::Other)?;

    Ok(serde_json::json!({
        "dir": dir.display().to_string(),
        "port": pairing.port,
        "token": pairing.token,
        "connected": bridge.connected().await,
        "chrome_available": crate::browser::chromium::system_chrome().is_some(),
    }))
}

/// Show the prepared extension folder in the OS file manager.
///
/// Chrome's "load unpacked" dialog wants a folder; opening it here means the user
/// picks it out of a window that is already in the right place instead of typing
/// a path under AppData from memory.
#[tauri::command]
pub async fn browser_extension_reveal() -> Result<(), AppError> {
    let dir = crate::browser::extension_package::existing_dir().ok_or_else(|| {
        AppError::Other("The extension folder has not been prepared yet.".into())
    })?;
    open_path(&dir)
}

/// Open `chrome://extensions` in the user's own Chrome.
///
/// It has to be launched as an argument to the browser: `chrome://` is not a
/// scheme the OS shell will route, so "open this URL" would silently do nothing.
#[tauri::command]
pub async fn browser_open_extensions_page() -> Result<(), AppError> {
    let chrome = crate::browser::chromium::system_chrome().ok_or_else(|| {
        AppError::Other(
            "No Chrome, Chromium or Edge was found to open. Open chrome://extensions yourself, \
             then load the folder shown above."
                .into(),
        )
    })?;
    use crate::util::no_window::NoWindow;
    std::process::Command::new(&chrome)
        .no_window()
        .arg("chrome://extensions")
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            AppError::Other(format!(
                "Could not start {}: {error}",
                chrome.display()
            ))
        })
}

/// Open a path in the platform's file manager.
fn open_path(path: &std::path::Path) -> Result<(), AppError> {
    use crate::util::no_window::NoWindow;
    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = ("explorer.exe", vec![path.as_os_str()]);
    #[cfg(target_os = "macos")]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = ("open", vec![path.as_os_str()]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let (program, args): (&str, Vec<&std::ffi::OsStr>) = ("xdg-open", vec![path.as_os_str()]);

    let status = std::process::Command::new(program)
        .no_window()
        .args(args)
        .spawn();
    match status {
        Ok(_) => Ok(()),
        // Windows Explorer is the one that reports a non-zero exit on success, so
        // only a failure to *start* the helper is worth reporting.
        Err(error) => Err(AppError::Other(format!(
            "Could not open {}: {error}",
            path.display()
        ))),
    }
}

/// Whether the app-managed Chromium is already downloaded.
///
/// Looks in every candidate root, not just the preferred one: an install that
/// landed in a fallback folder (because the preferred one was not writable) is
/// still an install, and reporting it as missing would offer the user a 150 MB
/// download they do not need.
#[tauri::command]
pub async fn browser_chromium_status() -> Result<serde_json::Value, AppError> {
    use crate::browser::install;
    let Some(platform) = install::Platform::current() else {
        return Ok(serde_json::json!({"supported": false}));
    };
    let candidates = install::install_root_candidates();
    if candidates.is_empty() {
        return Ok(serde_json::json!({"supported": true, "installed": false}));
    }
    Ok(match install::detect_in_any(&candidates, platform).1 {
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
