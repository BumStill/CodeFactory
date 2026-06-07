// SPDX-License-Identifier: Apache-2.0
pub mod client;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::settings::McpServerConfig;
pub use client::{McpClient, McpTool};

pub struct McpManager {
    clients: Arc<Mutex<HashMap<String, McpClient>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn start_server(
        &self,
        config: McpServerConfig,
    ) -> crate::errors::Result<Vec<McpTool>> {
        let server_id = config.id.clone();
        let client = McpClient::spawn(config).await?;
        let tools = client.tools.clone();
        self.clients.lock().await.insert(server_id, client);
        Ok(tools)
    }

    pub async fn stop_server(&self, server_id: &str) -> crate::errors::Result<()> {
        self.clients.lock().await.remove(server_id);
        Ok(())
    }

    pub async fn call_tool(
        &self,
        server_id: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> crate::errors::Result<String> {
        let mut clients = self.clients.lock().await;
        let client = clients.get_mut(server_id).ok_or_else(|| {
            crate::errors::AppError::Other(format!(
                "MCP server '{server_id}' is not running"
            ))
        })?;
        client.call_tool(tool, args).await
    }

    pub async fn list_all_tools(&self) -> Vec<McpTool> {
        self.clients
            .lock()
            .await
            .values()
            .flat_map(|c| c.tools.clone())
            .collect()
    }

    // Scaffolding: MCP server self-healing — respawns crashed stdio servers.
    // Not wired into live tool dispatch yet; this `#[allow]` cascades to keep
    // `McpClient::is_alive` and the `child` handle it inspects alive too.
    #[allow(dead_code)]
    pub async fn restart_dead_servers(
        &self,
        configs: &[McpServerConfig],
    ) -> Vec<String> {
        let dead_ids: Vec<String> = {
            let mut clients = self.clients.lock().await;
            let mut dead = Vec::new();
            for (id, c) in clients.iter_mut() {
                if !c.is_alive() {
                    dead.push(id.clone());
                }
            }
            dead
        };

        let mut restarted = Vec::new();
        for id in &dead_ids {
            self.clients.lock().await.remove(id);
            if let Some(cfg) = configs.iter().find(|c| &c.id == id) {
                if cfg.enabled {
                    if let Ok(client) = McpClient::spawn(cfg.clone()).await {
                        self.clients.lock().await.insert(id.clone(), client);
                        restarted.push(id.clone());
                    }
                }
            }
        }
        restarted
    }

    /// Check whether a given tool name is an MCP tool (returns server_id if so).
    pub async fn find_tool_server(&self, tool_name: &str) -> Option<String> {
        let clients = self.clients.lock().await;
        for (server_id, client) in clients.iter() {
            if client.tools.iter().any(|t| t.name == tool_name) {
                return Some(server_id.clone());
            }
        }
        None
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
