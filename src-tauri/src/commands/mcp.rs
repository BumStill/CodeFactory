// SPDX-License-Identifier: Apache-2.0
use std::sync::Arc;
use tauri::State;

use crate::config::settings::{save, McpServerConfig};
use crate::mcp::{McpManager, McpTool};
use crate::AppState;

type McpState<'a> = State<'a, Arc<McpManager>>;

#[tauri::command]
pub async fn list_mcp_servers(state: State<'_, AppState>) -> Result<Vec<McpServerConfig>, String> {
    let settings = state.settings.read().await;
    Ok(settings.mcp_servers.clone())
}

#[tauri::command]
pub async fn add_mcp_server(
    config: McpServerConfig,
    state: State<'_, AppState>,
    mcp: McpState<'_>,
) -> Result<(), String> {
    let enabled = config.enabled;
    {
        let mut settings = state.settings.write().await;
        settings.mcp_servers.retain(|s| s.id != config.id);
        settings.mcp_servers.push(config.clone());
        save(&settings).map_err(|e| e.to_string())?;
    }
    if enabled {
        mcp.start_server(config).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn update_mcp_server(
    id: String,
    config: McpServerConfig,
    state: State<'_, AppState>,
    mcp: McpState<'_>,
) -> Result<(), String> {
    mcp.stop_server(&id).await.map_err(|e| e.to_string())?;

    let enabled = config.enabled;
    {
        let mut settings = state.settings.write().await;
        settings.mcp_servers.retain(|s| s.id != id);
        settings.mcp_servers.push(config.clone());
        save(&settings).map_err(|e| e.to_string())?;
    }
    if enabled {
        mcp.start_server(config).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_mcp_server(
    id: String,
    state: State<'_, AppState>,
    mcp: McpState<'_>,
) -> Result<(), String> {
    mcp.stop_server(&id).await.map_err(|e| e.to_string())?;
    let mut settings = state.settings.write().await;
    settings.mcp_servers.retain(|s| s.id != id);
    save(&settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn enable_mcp_server(
    id: String,
    state: State<'_, AppState>,
    mcp: McpState<'_>,
) -> Result<Vec<McpTool>, String> {
    let config = {
        let mut settings = state.settings.write().await;
        let server = settings
            .mcp_servers
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("MCP server '{id}' not found"))?;
        server.enabled = true;
        let cfg = server.clone();
        save(&settings).map_err(|e| e.to_string())?;
        cfg
    };
    mcp.start_server(config).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn disable_mcp_server(
    id: String,
    state: State<'_, AppState>,
    mcp: McpState<'_>,
) -> Result<(), String> {
    {
        let mut settings = state.settings.write().await;
        if let Some(server) = settings.mcp_servers.iter_mut().find(|s| s.id == id) {
            server.enabled = false;
        }
        save(&settings).map_err(|e| e.to_string())?;
    }
    mcp.stop_server(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_mcp_tools(mcp: McpState<'_>) -> Result<Vec<McpTool>, String> {
    Ok(mcp.list_all_tools().await)
}

#[tauri::command]
pub async fn test_mcp_tool(
    server_id: String,
    tool_name: String,
    args: serde_json::Value,
    mcp: McpState<'_>,
) -> Result<String, String> {
    mcp.call_tool(&server_id, &tool_name, args)
        .await
        .map_err(|e| e.to_string())
}
