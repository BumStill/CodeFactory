// SPDX-License-Identifier: Apache-2.0
//! Request-level token usage, budget, and legacy cost compatibility commands.

use chrono::{DateTime, Datelike, Duration, NaiveDate, SecondsFormat, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Serialize)]
pub struct CostSummary {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone)]
pub struct UsageEventInput {
    pub request_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub surface: String,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_tokens: i64,
    pub actual_cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub cost_source: String,
    /// RFC3339 UTC timestamp. Production callers normally leave this empty;
    /// explicit values make local-day and migration acceptance deterministic.
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UsageSummary {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_tokens: i64,
    pub requests: i64,
    pub actual_cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
    pub cost_source: String,
    pub data_status: String,
    pub missing_usage_count: i64,
    pub source_counts: HashMap<String, i64>,
}

impl UsageSummary {
    fn total_tokens(&self) -> i64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageHeatmapDay {
    pub local_date: String,
    pub status: String,
    pub total_tokens: Option<i64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_tokens: i64,
    pub requests: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageBreakdown {
    pub surface: String,
    pub total_tokens: i64,
    pub requests: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopUsageSession {
    pub session_id: String,
    pub job_session_id: Option<String>,
    pub title: String,
    pub surface: String,
    pub task_id: Option<String>,
    pub total_tokens: i64,
    pub requests: i64,
    pub share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageDashboard {
    pub range_days: i64,
    pub start_utc: String,
    pub end_utc: String,
    pub data_status: String,
    pub summary: UsageSummary,
    pub heatmap: Vec<UsageHeatmapDay>,
    pub breakdowns: Vec<UsageBreakdown>,
    pub top_sessions: Vec<TopUsageSession>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageDayDetail {
    pub local_date: String,
    pub start_utc: String,
    pub end_utc: String,
    pub data_status: String,
    pub summary: UsageSummary,
    pub breakdowns: Vec<UsageBreakdown>,
    pub top_sessions: Vec<TopUsageSession>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageBudgetPeriodStatus {
    pub period_kind: String,
    pub period_key: String,
    pub usage_tokens: i64,
    pub limit_tokens: u64,
    pub ratio: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageBudgetAlert {
    pub receipt_id: String,
    pub period_kind: String,
    pub period_key: String,
    pub threshold: f64,
    pub usage_tokens: i64,
    pub limit_tokens: u64,
    pub triggered_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageBudgetStatus {
    pub daily: UsageBudgetPeriodStatus,
    pub monthly: UsageBudgetPeriodStatus,
    pub new_alert: Option<UsageBudgetAlert>,
}

fn valid_cost(value: Option<f64>) -> bool {
    value.is_none_or(|v| v.is_finite() && v >= 0.0)
}

fn validate_usage_event(event: &UsageEventInput) -> crate::errors::Result<()> {
    if event.request_id.trim().is_empty()
        || event.session_id.trim().is_empty()
        || event.provider.trim().is_empty()
        || event.endpoint.trim().is_empty()
        || event.model.trim().is_empty()
    {
        return Err(AppError::Other(
            "usage event identity fields cannot be empty".into(),
        ));
    }
    if event.input_tokens < 0
        || event.output_tokens < 0
        || event.reasoning_tokens < 0
        || event.cached_tokens < 0
        || !valid_cost(event.actual_cost_usd)
        || !valid_cost(event.estimated_cost_usd)
    {
        return Err(AppError::Other(
            "usage event contains invalid counters or cost".into(),
        ));
    }
    if !matches!(
        event.surface.as_str(),
        "interactive" | "autonomous" | "subagent" | "eval"
    ) {
        return Err(AppError::Other(format!(
            "unsupported usage surface: {}",
            event.surface
        )));
    }
    match event.cost_source.as_str() {
        "provider_actual"
            if event.actual_cost_usd.is_none() || event.estimated_cost_usd.is_some() =>
        {
            return Err(AppError::Other(
                "provider_actual requires actual cost and forbids estimate".into(),
            ));
        }
        "model_price_estimate"
            if event.estimated_cost_usd.is_none() || event.actual_cost_usd.is_some() =>
        {
            return Err(AppError::Other(
                "model_price_estimate requires estimate and forbids actual cost".into(),
            ));
        }
        "subscription" | "local"
            if event.actual_cost_usd.is_some() || event.estimated_cost_usd.is_some() =>
        {
            return Err(AppError::Other(format!(
                "{} usage cannot carry per-token dollars",
                event.cost_source
            )));
        }
        "unknown" if event.actual_cost_usd.is_some() || event.estimated_cost_usd.is_some() => {
            return Err(AppError::Other(
                "unknown cost source cannot carry dollars".into(),
            ));
        }
        "provider_actual" | "model_price_estimate" | "subscription" | "local" | "unknown" => {}
        other => {
            return Err(AppError::Other(format!(
                "unsupported usage cost source: {other}"
            )));
        }
    }
    if let Some(created_at) = event.created_at.as_deref() {
        DateTime::parse_from_rfc3339(created_at)
            .map_err(|_| AppError::Other("usage created_at must be RFC3339".into()))?;
    }
    Ok(())
}

/// Insert one provider request exactly once. The request id belongs to the
/// transport attempt, not the chat turn: tool rounds and completion recovery
/// therefore remain visible, while replaying a persistence callback is safe.
pub async fn record_usage_event(
    db: &SqlitePool,
    event: UsageEventInput,
) -> crate::errors::Result<bool> {
    validate_usage_event(&event)?;
    if event.input_tokens == 0 && event.output_tokens == 0 {
        return Ok(false);
    }
    let created_at = match event.created_at {
        Some(value) => DateTime::parse_from_rfc3339(&value)
            .map_err(|_| AppError::Other("usage created_at must be RFC3339".into()))?
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        None => Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    };
    let result = sqlx::query(
        "INSERT OR IGNORE INTO model_usage_events (
            id, request_id, attempt_id, session_id, task_id, surface, provider, endpoint, model,
            input_tokens, output_tokens, reasoning_tokens, cached_tokens,
            actual_cost_usd, estimated_cost_usd, cost_source, source, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'provider_usage', ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&event.request_id)
    .bind(&event.request_id)
    .bind(&event.session_id)
    .bind(&event.task_id)
    .bind(&event.surface)
    .bind(&event.provider)
    .bind(&event.endpoint)
    .bind(&event.model)
    .bind(event.input_tokens)
    .bind(event.output_tokens)
    .bind(event.reasoning_tokens)
    .bind(event.cached_tokens)
    .bind(event.actual_cost_usd)
    .bind(event.estimated_cost_usd)
    .bind(&event.cost_source)
    .bind(created_at)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

fn parse_now(now_utc: Option<String>) -> crate::errors::Result<DateTime<Utc>> {
    match now_utc {
        Some(value) => Ok(DateTime::parse_from_rfc3339(&value)
            .map_err(|_| AppError::Other("now_utc must be RFC3339".into()))?
            .with_timezone(&Utc)),
        None => Ok(Utc::now()),
    }
}

fn utc_bounds_for_local_date(
    local_date: NaiveDate,
    timezone_offset_minutes: i32,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let start_local = local_date
        .and_hms_opt(0, 0, 0)
        .expect("midnight is always a valid naive time");
    let start_utc = DateTime::<Utc>::from_naive_utc_and_offset(
        start_local - Duration::minutes(timezone_offset_minutes as i64),
        Utc,
    );
    (start_utc, start_utc + Duration::days(1))
}

fn rfc3339_millis(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

type AggregateRow = (i64, i64, i64, i64, i64, Option<f64>, Option<f64>, String);

async fn usage_source_counts(
    db: &SqlitePool,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
) -> crate::errors::Result<HashMap<String, i64>> {
    Ok(sqlx::query_as::<_, (String, i64)>(
        "SELECT source, COUNT(*) FROM model_usage_events
         WHERE created_at >= ? AND created_at < ? GROUP BY source",
    )
    .bind(rfc3339_millis(start_utc))
    .bind(rfc3339_millis(end_utc))
    .fetch_all(db)
    .await?
    .into_iter()
    .collect())
}

async fn missing_usage_count(
    db: &SqlitePool,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
) -> crate::errors::Result<i64> {
    let messages_exist: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages'",
    )
    .fetch_one(db)
    .await?;
    if messages_exist == 0 {
        return Ok(0);
    }
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages m
         WHERE m.role = 'assistant' AND m.created_at >= ? AND m.created_at < ?
           AND ((m.usage_request_id IS NOT NULL AND NOT EXISTS (
                 SELECT 1 FROM model_usage_events u WHERE u.request_id = m.usage_request_id
               ))
             OR (m.usage_request_id IS NULL
                 AND m.input_tokens IS NULL AND m.output_tokens IS NULL))",
    )
    .bind(start_utc.timestamp_millis())
    .bind(end_utc.timestamp_millis())
    .fetch_one(db)
    .await?)
}

async fn aggregate_summary(
    db: &SqlitePool,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
) -> crate::errors::Result<UsageSummary> {
    let row: AggregateRow = sqlx::query_as(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(reasoning_tokens),0), COALESCE(SUM(cached_tokens),0),
                COUNT(*), SUM(actual_cost_usd), SUM(estimated_cost_usd),
                COALESCE(GROUP_CONCAT(DISTINCT cost_source),'')
         FROM model_usage_events WHERE created_at >= ? AND created_at < ?",
    )
    .bind(rfc3339_millis(start_utc))
    .bind(rfc3339_millis(end_utc))
    .fetch_one(db)
    .await?;
    let cost_source = if row.7.is_empty() {
        "unknown".to_string()
    } else if row.7.contains(',') {
        "mixed".to_string()
    } else {
        row.7
    };
    let source_counts = usage_source_counts(db, start_utc, end_utc).await?;
    let missing_usage_count = missing_usage_count(db, start_utc, end_utc).await?;
    let data_status = if missing_usage_count > 0
        || source_counts
            .keys()
            .any(|source| source != "provider_usage")
    {
        "partial"
    } else {
        "complete"
    };
    Ok(UsageSummary {
        input_tokens: row.0,
        output_tokens: row.1,
        reasoning_tokens: row.2,
        cached_tokens: row.3,
        requests: row.4,
        actual_cost_usd: row.5,
        estimated_cost_usd: row.6,
        cost_source,
        data_status: data_status.into(),
        missing_usage_count,
        source_counts,
    })
}

async fn usage_breakdowns(
    db: &SqlitePool,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
) -> crate::errors::Result<Vec<UsageBreakdown>> {
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT surface, COALESCE(SUM(input_tokens + output_tokens),0), COUNT(*)
         FROM model_usage_events WHERE created_at >= ? AND created_at < ?
         GROUP BY surface ORDER BY SUM(input_tokens + output_tokens) DESC",
    )
    .bind(rfc3339_millis(start_utc))
    .bind(rfc3339_millis(end_utc))
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(surface, total_tokens, requests)| UsageBreakdown {
            surface,
            total_tokens,
            requests,
        })
        .collect())
}

async fn top_usage_sessions(
    db: &SqlitePool,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
    total_tokens: i64,
) -> crate::errors::Result<Vec<TopUsageSession>> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            String,
            String,
            Option<String>,
            i64,
            i64,
        ),
    >(
        "SELECT u.session_id, MAX(t.session_id), COALESCE(MAX(s.title), '历史会话'),
                MAX(u.surface), MAX(u.task_id),
                COALESCE(SUM(u.input_tokens + u.output_tokens),0), COUNT(*)
         FROM model_usage_events u LEFT JOIN sessions s ON s.id = u.session_id
         LEFT JOIN task_runs t ON t.id = u.task_id
         WHERE u.created_at >= ? AND u.created_at < ?
         GROUP BY u.session_id ORDER BY SUM(u.input_tokens + u.output_tokens) DESC LIMIT 20",
    )
    .bind(rfc3339_millis(start_utc))
    .bind(rfc3339_millis(end_utc))
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(session_id, job_session_id, title, surface, task_id, session_tokens, requests)| {
                TopUsageSession {
                    session_id,
                    job_session_id,
                    title,
                    surface,
                    task_id,
                    total_tokens: session_tokens,
                    requests,
                    share: if total_tokens > 0 {
                        session_tokens as f64 / total_tokens as f64
                    } else {
                        0.0
                    },
                }
            },
        )
        .collect())
}

pub async fn get_usage_dashboard_data(
    db: &SqlitePool,
    range_days: i64,
    timezone_offset_minutes: i32,
    now_utc: Option<String>,
) -> crate::errors::Result<UsageDashboard> {
    let range_days = range_days.clamp(1, 366);
    let now = parse_now(now_utc)?;
    let today = (now + Duration::minutes(timezone_offset_minutes as i64)).date_naive();
    let first_day = today - Duration::days(range_days - 1);
    let (range_start, _) = utc_bounds_for_local_date(first_day, timezone_offset_minutes);
    let (_, range_end) = utc_bounds_for_local_date(today, timezone_offset_minutes);
    let offset_modifier = format!("{:+} minutes", timezone_offset_minutes);

    let grouped = sqlx::query_as::<_, (String, i64, i64, i64, i64, i64, i64)>(
        "SELECT date(created_at, ?), COALESCE(SUM(input_tokens),0),
                COALESCE(SUM(output_tokens),0), COALESCE(SUM(reasoning_tokens),0),
                COALESCE(SUM(cached_tokens),0), COUNT(*),
                SUM(CASE WHEN source = 'provider_usage' THEN 0 ELSE 1 END)
         FROM model_usage_events WHERE created_at >= ? AND created_at < ?
         GROUP BY date(created_at, ?)",
    )
    .bind(&offset_modifier)
    .bind(rfc3339_millis(range_start))
    .bind(rfc3339_millis(range_end))
    .bind(&offset_modifier)
    .fetch_all(db)
    .await?;
    let grouped: HashMap<String, (i64, i64, i64, i64, i64, i64)> = grouped
        .into_iter()
        .map(|row| (row.0, (row.1, row.2, row.3, row.4, row.5, row.6)))
        .collect();

    let tracking_started: String =
        sqlx::query_scalar("SELECT started_at FROM usage_tracking_metadata WHERE singleton = 1")
            .fetch_one(db)
            .await?;
    let tracking_started_local = (DateTime::parse_from_rfc3339(&tracking_started)
        .map_err(|_| AppError::Other("invalid usage tracking metadata".into()))?
        .with_timezone(&Utc)
        + Duration::minutes(timezone_offset_minutes as i64))
    .date_naive();

    let missing_by_day: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(
        "SELECT strftime('%Y-%m-%d', m.created_at / 1000.0, 'unixepoch', ?), COUNT(*)
         FROM messages m
         WHERE m.role = 'assistant' AND m.created_at >= ? AND m.created_at < ?
           AND ((m.usage_request_id IS NOT NULL AND NOT EXISTS (
                 SELECT 1 FROM model_usage_events u WHERE u.request_id = m.usage_request_id
               ))
             OR (m.usage_request_id IS NULL
                 AND m.input_tokens IS NULL AND m.output_tokens IS NULL))
         GROUP BY strftime('%Y-%m-%d', m.created_at / 1000.0, 'unixepoch', ?)",
    )
    .bind(&offset_modifier)
    .bind(range_start.timestamp_millis())
    .bind(range_end.timestamp_millis())
    .bind(&offset_modifier)
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();

    let mut heatmap = Vec::with_capacity(range_days as usize);
    for offset in 0..range_days {
        let date = first_day + Duration::days(offset);
        let key = date.format("%Y-%m-%d").to_string();
        if let Some((input, output, reasoning, cached, requests, historical)) = grouped.get(&key) {
            let missing_usage = missing_by_day.contains_key(&key);
            heatmap.push(UsageHeatmapDay {
                local_date: key,
                status: if *historical > 0 || missing_usage {
                    "partial"
                } else {
                    "recorded"
                }
                .into(),
                total_tokens: Some(input.saturating_add(*output)),
                input_tokens: *input,
                output_tokens: *output,
                reasoning_tokens: *reasoning,
                cached_tokens: *cached,
                requests: Some(*requests),
            });
        } else {
            let missing_usage = missing_by_day.contains_key(&key);
            let missing = date < tracking_started_local || missing_usage;
            heatmap.push(UsageHeatmapDay {
                local_date: key,
                status: if missing_usage {
                    "partial"
                } else if missing {
                    "missing"
                } else {
                    "recorded"
                }
                .into(),
                total_tokens: (!missing).then_some(0),
                input_tokens: 0,
                output_tokens: 0,
                reasoning_tokens: 0,
                cached_tokens: 0,
                requests: (!missing).then_some(0),
            });
        }
    }

    let summary = aggregate_summary(db, range_start, range_end).await?;
    let breakdowns = usage_breakdowns(db, range_start, range_end).await?;
    let top_sessions =
        top_usage_sessions(db, range_start, range_end, summary.total_tokens()).await?;
    Ok(UsageDashboard {
        range_days,
        start_utc: rfc3339_millis(range_start),
        end_utc: rfc3339_millis(range_end),
        data_status: summary.data_status.clone(),
        summary,
        heatmap,
        breakdowns,
        top_sessions,
    })
}

pub async fn get_usage_day_detail_data(
    db: &SqlitePool,
    local_date: &str,
    timezone_offset_minutes: i32,
) -> crate::errors::Result<UsageDayDetail> {
    let date = NaiveDate::parse_from_str(local_date, "%Y-%m-%d")
        .map_err(|_| AppError::Other("local_date must be YYYY-MM-DD".into()))?;
    let (start, end) = utc_bounds_for_local_date(date, timezone_offset_minutes);
    let summary = aggregate_summary(db, start, end).await?;
    let breakdowns = usage_breakdowns(db, start, end).await?;
    let top_sessions = top_usage_sessions(db, start, end, summary.total_tokens()).await?;
    Ok(UsageDayDetail {
        local_date: local_date.to_string(),
        start_utc: rfc3339_millis(start),
        end_utc: rfc3339_millis(end),
        data_status: summary.data_status.clone(),
        summary,
        breakdowns,
        top_sessions,
    })
}

async fn tokens_in_range(
    db: &SqlitePool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> crate::errors::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(SUM(input_tokens + output_tokens), 0)
         FROM model_usage_events WHERE created_at >= ? AND created_at < ?",
    )
    .bind(rfc3339_millis(start))
    .bind(rfc3339_millis(end))
    .fetch_one(db)
    .await?)
}

pub async fn evaluate_usage_budget_data(
    db: &SqlitePool,
    daily_limit: u64,
    monthly_limit: u64,
    thresholds: &[f64],
    timezone_offset_minutes: i32,
    now_utc: Option<String>,
) -> crate::errors::Result<UsageBudgetStatus> {
    let now = parse_now(now_utc)?;
    let local_today = (now + Duration::minutes(timezone_offset_minutes as i64)).date_naive();
    let month_start = local_today
        .with_day(1)
        .expect("every calendar month has a first day");
    let next_month = if month_start.month() == 12 {
        NaiveDate::from_ymd_opt(month_start.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(month_start.year(), month_start.month() + 1, 1)
    }
    .expect("next month boundary is valid");
    let (day_start, day_end) = utc_bounds_for_local_date(local_today, timezone_offset_minutes);
    let (month_start_utc, _) = utc_bounds_for_local_date(month_start, timezone_offset_minutes);
    let (month_end_utc, _) = utc_bounds_for_local_date(next_month, timezone_offset_minutes);
    let daily_tokens = tokens_in_range(db, day_start, day_end).await?;
    let monthly_tokens = tokens_in_range(db, month_start_utc, month_end_utc).await?;
    let daily = UsageBudgetPeriodStatus {
        period_kind: "day".into(),
        period_key: local_today.format("%Y-%m-%d").to_string(),
        usage_tokens: daily_tokens,
        limit_tokens: daily_limit,
        ratio: (daily_limit > 0).then_some(daily_tokens as f64 / daily_limit as f64),
    };
    let monthly = UsageBudgetPeriodStatus {
        period_kind: "month".into(),
        period_key: month_start.format("%Y-%m").to_string(),
        usage_tokens: monthly_tokens,
        limit_tokens: monthly_limit,
        ratio: (monthly_limit > 0).then_some(monthly_tokens as f64 / monthly_limit as f64),
    };
    let mut valid_thresholds: Vec<f64> = thresholds
        .iter()
        .copied()
        .filter(|threshold| threshold.is_finite() && *threshold > 0.0)
        .collect();
    valid_thresholds.sort_by(f64::total_cmp);
    valid_thresholds.dedup_by(|left, right| left.total_cmp(right).is_eq());
    let triggered_at = rfc3339_millis(now);
    let mut inserted_alerts = Vec::new();
    for period in [&daily, &monthly] {
        let Some(ratio) = period.ratio else { continue };
        for threshold in valid_thresholds
            .iter()
            .copied()
            .filter(|threshold| ratio >= *threshold)
        {
            let receipt_id = Uuid::new_v4().to_string();
            let result = sqlx::query(
                "INSERT OR IGNORE INTO usage_budget_receipts
                 (id, period_kind, period_key, threshold, usage_tokens, limit_tokens, triggered_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&receipt_id)
            .bind(&period.period_kind)
            .bind(&period.period_key)
            .bind(threshold)
            .bind(period.usage_tokens)
            .bind(period.limit_tokens as i64)
            .bind(&triggered_at)
            .execute(db)
            .await?;
            if result.rows_affected() == 1 {
                inserted_alerts.push(UsageBudgetAlert {
                    receipt_id,
                    period_kind: period.period_kind.clone(),
                    period_key: period.period_key.clone(),
                    threshold,
                    usage_tokens: period.usage_tokens,
                    limit_tokens: period.limit_tokens,
                    triggered_at: triggered_at.clone(),
                });
            }
        }
    }
    inserted_alerts.sort_by(|left, right| left.threshold.total_cmp(&right.threshold));
    Ok(UsageBudgetStatus {
        daily,
        monthly,
        new_alert: inserted_alerts.pop(),
    })
}

#[tauri::command]
pub async fn get_usage_dashboard(
    range_days: i64,
    timezone_offset_minutes: i32,
    state: tauri::State<'_, crate::AppState>,
) -> Result<UsageDashboard, AppError> {
    let db = state.db.read().await;
    get_usage_dashboard_data(&db, range_days, timezone_offset_minutes, None).await
}

#[tauri::command]
pub async fn get_usage_day_detail(
    local_date: String,
    timezone_offset_minutes: i32,
    state: tauri::State<'_, crate::AppState>,
) -> Result<UsageDayDetail, AppError> {
    let db = state.db.read().await;
    get_usage_day_detail_data(&db, &local_date, timezone_offset_minutes).await
}

#[tauri::command]
pub async fn get_usage_budget_status(
    timezone_offset_minutes: i32,
    state: tauri::State<'_, crate::AppState>,
) -> Result<UsageBudgetStatus, AppError> {
    let budget = state.settings.read().await.usage_budget.clone();
    let thresholds = if budget.alerts_enabled {
        budget.alert_thresholds
    } else {
        Vec::new()
    };
    let db = state.db.read().await;
    evaluate_usage_budget_data(
        &db,
        budget.daily_token_limit,
        budget.monthly_token_limit,
        &thresholds,
        timezone_offset_minutes,
        None,
    )
    .await
}

#[tauri::command]
pub async fn get_session_usage(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<UsageSummary, AppError> {
    let db = state.db.read().await;
    let row: AggregateRow = sqlx::query_as(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(reasoning_tokens),0), COALESCE(SUM(cached_tokens),0),
                COUNT(*), SUM(actual_cost_usd), SUM(estimated_cost_usd),
                COALESCE(GROUP_CONCAT(DISTINCT cost_source),'')
         FROM model_usage_events WHERE session_id = ?",
    )
    .bind(&session_id)
    .fetch_one(&*db)
    .await?;
    let source_counts: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(
        "SELECT source, COUNT(*) FROM model_usage_events WHERE session_id = ? GROUP BY source",
    )
    .bind(&session_id)
    .fetch_all(&*db)
    .await?
    .into_iter()
    .collect();
    Ok(UsageSummary {
        input_tokens: row.0,
        output_tokens: row.1,
        reasoning_tokens: row.2,
        cached_tokens: row.3,
        requests: row.4,
        actual_cost_usd: row.5,
        estimated_cost_usd: row.6,
        cost_source: if row.7.is_empty() {
            "unknown".into()
        } else if row.7.contains(',') {
            "mixed".into()
        } else {
            row.7
        },
        data_status: if source_counts
            .keys()
            .any(|source| source != "provider_usage")
        {
            "partial".into()
        } else {
            "complete".into()
        },
        missing_usage_count: 0,
        source_counts,
    })
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
        "today" => (
            "WHERE substr(created_at,1,10) = ?",
            Some(Utc::now().format("%Y-%m-%d").to_string()),
        ),
        "month" => (
            "WHERE substr(created_at,1,7) = ?",
            Some(Utc::now().format("%Y-%m").to_string()),
        ),
        _ => ("", None),
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
        .map(
            |(model, input_tokens, output_tokens, cost_usd, calls)| CostByModel {
                model,
                input_tokens,
                output_tokens,
                cost_usd,
                calls,
            },
        )
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
        .map(
            |(
                id,
                session_id,
                model,
                endpoint,
                input_tokens,
                output_tokens,
                cost_usd,
                created_at,
            )| {
                RecentCostEntry {
                    id,
                    session_id,
                    model,
                    endpoint,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    created_at,
                }
            },
        )
        .collect())
}
