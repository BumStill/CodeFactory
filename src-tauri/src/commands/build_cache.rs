// SPDX-License-Identifier: Apache-2.0
//! CF-BLD-R4: build-cache occupancy that a user can see, and one action that
//! cleans it up.
//!
//! Both commands are deliberately thin: the decisions live in
//! `crate::build_cache` and `agent::execution_workspace::cache_ownership`, so
//! the panel, the menu entry and the supervisor all run the exact same code
//! path rather than three re-implementations of "which cache may I delete".

use tauri::{AppHandle, Manager};

use crate::build_cache::{self, BuildCacheReport, MaintenanceOutcome};
use crate::errors::AppError;
use crate::AppState;

/// The container that holds one directory per managed task.
pub(crate) fn workspace_container(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("resolve app data directory: {error}")))?;
    Ok(data.join("execution-workspaces"))
}

/// CF-BLD-R4: how much the task caches and the build cache take, and whether
/// heavy builds are running or waiting for a slot.
#[tauri::command]
pub async fn build_cache_report(app: AppHandle) -> Result<BuildCacheReport, AppError> {
    let container = workspace_container(&app)?;
    Ok(build_cache::current_report(&container, &build_cache::heavy_build_limiter()).await)
}

/// CF-BLD-R4: one action that reclaims what is safe to reclaim — caches of
/// tasks that already ended, then the long-orphaned backlog, then the ceiling.
/// A cache with a live builder, or one whose task can still continue, survives.
#[tauri::command]
pub async fn build_cache_cleanup(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<MaintenanceOutcome, AppError> {
    let container = workspace_container(&app)?;
    let pool = state.db.read().await.clone();
    let (ended_owners, orphan_owners, protected_owners) =
        crate::agent::execution_workspace::cache_ownership(&pool, &container)
            .await
            .map_err(|error| AppError::Other(error.to_string()))?;
    let report = tauri::async_runtime::spawn_blocking(move || {
        let log = build_cache::AuditLog::new(build_cache::audit_log_path(&container));
        build_cache::startup_sweep(
            &container,
            &ended_owners,
            &orphan_owners,
            &protected_owners,
            Some(&log),
        )
    })
    .await
    .map_err(|error| AppError::Other(format!("build cache cleanup join failed: {error}")))?;
    Ok(report)
}
