// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use serde_json::{json, Value};

use super::{unified_diff_for_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

#[derive(Deserialize)]
struct Args {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "edit_file".into(),
            description: "Replace exact string occurrences in a file. old_string must be unique unless replace_all is true.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":        { "type": "string" },
                    "old_string":  { "type": "string" },
                    "new_string":  { "type": "string" },
                    "replace_all": { "type": "boolean", "default": false }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Ok(a) = serde_json::from_value::<Args>(args) else {
        return Ok(ToolOutput::err("Invalid arguments"));
    };

    let path = ctx.cwd.join(&a.path);
    let original = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "Cannot read {}: {e}",
                path.display()
            )))
        }
    };

    if !a.replace_all {
        let count = original.matches(&a.old_string).count();
        if count == 0 {
            return Ok(ToolOutput::err("old_string not found in file"));
        }
        if count > 1 {
            return Ok(ToolOutput::err(format!(
                "old_string matches {count} times — use replace_all:true or provide more context"
            )));
        }
    }

    let updated = if a.replace_all {
        original.replace(&a.old_string, &a.new_string)
    } else {
        original.replacen(&a.old_string, &a.new_string, 1)
    };

    std::fs::write(&path, &updated)?;
    let diff = unified_diff_for_path(&a.path, &original, &updated);
    Ok(ToolOutput::ok(format!(
        "Edited {}\n\n```diff\n{diff}```",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("codefactory-edit-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[tokio::test]
    async fn edit_file_result_includes_unified_diff_for_changed_file() {
        let cwd = temp_dir();
        let file_path = cwd.join("src").join("main.rs");
        std::fs::create_dir_all(file_path.parent().expect("parent")).expect("create parent");
        std::fs::write(&file_path, "fn main() {\n    old_call();\n}\n").expect("seed file");

        let output = execute(
            json!({
                "path": "src/main.rs",
                "old_string": "old_call();",
                "new_string": "new_call();"
            }),
            &ExecCtx {
                cwd: cwd.clone(),
                full_access: false,
            },
        )
        .await
        .expect("edit succeeds");

        let _ = std::fs::remove_dir_all(cwd);

        assert!(!output.is_error);
        assert!(output.content.contains("Edited "));
        assert!(output.content.contains("--- a/src/main.rs"));
        assert!(output.content.contains("+++ b/src/main.rs"));
        assert!(output.content.contains("@@ -1,3 +1,3 @@"));
        assert!(output.content.contains(" fn main() {"));
        assert!(output.content.contains("-    old_call();"));
        assert!(output.content.contains("+    new_call();"));
        assert!(output.content.contains(" }"));
    }
}
