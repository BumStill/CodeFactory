// SPDX-License-Identifier: Apache-2.0
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

#[tauri::command]
pub async fn list_dir(path: String, depth: u32) -> Result<Vec<FileNode>, AppError> {
    let effective_depth = depth.min(3);
    let nodes = read_dir_recursive(&path, effective_depth)?;
    Ok(nodes)
}

fn read_dir_recursive(path: &str, depth: u32) -> Result<Vec<FileNode>, AppError> {
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
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let is_dir = file_type.is_dir();

        // Skip hidden files/dirs (starting with .) and ignored dirs
        if name.starts_with('.') && name != ".gitignore" && name != ".env" {
            if is_dir {
                continue;
            }
        }
        if is_dir && IGNORED_DIRS.contains(&name.as_str()) {
            continue;
        }

        let abs_path = entry.path().to_string_lossy().to_string();

        let children = if is_dir && depth > 0 {
            Some(read_dir_recursive(&abs_path, depth - 1).unwrap_or_default())
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
