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

// ── Aggregations for the Cost Dashboard ──────────────────────────────────────

#[derive(Serialize)]
pub struct CostByModel {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub calls: i64,
}

/// Aggregate cost by model for a date scope.
///
/// `scope` is one of: "today", "month", "all". Anything else is treated as
/// "all" — keeps the frontend tolerant. Returned in descending cost order
/// so the top spenders are first.
#[tauri::command]
pub async fn get_costs_by_model(
    scope: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<CostByModel>, AppError> {
    let db = state.db.read().await;
    let (where_clause, bind_val): (&str, Option<String>) = match scope.as_str() {
        "today" => ("WHERE substr(created_at,1,10) = ?", Some(Utc::now().format("%Y-%m-%d").to_string())),
        "month" => ("WHERE substr(created_at,1,7) = ?",  Some(Utc::now().format("%Y-%m").to_string())),
        _       => ("", None),
    };
    let sql = format!(
        "SELECT model, \
                COALESCE(SUM(input_tokens),0)  AS input_tokens, \
                COALESCE(SUM(output_tokens),0) AS output_tokens, \
                COALESCE(SUM(cost_usd),0.0)    AS cost_usd, \
                COUNT(*)                       AS calls \
         FROM cost_entries {} \
         GROUP BY model ORDER BY cost_usd DESC",
        where_clause
    );

    let mut query = sqlx::query_as::<_, (String, i64, i64, f64, i64)>(&sql);
    if let Some(v) = bind_val {
        query = query.bind(v);
    }
    let rows = query.fetch_all(&*db).await?;
    Ok(rows
        .into_iter()
        .map(|(model, input_tokens, output_tokens, cost_usd, calls)| CostByModel {
            model, input_tokens, output_tokens, cost_usd, calls,
        })
        .collect())
}

#[derive(Serialize)]
pub struct RecentCostEntry {
    pub id: String,
    pub session_id: String,
    pub model: String,
    pub endpoint: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub created_at: String,
}

/// Most-recent N cost entries (default 50). Cheap query — backs the
/// "最近活动" list in the cost dashboard.
#[tauri::command]
pub async fn list_recent_cost_entries(
    limit: Option<i64>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<RecentCostEntry>, AppError> {
    let db = state.db.read().await;
    let lim = limit.unwrap_or(50).clamp(1, 500);
    let rows = sqlx::query_as::<_, (String, String, String, String, i64, i64, f64, String)>(
        "SELECT id, session_id, model, endpoint, input_tokens, output_tokens, cost_usd, created_at \
         FROM cost_entries ORDER BY created_at DESC LIMIT ?",
    )
    .bind(lim)
    .fetch_all(&*db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, session_id, model, endpoint, input_tokens, output_tokens, cost_usd, created_at)| {
            RecentCostEntry {
                id, session_id, model, endpoint,
                input_tokens, output_tokens, cost_usd, created_at,
            }
        })
        .collect())
}
