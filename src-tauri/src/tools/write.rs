// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use serde_json::{json, Value};

use super::{unified_diff_for_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "write_file".into(),
            description: "Create or overwrite a file with the given content.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":    { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Ok(a) = serde_json::from_value::<Args>(args) else {
        return Ok(ToolOutput::err("Invalid arguments"));
    };

    let path = ctx.cwd.join(&a.path);
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &a.content)?;
    let diff = unified_diff_for_path(&a.path, &original, &a.content);
    Ok(ToolOutput::ok(format!(
        "Written {} bytes to {}\n\n```diff\n{diff}```",
        a.content.len(),
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("codefactory-write-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[tokio::test]
    async fn write_file_result_includes_unified_diff_for_changed_file() {
        let cwd = temp_dir();
        let file_path = cwd.join("notes.txt");
        std::fs::write(&file_path, "alpha\nold\n").expect("seed file");

        let output = execute(
            json!({
                "path": "notes.txt",
                "content": "alpha\nnew\n"
            }),
            &ExecCtx {
                cwd: cwd.clone(),
                full_access: false,
            },
        )
        .await
        .expect("write succeeds");

        let _ = std::fs::remove_dir_all(cwd);

        assert!(!output.is_error);
        assert!(output.content.contains("Written 10 bytes to"));
        assert!(output.content.contains("--- a/notes.txt"));
        assert!(output.content.contains("+++ b/notes.txt"));
        assert!(output.content.contains("@@ -1,2 +1,2 @@"));
        assert!(output.content.contains(" alpha"));
        assert!(output.content.contains("-old"));
        assert!(output.content.contains("+new"));
    }
}
