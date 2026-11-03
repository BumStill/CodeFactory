// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use tauri::State;

use crate::errors::AppError;
use crate::knowledge::{KnowledgeLibrary, KnowledgeScanSummary};
use crate::AppState;

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterKnowledgeLibraryRequest {
    pub name: String,
    pub root_path: String,
}

#[tauri::command]
pub async fn register_knowledge_library(
    request: RegisterKnowledgeLibraryRequest,
    state: State<'_, AppState>,
) -> Result<KnowledgeLibrary, AppError> {
    let pool = state.db.read().await.clone();
    let library = crate::knowledge::add_library(&pool, request.name, request.root_path).await?;
    // M191: the index starts building on registration — the user never has to
    // press "scan". Watchers are armed here too so later edits are picked up.
    crate::knowledge_watch::refresh(&pool).await;
    crate::knowledge_watch::spawn_scan(pool.clone(), library.id.clone());
    crate::knowledge::library_by_id_public(&pool, &library.id).await
}

#[tauri::command]
pub async fn list_knowledge_libraries(
    state: State<'_, AppState>,
) -> Result<Vec<KnowledgeLibrary>, AppError> {
    let pool = state.db.read().await.clone();
    crate::knowledge::list_libraries(&pool).await
}

#[tauri::command]
pub async fn scan_knowledge_library(
    library_id: String,
    state: State<'_, AppState>,
) -> Result<KnowledgeScanSummary, AppError> {
    let pool = state.db.read().await.clone();
    crate::knowledge::scan_library(&pool, &library_id).await
}

#[tauri::command]
pub async fn set_knowledge_library_enabled(
    library_id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await.clone();
    crate::knowledge::set_library_enabled(&pool, &library_id, enabled).await?;
    crate::knowledge_watch::refresh(&pool).await;
    if enabled {
        // Re-enabling a library rebuilds anything that changed while it was off.
        crate::knowledge_watch::spawn_scan(pool.clone(), library_id);
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_knowledge_library(
    library_id: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await.clone();
    crate::knowledge::delete_library(&pool, &library_id).await?;
    crate::knowledge_watch::refresh(&pool).await;
    Ok(())
}

/// Which files in a library currently fail to index, and why (M191 R6).
#[tauri::command]
pub async fn list_knowledge_library_failures(
    library_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<crate::knowledge::KnowledgeDocumentFailure>, AppError> {
    let pool = state.db.read().await.clone();
    crate::knowledge::library_failures(&pool, &library_id).await
}
