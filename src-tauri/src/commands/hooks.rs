// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::config::settings;
use crate::AppState;
use crate::util::no_window::NoWindow;

// ── Hook data structures ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookAction {
    LogToFile { path: String },
    RunCommand { command: String, cwd: Option<String> },
    EmitEvent { event_name: String },
    AutoGitCommit { message_template: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    pub id: String,
    pub name: String,
    pub event: String,
    pub action: HookAction,
    pub enabled: bool,
    pub filter: Option<String>,
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_hooks(state: State<'_, AppState>) -> Result<Vec<HookConfig>, String> {
    let settings = state.settings.read().await;
    Ok(settings.hooks.clone())
}

#[tauri::command]
pub async fn add_hook(config: HookConfig, state: State<'_, AppState>) -> Result<(), String> {
    let mut settings = state.settings.write().await;
    settings.hooks.push(config);
    settings::save(&settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_hook(
    id: String,
    config: HookConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut settings = state.settings.write().await;
    if let Some(existing) = settings.hooks.iter_mut().find(|h| h.id == id) {
        *existing = config;
        settings::save(&settings).map_err(|e| e.to_string())
    } else {
        Err(format!("Hook '{id}' not found"))
    }
}

#[tauri::command]
pub async fn delete_hook(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut settings = state.settings.write().await;
    let len_before = settings.hooks.len();
    settings.hooks.retain(|h| h.id != id);
    if settings.hooks.len() == len_before {
        return Err(format!("Hook '{id}' not found"));
    }
    settings::save(&settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_hook(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let hook = {
        let settings = state.settings.read().await;
        settings
            .hooks
            .iter()
            .find(|h| h.id == id)
            .cloned()
            .ok_or_else(|| format!("Hook '{id}' not found"))?
    };

    let mock_event = serde_json::json!({
        "event": hook.event,
        "test": true,
        "hook_id": id,
    });

    match &hook.action {
        HookAction::LogToFile { path } => {
            let line = format!("{}\n", serde_json::to_string(&mock_event).unwrap_or_default());
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(line.as_bytes())
                })
                .map_err(|e| e.to_string())?;
            Ok(format!("Appended test entry to {path}"))
        }
        HookAction::RunCommand { command, cwd } => {
            let output = std::process::Command::new("powershell").no_window()
                .args(["-NonInteractive", "-Command", command])
                .current_dir(cwd.as_deref().unwrap_or("."))
                .output()
                .map_err(|e| e.to_string())?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Ok(format!(
                "exit={}\nstdout: {stdout}\nstderr: {stderr}",
                output.status.code().unwrap_or(-1)
            ))
        }
        HookAction::EmitEvent { event_name } => {
            Ok(format!("Would emit Tauri event '{event_name}' with mock payload"))
        }
        HookAction::AutoGitCommit { message_template } => {
            let msg = message_template
                .replace("{task_title}", "test-task")
                .replace("{req_id}", "TEST-0");
            Ok(format!(
                "AutoGitCommit test: would run `git add -A && git commit -m \"{msg}\"`"
            ))
        }
    }
}
