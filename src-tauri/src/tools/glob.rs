// SPDX-License-Identifier: Apache-2.0
use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use walkdir::WalkDir;

use super::{workspace_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "glob".into(),
            description: "Find files matching a glob pattern (e.g. '**/*.rs').".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path":    { "type": "string", "description": "Search root inside the workspace (default: cwd)" }
                },
                "required": ["pattern"]
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Ok(a) = serde_json::from_value::<Args>(args) else {
        return Ok(ToolOutput::err("Invalid arguments"));
    };

    let root = match &a.path {
        Some(p) => match workspace_path::resolve_existing(&ctx.cwd, p) {
            Ok(path) => path,
            Err(err) => return Ok(ToolOutput::err(err.message())),
        },
        None => match workspace_path::resolve_existing(&ctx.cwd, ".") {
            Ok(path) => path,
            Err(err) => return Ok(ToolOutput::err(err.message())),
        },
    };

    let glob = match Glob::new(&a.pattern) {
        Ok(g) => g,
        Err(e) => return Ok(ToolOutput::err(format!("Invalid glob pattern: {e}"))),
    };
    let set = GlobSetBuilder::new().add(glob).build().unwrap();

    let mut matches: Vec<String> = WalkDir::new(&root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let rel = e.path().strip_prefix(&root).unwrap_or(e.path());
            set.is_match(rel)
        })
        .map(|e| {
            e.path()
                .strip_prefix(&root)
                .unwrap_or(e.path())
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    matches.sort();
    matches.truncate(500);

    Ok(ToolOutput::ok(matches.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn glob_rejects_search_root_outside_workspace() {
        let root =
            std::env::temp_dir().join(format!("codefactory-glob-boundary-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::write(root.join("outside.rs"), "fn secret() {}").expect("seed outside file");

        let output = execute(
            json!({
                "pattern": "**/*.rs",
                "path": ".."
            }),
            &ExecCtx::new(workspace, None),
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(root);

        assert!(output.is_error);
        assert!(output.content.contains("outside the workspace"));
    }
}
