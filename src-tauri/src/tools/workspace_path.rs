// SPDX-License-Identifier: Apache-2.0
use std::io;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum WorkspacePathError {
    Outside {
        requested: String,
        workspace: PathBuf,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl WorkspacePathError {
    pub fn message(&self) -> String {
        match self {
            Self::Outside {
                requested,
                workspace,
            } => format!(
                "Path '{}' is outside the workspace '{}'",
                requested,
                workspace.display()
            ),
            Self::Io { path, source } => format!("Cannot access {}: {source}", path.display()),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Io { path, .. } => Some(path),
            Self::Outside { .. } => None,
        }
    }
}

pub fn resolve_existing(cwd: &Path, requested: &str) -> Result<PathBuf, WorkspacePathError> {
    let workspace = canonicalize_workspace(cwd)?;
    let candidate = candidate_path(&workspace, requested);

    if !is_inside_workspace(&candidate, &workspace) {
        return Err(outside(requested, workspace));
    }

    let canonical = candidate
        .canonicalize()
        .map_err(|source| WorkspacePathError::Io {
            path: candidate.clone(),
            source,
        })?;

    if !is_inside_workspace(&canonical, &workspace) {
        return Err(outside(requested, workspace));
    }

    Ok(canonical)
}

pub fn resolve_writable(cwd: &Path, requested: &str) -> Result<PathBuf, WorkspacePathError> {
    let workspace = canonicalize_workspace(cwd)?;
    let candidate = candidate_path(&workspace, requested);

    if !is_inside_workspace(&candidate, &workspace) {
        return Err(outside(requested, workspace));
    }

    if candidate.exists() {
        let canonical = candidate
            .canonicalize()
            .map_err(|source| WorkspacePathError::Io {
                path: candidate.clone(),
                source,
            })?;
        if !is_inside_workspace(&canonical, &workspace) {
            return Err(outside(requested, workspace));
        }
        return Ok(canonical);
    }

    let mut ancestor = candidate.parent().map(Path::to_path_buf);
    while let Some(path) = ancestor {
        if path.exists() {
            let canonical = path
                .canonicalize()
                .map_err(|source| WorkspacePathError::Io {
                    path: path.clone(),
                    source,
                })?;
            if !is_inside_workspace(&canonical, &workspace) {
                return Err(outside(requested, workspace));
            }
            return Ok(candidate);
        }
        ancestor = path.parent().map(Path::to_path_buf);
    }

    Err(WorkspacePathError::Io {
        path: candidate,
        source: io::Error::new(io::ErrorKind::NotFound, "no existing parent directory"),
    })
}

fn canonicalize_workspace(cwd: &Path) -> Result<PathBuf, WorkspacePathError> {
    cwd.canonicalize().map_err(|source| WorkspacePathError::Io {
        path: cwd.to_path_buf(),
        source,
    })
}

fn candidate_path(workspace: &Path, requested: &str) -> PathBuf {
    let requested = Path::new(requested);
    let absolute = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        workspace.join(requested)
    };
    normalize_lexically(&absolute)
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn is_inside_workspace(path: &Path, workspace: &Path) -> bool {
    path.starts_with(workspace)
}

fn outside(requested: &str, workspace: PathBuf) -> WorkspacePathError {
    WorkspacePathError::Outside {
        requested: requested.to_string(),
        workspace,
    }
}
