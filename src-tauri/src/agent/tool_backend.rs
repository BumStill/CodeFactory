// SPDX-License-Identifier: Apache-2.0
//! Desktop tool-execution backend (keystone slice 4.3).
//!
//! The in-process implementation of [`codefactory_agent_loop::tool::ToolBackend`]:
//! it builds a [`crate::tools::ExecCtx`] from the per-call [`ToolCtx`] plus its
//! own long-lived handles and runs the tool MCP-first / native-dispatch, exactly
//! as both provider loop bodies did inline before. Both loops now route through
//! one `execute`, so the duplicated dispatch block lives in a single place.
//!
//! This owns the `AppHandle` privately (under `#[cfg(not(test))]`, mirroring
//! `ExecCtx.app`), so the loop only ever calls it through the trait and the
//! unit-test EXE links no Tauri entrypoints (#166). It is constructed only in
//! `run_openai`/`run_anthropic`, which the test EXE dead-strips.

use codefactory_agent_loop::tool::{ToolBackend, ToolCtx, ToolError, ToolInvocationResult};

use crate::openrouter::types::{ToolCall, ToolDefinition};

/// In-process tool backend for the desktop app. Holds the long-lived handles;
/// per-call context (cwd, session, task, knowledge scope) arrives via [`ToolCtx`].
pub(super) struct DesktopToolBackend {
    /// Owned privately so the loop never sees an `AppHandle`. Absent in the
    /// test config — this struct is constructed only in the (dead-stripped)
    /// provider loops, never in a `#[cfg(test)]` test.
    #[cfg(not(test))]
    pub(super) app: Option<tauri::AppHandle>,
    pub(super) db: sqlx::SqlitePool,
    pub(super) mcp_manager: std::sync::Arc<crate::mcp::McpManager>,
    pub(super) settings: std::sync::Arc<tokio::sync::RwLock<crate::config::settings::Settings>>,
}

#[async_trait::async_trait]
impl ToolBackend for DesktopToolBackend {
    async fn list_schemas(&self) -> Vec<ToolDefinition> {
        // Desktop surface = native tools + every connected MCP tool. (The
        // anonymous KB-tool strip stays in the loop for now — it depends on the
        // run's anonymous flag; folded in when the loop moves in slice 4.6.)
        let mut defs = crate::tools::all_definitions();
        for mcp_tool in &self.mcp_manager.list_all_tools().await {
            defs.push(super::mcp_tool_to_definition(mcp_tool));
        }
        defs
    }

    async fn execute(
        &self,
        call: &ToolCall,
        args: &serde_json::Value,
        ctx: &ToolCtx,
    ) -> Result<ToolInvocationResult, ToolError> {
        let exec_ctx = crate::tools::ExecCtx {
            cwd: ctx.working_directory.clone(),
            #[cfg(not(test))]
            app: self.app.clone(),
            db: Some(self.db.clone()),
            session_id: ctx.session_id.clone(),
            root_turn_id: ctx.root_turn_id.clone(),
            task_id: ctx.task_id.clone(),
            knowledge_library_ids: ctx.knowledge_library_ids.clone(),
            settings: Some(self.settings.read().await.clone()),
        };

        // MCP-first, then native dispatch — precedence and the `Unknown tool`
        // sentinel are preserved. An MCP error becomes an `is_error` result the
        // model sees; a native-dispatch `Err` is FATAL and aborts the turn.
        let output = if let Some(server_id) =
            self.mcp_manager.find_tool_server(&call.function.name).await
        {
            match self
                .mcp_manager
                .call_tool(&server_id, &call.function.name, args.clone())
                .await
            {
                Ok(text) => crate::tools::ToolOutput::ok(text),
                Err(e) => crate::tools::ToolOutput::err(format!("MCP error: {e}")),
            }
        } else {
            match crate::tools::dispatch(&call.function.name, args.clone(), &exec_ctx).await {
                Ok(output) => output,
                Err(error) => {
                    return Err(ToolError {
                        message: error.to_string(),
                    })
                }
            }
        };

        let (command, kind) =
            codefactory_agent_loop::policy::completion_command_and_kind(&call.function.name, args);
        Ok(ToolInvocationResult {
            content: output.content,
            is_error: output.is_error,
            // The loop now feeds the gate from these fields (slice 4.8c b2), so
            // the backend owns classification. This is exactly the rule the loop
            // applied inline before — `classify_command` for `bash`, the
            // arg-derived command + ReadOnly otherwise — so desktop gate
            // behaviour (#135/#136) is unchanged.
            command,
            kind,
            return_code: None,
            stdout: String::new(),
            stderr: String::new(),
            error: None,
            next_working_directory: None,
            duration_ms: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    //! In `#[cfg(test)]` builds `DesktopToolBackend` has NO `app` field, so
    //! these construct the REAL backend with no `AppHandle` — that headless
    //! constructibility is the whole point of the seam, and it keeps the
    //! unit-test EXE clear of Tauri entrypoints (#166; McpManager/Settings/pool
    //! own no `AppHandle`). This locks the contract the loop relies on: the full
    //! native tool surface runs through `execute`, MCP-first with a native
    //! fallback, and an unknown tool is a clean `is_error` result, not a fatal
    //! `Err`.
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn backend() -> DesktopToolBackend {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        DesktopToolBackend {
            db,
            mcp_manager: std::sync::Arc::new(crate::mcp::McpManager::new()),
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::config::settings::Settings::default(),
            )),
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "t".into(),
            r#type: "function".into(),
            function: crate::openrouter::types::FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    #[tokio::test]
    async fn desktop_backend_runs_the_native_surface_headless() {
        let dir = std::env::temp_dir().join(format!("cf-desktop-backend-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let backend = backend().await;
        let ctx = ToolCtx {
            working_directory: dir.clone(),
            ..Default::default()
        };

        let out = backend
            .execute(
                &call("write_file"),
                &serde_json::json!({ "path": "n.txt", "content": "hello backend" }),
                &ctx,
            )
            .await
            .expect("write is not fatal");
        assert!(!out.is_error, "write via backend: {}", out.content);

        let out = backend
            .execute(
                &call("read_file"),
                &serde_json::json!({ "path": "n.txt" }),
                &ctx,
            )
            .await
            .expect("read is not fatal");
        assert!(!out.is_error && out.content.contains("hello backend"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unknown_tool_is_an_is_error_result_not_a_fatal_err() {
        let backend = backend().await;
        let ctx = ToolCtx {
            working_directory: std::env::temp_dir(),
            ..Default::default()
        };
        let out = backend
            .execute(&call("no_such_tool"), &serde_json::json!({}), &ctx)
            .await
            .expect("unknown tool returns a result, never aborts the turn");
        assert!(out.is_error);
        assert!(out.content.contains("Unknown tool"));
    }
}
