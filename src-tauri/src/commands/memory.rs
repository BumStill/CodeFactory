// SPDX-License-Identifier: Apache-2.0
//! Project memory — a per-repo plain-text file that gets prepended to the
//! system prompt every time the agent runs in that cwd. Lets the user
//! teach the AI about their codebase once and have it stick across
//! sessions.
//!
//! Conventions:
//!   - Lives at `<cwd>/.codefactory/memory.md`
//!   - Plain markdown, no special syntax required
//!   - Automatic learning appends safe, stable facts here; Profile edits it
//!   - Legacy top-level `CODEFACTORY.md` still read for back-compat
//!     (see agent/mod.rs build_system_prompt)
//!
//! These commands are deliberately minimal: read whole file, append a
//! line. Editing existing content goes through the user's editor — we
//! don't try to be a text editor on top of a text editor.

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

/// Overwrite the project memory file with arbitrary content. Used by the
/// Profile page where the user edits their full memory as one document.
/// No header is prepended — whatever the caller passes is exactly what
/// lands on disk.
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

}
