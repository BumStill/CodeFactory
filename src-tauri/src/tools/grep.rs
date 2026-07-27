// SPDX-License-Identifier: Apache-2.0
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use walkdir::WalkDir;

use super::{workspace_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

const MAX_RESULTS: usize = 500;
const MAX_MATCHING_LINE_CHARS: usize = 4_000;
const MAX_OUTPUT_CHARS: usize = 64_000;

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
            description: "Search for a regex pattern in files. Returns bounded matching excerpts with file:line context.".into(),
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
    let mut output_chars = 0_usize;

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
            if let Some(matched) = re.find(line) {
                let excerpt = matching_line_excerpt(line, matched.start(), matched.end());
                let result = format!("{}:{}: {}", rel.display(), i + 1, excerpt);
                let result_chars = result.chars().count();
                let separator_chars = usize::from(!results.is_empty());
                if output_chars + separator_chars + result_chars > MAX_OUTPUT_CHARS {
                    results.push(format!(
                        "[truncated at {MAX_OUTPUT_CHARS} output characters]"
                    ));
                    break 'outer;
                }
                output_chars += separator_chars + result_chars;
                results.push(result);
                if results.len() >= MAX_RESULTS {
                    results.push(format!("[truncated at {MAX_RESULTS} results]"));
                    break 'outer;
                }
            }
        }
    }

    Ok(ToolOutput::ok(results.join("\n")))
}

fn matching_line_excerpt(line: &str, match_start: usize, match_end: usize) -> String {
    let total_chars = line.chars().count();
    if total_chars <= MAX_MATCHING_LINE_CHARS {
        return line.to_owned();
    }

    let match_start_chars = line[..match_start].chars().count();
    let match_chars = line[match_start..match_end].chars().count();
    if match_chars >= MAX_MATCHING_LINE_CHARS {
        let matched = &line[match_start..match_end];
        let head_chars = MAX_MATCHING_LINE_CHARS / 2;
        let tail_chars = MAX_MATCHING_LINE_CHARS - head_chars;
        let head_end = byte_index_at_char(matched, head_chars);
        let tail_start = byte_index_at_char(matched, match_chars - tail_chars);
        return format!(
            "{}…[matching text truncated]…{}",
            &matched[..head_end],
            &matched[tail_start..],
        );
    }
    let context_budget = MAX_MATCHING_LINE_CHARS.saturating_sub(match_chars);
    let before_chars = context_budget / 2;
    let after_chars = context_budget.saturating_sub(before_chars);
    let excerpt_start_chars = match_start_chars.saturating_sub(before_chars);
    let excerpt_end_chars = (match_start_chars + match_chars + after_chars).min(total_chars);
    let excerpt_start = byte_index_at_char(line, excerpt_start_chars);
    let excerpt_end = byte_index_at_char(line, excerpt_end_chars);

    let before_marker = if excerpt_start_chars > 0 {
        "…[line truncated before match]…"
    } else {
        ""
    };
    let after_marker = if excerpt_end_chars < total_chars {
        "…[line truncated after match]…"
    } else {
        ""
    };
    format!(
        "{before_marker}{}{after_marker}",
        &line[excerpt_start..excerpt_end]
    )
}

fn byte_index_at_char(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(value.len())
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
            &ExecCtx::new(workspace, None),
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(root);

        assert!(output.is_error);
        assert!(output.content.contains("outside the workspace"));
    }

    #[tokio::test]
    async fn grep_bounds_a_matching_minified_line_and_preserves_the_match() {
        let root =
            std::env::temp_dir().join(format!("codefactory-grep-minified-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create workspace");
        let minified = format!(
            "{}RememberButton{}",
            "a".repeat(1_150_000),
            "z".repeat(1_150_000),
        );
        std::fs::write(root.join("bundle.js"), minified).expect("seed minified asset");

        let output = execute(
            json!({
                "pattern": "RememberButton"
            }),
            &ExecCtx::new(root.clone(), None),
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(root);

        assert!(!output.is_error);
        assert!(output.content.contains("RememberButton"));
        assert!(
            output.content.contains("line truncated"),
            "the model must know the matching line was shortened",
        );
        assert!(
            output.content.chars().count() <= 70_000,
            "a single minified line must not create a multi-megabyte tool result",
        );
    }

    #[tokio::test]
    async fn grep_bounds_total_output_and_marks_truncation() {
        let root =
            std::env::temp_dir().join(format!("codefactory-grep-total-cap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create workspace");
        let content = (0..100)
            .map(|i| format!("needle-{i}-{}", "x".repeat(1_000)))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(root.join("many.txt"), content).expect("seed matching lines");

        let output = execute(
            json!({
                "pattern": "needle-"
            }),
            &ExecCtx::new(root.clone(), None),
        )
        .await
        .expect("tool returns output");

        let _ = std::fs::remove_dir_all(root);

        assert!(!output.is_error);
        assert!(output
            .content
            .contains("truncated at 64000 output characters"));
        assert!(
            output.content.chars().count() <= MAX_OUTPUT_CHARS + 100,
            "the total grep response must stay close to the declared hard budget",
        );
        assert!(
            output.content.matches("many.txt:").count() < 100,
            "the tool must stop scanning after the total output budget is exhausted",
        );
    }
}
