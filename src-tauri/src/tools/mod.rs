// SPDX-License-Identifier: Apache-2.0
pub mod bash;
pub mod delivery;
pub mod delegate_tasks;
pub mod docx;
pub mod edit;
pub mod file_lock;
pub mod glob;
pub mod grep;
pub mod knowledge;
pub mod path_sanity;
pub mod pptx;
pub mod pptx_edit;
pub mod read;
pub mod shell_policy;
pub mod skill_mgmt;
pub mod test_path;
pub mod workspace_path;
pub mod write;
pub mod xlsx;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

use crate::errors::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

pub struct ExecCtx {
    pub cwd: PathBuf,
    pub app: Option<tauri::AppHandle>,
    pub db: Option<sqlx::SqlitePool>,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub knowledge_library_ids: Option<Vec<String>>,
    /// A snapshot of settings for tools that need policy/config (the delivery
    /// tool reads the configured delivery ceiling + git remote tokens). None
    /// in contexts that don't supply it.
    pub settings: Option<crate::config::settings::Settings>,
}

#[cfg(test)]
impl ExecCtx {
    pub fn new(cwd: PathBuf, db: Option<sqlx::SqlitePool>) -> Self {
        Self {
            cwd,
            app: None,
            db,
            session_id: None,
            task_id: None,
            knowledge_library_ids: None,
            settings: None,
        }
    }
}

pub fn all_definitions() -> Vec<crate::openrouter::types::ToolDefinition> {
    vec![
        read::definition(),
        write::definition(),
        edit::definition(),
        glob::definition(),
        grep::definition(),
        knowledge::search_definition(),
        knowledge::get_chunk_definition(),
        bash::definition(),
        pptx::definition(),
        pptx_edit::read_definition(),
        pptx_edit::edit_definition(),
        pptx_edit::format_definition(),
        docx::definition(),
        skill_mgmt::create_definition(),
        skill_mgmt::update_definition(),
        skill_mgmt::list_definition(),
        skill_mgmt::delete_definition(),
        skill_mgmt::search_definition(),
        skill_mgmt::fetch_definition(),
        xlsx::read_definition(),
        xlsx::edit_definition(),
        delegate_tasks::definition(),
        delivery::definition(),
    ]
}

pub async fn dispatch(name: &str, args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    match name {
        "read_file" => read::execute(args, ctx).await,
        "write_file" => write::execute(args, ctx).await,
        "edit_file" => edit::execute(args, ctx).await,
        "glob" => glob::execute(args, ctx).await,
        "grep" => grep::execute(args, ctx).await,
        "kb_search" => knowledge::execute_search(args, ctx).await,
        "kb_get_chunk" => knowledge::execute_get_chunk(args, ctx).await,
        "bash" => bash::execute(args, ctx).await,
        "write_pptx" => pptx::execute(args, ctx).await,
        "read_pptx" => pptx_edit::execute_read(args, ctx).await,
        "edit_pptx" => pptx_edit::execute_edit(args, ctx).await,
        "format_pptx" => pptx_edit::execute_format(args, ctx).await,
        "write_docx" => docx::execute(args, ctx).await,
        "skill_create" => skill_mgmt::execute_create(args, ctx).await,
        "skill_update" => skill_mgmt::execute_update(args, ctx).await,
        "skill_list" => skill_mgmt::execute_list(args, ctx).await,
        "skill_delete" => skill_mgmt::execute_delete(args, ctx).await,
        "skill_search" => skill_mgmt::execute_search(args, ctx).await,
        "skill_fetch" => skill_mgmt::execute_fetch(args, ctx).await,
        "read_xlsx" => xlsx::execute_read(args, ctx).await,
        "edit_xlsx" => xlsx::execute_edit(args, ctx).await,
        "delegate_tasks" => delegate_tasks::execute(args, ctx).await,
        "deliver_changes" => delivery::execute(args, ctx).await,
        other => Ok(ToolOutput::err(format!("Unknown tool: {other}"))),
    }
}

pub fn unified_diff_for_path(path: &str, before: &str, after: &str) -> String {
    let display_path = path.replace('\\', "/");
    let old_lines: Vec<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().collect();

    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0;
    while suffix < old_lines.len().saturating_sub(prefix)
        && suffix < new_lines.len().saturating_sub(prefix)
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let context = 3;
    let old_context_start = prefix.saturating_sub(context);
    let new_context_start = prefix.saturating_sub(context);
    let old_changed_end = old_lines.len().saturating_sub(suffix);
    let new_changed_end = new_lines.len().saturating_sub(suffix);
    let suffix_context = suffix.min(context);
    let old_context_end = old_changed_end + suffix_context;
    let new_context_end = new_changed_end + suffix_context;
    let old_count = old_context_end.saturating_sub(old_context_start);
    let new_count = new_context_end.saturating_sub(new_context_start);
    let old_start = if old_count == 0 {
        0
    } else {
        old_context_start + 1
    };
    let new_start = if new_count == 0 {
        0
    } else {
        new_context_start + 1
    };

    let mut diff = String::new();
    diff.push_str(&format!("--- a/{display_path}\n"));
    diff.push_str(&format!("+++ b/{display_path}\n"));
    diff.push_str(&format!(
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
    ));

    for line in &old_lines[old_context_start..prefix] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &old_lines[prefix..old_changed_end] {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &new_lines[prefix..new_changed_end] {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &old_lines[old_lines.len().saturating_sub(suffix)..old_context_end] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }

    diff
}
