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
