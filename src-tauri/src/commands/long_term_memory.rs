// SPDX-License-Identifier: Apache-2.0
//! M235: the Settings memory page and `--dispatch` read and change memories
//! through these same commands.

use serde::Serialize;
use tauri::{command, State};

use crate::agent::long_term_memory::{self, MemoryRecord};
use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct MemoryEntry {
    #[serde(flatten)]
    pub record: MemoryRecord,
    pub source_text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MemoryOverview {
    pub enabled: bool,
    pub memories: Vec<MemoryEntry>,
}

#[command]
pub async fn list_long_term_memories(
    state: State<'_, AppState>,
) -> Result<MemoryOverview, AppError> {
    let pool = state.db.read().await;
    let enabled = long_term_memory::is_enabled(&pool).await?;
    let mut memories = Vec::new();
    for record in long_term_memory::list_all(&pool).await? {
        let source_text = long_term_memory::source_text(&pool, &record).await?;
        memories.push(MemoryEntry { record, source_text });
    }
    Ok(MemoryOverview { enabled, memories })
}

#[command]
pub async fn edit_long_term_memory(
    id: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await;
    long_term_memory::edit(&pool, &id, &content)
        .await
        .map_err(AppError::Other)
}

#[command]
pub async fn delete_long_term_memory(
    id: String,
    state: State<'_, AppState>,
) -> Result<u64, AppError> {
    let pool = state.db.read().await;
    Ok(long_term_memory::delete(&pool, &id).await?)
}

#[command]
pub async fn set_long_term_memory_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let pool = state.db.read().await;
    Ok(long_term_memory::set_enabled(&pool, enabled).await?)
}
