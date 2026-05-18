// SPDX-License-Identifier: Apache-2.0
//! Token-cost tracking commands.
//!
//! Default pricing ($/1M tokens, can be extended to per-model config later):
//!   input  = $1.00 / 1M
//!   output = $3.00 / 1M

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::errors::AppError;

const INPUT_PRICE_PER_M: f64 = 1.0;
const OUTPUT_PRICE_PER_M: f64 = 3.0;

fn estimate_cost(input_tokens: i64, output_tokens: i64) -> f64 {
    (input_tokens as f64 / 1_000_000.0) * INPUT_PRICE_PER_M
        + (output_tokens as f64 / 1_000_000.0) * OUTPUT_PRICE_PER_M
}

#[derive(Serialize)]
pub struct CostSummary {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

/// Internal helper — called by the agent loop, not the frontend.
/// Returns early without error if the table doesn't exist yet (migration not run).
pub async fn record_cost_entry(
    db: &SqlitePool,
    session_id: &str,
    model: &str,
    endpoint: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> crate::errors::Result<()> {
    if input_tokens == 0 && output_tokens == 0 {
        return Ok(());
    }
    let id = Uuid::new_v4().to_string();
    let cost = estimate_cost(input_tokens, output_tokens);
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO cost_entries (id, session_id, model, endpoint, input_tokens, output_tokens, cost_usd, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(session_id)
    .bind(model)
    .bind(endpoint)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(cost)
    .bind(&now)
    .execute(db)
    .await?;

    Ok(())
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_session_cost(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<CostSummary, AppError> {
    let db = state.db.read().await;
    let row = sqlx::query_as::<_, (i64, i64, f64)>(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0.0) \
         FROM cost_entries WHERE session_id = ?",
    )
    .bind(&session_id)
    .fetch_one(&*db)
    .await?;

    Ok(CostSummary {
        input_tokens: row.0,
        output_tokens: row.1,
        cost_usd: row.2,
    })
}

#[tauri::command]
pub async fn get_today_cost(
    state: tauri::State<'_, crate::AppState>,
) -> Result<CostSummary, AppError> {
    let db = state.db.read().await;
    // SQLite date() returns YYYY-MM-DD in UTC; our timestamps are RFC3339
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let row = sqlx::query_as::<_, (i64, i64, f64)>(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0.0) \
         FROM cost_entries WHERE substr(created_at,1,10) = ?",
    )
    .bind(&today)
    .fetch_one(&*db)
    .await?;

    Ok(CostSummary {
        input_tokens: row.0,
        output_tokens: row.1,
        cost_usd: row.2,
    })
}

#[tauri::command]
pub async fn get_monthly_cost(
    state: tauri::State<'_, crate::AppState>,
) -> Result<CostSummary, AppError> {
    let db = state.db.read().await;
    // Match YYYY-MM prefix
    let month = Utc::now().format("%Y-%m").to_string();
    let row = sqlx::query_as::<_, (i64, i64, f64)>(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0.0) \
         FROM cost_entries WHERE substr(created_at,1,7) = ?",
    )
    .bind(&month)
    .fetch_one(&*db)
    .await?;

    Ok(CostSummary {
        input_tokens: row.0,
        output_tokens: row.1,
        cost_usd: row.2,
    })
}
