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
    #[serde(default)]
    old_string: Option<String>,
    #[serde(default)]
    new_string: Option<String>,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    edits: Option<Vec<EditOp>>,
}

/// One exact-string replacement inside a batch (`edits`) call.
#[derive(Deserialize, Clone)]
pub(crate) struct EditOp {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// A parsed `edit_file` call. `batch` distinguishes the M263 `edits` form from
/// the legacy single `old_string`/`new_string` form, whose error wording and
/// no-match tolerance stay exactly as they were.
pub(crate) struct EditPlan {
    ops: Vec<EditOp>,
    batch: bool,
}

impl EditPlan {
    pub(crate) fn change_count(&self) -> usize {
        self.ops.len()
    }
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "edit_file".into(),
            description: "Apply exact-string replacements to ONE file in a single call. \
For several changes to the same file, send them all in `edits` (one entry per change) — \
that is one call and one approval; do not write a Python/shell script to rewrite files. \
Each old_string must be unique unless replace_all is true. A batch is atomic: if any entry \
is missing or ambiguous, nothing is written and the error names the entry."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":        { "type": "string", "description": "Workspace-relative path, or an absolute path inside the workspace" },
                    "old_string":  { "type": "string", "description": "Single edit: exact text to replace (use `edits` instead for several changes)" },
                    "new_string":  { "type": "string", "description": "Single edit: replacement text" },
                    "replace_all": { "type": "boolean", "default": false, "description": "Single edit: replace every occurrence" },
                    "edits": {
                        "type": "array",
                        "description": "Batch form: every change for this file in one call. Applied in order, atomically.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string":  { "type": "string" },
                                "new_string":  { "type": "string" },
                                "replace_all": { "type": "boolean", "default": false }
                            },
                            "required": ["old_string", "new_string"]
                        }
                    }
                },
                "required": ["path"]
            }),
        },
    }
}

/// Parse `edit_file` arguments into a plan. Shared with `tool_backend`'s
/// post-write observation so admission and execution never disagree about the
/// resulting bytes.
pub(crate) fn plan_from_args(args: &Value) -> std::result::Result<EditPlan, String> {
    let a: Args = serde_json::from_value(args.clone()).map_err(|e| {
        format!(
            "Invalid arguments for edit_file: {e}. Received: {}",
            serde_json::to_string(args)
                .unwrap_or_else(|_| "<unprintable>".into())
                .chars()
                .take(300)
                .collect::<String>(),
        )
    })?;

    match (a.edits, a.old_string, a.new_string) {
        (Some(edits), None, None) if !edits.is_empty() => Ok(EditPlan { ops: edits, batch: true }),
        (Some(edits), None, None) => {
            let _ = edits;
            Err("Invalid arguments for edit_file: `edits` is empty — list at least one \
{old_string, new_string} entry, or use write_file to rewrite the whole file."
                .into())
        }
        (Some(_), _, _) => Err("Invalid arguments for edit_file: send either `edits` (batch) \
or `old_string`/`new_string` (single) — not both."
            .into()),
        (None, Some(old_string), Some(new_string)) => Ok(EditPlan {
            ops: vec![EditOp {
                old_string,
                new_string,
                replace_all: a.replace_all,
            }],
            batch: false,
        }),
        (None, Some(_), None) | (None, None, Some(_)) => Err(
            "Invalid arguments for edit_file: `old_string` and `new_string` must be provided \
together (or send `edits` for several changes)."
                .into(),
        ),
        (None, None, None) => Err("Invalid arguments for edit_file: missing `old_string` and \
`new_string` (or send `edits` for several changes)."
            .into()),
    }
}

/// Apply a plan to the file text in memory. Any entry that does not match
/// exactly once (or at least once when `replace_all` is set in a batch) aborts
/// the whole batch, so a partial write can never reach the disk.
pub(crate) fn apply_plan(original: &str, plan: &EditPlan) -> std::result::Result<String, String> {
    let total = plan.ops.len();
    let mut current = original.to_string();
    for (index, op) in plan.ops.iter().enumerate() {
        let count = current.matches(&op.old_string).count();
        if !op.replace_all {
            if count == 0 {
                return Err(if plan.batch {
                    format!(
                        "edits[{}] of {total}: old_string not found in file. Nothing was written; \
re-read the file and fix that entry.",
                        index + 1
                    )
                } else {
                    "old_string not found in file".to_string()
                });
            }
            if count > 1 {
                return Err(if plan.batch {
                    format!(
                        "edits[{}] of {total}: old_string matches {count} times — add surrounding \
context or set replace_all:true. Nothing was written.",
                        index + 1
                    )
                } else {
                    format!(
                        "old_string matches {count} times — use replace_all:true or provide more context"
                    )
                });
            }
            current = current.replacen(&op.old_string, &op.new_string, 1);
        } else {
            // In a batch a replace_all entry must still match something; a zero
            // match means the model's picture of the file is wrong.
            if plan.batch && count == 0 {
                return Err(format!(
                    "edits[{}] of {total}: old_string not found in file. Nothing was written; \
re-read the file and fix that entry.",
                    index + 1
                ));
            }
            current = current.replace(&op.old_string, &op.new_string);
        }
    }
    Ok(current)
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let requested_path = match args.get("path").and_then(Value::as_str) {
        Some(path) => path.to_string(),
        None => {
            return Ok(ToolOutput::err(
                "Invalid arguments for edit_file: missing string `path`.",
            ))
        }
    };
    let plan = match plan_from_args(&args) {
        Ok(plan) => plan,
        Err(message) => return Ok(ToolOutput::err(message)),
    };

    let path = match workspace_path::resolve_existing(&ctx.cwd, &requested_path) {
        Ok(path) => path,
        Err(err) => {
            if let Some(path) = err.path() {
                if let Some(s) = path_sanity::check(path) {
                    return Ok(ToolOutput::err(path_sanity::format_error(
                        &s,
                        path,
                        "edit_file",
                    )));
                }
            }
            return Ok(ToolOutput::err(err.message()));
        }
    };

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
                return Ok(ToolOutput::err(path_sanity::format_error(
                    &s,
                    &path,
                    "edit_file",
                )));
            }
            return Ok(ToolOutput::err(format!(
                "Cannot read {}: {e}",
                path.display()
            )));
        }
    };

    let updated = match apply_plan(&original, &plan) {
        Ok(updated) => updated,
        Err(message) => return Ok(ToolOutput::err(message)),
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

    let diff = unified_diff_for_path(&requested_path, &original, &updated);
    let mut body = format!("Edited {}\n\n```diff\n{diff}```", path.display());
    if plan.change_count() > 1 {
        body = format!(
            "Applied {} changes to {} in this call.\n\n```diff\n{diff}```",
            plan.change_count(),
            path.display()
        );
    }
    // Test-file discipline reminder — see test_path::TEST_MODIFIED_BANNER
    // and the system-prompt TDD section for the contract.
    if test_path::is_test_path(&std::path::Path::new(&requested_path)) {
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
            &ExecCtx::new(cwd.clone(), None),
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

    // M263 R1: one call, several exact replacements in the same file. Before
    // this, the model had to approve one edit_file per change, so it escaped to
    // `python3 - <<'PY' … s.replace(old, new) … PY` and cost 3-8 approvals a task.
    #[tokio::test]
    async fn edit_file_applies_multiple_edits_in_one_call() {
        let cwd = temp_dir();
        let file_path = cwd.join("app.rs");
        std::fs::write(
            &file_path,
            "fn a() { one(); }\nfn b() { two(); }\nfn c() { two(); }\n",
        )
        .expect("seed file");

        let output = execute(
            json!({
                "path": "app.rs",
                "edits": [
                    { "old_string": "one();", "new_string": "first();" },
                    { "old_string": "two();", "new_string": "second();", "replace_all": true },
                    { "old_string": "fn c() {", "new_string": "fn last() {" }
                ]
            }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("edit succeeds");

        let content = std::fs::read_to_string(&file_path).expect("read file");
        let _ = std::fs::remove_dir_all(cwd);

        assert!(!output.is_error, "{}", output.content);
        assert_eq!(
            content,
            "fn a() { first(); }\nfn b() { second(); }\nfn last() { second(); }\n"
        );
        assert!(output.content.contains("Applied 3 changes"));
        assert!(output.content.contains("-fn a() { one(); }"));
        assert!(output.content.contains("+fn a() { first(); }"));
    }

    // M263 R1: a missing target anywhere in the batch writes nothing at all and
    // names the offending entry, so the model can repair just that one.
    #[tokio::test]
    async fn edit_file_multi_edit_is_atomic_and_names_the_missing_entry() {
        let cwd = temp_dir();
        let file_path = cwd.join("app.rs");
        let seeded = "alpha\nbeta\n";
        std::fs::write(&file_path, seeded).expect("seed file");

        let output = execute(
            json!({
                "path": "app.rs",
                "edits": [
                    { "old_string": "alpha", "new_string": "ALPHA" },
                    { "old_string": "gamma", "new_string": "GAMMA" }
                ]
            }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");

        let content = std::fs::read_to_string(&file_path).expect("read file");
        let _ = std::fs::remove_dir_all(cwd);

        assert!(output.is_error, "{}", output.content);
        assert!(
            output.content.contains("edits[2]"),
            "error must name the failing entry: {}",
            output.content
        );
        assert!(output.content.contains("not found"));
        assert_eq!(content, seeded, "a failed batch must not write anything");
    }

    #[tokio::test]
    async fn edit_file_multi_edit_reports_an_ambiguous_entry_and_writes_nothing() {
        let cwd = temp_dir();
        let file_path = cwd.join("app.rs");
        let seeded = "dup\ndup\n";
        std::fs::write(&file_path, seeded).expect("seed file");

        let output = execute(
            json!({
                "path": "app.rs",
                "edits": [{ "old_string": "dup", "new_string": "once" }]
            }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");

        let content = std::fs::read_to_string(&file_path).expect("read file");
        let _ = std::fs::remove_dir_all(cwd);

        assert!(output.is_error, "{}", output.content);
        assert!(output.content.contains("edits[1]"));
        assert!(output.content.contains("2 times"));
        assert_eq!(content, seeded);
    }

    #[tokio::test]
    async fn edit_file_rejects_mixing_single_and_batch_arguments() {
        let cwd = temp_dir();
        let file_path = cwd.join("app.rs");
        let seeded = "alpha\n";
        std::fs::write(&file_path, seeded).expect("seed file");

        let output = execute(
            json!({
                "path": "app.rs",
                "old_string": "alpha",
                "new_string": "ALPHA",
                "edits": [{ "old_string": "alpha", "new_string": "ALPHA" }]
            }),
            &ExecCtx::new(cwd.clone(), None),
        )
        .await
        .expect("tool returns output");

        let content = std::fs::read_to_string(&file_path).expect("read file");
        let _ = std::fs::remove_dir_all(cwd);

        assert!(output.is_error, "{}", output.content);
        assert_eq!(content, seeded);
    }

    #[tokio::test]
    async fn edit_file_rejects_parent_traversal_outside_workspace() {
        let root = temp_dir();
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&cwd).expect("create workspace");
        let outside = root.join("outside.txt");
        std::fs::write(&outside, "old").expect("seed outside file");

        let output = execute(
            json!({
                "path": "../outside.txt",
                "old_string": "old",
                "new_string": "new"
            }),
            &ExecCtx::new(cwd, None),
        )
        .await
        .expect("tool returns output");

        let outside_content = std::fs::read_to_string(&outside).expect("read outside file");
        let _ = std::fs::remove_dir_all(root);

        assert!(output.is_error);
        assert!(output.content.contains("outside the workspace"));
        assert_eq!(outside_content, "old");
    }
}
