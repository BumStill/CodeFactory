// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::config::settings::McpServerConfig;
use crate::util::no_window::NoWindow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub server_id: String,
}

pub struct McpClient {
    pub config: McpServerConfig,
    child: Child,
    stdin: ChildStdin,
    stdout_lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    pub tools: Vec<McpTool>,
}

impl McpClient {
    pub async fn spawn(config: McpServerConfig) -> crate::errors::Result<Self> {
        let mut cmd = Command::new(&config.command).no_window();
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            crate::errors::AppError::Other(format!("Failed to spawn MCP server '{}': {e}", config.command))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| crate::errors::AppError::Other("No stdin on MCP child".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| crate::errors::AppError::Other("No stdout on MCP child".into()))?;
        let stdout_lines = BufReader::new(stdout).lines();

        let mut client = McpClient {
            config,
            child,
            stdin,
            stdout_lines,
            next_id: 1,
            tools: vec![],
        };

        // Initialize
        client.initialize().await?;
        // List tools
        client.tools = client.fetch_tools().await.unwrap_or_default();

        Ok(client)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn send_request(&mut self, method: &str, params: Value) -> crate::errors::Result<Value> {
        let id = self.next_id();
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });
        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| crate::errors::AppError::Other(format!("MCP write error: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| crate::errors::AppError::Other(format!("MCP flush error: {e}")))?;

        // Read lines until we get a response matching our id
        loop {
            let raw_line = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.stdout_lines.next_line(),
            )
            .await
            .map_err(|_| crate::errors::AppError::Other("MCP response timeout".into()))?
            .map_err(|e| crate::errors::AppError::Other(format!("MCP read error: {e}")))?
            .ok_or_else(|| crate::errors::AppError::Other("MCP server closed stdout".into()))?;

            let parsed: Value = match serde_json::from_str(&raw_line) {
                Ok(v) => v,
                Err(_) => continue, // skip non-JSON lines
            };

            // Check if this is our response (matching id)
            if parsed.get("id").and_then(|v| v.as_u64()) == Some(id) {
                if let Some(err) = parsed.get("error") {
                    return Err(crate::errors::AppError::Other(format!("MCP error: {err}")));
                }
                return Ok(parsed.get("result").cloned().unwrap_or(Value::Null));
            }
            // Otherwise it's a notification or different id — skip it
        }
    }

    async fn send_notification(&mut self, method: &str, params: Value) -> crate::errors::Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| crate::errors::AppError::Other(format!("MCP write error: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| crate::errors::AppError::Other(format!("MCP flush error: {e}")))?;
        Ok(())
    }

    async fn initialize(&mut self) -> crate::errors::Result<()> {
        self.send_request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "codefactory",
                    "version": "0.1.0"
                }
            }),
        )
        .await?;
        self.send_notification("notifications/initialized", serde_json::json!({}))
            .await?;
        Ok(())
    }

    async fn fetch_tools(&mut self) -> crate::errors::Result<Vec<McpTool>> {
        let result = self
            .send_request("tools/list", serde_json::json!({}))
            .await?;

        let tools_arr = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let server_id = self.config.id.clone();
        let tools = tools_arr
            .into_iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                let description = t
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input_schema = t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));
                Some(McpTool {
                    name,
                    description,
                    input_schema,
                    server_id: server_id.clone(),
                })
            })
            .collect();

        Ok(tools)
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> crate::errors::Result<String> {
        let result = self
            .send_request(
                "tools/call",
                serde_json::json!({
                    "name": name,
                    "arguments": arguments,
                }),
            )
            .await?;

        // Extract text from content array
        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
            let text: Vec<String> = content
                .iter()
                .filter_map(|c| {
                    if c.get("type").and_then(|v| v.as_str()) == Some("text") {
                        c.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())
                    } else {
                        Some(serde_json::to_string(c).unwrap_or_default())
                    }
                })
                .collect();
            return Ok(text.join("\n"));
        }

        Ok(serde_json::to_string(&result).unwrap_or_default())
    }

    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,   // still running
            Ok(Some(_)) => false, // exited
            Err(_) => false,
        }
    }
}
