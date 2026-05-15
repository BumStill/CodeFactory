// SPDX-License-Identifier: Apache-2.0
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

use crate::config::settings::{PermissionPolicy, Settings};
use crate::errors::Result;
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
    cwd: PathBuf,
    http: Client,
    settings: Arc<RwLock<Settings>>,
    pending_permissions: PendingPermissionMap,
}

impl AgentLoop {
    pub fn new(
        app: AppHandle,
        db: SqlitePool,
        session_id: String,
        model_id: String,
        base_url: String,
        api_key: String,
        cwd: PathBuf,
        settings: Arc<RwLock<Settings>>,
        pending_permissions: PendingPermissionMap,
    ) -> Self {
        Self {
            app,
            db,
            session_id,
            model_id,
            base_url,
            api_key,
            cwd,
            http: Client::new(),
            settings,
            pending_permissions,
        }
    }

    pub async fn run(&mut self, history: Vec<Message>) -> Result<()> {
        let mut messages = self.build_messages(history);
        let tool_defs = tools::all_definitions();
        let event_name = format!("stream:{}", self.session_id);

        for _ in 0..MAX_ITERATIONS {
            let (text, tool_calls, usage) = self.call_model(&messages, &tool_defs).await?;

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
                    self.app
                        .emit(
                            &event_name,
                            StreamEvent::Done {
                                input_tokens: u.prompt_tokens,
                                output_tokens: u.completion_tokens,
                            },
                        )
                        .ok();
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

                let full_access = {
                    let settings = self.settings.read().await;
                    settings.permissions.full_access
                };
                let ctx = ExecCtx {
                    cwd: self.cwd.clone(),
                    full_access,
                };

                let output = tools::dispatch(&tc.function.name, args, &ctx).await?;

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

    async fn call_model(
        &self,
        messages: &[ChatMessage],
        tool_defs: &[ToolDefinition],
    ) -> Result<(String, Vec<ToolCall>, Option<Usage>)> {
        let event_name = format!("stream:{}", self.session_id);
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let req = ChatRequest {
            model: self.model_id.clone(),
            messages: messages.to_vec(),
            tools: Some(tool_defs.to_vec()),
            tool_choice: Some(serde_json::json!("auto")),
            stream: true,
            temperature: 0.2,
            max_tokens: 8192,
            usage: Some(UsageOptions { include: true }),
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

    fn build_messages(&self, history: Vec<Message>) -> Vec<ChatMessage> {
        let mut msgs = vec![ChatMessage {
            role: "system".into(),
            content: MessageContent::Text(build_system_prompt(&self.cwd)),
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

fn build_system_prompt(cwd: &Path) -> String {
    let mut prompt = SYSTEM_PROMPT.to_string();
    let memory_path = cwd.join("CODEFACTORY.md");
    let Ok(memory) = std::fs::read_to_string(&memory_path) else {
        return prompt;
    };
    let memory = memory.trim();
    if memory.is_empty() {
        return prompt;
    }

    prompt.push_str("\n\n# Project Memory (CODEFACTORY.md)\n");
    prompt.push_str(memory);
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
