// SPDX-License-Identifier: Apache-2.0
use regex::Regex;
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
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "grep".into(),
            description: "Search for a regex pattern in files. Returns matching lines with file:line context.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern":          { "type": "string" },
                    "path":             { "type": "string", "description": "Search root inside the workspace (default: cwd)" },
                    "glob":             { "type": "string", "description": "File filter, e.g. '*.rs'" },
                    "case_insensitive": { "type": "boolean", "default": false }
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

    let re = match Regex::new(&if a.case_insensitive {
        format!("(?i){}", a.pattern)
    } else {
        a.pattern.clone()
    }) {
        Ok(r) => r,
        Err(e) => return Ok(ToolOutput::err(format!("Invalid regex: {e}"))),
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

    let glob_set = a.glob.as_deref().and_then(|g| {
        globset::Glob::new(g)
            .ok()
            .and_then(|g| globset::GlobSetBuilder::new().add(g).build().ok())
    });

    let mut results: Vec<String> = Vec::new();

    'outer: for entry in WalkDir::new(&root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        if let Some(gs) = &glob_set {
            let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            if !gs.is_match(rel) {
                continue;
            }
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                results.push(format!("{}:{}: {}", rel.display(), i + 1, line));
                if results.len() >= 500 {
                    results.push("[truncated at 500 results]".into());
                    break 'outer;
                }
            }
        }
    }

    Ok(ToolOutput::ok(results.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn grep_rejects_search_root_outside_workspace() {
        let root =
            std::env::temp_dir().join(format!("codefactory-grep-boundary-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::write(root.join("outside.txt"), "secret").expect("seed outside file");

        let output = execute(
            json!({
                "pattern": "secret",
                "path": ".."
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
