// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    file_lock, path_sanity, test_path, unified_diff_for_path, workspace_path, ExecCtx, ToolOutput,
};
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
                    "path":    { "type": "string", "description": "Workspace-relative path, or an absolute path inside the workspace" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let a: Args = match serde_json::from_value(args.clone()) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "Invalid arguments for write_file: {e}. Received: {}",
                serde_json::to_string(&args)
                    .unwrap_or_else(|_| "<unprintable>".into())
                    .chars()
                    .take(300)
                    .collect::<String>(),
            )))
        }
    };

    let path = match workspace_path::resolve_writable(&ctx.cwd, &a.path) {
        Ok(path) => path,
        Err(err) => return Ok(ToolOutput::err(err.message())),
    };

    // Hallucinated-path guard: catch obvious typos against existing siblings
    // before any IO. Returns a corrective suggestion so the model can retry
    // with the right path on the next turn. Genuine new directories with
    // distinct names (no near-neighbour) sail through.
    if let Some(s) = path_sanity::check(&path) {
        return Ok(ToolOutput::err(path_sanity::format_error(
            &s,
            &path,
            "write_file",
        )));
    }

    // Serialise read+write per-file so a parallel edit on the same file can't
    // race with us. Different files still run fully in parallel.
    let _guard = file_lock::acquire(&path).await;

    let original = tokio::fs::read_to_string(&path).await.unwrap_or_default();

    // atomic_write handles mkdir, write to temp file, and atomic rename with
    // Windows sharing-violation retry.
    let expected_bytes = a.content.as_bytes();
    file_lock::atomic_write(&path, expected_bytes).await?;

    // Read back and verify byte-for-byte. The user reported a class of bugs
    // where write_file appeared to succeed but the on-disk content was
    // missing characters / had stray edits. Most root causes were upstream
    // (SSE chunk truncation, since fixed), but a post-write hash check is
    // cheap insurance and produces an actionable error instead of silent
    // corruption when it does happen.
    let actual = tokio::fs::read(&path).await?;
    if actual != expected_bytes {
        return Ok(ToolOutput::err(format!(
            "write_file integrity check failed for {}: expected {} bytes, found {} bytes on disk. \
             This usually means the upstream tool-call arguments arrived corrupted. \
             Re-issue the write with the exact content.",
            path.display(),
            expected_bytes.len(),
            actual.len(),
        )));
    }

    let diff = unified_diff_for_path(&a.path, &original, &a.content);
    let mut body = format!(
        "Written {} bytes to {}\n\n```diff\n{diff}```",
        a.content.len(),
        path.display()
    );
    // If we just touched a test file, prepend the discipline banner so the
    // model sees it in its own context and is obliged (by the system
    // prompt) to justify the change.
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
            &ExecCtx { cwd: cwd.clone() },
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

    #[tokio::test]
    async fn write_file_rejects_parent_traversal_outside_workspace() {
        let root = temp_dir();
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&cwd).expect("create workspace");
        let outside = root.join("owned.txt");

        let output = execute(
            json!({
                "path": "../owned.txt",
                "content": "owned"
            }),
            &ExecCtx { cwd },
        )
        .await
        .expect("tool returns output");

        let outside_exists = outside.exists();
        let _ = std::fs::remove_dir_all(root);

        assert!(output.is_error);
        assert!(output.content.contains("outside the workspace"));
        assert!(!outside_exists);
    }
}
