// SPDX-License-Identifier: Apache-2.0
//! Project memory — a per-repo plain-text file that gets prepended to the
//! system prompt every time the agent runs in that cwd. Lets the user
//! teach the AI about their codebase once and have it stick across
//! sessions.
//!
//! Conventions:
//!   - Lives at `<cwd>/.codefactory/memory.md`
//!   - Plain markdown, no special syntax required
//!   - UI's "Remember" button appends new facts here
//!   - Legacy top-level `CODEFACTORY.md` still read for back-compat
//!     (see agent/mod.rs build_system_prompt)
//!
//! These commands are deliberately minimal: read whole file, append a
//! line. Editing existing content goes through the user's editor — we
//! don't try to be a text editor on top of a text editor.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::command;

use crate::errors::AppError;

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectMemory {
    pub path: String,
    pub content: String,
    pub exists: bool,
}

fn memory_path(cwd: &Path) -> PathBuf {
    cwd.join(".codefactory").join("memory.md")
}

#[command]
pub async fn read_project_memory(cwd: String) -> Result<ProjectMemory, AppError> {
    let path = memory_path(Path::new(&cwd));
    let exists = path.exists();
    let content = if exists {
        tokio::fs::read_to_string(&path).await.unwrap_or_default()
    } else {
        String::new()
    };
    Ok(ProjectMemory {
        path: path.to_string_lossy().into_owned(),
        content,
        exists,
    })
}

/// Append a fact to the project memory file, creating the directory and
/// file if needed. Each entry is dated and separated by a blank line so
/// the file stays readable as it grows.
/// Overwrite the project memory file with arbitrary content. Used by the
/// Profile page where the user edits their full memory as one document.
/// Unlike `append_project_memory`, no header is prepended — whatever the
/// caller passes is exactly what lands on disk.
#[command]
pub async fn write_project_memory(cwd: String, content: String) -> Result<ProjectMemory, AppError> {
    let path = memory_path(Path::new(&cwd));
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, &content).await?;
    Ok(ProjectMemory {
        path: path.to_string_lossy().into_owned(),
        content,
        exists: true,
    })
}

#[command]
pub async fn append_project_memory(cwd: String, fact: String) -> Result<ProjectMemory, AppError> {
    let fact = fact.trim();
    if fact.is_empty() {
        return Err(AppError::Other("Cannot save empty fact to memory".into()));
    }

    let path = memory_path(Path::new(&cwd));
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let existed = path.exists();
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let separator = if existed { "\n\n" } else { "" };
    let header_needed = !existed;

    let mut to_write = String::new();
    if header_needed {
        to_write.push_str("# Project memory\n\n");
        to_write.push_str("Auto-injected into every chat session in this repo.\n");
        to_write.push_str("Use the Remember button in the chat UI to add new entries.\n");
        to_write.push_str("\n");
    }
    to_write.push_str(&format!("{separator}- ({date}) {fact}"));

    // Read existing, append. Simpler than open-in-append because we want
    // the optional file-header creation, and concurrent writes are
    // already serialised by the underlying tokio fs runtime.
    let existing = if existed {
        tokio::fs::read_to_string(&path).await.unwrap_or_default()
    } else {
        String::new()
    };
    let combined = format!("{existing}{to_write}");
    tokio::fs::write(&path, &combined).await?;

    Ok(ProjectMemory {
        path: path.to_string_lossy().into_owned(),
        content: combined,
        exists: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_cwd() -> PathBuf {
        let p = std::env::temp_dir().join(format!("cf-mem-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[tokio::test]
    async fn read_returns_empty_when_missing() {
        let cwd = tmp_cwd();
        let m = read_project_memory(cwd.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(!m.exists);
        assert_eq!(m.content, "");
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[tokio::test]
    async fn append_creates_file_with_header_and_entry() {
        let cwd = tmp_cwd();
        let cwd_s = cwd.to_string_lossy().into_owned();
        let m = append_project_memory(cwd_s.clone(), "this project uses pnpm not npm".into())
            .await
            .unwrap();
        assert!(m.content.starts_with("# Project memory"));
        assert!(m.content.contains("this project uses pnpm not npm"));

        // Second append doesn't duplicate the header but adds a separator
        let m2 = append_project_memory(cwd_s, "models live under src/models/".into())
            .await
            .unwrap();
        let header_count = m2.content.matches("# Project memory").count();
        assert_eq!(header_count, 1, "header should appear exactly once");
        assert!(m2.content.contains("models live under src/models/"));
        // Two entries (with separator) means at least two "- (" date prefixes
        let entry_count = m2.content.matches("- (").count();
        assert_eq!(entry_count, 2);
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[tokio::test]
    async fn rejects_empty_fact() {
        let cwd = tmp_cwd();
        let cwd_s = cwd.to_string_lossy().into_owned();
        let r = append_project_memory(cwd_s, "   \n\t".into()).await;
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(cwd);
    }
}
