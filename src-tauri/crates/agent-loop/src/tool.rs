// SPDX-License-Identifier: Apache-2.0
//! The tool-execution seam (keystone slice 4.2).
//!
//! [`ToolBackend`] is how the one shared loop stays agnostic to *where* tools
//! run. The desktop app's `DesktopToolBackend` (bin crate) executes in-process
//! via `crate::tools` and owns the `AppHandle`; the Terminal-Bench sidecar's
//! `DelegatingToolBackend` emits a JSONL `tool_request` and awaits a
//! `tool_result` from the Harbor container. The trait NEVER sees tauri — the
//! desktop impl closes over the `AppHandle` privately, so the loop only ever
//! holds `Arc<dyn ToolBackend>` and the unit-test EXE links no Tauri
//! entrypoints (#166).
//!
//! Nothing consumes this yet; the desktop impl lands in slice 4.3.

use crate::types::{ToolCall, ToolDefinition};
use codefactory_agent_core::ToolKind;
use std::path::PathBuf;

/// Per-invocation context the loop hands to a backend. No `AppHandle`, no
/// `SqlitePool` — those live inside the concrete backend, not on the wire
/// between the loop and the trait.
#[derive(Debug, Clone, Default)]
pub struct ToolCtx {
    pub working_directory: PathBuf,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub knowledge_library_ids: Option<Vec<String>>,
    /// Budget-clamped per-call timeout. The sidecar puts this on its
    /// `tool_request`; the desktop backend applies it to its `ExecCtx`.
    pub timeout_sec: Option<u64>,
}

/// The result of executing one model tool call — rich enough for the loop to
/// (a) build a [`codefactory_agent_core::ToolOutcome`] to feed the gate,
/// (b) emit a `StreamEvent::ToolResult`, and (c) append to history — WITHOUT
/// the loop knowing which backend produced it.
#[derive(Debug, Clone)]
pub struct ToolInvocationResult {
    /// Model-facing body + `ToolResult` event content.
    pub content: String,
    pub is_error: bool,
    /// The executed command string → `ToolOutcome.command`.
    pub command: String,
    /// The backend classifies: a shell command via `classify_command`, a named
    /// tool via its prefix rule.
    pub kind: ToolKind,
    pub return_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    /// Sidecar cd-tracking (the Harbor container reports where cwd ended up);
    /// the desktop backend leaves this `None`.
    pub next_working_directory: Option<String>,
    pub duration_ms: u64,
}

/// A fatal tool-execution failure that must ABORT the turn — distinct from a
/// tool that ran and returned an error result (`is_error` on a
/// [`ToolInvocationResult`], which the loop feeds back to the model). The
/// desktop backend maps a `tools::dispatch` `Err` to this; the loop records it
/// and propagates, preserving the pre-refactor "dispatch error ends the turn"
/// behaviour. The message is carried verbatim.
#[derive(Debug, Clone)]
pub struct ToolError {
    pub message: String,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

/// Where and how the loop's tool calls execute.
#[async_trait::async_trait]
pub trait ToolBackend: Send + Sync {
    /// Schemas advertised to the model. Async because MCP discovery is async on
    /// the desktop. Desktop = `tools::all_definitions()` + MCP; sidecar =
    /// exactly `[run_shell]`.
    async fn list_schemas(&self) -> Vec<ToolDefinition>;

    /// Execute ONE model tool call. `args` is the already-parsed argument object
    /// (the loop parses it once and reuses it for permissioning/hooks, so we
    /// pass it in rather than re-parsing). The delegating impl translates a
    /// `run_shell` call → command string internally; the desktop impl does
    /// MCP-first / native-dispatch fallback. `Ok(result)` — the tool ran (even
    /// if `result.is_error`); `Err(ToolError)` — a fatal failure that aborts
    /// the turn.
    async fn execute(
        &self,
        call: &ToolCall,
        args: &serde_json::Value,
        ctx: &ToolCtx,
    ) -> Result<ToolInvocationResult, ToolError>;

    /// Classify a call BEFORE it runs, for the pre-execution policy checks
    /// (budget denial, inspection budget) — keystone slice 4.8c b5.
    ///
    /// The backend owns this because the default rule only calls
    /// `classify_command` when the tool is literally named `bash`: correct for
    /// the desktop, but it would classify EVERY eval-sidecar call (`run_shell`)
    /// as `ReadOnly`, so the inspection-budget rule would fire on everything and
    /// the budget evaluator would never see a mutation. b2 fixed the same trap
    /// on the post-execution side; this closes it pre-execution, so the backend
    /// owns classification on both sides.
    fn classify(&self, call: &ToolCall, args: &serde_json::Value) -> (String, ToolKind) {
        crate::policy::completion_command_and_kind(&call.function.name, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A no-op backend used to prove the trait is object-safe and to stand in
    /// for a real backend in future loop tests. Owns NOTHING tauri-related.
    struct StubBackend;

    #[async_trait::async_trait]
    impl ToolBackend for StubBackend {
        async fn list_schemas(&self) -> Vec<ToolDefinition> {
            Vec::new()
        }
        async fn execute(
            &self,
            call: &ToolCall,
            _args: &serde_json::Value,
            _ctx: &ToolCtx,
        ) -> Result<ToolInvocationResult, ToolError> {
            if call.function.name == "boom" {
                return Err(ToolError {
                    message: "fatal".into(),
                });
            }
            Ok(ToolInvocationResult {
                content: format!("stub:{}", call.function.name),
                is_error: false,
                command: call.function.name.clone(),
                kind: ToolKind::ReadOnly,
                return_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                error: None,
                next_working_directory: None,
                duration_ms: 0,
            })
        }
    }

    #[tokio::test]
    async fn tool_backend_is_object_safe_and_dispatches() {
        let backend: std::sync::Arc<dyn ToolBackend> = std::sync::Arc::new(StubBackend);
        assert!(backend.list_schemas().await.is_empty());
        let call = ToolCall {
            id: "c1".into(),
            r#type: "function".into(),
            function: crate::types::FunctionCall {
                name: "run_shell".into(),
                arguments: "{}".into(),
            },
        };
        let out = backend
            .execute(&call, &serde_json::json!({}), &ToolCtx::default())
            .await
            .expect("stub ok");
        assert_eq!(out.content, "stub:run_shell");
        assert!(matches!(out.kind, ToolKind::ReadOnly));
    }

    #[tokio::test]
    async fn fatal_tool_error_surfaces_as_err() {
        let backend = StubBackend;
        let call = ToolCall {
            id: "c2".into(),
            r#type: "function".into(),
            function: crate::types::FunctionCall {
                name: "boom".into(),
                arguments: "{}".into(),
            },
        };
        let err = backend
            .execute(&call, &serde_json::json!({}), &ToolCtx::default())
            .await
            .expect_err("boom is fatal");
        assert_eq!(err.to_string(), "fatal");
    }
}
