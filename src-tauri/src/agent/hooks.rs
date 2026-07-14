// SPDX-License-Identifier: Apache-2.0
//! Hook runner: fires lifecycle events and executes configured hook actions.

use serde::{Deserialize, Serialize};
use std::io::Write;
use tauri::{AppHandle, Emitter};

use crate::commands::hooks::{run_hook_command, HookAction, HookConfig};
use crate::config::settings::Settings;
use crate::util::no_window::NoWindow;

// ── Hook event types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HookEvent {
    PreTool {
        tool_name: String,
        args: serde_json::Value,
    },
    PostTool {
        tool_name: String,
        result: String,
        duration_ms: u64,
    },
    PreTask {
        task_id: String,
        title: String,
    },
    PostTask {
        task_id: String,
        status: String,
        summary: String,
    },
    SessionStart {
        session_id: String,
    },
    SessionEnd {
        session_id: String,
    },
    SpecApproved {
        spec_path: String,
        req_id: Option<String>,
    },
    VerificationFailed {
        task_id: String,
        check: String,
        output: String,
    },
}

impl HookEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            HookEvent::PreTool { .. } => "pre_tool",
            HookEvent::PostTool { .. } => "post_tool",
            HookEvent::PreTask { .. } => "pre_task",
            HookEvent::PostTask { .. } => "post_task",
            HookEvent::SessionStart { .. } => "session_start",
            HookEvent::SessionEnd { .. } => "session_end",
            HookEvent::SpecApproved { .. } => "spec_approved",
            HookEvent::VerificationFailed { .. } => "verification_failed",
        }
    }
}

// ── HookRunner ────────────────────────────────────────────────────────────────

pub struct HookRunner {
    configs: Vec<HookConfig>,
    app_handle: AppHandle,
}

fn pre_tool_command_cancelled(result: &std::io::Result<std::process::Output>) -> bool {
    match result {
        Ok(output) => !output.status.success(),
        Err(_) => true,
    }
}

impl HookRunner {
    pub fn from_settings(settings: &Settings, app_handle: AppHandle) -> Self {
        Self {
            configs: settings
                .hooks
                .iter()
                .filter(|h| h.enabled)
                .cloned()
                .collect(),
            app_handle,
        }
    }

    /// Anonymous sessions promise no persistent learning/audit residue. Hooks
    /// may write files or run external commands, so they are disabled entirely
    /// for that mode instead of trying to classify individual hook actions.
    pub fn disabled(app_handle: AppHandle) -> Self {
        Self {
            configs: Vec::new(),
            app_handle,
        }
    }

    /// Fire a hook event. Returns `false` if a `pre_tool` hook cancels the call.
    pub async fn fire(&self, event: HookEvent) -> bool {
        let event_name = event.event_name();
        let event_json = serde_json::to_value(&event).unwrap_or_default();

        for config in &self.configs {
            if config.event != event_name {
                continue;
            }

            // Apply optional filter (matches tool_name or task_id)
            if let Some(filter) = &config.filter {
                let haystack = match &event {
                    HookEvent::PreTool { tool_name, .. } => tool_name.as_str(),
                    HookEvent::PostTool { tool_name, .. } => tool_name.as_str(),
                    HookEvent::PreTask { task_id, .. } => task_id.as_str(),
                    HookEvent::PostTask { task_id, .. } => task_id.as_str(),
                    _ => "",
                };
                if !haystack.contains(filter.as_str()) && haystack != filter.as_str() {
                    continue;
                }
            }

            let cancelled = self.run_action(config, &event, &event_json).await;
            // For pre_tool events: if action returned cancel signal, propagate
            if matches!(event, HookEvent::PreTool { .. }) && cancelled {
                return false;
            }
        }

        true
    }

    /// Runs one hook action. Returns true only when it's a RunCommand for a
    /// pre_tool hook and the process exits non-zero (= cancel).
    async fn run_action(
        &self,
        config: &HookConfig,
        event: &HookEvent,
        event_json: &serde_json::Value,
    ) -> bool {
        match &config.action {
            HookAction::LogToFile { path } => {
                let line = format!(
                    "{}\n",
                    serde_json::to_string(event_json).unwrap_or_default()
                );
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = f.write_all(line.as_bytes());
                }
                false
            }

            HookAction::RunCommand { command, cwd } => {
                let is_pre_tool = matches!(event, HookEvent::PreTool { .. });
                let result = run_hook_command(
                    command,
                    cwd.as_deref().map(std::path::Path::new),
                );

                if is_pre_tool {
                    // Non-zero exit = cancel. A configured pre-tool guard that
                    // cannot start must fail closed; silently allowing the
                    // underlying tool would defeat the hook's safety purpose.
                    if let Err(error) = &result {
                        tracing::warn!("pre-tool hook command failed to start: {error}");
                    }
                    pre_tool_command_cancelled(&result)
                } else {
                    // Fire-and-forget style: just spawn
                    let _ = result;
                    false
                }
            }

            HookAction::EmitEvent { event_name } => {
                self.app_handle.emit(event_name, event_json.clone()).ok();
                false
            }

            HookAction::AutoGitCommit { message_template } => {
                // Only meaningful on PostTask
                if let HookEvent::PostTask {
                    task_id, summary, ..
                } = event
                {
                    let msg = message_template
                        .replace("{task_title}", summary)
                        .replace("{task_id}", task_id)
                        .replace("{req_id}", task_id);
                    let _ = std::process::Command::new("powershell")
                        .no_window()
                        .args([
                            "-NonInteractive",
                            "-Command",
                            &format!("git add -A; git commit -m '{msg}'"),
                        ])
                        .output();
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_tool_command_is_fail_closed_for_nonzero_and_runner_failure() {
        let cwd = std::env::temp_dir().join(format!(
            "codefactory-pre-tool-hook-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&cwd).unwrap();

        let success = run_hook_command("exit 0", Some(&cwd));
        assert!(!pre_tool_command_cancelled(&success));

        let nonzero = run_hook_command("exit 9", Some(&cwd));
        assert!(pre_tool_command_cancelled(&nonzero));

        let missing_cwd = cwd.join("missing");
        let runner_failure = run_hook_command("exit 0", Some(&missing_cwd));
        assert!(runner_failure.is_err());
        assert!(pre_tool_command_cancelled(&runner_failure));

        std::fs::remove_dir_all(cwd).unwrap();
    }
}
