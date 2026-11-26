// SPDX-License-Identifier: Apache-2.0

use tauri::State;

use crate::AppState;

pub(crate) use crate::commands::liveness_store::LivenessMetrics;

/// M287 R8/M287b R3 read-only desktop command. It exposes aggregate events
/// and the in-process supervisor health signal without session content.
#[tauri::command]
pub async fn query_session_liveness_metrics(
    state: State<'_, AppState>,
    hours: Option<i64>,
) -> Result<LivenessMetrics, String> {
    let pool = state.db.read().await.clone();
    query_session_liveness_metrics_from_pool(&pool, hours).await
}

pub async fn query_session_liveness_metrics_from_pool(
    pool: &sqlx::SqlitePool,
    hours: Option<i64>,
) -> Result<LivenessMetrics, String> {
    crate::commands::liveness_store::metrics(
        pool,
        hours.unwrap_or(24),
        chrono::Utc::now().timestamp_millis(),
    )
    .await
    .map_err(|error| format!("读取会话活性指标失败：{error}"))
}
