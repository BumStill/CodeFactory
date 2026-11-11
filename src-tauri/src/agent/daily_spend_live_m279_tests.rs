// SPDX-License-Identifier: Apache-2.0
//! M279: the USD ledger must survive an app restart.

#[cfg(test)]
mod m279_tests {
    use crate::agent::daily_spend_live::rebuild_usd;
    use crate::agent::openai_api::UsdSpendLedger;
    use sqlx::SqlitePool;

    fn at(iso: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .timestamp_millis()
    }

    #[tokio::test]
    async fn rebuild_usd_restores_today_openai_events_only() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE model_usage_events (
                created_at TEXT NOT NULL,
                endpoint TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                cached_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                estimated_cost_usd REAL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let now = at("2026-10-30T02:00:00Z");
        let yesterday = now - 86_400_000;
        let insert = "INSERT INTO model_usage_events
            (created_at, endpoint, model, input_tokens, cached_tokens, output_tokens, estimated_cost_usd)
            VALUES (datetime(? / 1000.0, 'unixepoch'), ?, ?, ?, ?, ?, ?)";
        // Today, stored cost wins.
        sqlx::query(insert)
            .bind(now)
            .bind("openai")
            .bind("gpt-5.6")
            .bind(1_000_i64)
            .bind(0_i64)
            .bind(1_000_i64)
            .bind(Some(0.24_f64))
            .execute(&pool)
            .await
            .unwrap();
        // Today, NULL cost → priced from the same rate card as live recording.
        sqlx::query(insert)
            .bind(now)
            .bind("openai")
            .bind("gpt-5.6")
            .bind(1_000_i64)
            .bind(0_i64)
            .bind(1_000_i64)
            .bind(None::<f64>)
            .execute(&pool)
            .await
            .unwrap();
        // Previous local day and a non-USD endpoint must not leak in.
        sqlx::query(insert)
            .bind(yesterday)
            .bind("openai")
            .bind("gpt-5.6")
            .bind(9_999_i64)
            .bind(0_i64)
            .bind(9_999_i64)
            .bind(Some(9.99_f64))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(insert)
            .bind(now)
            .bind("deepseek")
            .bind("deepseek-v4-pro")
            .bind(1_000_i64)
            .bind(0_i64)
            .bind(1_000_i64)
            .bind(None::<f64>)
            .execute(&pool)
            .await
            .unwrap();

        let ledger = UsdSpendLedger::default();
        rebuild_usd(&pool, &ledger, now).await.unwrap();

        let fallback = crate::agent::openai_api::estimate_cost_usd("gpt-5.6", 1_000, 0, 1_000);
        let expected = 0.24 + fallback;
        let recovered = ledger.spent_usd("openai", now);
        assert!(
            (recovered - expected).abs() < 1e-9,
            "restart must restore exactly today's USD events (got {recovered}, want {expected})"
        );
        // The restored total is real headroom consumption, not free again.
        assert!(ledger.try_reserve("openai", 0.01, expected, now).is_none());
        assert!(ledger.try_reserve("openai", 0.01, expected + 1.0, now).is_some());
    }
}
