// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use serde_json::{json, Value};

use super::{file_lock, path_sanity, test_path, unified_diff_for_path, ExecCtx, ToolOutput};
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
    let a: Args = match serde_json::from_value(args.clone()) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!(
            "Invalid arguments for edit_file: {e}. Received: {}",
            serde_json::to_string(&args).unwrap_or_else(|_| "<unprintable>".into())
                .chars().take(300).collect::<String>(),
        ))),
    };

    let path = ctx.cwd.join(&a.path);

    // Hold the per-file lock across the read+modify+write cycle so concurrent
    // edits to the same file serialise (otherwise one writer's changes get
    // clobbered by the other's read-then-write).
    let _guard = file_lock::acquire(&path).await;

    let original = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) => {
            // Typo-detection: catch hallucinated paths early with a useful
            // correction instead of a bare "file not found".
            if let Some(s) = path_sanity::check(&path) {
                return Ok(ToolOutput::err(path_sanity::format_error(&s, &path, "edit_file")));
            }
            return Ok(ToolOutput::err(format!(
                "Cannot read {}: {e}",
                path.display()
            )));
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

    let expected_bytes = updated.as_bytes();
    file_lock::atomic_write(&path, expected_bytes).await?;

    // Read-back integrity check (see write.rs for rationale).
    let actual = tokio::fs::read(&path).await?;
    if actual != expected_bytes {
        return Ok(ToolOutput::err(format!(
            "edit_file integrity check failed for {}: expected {} bytes, found {} bytes. \
             Re-issue the edit.",
            path.display(),
            expected_bytes.len(),
            actual.len(),
        )));
    }

    let diff = unified_diff_for_path(&a.path, &original, &updated);
    let mut body = format!(
        "Edited {}\n\n```diff\n{diff}```",
        path.display()
    );
    // Test-file discipline reminder — see test_path::TEST_MODIFIED_BANNER
    // and the system-prompt TDD section for the contract.
    if test_path::is_test_path(&std::path::Path::new(&a.path)) {
        body = format!("{}\n{}", test_path::TEST_MODIFIED_BANNER, body);
    }
    Ok(ToolOutput::ok(body))
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
