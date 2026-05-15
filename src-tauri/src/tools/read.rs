// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

use super::{ExecCtx, ToolOutput};
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
                    "path": { "type": "string", "description": "Absolute or cwd-relative path" },
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

    let path = ctx.cwd.join(&a.path);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "Cannot open {}: {e}",
                path.display()
            )))
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
