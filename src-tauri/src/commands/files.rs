// SPDX-License-Identifier: Apache-2.0
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Option<Vec<FileNode>>,
}

const IGNORED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".venv",
    "venv",
    ".cache",
];

/// List the contents of `path` (up to `depth` levels), confined to `root`.
///
/// `root` is the session's workspace directory; the renderer passes it on
/// every call. `path` must be `root` itself or a descendant — both are
/// canonicalized first, so `..` segments, symlinks, and absolute paths that
/// would escape the workspace are rejected instead of silently enumerated.
/// This is a UI convenience browser only (the `FileTree`), never an agent
/// tool, so confining it to the workspace root is the intended behavior.
#[tauri::command]
pub async fn list_dir(path: String, root: String, depth: u32) -> Result<Vec<FileNode>, AppError> {
    let effective_depth = depth.min(3);
    list_dir_confined(Path::new(&root), Path::new(&path), effective_depth)
}

fn list_dir_confined(root: &Path, path: &Path, depth: u32) -> Result<Vec<FileNode>, AppError> {
    let canon_root = std::fs::canonicalize(root)
        .map_err(|e| AppError::Other(format!("workspace root unavailable: {e}")))?;
    let canon_path = std::fs::canonicalize(path)
        .map_err(|e| AppError::Other(format!("path unavailable: {e}")))?;
    if !canon_path.starts_with(&canon_root) {
        return Err(AppError::Other(
            "path is outside the workspace root".into(),
        ));
    }
    read_dir_recursive(&canon_path, depth)
}

fn read_dir_recursive(path: &Path, depth: u32) -> Result<Vec<FileNode>, AppError> {
    let mut entries = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .collect::<Vec<_>>();

    // Sort: dirs first, then alphabetically by name (case-insensitive)
    entries.sort_by(|a, b| {
        let a_is_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let b_is_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a
                .file_name()
                .to_string_lossy()
                .to_lowercase()
                .cmp(&b.file_name().to_string_lossy().to_lowercase()),
        }
    });

    let mut nodes = Vec::new();
    for entry in entries {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().to_string();

        // Omit every dotfile/dotdir. The tree must never surface `.env` (or any
        // other hidden path) to the renderer; the previous `.env`/`.gitignore`
        // whitelist did exactly that.
        if name.starts_with('.') {
            continue;
        }

        // `file_type()` does not follow symlinks, so a symlinked directory is
        // reported as a non-dir and is never recursed into — the walk cannot
        // leave `root` that way.
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let is_dir = file_type.is_dir();

        if is_dir && IGNORED_DIRS.contains(&name.as_str()) {
            continue;
        }

        let child_path = entry.path();
        let abs_path = child_path.to_string_lossy().to_string();

        let children = if is_dir && depth > 0 {
            Some(read_dir_recursive(&child_path, depth - 1).unwrap_or_default())
        } else if is_dir {
            Some(Vec::new())
        } else {
            None
        };

        nodes.push(FileNode {
            name,
            path: abs_path,
            is_dir,
            children,
        });
    }

    Ok(nodes)
}

// ── Chat attachments ─────────────────────────────────────────────────────────

/// Saved attachment metadata returned to the UI so it can render a preview
/// and build the markdown link that gets embedded in the outgoing message.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SavedAttachment {
    /// Absolute path on disk, used for the `file://` markdown link.
    pub path: String,
    /// Filename only — what the user sees in the chip.
    pub name: String,
    pub size_bytes: usize,
}

/// Save a base64-encoded blob (typically a screenshot or dropped image)
/// under `<cwd>/.codefactory/attachments/{epoch}-{rand}.{ext}` and return
/// the new path. Idempotent on directory creation.
///
/// We intentionally do NOT inline the bytes into the message string —
/// every base64 KB would become hundreds of model tokens. The message
/// embeds a `file://` markdown link instead; vision-aware model routing
/// is a follow-up PR.
#[tauri::command]
pub async fn save_chat_attachment(
    cwd: String,
    filename: String,
    data_base64: String,
) -> Result<SavedAttachment, String> {
    use base64::{engine::general_purpose, Engine as _};
    use std::path::PathBuf;

    let dir = PathBuf::from(&cwd).join(".codefactory").join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir failed: {e}"))?;

    // Decode + sanitize filename. We keep only the extension from the
    // caller; the basename is regenerated so user-supplied names can't
    // path-traverse out of the attachments dir.
    let bytes = general_purpose::STANDARD
        .decode(&data_base64)
        .map_err(|e| format!("base64 decode failed: {e}"))?;
    let ext = PathBuf::from(&filename)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
        .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "bin".into());
    let epoch = chrono::Utc::now().timestamp_millis();
    let rand = uuid::Uuid::new_v4().simple().to_string();
    let safe_name = format!("{epoch}-{}.{ext}", &rand[..8]);
    let path = dir.join(&safe_name);

    std::fs::write(&path, &bytes).map_err(|e| format!("write failed: {e}"))?;

    Ok(SavedAttachment {
        path: path.to_string_lossy().to_string(),
        name: safe_name,
        size_bytes: bytes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn names(nodes: &[FileNode]) -> Vec<String> {
        nodes.iter().map(|n| n.name.clone()).collect()
    }

    #[test]
    fn lists_workspace_children() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("main.rs"), "x").unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        let nodes = list_dir_confined(root.path(), root.path(), 1).unwrap();
        let n = names(&nodes);
        assert!(n.contains(&"main.rs".to_string()), "{n:?}");
        assert!(n.contains(&"src".to_string()), "{n:?}");
    }

    #[test]
    fn omits_dotfiles_including_env() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join(".env"), "SECRET=1").unwrap();
        fs::write(root.path().join(".gitignore"), "target").unwrap();
        fs::create_dir(root.path().join(".hidden")).unwrap();
        fs::write(root.path().join("visible.txt"), "ok").unwrap();
        let nodes = list_dir_confined(root.path(), root.path(), 1).unwrap();
        assert_eq!(
            names(&nodes),
            vec!["visible.txt".to_string()],
            "only non-dotfiles may surface"
        );
    }

    #[test]
    fn rejects_path_outside_root() {
        let root = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        fs::write(other.path().join("secret.txt"), "x").unwrap();
        assert!(
            list_dir_confined(root.path(), other.path(), 1).is_err(),
            "a sibling directory must be rejected, not enumerated"
        );
    }

    #[test]
    fn rejects_dotdot_escape() {
        let root = TempDir::new().unwrap();
        // `root/..` canonicalizes to the parent — outside the workspace.
        let escape = root.path().join("..");
        assert!(
            list_dir_confined(root.path(), &escape, 1).is_err(),
            "the parent of root must be rejected"
        );
    }

    #[test]
    fn allows_descendant_path() {
        let root = TempDir::new().unwrap();
        let sub = root.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("child.rs"), "x").unwrap();
        let nodes = list_dir_confined(root.path(), &sub, 1).unwrap();
        assert_eq!(names(&nodes), vec!["child.rs".to_string()]);
    }

    #[tokio::test]
    async fn command_confines_and_rejects_outside_root() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("ok.txt"), "x").unwrap();
        let root_s = root.path().to_string_lossy().to_string();

        // In-root call succeeds through the public command entry point.
        let ok = list_dir(root_s.clone(), root_s.clone(), 99).await.unwrap();
        assert_eq!(names(&ok), vec!["ok.txt".to_string()]);

        // A path outside the workspace root is rejected, not enumerated.
        let outside = TempDir::new().unwrap();
        let bad = list_dir(
            outside.path().to_string_lossy().to_string(),
            root_s,
            1,
        )
        .await;
        assert!(bad.is_err());
    }
}
