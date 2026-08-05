// SPDX-License-Identifier: Apache-2.0
use std::path::{Path, PathBuf};
use serde::Serialize;
use crate::errors::AppError;

#[derive(Debug, Clone, Serialize)]
pub struct DocumentPreview { pub path: String, pub relative_path: String, pub name: String, pub extension: String, pub content: String, pub truncated: bool }

fn resolve_document_path(cwd: &Path, requested: &str) -> Result<(PathBuf, String), AppError> {
    let raw = requested.strip_prefix("file://").unwrap_or(requested);
    let candidate = { let path = Path::new(raw); if path.is_absolute() { path.to_path_buf() } else { cwd.join(path) } };
    let root = std::fs::canonicalize(cwd).map_err(|e| AppError::Other(format!("workspace root unavailable: {e}")))?;
    let resolved = std::fs::canonicalize(&candidate).map_err(|e| AppError::Other(format!("document unavailable: {e}")))?;
    if !resolved.starts_with(&root) { return Err(AppError::Other("document is outside the workspace root".into())); }
    if !resolved.is_file() { return Err(AppError::Other("document is not a file".into())); }
    let relative = resolved.strip_prefix(&root).unwrap_or(&resolved).to_string_lossy().replace('\\', "/");
    Ok((resolved, relative))
}

#[tauri::command]
pub async fn read_document(cwd: String, path: String) -> Result<DocumentPreview, AppError> {
    let (resolved, relative_path) = resolve_document_path(Path::new(&cwd), &path)?;
    let bytes = tokio::fs::read(&resolved).await?;
    const MAX_PREVIEW_BYTES: usize = 512 * 1024;
    let truncated = bytes.len() > MAX_PREVIEW_BYTES;
    let content = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PREVIEW_BYTES)]).into_owned();
    let name = resolved.file_name().and_then(|v| v.to_str()).unwrap_or("文档").to_string();
    let extension = resolved.extension().and_then(|v| v.to_str()).unwrap_or("txt").to_ascii_lowercase();
    Ok(DocumentPreview { path: resolved.to_string_lossy().into_owned(), relative_path, name, extension, content, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[test]
    fn resolves_relative_document_inside_workspace() {
        let root = TempDir::new().unwrap(); std::fs::create_dir(root.path().join("docs")).unwrap(); std::fs::write(root.path().join("docs/plan.md"), "# Plan").unwrap();
        let (path, relative) = resolve_document_path(root.path(), "docs/plan.md").unwrap(); assert!(path.ends_with("docs/plan.md")); assert_eq!(relative, "docs/plan.md");
    }
    #[test]
    fn rejects_documents_outside_workspace() {
        let root = TempDir::new().unwrap(); let outside = TempDir::new().unwrap(); let path = outside.path().join("secret.md"); std::fs::write(&path, "secret").unwrap();
        let error = resolve_document_path(root.path(), path.to_str().unwrap()).unwrap_err(); assert!(error.to_string().contains("outside"));
    }
}
