// SPDX-License-Identifier: Apache-2.0

pub(crate) use crate::commands::liveness_store::LivenessMetrics;

/// R8 read-only implementation. The public desktop entry is the existing
/// `get_objective_health` command, which includes this aggregate without adding
/// a second command-registration surface.
pub async fn query_session_liveness_metrics(
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
