// SPDX-License-Identifier: Apache-2.0
//! Acceptance tests for request-level model usage accounting.
//!
//! These tests intentionally describe the new contract before its production
//! implementation. They exercise the real SQLite startup path so a green result
//! proves more than an isolated query over a test-only schema.

use serde_json::Value;
use sqlx::migrate::MigrateDatabase;
use sqlx::Row;

use super::costs::{
    evaluate_usage_budget_data, get_usage_dashboard_data, get_usage_day_detail_data,
    record_usage_event, UsageEventInput,
};

async fn migrated_pool() -> (tempfile::TempDir, sqlx::SqlitePool) {
    let dir = tempfile::tempdir().expect("create usage acceptance tempdir");
    let path = dir.path().join("usage-acceptance.db");
    let url = format!("sqlite://{}", path.display());
    let pool = crate::storage::db::connect(&url)
        .await
        .expect("open and migrate usage acceptance database");
    (dir, pool)
}

fn usage_event(
    request_id: &str,
    session_id: &str,
    surface: &str,
    created_at: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> UsageEventInput {
    UsageEventInput {
        request_id: request_id.into(),
        session_id: session_id.into(),
        task_id: None,
        surface: surface.into(),
        provider: "openrouter".into(),
        endpoint: "primary".into(),
        model: "openai/gpt-5.6".into(),
        input_tokens,
        output_tokens,
        reasoning_tokens: 0,
        cached_tokens: 0,
        actual_cost_usd: None,
        estimated_cost_usd: None,
        cost_source: "unknown".into(),
        created_at: Some(created_at.into()),
    }
}

fn as_json<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).expect("usage response must serialize for the Tauri frontend")
}

fn json_i64(value: &Value, pointer: &str) -> i64 {
    value
        .pointer(pointer)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("missing integer at {pointer}: {value}"))
}

fn json_f64(value: &Value, pointer: &str) -> f64 {
    value
        .pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("missing number at {pointer}: {value}"))
}

fn day<'a>(dashboard: &'a Value, date: &str) -> &'a Value {
    dashboard["heatmap"]
        .as_array()
        .expect("dashboard.heatmap must be an array")
        .iter()
        .find(|entry| entry["local_date"] == date)
        .unwrap_or_else(|| panic!("missing heatmap day {date}: {dashboard}"))
}

#[tokio::test]
async fn fresh_and_historical_databases_receive_request_level_usage_schema() {
    let (_dir, pool) = migrated_pool().await;

    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('model_usage_events')")
            .fetch_all(&pool)
            .await
            .expect("inspect model_usage_events columns");

    for required in [
        "request_id",
        "session_id",
        "task_id",
        "surface",
        "provider",
        "endpoint",
        "model",
        "input_tokens",
        "output_tokens",
        "reasoning_tokens",
        "cached_tokens",
        "actual_cost_usd",
        "estimated_cost_usd",
        "cost_source",
        "created_at",
    ] {
        assert!(
            columns.iter().any(|column| column == required),
            "request-level usage schema must expose {required}; got {columns:?}"
        );
    }

    let indexes = sqlx::query("PRAGMA index_list('model_usage_events')")
        .fetch_all(&pool)
        .await
        .expect("inspect model_usage_events indexes");
    assert!(
        indexes.iter().any(|row| row.get::<i64, _>("unique") == 1),
        "model_usage_events must enforce a unique request id for retry/resume idempotency"
    );
}

#[test]
fn openai_and_anthropic_persist_usage_before_terminal_tool_branch() {
    let source = include_str!("../agent/mod.rs");
    for (provider, start, end) in [
        (
            "openai",
            "    async fn run_openai(",
            // End marker: the first method after run_openai. (call_openai_transport
            // moved to model_transport.rs in slice 4.5a and request_permission to
            // permission_gateway.rs in slice 4.6; finish_cancelled_tool_batch is now
            // the tight bound of the run_openai section.)
            "    async fn finish_cancelled_tool_batch(",
        ),
        (
            "anthropic",
            "    async fn run_anthropic(",
            // End marker: the first free fn after run_anthropic. (openai_tool_controls
            // moved to agent-loop in slice 4.6; validate_openai_sse_completion is now
            // the tight bound of the run_anthropic section.)
            "\n}\n\nfn validate_openai_sse_completion(",
        ),
    ] {
        let section = source
            .split_once(start)
            .unwrap_or_else(|| panic!("missing {provider} loop start"))
            .1
            .split_once(end)
            .unwrap_or_else(|| panic!("missing {provider} loop end"))
            .0;
        let loop_pos = section
            .find("for iteration in 0..max_iterations")
            .unwrap_or_else(|| panic!("missing {provider} provider-round loop"));
        let record_pos = section
            .find("record_usage_event")
            .unwrap_or_else(|| panic!("{provider} does not persist request-level usage"));
        let terminal_pos = section
            .find("if tool_calls.is_empty()")
            .unwrap_or_else(|| panic!("missing {provider} terminal/tool branch"));

        assert!(
            loop_pos < record_pos && record_pos < terminal_pos,
            "{provider} must persist usage inside every provider round and before the tool-call branch; otherwise intermediate tool rounds are omitted"
        );
    }
}

#[test]
fn completed_usage_is_persisted_before_post_response_cancellation() {
    let source = include_str!("../agent/mod.rs");
    for (provider, start, end, response_marker) in [
        (
            "openai",
            "    async fn run_openai(",
            // End marker: the first method after run_openai. (call_openai_transport
            // moved to model_transport.rs in slice 4.5a and request_permission to
            // permission_gateway.rs in slice 4.6; finish_cancelled_tool_batch is now
            // the tight bound of the run_openai section.)
            "    async fn finish_cancelled_tool_batch(",
            "let (text, tool_calls, usage, reasoning) = match call_result",
        ),
        (
            "anthropic",
            "    async fn run_anthropic(",
            // End marker: the first free fn after run_anthropic. (openai_tool_controls
            // moved to agent-loop in slice 4.6; validate_openai_sse_completion is now
            // the tight bound of the run_anthropic section.)
            "\n}\n\nfn validate_openai_sse_completion(",
            "let resp = match first_attempt",
        ),
    ] {
        let section = source
            .split_once(start)
            .unwrap_or_else(|| panic!("missing {provider} loop start"))
            .1
            .split_once(end)
            .unwrap_or_else(|| panic!("missing {provider} loop end"))
            .0;
        let after_response = section
            .split_once(response_marker)
            .unwrap_or_else(|| panic!("missing {provider} completed response marker"))
            .1;
        let record_pos = after_response
            .find("record_usage_event")
            .unwrap_or_else(|| panic!("{provider} does not persist response usage"));
        let cancel_pos = if provider == "anthropic" {
            after_response
                .find("if resp.cancelled || self.is_cancelled()")
                .expect("missing anthropic post-response cancellation")
        } else {
            after_response
                .find("if self.is_cancelled()")
                .expect("missing openai post-response cancellation")
        };

        assert!(
            record_pos < cancel_pos,
            "{provider} must persist already-received Usage before honoring post-response cancellation"
        );
    }
}

#[tokio::test]
async fn each_provider_round_is_recorded_once_and_request_id_is_idempotent() {
    let (_dir, pool) = migrated_pool().await;

    let tool_round = usage_event(
        "provider-request-tool",
        "session-rounds",
        "interactive",
        "2026-07-22T01:00:00Z",
        1_000,
        100,
    );
    record_usage_event(&pool, tool_round)
        .await
        .expect("record tool-call intermediate provider round");

    // Retry/resume may replay the persistence call. INSERT OR IGNORE semantics
    // must preserve the first row instead of double billing it.
    let duplicate = usage_event(
        "provider-request-tool",
        "session-rounds",
        "interactive",
        "2026-07-22T01:00:01Z",
        9_999,
        999,
    );
    record_usage_event(&pool, duplicate)
        .await
        .expect("duplicate request id is an idempotent no-op");

    let final_round = usage_event(
        "provider-request-final",
        "session-rounds",
        "interactive",
        "2026-07-22T01:01:00Z",
        2_000,
        200,
    );
    record_usage_event(&pool, final_round)
        .await
        .expect("record terminal provider round");

    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT request_id, input_tokens, output_tokens FROM model_usage_events \
         WHERE session_id = ? ORDER BY created_at",
    )
    .bind("session-rounds")
    .fetch_all(&pool)
    .await
    .expect("read recorded provider rounds");

    assert_eq!(
        rows,
        vec![
            ("provider-request-tool".into(), 1_000, 100),
            ("provider-request-final".into(), 2_000, 200),
        ],
        "tool-call and final rounds must both count, while a replayed request id counts once"
    );
}

#[tokio::test]
async fn cost_sources_keep_subscription_actual_and_estimate_semantically_separate() {
    let (_dir, pool) = migrated_pool().await;

    let mut subscription = usage_event(
        "subscription-request",
        "session-costs",
        "interactive",
        "2026-07-22T02:00:00Z",
        10_000,
        500,
    );
    subscription.provider = "chatgpt".into();
    subscription.cost_source = "subscription".into();
    record_usage_event(&pool, subscription)
        .await
        .expect("subscription tokens are observable without fake dollars");

    let mut actual = usage_event(
        "actual-request",
        "session-costs",
        "eval",
        "2026-07-22T03:00:00Z",
        20_000,
        1_000,
    );
    actual.actual_cost_usd = Some(0.42);
    actual.cost_source = "provider_actual".into();
    record_usage_event(&pool, actual)
        .await
        .expect("provider-reported actual cost is accepted");

    let mut estimate = usage_event(
        "estimate-request",
        "session-costs",
        "autonomous",
        "2026-07-22T04:00:00Z",
        30_000,
        1_500,
    );
    estimate.estimated_cost_usd = Some(0.25);
    estimate.cost_source = "model_price_estimate".into();
    record_usage_event(&pool, estimate)
        .await
        .expect("explicit model-price estimate is accepted");

    let rows: Vec<(String, Option<f64>, Option<f64>, String)> = sqlx::query_as(
        "SELECT request_id, actual_cost_usd, estimated_cost_usd, cost_source \
         FROM model_usage_events WHERE session_id = ? ORDER BY created_at",
    )
    .bind("session-costs")
    .fetch_all(&pool)
    .await
    .expect("read cost source rows");
    assert_eq!(
        rows,
        vec![
            (
                "subscription-request".into(),
                None,
                None,
                "subscription".into()
            ),
            (
                "actual-request".into(),
                Some(0.42),
                None,
                "provider_actual".into()
            ),
            (
                "estimate-request".into(),
                None,
                Some(0.25),
                "model_price_estimate".into()
            ),
        ]
    );

    let dashboard = as_json(
        get_usage_dashboard_data(&pool, 1, 0, Some("2026-07-22T23:00:00Z".into()))
            .await
            .expect("aggregate cost sources"),
    );
    assert!((json_f64(&dashboard, "/summary/actual_cost_usd") - 0.42).abs() < 1e-9);
    assert!((json_f64(&dashboard, "/summary/estimated_cost_usd") - 0.25).abs() < 1e-9);
    assert_eq!(json_i64(&dashboard, "/summary/requests"), 3);

    let mut invalid_subscription = usage_event(
        "invalid-subscription",
        "session-costs",
        "interactive",
        "2026-07-22T05:00:00Z",
        1,
        1,
    );
    invalid_subscription.cost_source = "subscription".into();
    invalid_subscription.actual_cost_usd = Some(99.0);
    assert!(
        record_usage_event(&pool, invalid_subscription)
            .await
            .is_err(),
        "subscription accounting must reject fake per-token actual cost"
    );
}

#[tokio::test]
async fn dashboard_uses_local_day_boundaries_and_emits_contiguous_heatmap_days() {
    let (_dir, pool) = migrated_pool().await;

    for event in [
        usage_event(
            "before-shanghai-midnight",
            "session-yesterday",
            "interactive",
            "2026-07-21T15:59:59Z",
            100,
            10,
        ),
        usage_event(
            "at-shanghai-midnight",
            "session-today",
            "eval",
            "2026-07-21T16:00:00Z",
            200,
            20,
        ),
        usage_event(
            "later-shanghai-day",
            "session-today",
            "eval",
            "2026-07-22T12:00:00Z",
            300,
            30,
        ),
    ] {
        record_usage_event(&pool, event)
            .await
            .expect("seed timezone boundary event");
    }

    let dashboard = as_json(
        get_usage_dashboard_data(&pool, 4, 8 * 60, Some("2026-07-22T12:30:00Z".into()))
            .await
            .expect("aggregate local-day heatmap"),
    );
    assert_eq!(
        dashboard["heatmap"]
            .as_array()
            .expect("dashboard.heatmap array")
            .len(),
        4,
        "heatmap needs one cell per requested local calendar day, including zero-use days"
    );
    assert_eq!(json_i64(day(&dashboard, "2026-07-19"), "/input_tokens"), 0);
    assert_eq!(json_i64(day(&dashboard, "2026-07-20"), "/input_tokens"), 0);
    assert_eq!(
        json_i64(day(&dashboard, "2026-07-21"), "/input_tokens"),
        100
    );
    assert_eq!(
        json_i64(day(&dashboard, "2026-07-22"), "/input_tokens"),
        500
    );
    assert_eq!(json_i64(day(&dashboard, "2026-07-22"), "/requests"), 2);

    let detail = as_json(
        get_usage_day_detail_data(&pool, "2026-07-22", 8 * 60)
            .await
            .expect("load local date detail"),
    );
    assert_eq!(detail["local_date"], "2026-07-22");
    assert_eq!(json_i64(&detail, "/summary/input_tokens"), 500);
    assert_eq!(json_i64(&detail, "/summary/output_tokens"), 50);
    assert_eq!(json_i64(&detail, "/summary/requests"), 2);
    assert_eq!(
        detail["breakdowns"][0]["surface"], "eval",
        "day drill-down must retain execution-surface attribution"
    );
    assert_eq!(
        detail["top_sessions"]
            .as_array()
            .expect("detail.top_sessions array")
            .len(),
        1,
        "both local-day rounds belong to the same session drill-down"
    );
}

#[tokio::test]
async fn historical_cost_entries_backfill_as_estimates_without_polluting_actual_cost() {
    let dir = tempfile::tempdir().expect("legacy db tempdir");
    let path = dir.path().join("legacy-costs.db");
    let url = format!("sqlite://{}", path.display());
    sqlx::Sqlite::create_database(&url)
        .await
        .expect("create legacy sqlite database");
    let legacy = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("open legacy database");
    sqlx::query(
        "CREATE TABLE cost_entries (
            id TEXT PRIMARY KEY, session_id TEXT NOT NULL, model TEXT NOT NULL,
            endpoint TEXT NOT NULL, input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL, cost_usd REAL NOT NULL,
            created_at TEXT NOT NULL
        )",
    )
    .execute(&legacy)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO cost_entries
         (id, session_id, model, endpoint, input_tokens, output_tokens, cost_usd, created_at)
         VALUES ('legacy-1', 'legacy-session', 'legacy-model', 'legacy-endpoint',
                 700, 70, 0.12, '2026-07-22T06:00:00Z')",
    )
    .execute(&legacy)
    .await
    .unwrap();
    legacy.close().await;

    let pool = crate::storage::db::connect(&url)
        .await
        .expect("upgrade historical database");
    let backfill: (i64, Option<f64>, Option<f64>, String) = sqlx::query_as(
        "SELECT COUNT(*), MAX(actual_cost_usd), MAX(estimated_cost_usd), MAX(cost_source)
         FROM model_usage_events WHERE request_id = 'legacy:legacy-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect legacy usage backfill");
    assert_eq!(backfill.0, 1, "legacy row must backfill exactly once");
    assert_eq!(
        backfill.1, None,
        "legacy estimate is never actual provider spend"
    );
    assert_eq!(backfill.2, Some(0.12));
    assert_eq!(backfill.3, "legacy_estimate");
    let migration_receipt: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT scanned, inserted, skipped, conflicted
         FROM usage_migration_receipts WHERE version = 'usage-v1'",
    )
    .fetch_one(&pool)
    .await
    .expect("usage migration must leave an auditable receipt");
    assert_eq!(
        migration_receipt,
        (1, 1, 0, 0),
        "legacy migration receipt must distinguish scanned, inserted, skipped, and conflicted rows"
    );

    let dashboard = as_json(
        get_usage_dashboard_data(&pool, 1, 0, Some("2026-07-22T23:00:00Z".into()))
            .await
            .expect("aggregate legacy estimate"),
    );
    assert_eq!(json_i64(&dashboard, "/summary/input_tokens"), 700);
    assert_eq!(json_i64(&dashboard, "/summary/output_tokens"), 70);
    assert!(
        dashboard["summary"]["actual_cost_usd"].is_null(),
        "legacy estimates must not be serialized as zero actual spend"
    );
    assert!((json_f64(&dashboard, "/summary/estimated_cost_usd") - 0.12).abs() < 1e-9);

    pool.close().await;
    let reopened = crate::storage::db::connect(&url)
        .await
        .expect("rerun historical compatibility sync");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM model_usage_events WHERE request_id = 'legacy:legacy-1'",
    )
    .fetch_one(&reopened)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "legacy backfill must be idempotent across restarts"
    );
    let reopened_receipt: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT scanned, inserted, skipped, conflicted
         FROM usage_migration_receipts WHERE version = 'usage-v1'",
    )
    .fetch_one(&reopened)
    .await
    .unwrap();
    assert_eq!(
        reopened_receipt, migration_receipt,
        "restart must preserve the original migration receipt instead of rewriting history"
    );
}

#[tokio::test]
async fn restarting_after_live_usage_does_not_backfill_the_same_assistant_round_again() {
    let dir = tempfile::tempdir().expect("restart backfill tempdir");
    let path = dir.path().join("restart-backfill.db");
    let url = format!("sqlite://{}", path.display());
    let pool = crate::storage::db::connect(&url)
        .await
        .expect("open fresh database");
    let now_ms = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO sessions
         (id, title, cwd, model_id, created_at, updated_at, kind)
         VALUES ('live-session', 'live', '/tmp/project', 'gpt', ?, ?, 'project')",
    )
    .bind(now_ms)
    .bind(now_ms)
    .execute(&pool)
    .await
    .expect("seed live session");

    let mut live = usage_event(
        "live-run:0",
        "live-session",
        "interactive",
        &chrono::Utc::now().to_rfc3339(),
        1_000,
        100,
    );
    live.actual_cost_usd = Some(0.01);
    live.cost_source = "provider_actual".into();
    record_usage_event(&pool, live)
        .await
        .expect("persist live request usage");
    sqlx::query(
        "INSERT INTO messages
         (id, session_id, role, content, model_id, input_tokens, output_tokens, created_at)
         VALUES ('live-message', 'live-session', 'assistant', 'done', 'gpt', 1000, 100, ?)",
    )
    .bind(now_ms)
    .execute(&pool)
    .await
    .expect("persist assistant transcript for the same provider round");
    pool.close().await;

    let reopened = crate::storage::db::connect(&url)
        .await
        .expect("restart and rerun compatibility sync");
    let rows: Vec<(String, String, i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT request_id, source, input_tokens, output_tokens, actual_cost_usd
         FROM model_usage_events WHERE session_id = 'live-session' ORDER BY request_id",
    )
    .fetch_all(&reopened)
    .await
    .expect("read usage after restart");
    assert_eq!(
        rows,
        vec![(
            "live-run:0".into(),
            "provider_usage".into(),
            1_000,
            100,
            Some(0.01),
        )],
        "startup backfill must not duplicate a provider round already recorded live"
    );
}

#[tokio::test]
async fn usage_timestamps_are_normalized_to_canonical_utc_before_text_range_queries() {
    let (_dir, pool) = migrated_pool().await;
    let event = usage_event(
        "offset-timestamp",
        "timestamp-session",
        "interactive",
        "2026-07-22T00:00:00.500+08:00",
        10,
        1,
    );
    record_usage_event(&pool, event)
        .await
        .expect("record valid offset timestamp");

    let stored: String = sqlx::query_scalar(
        "SELECT created_at FROM model_usage_events WHERE request_id = 'offset-timestamp'",
    )
    .fetch_one(&pool)
    .await
    .expect("read canonical timestamp");
    let parsed = chrono::DateTime::parse_from_rfc3339(&stored).expect("stored RFC3339");
    assert_eq!(
        parsed.offset().local_minus_utc(),
        0,
        "stored usage timestamp must use UTC, not the caller's offset"
    );
    assert!(
        stored.ends_with('Z'),
        "stored usage timestamp must use one lexically sortable UTC representation; got {stored}"
    );
    assert_eq!(
        parsed.timestamp_millis(),
        chrono::DateTime::parse_from_rfc3339("2026-07-21T16:00:00.500Z")
            .unwrap()
            .timestamp_millis()
    );

    let detail = as_json(
        get_usage_day_detail_data(&pool, "2026-07-21", 0)
            .await
            .expect("query actual UTC day"),
    );
    assert_eq!(
        json_i64(&detail, "/summary/input_tokens"),
        10,
        "canonical storage must make half-open UTC range queries compare by instant"
    );
}

#[tokio::test]
async fn non_billable_cost_sources_remain_null_in_aggregated_api() {
    let (_dir, pool) = migrated_pool().await;
    for (index, source) in ["subscription", "local", "unknown"].into_iter().enumerate() {
        let date = format!("2026-07-{}T12:00:00Z", 20 + index);
        let mut event = usage_event(
            &format!("non-billable-{source}"),
            &format!("session-{source}"),
            "interactive",
            &date,
            100,
            10,
        );
        event.cost_source = source.into();
        record_usage_event(&pool, event)
            .await
            .unwrap_or_else(|error| panic!("record {source}: {error}"));

        let local_date = format!("2026-07-{}", 20 + index);
        let detail = as_json(
            get_usage_day_detail_data(&pool, &local_date, 0)
                .await
                .unwrap_or_else(|error| panic!("aggregate {source}: {error}")),
        );
        assert!(
            detail["summary"]["actual_cost_usd"].is_null(),
            "{source} has no provider-actual dollars and must serialize actual_cost_usd as null: {detail}"
        );
        assert!(
            detail["summary"]["estimated_cost_usd"].is_null(),
            "{source} has no price estimate and must serialize estimated_cost_usd as null: {detail}"
        );
    }
}

#[tokio::test]
async fn provider_round_without_usage_is_not_reported_as_recorded_zero() {
    let (_dir, pool) = migrated_pool().await;
    sqlx::query(
        "UPDATE usage_tracking_metadata
         SET started_at = '2026-07-22T00:00:00Z' WHERE singleton = 1",
    )
    .execute(&pool)
    .await
    .expect("fix tracking start");
    sqlx::query(
        "INSERT INTO sessions
         (id, title, cwd, model_id, created_at, updated_at, kind)
         VALUES ('missing-usage-session', 'missing usage', '/tmp/project', 'provider-model',
                 1784678400000, 1784678400000, 'project')",
    )
    .execute(&pool)
    .await
    .expect("seed session");
    sqlx::query(
        "INSERT INTO messages
         (id, session_id, role, content, model_id, input_tokens, output_tokens, created_at)
         VALUES ('missing-usage-response', 'missing-usage-session', 'assistant', 'completed',
                 'provider-model', NULL, NULL, 1784678400000)",
    )
    .execute(&pool)
    .await
    .expect("seed completed response whose provider omitted Usage");

    let dashboard = as_json(
        get_usage_dashboard_data(&pool, 1, 0, Some("2026-07-22T23:00:00Z".into()))
            .await
            .expect("aggregate day with missing provider Usage"),
    );
    assert!(
        matches!(
            dashboard["data_status"].as_str(),
            Some("partial") | Some("unavailable")
        ),
        "a completed provider response without Usage must make the aggregate partial/unavailable: {dashboard}"
    );
    assert!(
        matches!(
            day(&dashboard, "2026-07-22")["status"].as_str(),
            Some("partial") | Some("missing")
        ),
        "the heatmap must not label missing Usage as a recorded zero: {dashboard}"
    );
}

#[tokio::test]
async fn budget_threshold_receipts_are_exact_once_across_repeated_queries() {
    let (_dir, pool) = migrated_pool().await;
    let now = "2026-07-22T12:00:00Z";
    let thresholds = vec![0.5, 0.8, 1.0];

    record_usage_event(
        &pool,
        usage_event("budget-49", "budget-session", "interactive", now, 49_000, 0),
    )
    .await
    .unwrap();
    let below = as_json(
        evaluate_usage_budget_data(&pool, 100_000, 3_000_000, &thresholds, 0, Some(now.into()))
            .await
            .unwrap(),
    );
    assert!(below["new_alert"].is_null());

    for (request, tokens, expected) in [
        ("budget-51", 2_000, 0.5),
        ("budget-81", 30_000, 0.8),
        ("budget-101", 20_000, 1.0),
    ] {
        record_usage_event(
            &pool,
            usage_event(request, "budget-session", "interactive", now, tokens, 0),
        )
        .await
        .unwrap();
        let status = as_json(
            evaluate_usage_budget_data(&pool, 100_000, 3_000_000, &thresholds, 0, Some(now.into()))
                .await
                .unwrap(),
        );
        assert_eq!(status["new_alert"]["threshold"], expected);
        let repeated = as_json(
            evaluate_usage_budget_data(&pool, 100_000, 3_000_000, &thresholds, 0, Some(now.into()))
                .await
                .unwrap(),
        );
        assert!(
            repeated["new_alert"].is_null(),
            "same threshold must not alert twice"
        );
    }

    let daily_receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM usage_budget_receipts WHERE period_kind = 'day'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(daily_receipts, 3);
}
