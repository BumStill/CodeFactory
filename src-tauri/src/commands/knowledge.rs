// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use tauri::State;

use crate::errors::AppError;
use crate::knowledge::{
    KnowledgeLibrary, KnowledgeScanSummary, KnowledgeSearchQuery, KnowledgeSearchResult,
};
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
    let pool = state.db.read().await;
    crate::knowledge::add_library(&pool, request.name, request.root_path).await
}

#[tauri::command]
pub async fn list_knowledge_libraries(
    state: State<'_, AppState>,
) -> Result<Vec<KnowledgeLibrary>, AppError> {
    let pool = state.db.read().await;
    crate::knowledge::list_libraries(&pool).await
}

#[tauri::command]
pub async fn scan_knowledge_library(
    library_id: String,
    state: State<'_, AppState>,
) -> Result<KnowledgeScanSummary, AppError> {
    let pool = state.db.read().await;
    crate::knowledge::scan_library(&pool, &library_id).await
}

#[tauri::command]
pub async fn search_knowledge(
    query: KnowledgeSearchQuery,
    state: State<'_, AppState>,
) -> Result<Vec<KnowledgeSearchResult>, AppError> {
    let pool = state.db.read().await;
    crate::knowledge::search(&pool, query).await
}
