// SPDX-License-Identifier: Apache-2.0
pub mod anthropic_client;
pub mod hooks;
pub mod scheduler;
pub mod subagent;
pub mod verification;

use chrono::Utc;
use futures_util::StreamExt;
use reqwest::Client;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;
use tokio::time::timeout;
use uuid::Uuid;

use crate::config::settings::{ApiStyle, PermissionPolicy, Settings};
use crate::errors::Result;
use crate::mcp::McpManager;
use crate::openrouter::types::*;
use crate::storage::Message;
use crate::tools::{self, ExecCtx};
use crate::PendingPermissionMap;

const MAX_ITERATIONS: usize = 30;

const SYSTEM_PROMPT: &str = "\
You are CodeFactory, an AI coding assistant running on Windows.\n\
You have tools to read/write files, search code, and execute PowerShell commands.\n\
Work step by step. Read files before editing them. Prefer targeted edits over full rewrites.";

pub struct AgentLoop {
    app: AppHandle,
    db: SqlitePool,
    session_id: String,
    model_id: String,
    base_url: String,
    api_key: String,
    api_style: ApiStyle,
    cwd: PathBuf,
    http: Client,
    settings: Arc<RwLock<Settings>>,
    pending_permissions: PendingPermissionMap,
    mcp_manager: Arc<McpManager>,
}

impl AgentLoop {
    pub fn new(
        app: AppHandle,
        db: SqlitePool,
        session_id: String,
        model_id: String,
        base_url: String,
        api_key: String,
        api_style: ApiStyle,
        cwd: PathBuf,
        settings: Arc<RwLock<Settings>>,
        pending_permissions: PendingPermissionMap,
        mcp_manager: Arc<McpManager>,
    ) -> Self {
        Self {
            app,
            db,
            session_id,
            model_id,
            base_url,
            api_key,
            api_style,
            cwd,
            http: Client::new(),
            settings,
            pending_permissions,
            mcp_manager,
        }
    }

    pub async fn run(&mut self, history: Vec<Message>) -> Result<()> {
        let mut tool_defs = tools::all_definitions();
        // Append MCP tools as additional tool definitions
        let mcp_tools = self.mcp_manager.list_all_tools().await;
        for mcp_tool in &mcp_tools {
            tool_defs.push(mcp_tool_to_definition(mcp_tool));
        }
        let event_name = format!("stream:{}", self.session_id);
        let base_prompt = build_system_prompt(&self.cwd);
        let system_prompt =
            crate::commands::skills::get_active_system_prompt(&base_prompt, &self.app).await;
        let api_style = self.api_style.clone();

        match api_style {
            ApiStyle::Openai => {
                self.run_openai(history, &tool_defs, &event_name, &system_prompt)
                    .await
            }
            ApiStyle::Anthropic => {
                self.run_anthropic(history, &tool_defs, &event_name, &system_prompt)
                    .await
            }
        }
    }

    async fn run_openai(
        &mut self,
        history: Vec<Message>,
        tool_defs: &[ToolDefinition],
        event_name: &str,
        system_prompt: &str,
    ) -> Result<()> {
        let mut messages = self.build_openai_messages(history, system_prompt);
        let hook_runner = {
            let settings = self.settings.read().await;
            hooks::HookRunner::from_settings(&settings, self.app.clone())
        };

        for _ in 0..MAX_ITERATIONS {
            let (text, tool_calls, usage) = self.call_openai_model(&messages, tool_defs, event_name).await?;

            // Persist assistant turn — include tool_calls so history can be
            // reconstructed faithfully when the session is resumed.
            if !text.is_empty() || !tool_calls.is_empty() {
                self.persist_message(
                    "assistant",
                    &text,
                    usage.as_ref(),
                    if tool_calls.is_empty() { None } else { Some(&tool_calls) },
                )
                .await?;
            }

            if tool_calls.is_empty() {
                // Emit Done with accumulated usage if we have it
                if let Some(u) = &usage {
                    let inp = u.prompt_tokens as i64;
                    let out = u.completion_tokens as i64;
                    self.app
                        .emit(
                            &event_name,
                            StreamEvent::Done {
                                input_tokens: u.prompt_tokens,
                                output_tokens: u.completion_tokens,
                            },
                        )
                        .ok();
                    // Persist cost entry and notify frontend to refresh stats
                    let settings = self.settings.read().await;
                    let endpoint = settings.default_endpoint.clone();
                    drop(settings);
                    if let Err(e) = crate::commands::costs::record_cost_entry(
                        &self.db,
                        &self.session_id,
                        &self.model_id,
                        &endpoint,
                        inp,
                        out,
                    ).await {
                        tracing::warn!("Failed to record cost entry: {e}");
                    } else {
                        self.app.emit("token-usage-recorded", &self.session_id).ok();
                    }
                }
                break;
            }

            let mut result_messages = Vec::new();

            for tc in &tool_calls {
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();

                // Extract bash command for finer-grained permission matching
                let bash_cmd = if tc.function.name == "bash" {
                    args.get("command")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };

                self.app
                    .emit(
                        &event_name,
                        StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args: args.clone(),
                        },
                    )
                    .ok();

                let permission_policy = {
                    let settings = self.settings.read().await;
                    settings.permissions.clone()
                };

                let decision =
                    decide_permission(&permission_policy, &tc.function.name, bash_cmd.as_deref());

                let denial_content = match decision {
                    PermissionDecision::Allow => None,
                    PermissionDecision::Ask => {
                        if self.request_permission(&event_name, tc, args.clone()).await {
                            None
                        } else {
                            Some(
                                "Tool call denied by user. Please try a different approach."
                                    .to_string(),
                            )
                        }
                    }
                    PermissionDecision::Deny(reason) => {
                        tracing::warn!("Tool '{}' denied: {reason}", tc.function.name);
                        Some(format!(
                            "Tool call denied: {reason}. Please try a different approach."
                        ))
                    }
                };

                if let Some(content) = denial_content {
                    self.app
                        .emit(
                            &event_name,
                            StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                            },
                        )
                        .ok();
                    result_messages.push(ChatMessage {
                        role: "tool".into(),
                        content: MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.function.name.clone()),
                    });
                    continue;
                }

                // Pre-tool hook: may cancel
                let pre_allowed = hook_runner
                    .fire(hooks::HookEvent::PreTool {
                        tool_name: tc.function.name.clone(),
                        args: args.clone(),
                    })
                    .await;
                if !pre_allowed {
                    let content = "Tool call cancelled by hook.".to_string();
                    self.app
                        .emit(
                            &event_name,
                            StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                            },
                        )
                        .ok();
                    result_messages.push(ChatMessage {
                        role: "tool".into(),
                        content: MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                        name: Some(tc.function.name.clone()),
                    });
                    continue;
                }

                let full_access = {
                    let settings = self.settings.read().await;
                    settings.permissions.full_access
                };
                let ctx = ExecCtx {
                    cwd: self.cwd.clone(),
                    full_access,
                };

                let tool_start = std::time::Instant::now();
                // Check if this is an MCP tool
                let mcp_server = self.mcp_manager.find_tool_server(&tc.function.name).await;
                let output = if let Some(server_id) = mcp_server {
                    match self.mcp_manager.call_tool(&server_id, &tc.function.name, args).await {
                        Ok(text) => tools::ToolOutput::ok(text),
                        Err(e) => tools::ToolOutput::err(format!("MCP error: {e}")),
                    }
                } else {
                    tools::dispatch(&tc.function.name, args, &ctx).await?
                };
                let duration_ms = tool_start.elapsed().as_millis() as u64;

                // Post-tool hook
                hook_runner
                    .fire(hooks::HookEvent::PostTool {
                        tool_name: tc.function.name.clone(),
                        result: output.content.chars().take(500).collect(),
                        duration_ms,
                    })
                    .await;

                self.app
                    .emit(
                        &event_name,
                        StreamEvent::ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: output.content.clone(),
                            is_error: output.is_error,
                        },
                    )
                    .ok();

                let now = Utc::now().timestamp_millis();
                let msg_id = Uuid::new_v4().to_string();
                let tool_content = serde_json::json!({
                    "tool_call_id": tc.id,
                    "content": output.content
                })
                .to_string();

                sqlx::query(
                    "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,?,?,?,?)",
                )
                .bind(&msg_id)
                .bind(&self.session_id)
                .bind("tool")
                .bind(&tool_content)
                .bind(now)
                .execute(&self.db)
                .await?;

                result_messages.push(ChatMessage {
                    role: "tool".into(),
                    content: MessageContent::Text(output.content),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(tc.function.name.clone()),
                });
            }

            messages.push(ChatMessage {
                role: "assistant".into(),
                content: MessageContent::Text(text),
                tool_calls: Some(tool_calls),
                tool_call_id: None,
                name: None,
            });
            messages.extend(result_messages);
        }

        Ok(())
    }

    async fn request_permission(
        &self,
        event_name: &str,
        tc: &ToolCall,
        args: serde_json::Value,
    ) -> bool {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.pending_permissions
            .lock()
            .await
            .insert(tc.id.clone(), sender);

        self.app
            .emit(
                event_name,
                StreamEvent::PermissionRequest {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    args,
                },
            )
            .ok();

        match timeout(Duration::from_secs(600), receiver).await {
            Ok(Ok(allow)) => allow,
            Ok(Err(_)) => false,
            Err(_) => {
                self.pending_permissions.lock().await.remove(&tc.id);
                false
            }
        }
    }

    async fn call_openai_model(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
        event_name: &str,
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>)> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let req = ChatRequest {
            model: self.model_id.clone(),
            messages: messages.to_vec(),
            tools: Some(tool_defs.to_vec()),
            tool_choice: Some(serde_json::json!("auto")),
            stream: true,
            temperature: 0.2,
            max_tokens: 8192,
            stream_options: Some(StreamOptions { include_usage: true }),
        };

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .header("X-Title", "CodeFactory")
            .json(&req)
            .send()
            .await?
            .error_for_status()?;

        let mut byte_stream = response.bytes_stream();
        let mut text_buf = String::new();
        let mut tc_map: HashMap<u32, (String, String, String)> = HashMap::new();
        let mut usage: Option<Usage> = None;

        while let Some(chunk) = byte_stream.next().await {
            let bytes = chunk?;
            for line in String::from_utf8_lossy(&bytes).lines() {
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    break;
                }
                let Ok(sc) = serde_json::from_str::<StreamChunk>(data) else {
                    continue;
                };
                if let Some(u) = sc.usage {
                    usage = Some(u);
                }
                for choice in sc.choices {
                    let delta = choice.delta;
                    if let Some(t) = delta.content.filter(|s| !s.is_empty()) {
                        self.app
                            .emit(&event_name, StreamEvent::TextDelta { content: t.clone() })
                            .ok();
                        text_buf.push_str(&t);
                    }
                    if let Some(tcs) = delta.tool_calls {
                        for tc in tcs {
                            let e = tc_map.entry(tc.index).or_default();
                            if let Some(id) = tc.id {
                                e.0 = id;
                            }
                            if let Some(f) = tc.function {
                                if let Some(n) = f.name {
                                    e.1 = n;
                                }
                                if let Some(a) = f.arguments {
                                    e.2.push_str(&a);
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut tool_calls: Vec<ToolCall> = tc_map
            .into_iter()
            .filter(|(_, (id, name, _))| !id.is_empty() && !name.is_empty())
            .map(|(_, (id, name, args))| ToolCall {
                id,
                r#type: "function".into(),
                function: FunctionCall {
                    name,
                    arguments: args,
                },
            })
            .collect();
        tool_calls.sort_by_key(|tc| tc.id.clone());

        Ok((text_buf, tool_calls, usage))
    }

    async fn persist_message(
        &self,
        role: &str,
        content: &str,
        usage: Option<&Usage>,
        tool_calls: Option<&[ToolCall]>,
    ) -> Result<()> {
        let msg_id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        let input_tok = usage.map(|u| u.prompt_tokens as i64);
        let output_tok = usage.map(|u| u.completion_tokens as i64);
        let tool_calls_json = tool_calls
            .filter(|tcs| !tcs.is_empty())
            .map(|tcs| serde_json::to_string(tcs).unwrap_or_default());

        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, input_tokens, output_tokens, tool_calls, created_at) \
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(&msg_id)
        .bind(&self.session_id)
        .bind(role)
        .bind(content)
        .bind(input_tok)
        .bind(output_tok)
        .bind(tool_calls_json)
        .bind(now)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    fn build_openai_messages(&self, history: Vec<Message>, system_prompt: &str) -> Vec<ChatMessage> {
        let mut msgs = vec![ChatMessage {
            role: "system".into(),
            content: MessageContent::Text(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }];

        for m in history {
            match m.role.as_str() {
                "tool" => {
                    // Content stored as: {"tool_call_id": "…", "content": "…"}
                    let (tool_call_id, content) = parse_tool_message_content(&m.content);
                    msgs.push(ChatMessage {
                        role: "tool".into(),
                        content: MessageContent::Text(content),
                        tool_calls: None,
                        tool_call_id: Some(tool_call_id),
                        name: None,
                    });
                }
                "assistant" => {
                    // Restore tool_calls if they were persisted as JSON
                    let tool_calls: Option<Vec<ToolCall>> = m
                        .tool_calls
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok());
                    msgs.push(ChatMessage {
                        role: "assistant".into(),
                        content: MessageContent::Text(m.content),
                        tool_calls,
                        tool_call_id: None,
                        name: None,
                    });
                }
                _ => {
                    msgs.push(ChatMessage {
                        role: m.role,
                        content: MessageContent::Text(m.content),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
            }
        }
        msgs
    }

    // ── Anthropic-specific helpers ────────────────────────────────────────────

    /// Build the Anthropic `messages` array from stored history.
    /// System prompt is passed separately; tool results use `user` role
    /// with `tool_result` content blocks.
    fn build_anthropic_messages(&self, history: Vec<Message>) -> Vec<serde_json::Value> {
        let mut msgs: Vec<serde_json::Value> = Vec::new();

        for m in history {
            match m.role.as_str() {
                "tool" => {
                    let (tool_call_id, content) = parse_tool_message_content(&m.content);
                    msgs.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": content,
                        }]
                    }));
                }
                "assistant" => {
                    // Reconstruct content array: text + tool_use blocks
                    let tool_calls: Vec<ToolCall> = m
                        .tool_calls
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();

                    let mut content_blocks: Vec<serde_json::Value> = Vec::new();
                    if !m.content.is_empty() {
                        content_blocks.push(serde_json::json!({
                            "type": "text",
                            "text": m.content,
                        }));
                    }
                    for tc in &tool_calls {
                        let input: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments)
                                .unwrap_or(serde_json::json!({}));
                        content_blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input,
                        }));
                    }
                    if content_blocks.is_empty() {
                        content_blocks.push(serde_json::json!({ "type": "text", "text": "" }));
                    }
                    msgs.push(serde_json::json!({
                        "role": "assistant",
                        "content": content_blocks,
                    }));
                }
                "system" => {
                    // System is passed as a top-level param, skip inline.
                }
                _ => {
                    // user messages
                    msgs.push(serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                    }));
                }
            }
        }
        msgs
    }

    async fn run_anthropic(
        &mut self,
        history: Vec<Message>,
        tool_defs: &[ToolDefinition],
        event_name: &str,
        system_prompt: &str,
    ) -> Result<()> {
        let mut messages = self.build_anthropic_messages(history);
        let hook_runner = {
            let settings = self.settings.read().await;
            hooks::HookRunner::from_settings(&settings, self.app.clone())
        };

        for _ in 0..MAX_ITERATIONS {
            let resp = anthropic_client::stream_anthropic(
                &self.http,
                &self.base_url,
                &self.api_key,
                &self.model_id,
                system_prompt,
                messages.clone(),
                tool_defs,
                &self.app,
                event_name,
            )
            .await?;

            let text = resp.text;
            let tool_calls = resp.tool_calls;

            // Persist assistant turn
            if !text.is_empty() || !tool_calls.is_empty() {
                self.persist_message(
                    "assistant",
                    &text,
                    None,
                    if tool_calls.is_empty() { None } else { Some(&tool_calls) },
                )
                .await?;
            }

            if tool_calls.is_empty() {
                let inp = resp.input_tokens;
                let out = resp.output_tokens;
                self.app
                    .emit(
                        event_name,
                        StreamEvent::Done {
                            input_tokens: inp as u32,
                            output_tokens: out as u32,
                        },
                    )
                    .ok();
                // Persist cost entry
                let settings = self.settings.read().await;
                let endpoint = settings.default_endpoint.clone();
                drop(settings);
                if let Err(e) = crate::commands::costs::record_cost_entry(
                    &self.db,
                    &self.session_id,
                    &self.model_id,
                    &endpoint,
                    inp,
                    out,
                ).await {
                    tracing::warn!("Failed to record cost entry (anthropic): {e}");
                } else {
                    self.app.emit("token-usage-recorded", &self.session_id).ok();
                }
                break;
            }

            // Build assistant message with tool_use blocks for Anthropic
            let mut assistant_content: Vec<serde_json::Value> = Vec::new();
            if !text.is_empty() {
                assistant_content.push(serde_json::json!({ "type": "text", "text": text }));
            }
            for tc in &tool_calls {
                let input: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(serde_json::json!({}));
                assistant_content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.function.name,
                    "input": input,
                }));
            }
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": assistant_content,
            }));

            // Execute tools and collect tool_result blocks
            let mut tool_result_blocks: Vec<serde_json::Value> = Vec::new();

            for tc in &tool_calls {
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();

                let bash_cmd = if tc.function.name == "bash" {
                    args.get("command")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };

                let permission_policy = {
                    let settings = self.settings.read().await;
                    settings.permissions.clone()
                };
                let decision =
                    decide_permission(&permission_policy, &tc.function.name, bash_cmd.as_deref());

                let denial_content = match decision {
                    PermissionDecision::Allow => None,
                    PermissionDecision::Ask => {
                        if self.request_permission(event_name, tc, args.clone()).await {
                            None
                        } else {
                            Some(
                                "Tool call denied by user. Please try a different approach."
                                    .to_string(),
                            )
                        }
                    }
                    PermissionDecision::Deny(reason) => {
                        tracing::warn!("Tool '{}' denied: {reason}", tc.function.name);
                        Some(format!(
                            "Tool call denied: {reason}. Please try a different approach."
                        ))
                    }
                };

                if let Some(content) = denial_content {
                    self.app
                        .emit(
                            event_name,
                            StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                            },
                        )
                        .ok();
                    tool_result_blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tc.id,
                        "content": content,
                    }));
                    continue;
                }

                // Pre-tool hook: may cancel
                let pre_allowed = hook_runner
                    .fire(hooks::HookEvent::PreTool {
                        tool_name: tc.function.name.clone(),
                        args: args.clone(),
                    })
                    .await;
                if !pre_allowed {
                    let content = "Tool call cancelled by hook.".to_string();
                    self.app
                        .emit(
                            event_name,
                            StreamEvent::ToolResult {
                                tool_call_id: tc.id.clone(),
                                content: content.clone(),
                                is_error: true,
                            },
                        )
                        .ok();
                    tool_result_blocks.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tc.id,
                        "content": content,
                    }));
                    continue;
                }

                let full_access = {
                    let settings = self.settings.read().await;
                    settings.permissions.full_access
                };
                let ctx = ExecCtx {
                    cwd: self.cwd.clone(),
                    full_access,
                };

                let tool_start = std::time::Instant::now();
                // Check if this is an MCP tool
                let mcp_server = self.mcp_manager.find_tool_server(&tc.function.name).await;
                let output = if let Some(server_id) = mcp_server {
                    match self.mcp_manager.call_tool(&server_id, &tc.function.name, args).await {
                        Ok(text) => tools::ToolOutput::ok(text),
                        Err(e) => tools::ToolOutput::err(format!("MCP error: {e}")),
                    }
                } else {
                    tools::dispatch(&tc.function.name, args, &ctx).await?
                };
                let duration_ms = tool_start.elapsed().as_millis() as u64;

                // Post-tool hook
                hook_runner
                    .fire(hooks::HookEvent::PostTool {
                        tool_name: tc.function.name.clone(),
                        result: output.content.chars().take(500).collect(),
                        duration_ms,
                    })
                    .await;

                self.app
                    .emit(
                        event_name,
                        StreamEvent::ToolResult {
                            tool_call_id: tc.id.clone(),
                            content: output.content.clone(),
                            is_error: output.is_error,
                        },
                    )
                    .ok();

                // Persist tool result to DB
                let now = Utc::now().timestamp_millis();
                let msg_id = Uuid::new_v4().to_string();
                let tool_content = serde_json::json!({
                    "tool_call_id": tc.id,
                    "content": output.content
                })
                .to_string();
                sqlx::query(
                    "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?,?,?,?,?)",
                )
                .bind(&msg_id)
                .bind(&self.session_id)
                .bind("tool")
                .bind(&tool_content)
                .bind(now)
                .execute(&self.db)
                .await?;

                tool_result_blocks.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tc.id,
                    "content": output.content,
                }));
            }

            // Append a single user message with all tool_result blocks
            if !tool_result_blocks.is_empty() {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": tool_result_blocks,
                }));
            }
        }

        Ok(())
    }
}

/// Convert an MCP tool descriptor into the OpenAI-compatible ToolDefinition format.
fn mcp_tool_to_definition(tool: &crate::mcp::McpTool) -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}

/// Parse tool message content stored as `{"tool_call_id":"…","content":"…"}`.
/// Falls back gracefully if the JSON is malformed.
fn parse_tool_message_content(raw: &str) -> (String, String) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        let id = v
            .get("tool_call_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let content = v
            .get("content")
            .and_then(|x| x.as_str())
            .unwrap_or(raw)
            .to_string();
        return (id, content);
    }
    (String::new(), raw.to_string())
}

/// Read at most `max_chars` UTF-8 chars from a file, appending "…" if truncated.
fn read_file_capped(path: &Path, max_chars: usize) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    if raw.len() <= max_chars {
        Some(raw)
    } else {
        // Truncate at a char boundary
        let truncated: String = raw.chars().take(max_chars).collect();
        Some(format!("{truncated}\n[… truncated]"))
    }
}

/// Detect primary project type and return (label, config_file_path).
fn detect_project_config(cwd: &Path) -> Option<(&'static str, std::path::PathBuf)> {
    let candidates: &[(&str, &str)] = &[
        ("Rust (Cargo.toml)", "Cargo.toml"),
        ("Node.js (package.json)", "package.json"),
        ("Python (pyproject.toml)", "pyproject.toml"),
        ("Python (setup.py)", "setup.py"),
        ("Go (go.mod)", "go.mod"),
    ];
    for (label, file) in candidates {
        let path = cwd.join(file);
        if path.exists() {
            return Some((label, path));
        }
    }
    None
}

fn build_system_prompt(cwd: &Path) -> String {
    let mut prompt = SYSTEM_PROMPT.to_string();

    // ── 1. Project memory (CODEFACTORY.md) ──────────────────────────────────
    if let Some(memory) = read_file_capped(&cwd.join("CODEFACTORY.md"), 4000) {
        let memory = memory.trim();
        if !memory.is_empty() {
            prompt.push_str("\n\n# Project Memory (CODEFACTORY.md)\n");
            prompt.push_str(memory);
        }
    }

    // ── 2. README ────────────────────────────────────────────────────────────
    for readme in &["README.md", "README.txt", "readme.md"] {
        if let Some(content) = read_file_capped(&cwd.join(readme), 3000) {
            prompt.push_str(&format!("\n\n# Project README ({readme})\n"));
            prompt.push_str(&content);
            break; // only first found
        }
    }

    // ── 3. Project config (Cargo.toml / package.json / etc.) ────────────────
    if let Some((label, config_path)) = detect_project_config(cwd) {
        if let Some(content) = read_file_capped(&config_path, 2000) {
            prompt.push_str(&format!("\n\n# Project Config — {label}\n```\n"));
            prompt.push_str(&content);
            prompt.push_str("\n```");
        }
    }

    prompt
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PermissionDecision {
    Allow,
    Ask,
    Deny(String),
}

fn decide_permission(
    policy: &PermissionPolicy,
    tool_name: &str,
    cmd: Option<&str>,
) -> PermissionDecision {
    if policy.full_access {
        return PermissionDecision::Allow;
    }

    let key = match cmd {
        Some(c) => format!("{}({})", tool_name, c),
        None => tool_name.to_string(),
    };

    for pattern in &policy.deny {
        if glob_match(pattern, &key) || glob_match(pattern, tool_name) {
            return PermissionDecision::Deny(format!("Denied by policy: matches '{pattern}'"));
        }
    }

    for pattern in &policy.allow {
        if glob_match(pattern, &key) || glob_match(pattern, tool_name) {
            return PermissionDecision::Allow;
        }
    }

    for pattern in &policy.ask {
        if glob_match(pattern, &key) || glob_match(pattern, tool_name) {
            return PermissionDecision::Ask;
        }
    }

    PermissionDecision::Ask
}

/// Simple glob matcher: supports `*` (any chars) and exact prefix matching like `bash(git *)`.
fn glob_match(pattern: &str, input: &str) -> bool {
    if pattern == input {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return input.starts_with(prefix);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(allow: &[&str], ask: &[&str], deny: &[&str]) -> PermissionPolicy {
        PermissionPolicy {
            allow: allow.iter().map(|v| v.to_string()).collect(),
            ask: ask.iter().map(|v| v.to_string()).collect(),
            deny: deny.iter().map(|v| v.to_string()).collect(),
            full_access: false,
        }
    }

    #[test]
    fn ask_policy_requires_user_decision() {
        let policy = policy(&["read_file"], &["bash"], &[]);
        assert_eq!(
            decide_permission(&policy, "bash", Some("pnpm build")),
            PermissionDecision::Ask
        );
    }

    #[test]
    fn full_access_bypasses_configured_ask_and_deny_rules() {
        let mut policy = policy(&[], &["bash"], &["bash(*)"]);
        policy.full_access = true;
        assert_eq!(
            decide_permission(&policy, "bash", Some("pnpm build")),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn project_memory_from_codefactory_md_is_appended_to_system_prompt() {
        let cwd = std::env::temp_dir().join(format!(
            "codefactory-project-memory-test-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("CODEFACTORY.md"), "Use the repo-local memory.").unwrap();

        let prompt = build_system_prompt(&cwd);

        assert!(prompt.starts_with(SYSTEM_PROMPT));
        assert!(prompt.contains("CODEFACTORY.md"));
        assert!(prompt.contains("Use the repo-local memory."));

        std::fs::remove_dir_all(cwd).unwrap();
    }
}
