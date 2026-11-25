// SPDX-License-Identifier: Apache-2.0

pub(crate) use crate::agent::liveness_store::LivenessMetrics;

#[tauri::command]
pub async fn session_liveness_metrics(
    hours: Option<i64>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<LivenessMetrics, String> {
    crate::agent::liveness_store::metrics(
        &state.db,
        hours.unwrap_or(24),
        chrono::Utc::now().timestamp_millis(),
    )
    .await
    .map_err(|error| format!("读取会话活性指标失败：{error}"))
}
