// SPDX-License-Identifier: Apache-2.0
use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;
use serde_json::{json, Value};
use walkdir::WalkDir;

use super::{ExecCtx, ToolOutput};
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
                    "path":    { "type": "string", "description": "Search root (default: cwd)" }
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
        Some(p) => ctx.cwd.join(p),
        None => ctx.cwd.clone(),
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
