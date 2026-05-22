// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

use super::{path_sanity, workspace_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

#[derive(Deserialize)]
struct Args {
    path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "read_file".into(),
            description: "Read a file from disk. Returns line-numbered content.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative path, or an absolute path inside the workspace" },
                    "offset": { "type": "integer", "description": "Start line (1-based, optional)" },
                    "limit":  { "type": "integer", "description": "Max lines to read (default 2000)" }
                },
                "required": ["path"]
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Ok(a) = serde_json::from_value::<Args>(args) else {
        return Ok(ToolOutput::err("Invalid arguments"));
    };

    let path = match workspace_path::resolve_existing(&ctx.cwd, &a.path) {
        Ok(path) => path,
        Err(err) => {
            if let Some(path) = err.path() {
                if let Some(s) = path_sanity::check(path) {
                    return Ok(ToolOutput::err(path_sanity::format_error(
                        &s,
                        path,
                        "read_file",
                    )));
                }
            }
            return Ok(ToolOutput::err(err.message()));
        }
    };
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            // If the path doesn't exist, see if it looks like a typo of a
            // real file and suggest the correction in the error.
            if let Some(s) = path_sanity::check(&path) {
                return Ok(ToolOutput::err(path_sanity::format_error(
                    &s,
                    &path,
                    "read_file",
                )));
            }
            return Ok(ToolOutput::err(format!(
                "Cannot open {}: {e}",
                path.display()
            )));
        }
    };

    let offset = a.offset.unwrap_or(1).saturating_sub(1);
    let limit = a.limit.unwrap_or(2000);

    let mut out = String::new();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        if i < offset {
            continue;
        }
        if i >= offset + limit {
            out.push_str(&format!("\n[truncated after {} lines]", limit));
            break;
        }
        let line = line.map_err(|e| crate::errors::AppError::Io(e))?;
        out.push_str(&format!("{}\t{}\n", i + 1, line));
    }

    Ok(ToolOutput::ok(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_workspace() -> (std::path::PathBuf, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("codefactory-read-boundary-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        (root, workspace)
    }

    #[tokio::test]
    async fn read_file_rejects_parent_traversal_even_with_full_access() {
        let (root, workspace) = temp_workspace();
        let outside = root.join("outside.txt");
        std::fs::write(&outside, "secret").expect("seed outside file");

        let output = execute(
            json!({
                "path": "../outside.txt",
            }),
            &ExecCtx { cwd: workspace },
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(root);

        assert!(output.is_error);
        assert!(output.content.contains("outside the workspace"));
    }
}
