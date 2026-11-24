// SPDX-License-Identifier: Apache-2.0
//! M217: durable daily accounting shared by startup, routing and dispatch.
use super::quota_cap::daily_spend::{self, SpendLedger};
use crate::config::settings::Settings;
use sqlx::{Row, SqlitePool};
use std::sync::OnceLock;

static DB: OnceLock<SqlitePool> = OnceLock::new();
static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn install_database(pool: SqlitePool) { let _ = DB.set(pool); }

/// Rebuild into a fresh ledger, including legacy NULL prices. Timestamp parsing
/// happens in SQLite so historical RFC3339 offsets cannot evade the day bounds.
pub async fn rebuild(pool: &SqlitePool, settings: &Settings, ledger: &SpendLedger, now: i64) -> crate::errors::Result<()> {
    let start = daily_spend::day_start_ms(now).unwrap_or(now);
    let end = daily_spend::next_reset_ms(now).unwrap_or(now);
    let rows = sqlx::query("SELECT endpoint, estimated_cost_cny, input_tokens, cached_tokens, output_tokens FROM model_usage_events WHERE julianday(created_at) >= julianday(? / 1000.0, 'unixepoch') AND julianday(created_at) < julianday(? / 1000.0, 'unixepoch')")
        .bind(start).bind(end).fetch_all(pool).await?;
    let mut totals = std::collections::HashMap::<String, f64>::new();
    for row in rows {
        let endpoint: String = row.try_get("endpoint")?;
        let cost: Option<f64> = row.try_get("estimated_cost_cny")?;
        let cost = cost.or_else(|| settings.endpoint_daily_spend_cap(&endpoint).map(|config| config.prices.estimate_cny(
            row.get("input_tokens"), row.get("cached_tokens"), row.get("output_tokens")))).unwrap_or(0.0);
        if cost.is_finite() && cost >= 0.0 { *totals.entry(endpoint).or_default() += cost; }
    }
    ledger.restore_estimates(totals, now);
    let rows = sqlx::query("SELECT endpoint, opening_cny, topped_up_cny, current_cny FROM endpoint_daily_balances WHERE day_start_ms = ?")
        .bind(start).fetch_all(pool).await?;
    for row in rows {
        ledger.restore_balance(&row.get::<String, _>("endpoint"), row.get("opening_cny"), row.get("topped_up_cny"), row.get("current_cny"), now);
    }
    // M279 / R1：美元账本和人民币账本在同一个启动步骤重建。重启后当天已花的
    // 美元必须从用量事件恢复，否则日上限在重启瞬间被清零。
    rebuild_usd(pool, super::openai_api::ledger(), now).await?;
    Ok(())
}

/// M279 / R1: rebuild today's USD totals from `model_usage_events`. The stored
/// `estimated_cost_usd` wins; a legacy NULL row is priced from the same rate
/// card the live recording path uses, and an unknown model stays conservative
/// rather than free.
pub async fn rebuild_usd(
    pool: &SqlitePool,
    ledger: &super::openai_api::UsdSpendLedger,
    now: i64,
) -> crate::errors::Result<()> {
    let start = daily_spend::day_start_ms(now).unwrap_or(now);
    let end = daily_spend::next_reset_ms(now).unwrap_or(now);
    let rows = sqlx::query("SELECT endpoint, estimated_cost_usd, model, input_tokens, cached_tokens, output_tokens FROM model_usage_events WHERE julianday(created_at) >= julianday(? / 1000.0, 'unixepoch') AND julianday(created_at) < julianday(? / 1000.0, 'unixepoch')")
        .bind(start).bind(end).fetch_all(pool).await?;
    let mut totals = std::collections::HashMap::<String, f64>::new();
    for row in rows {
        let endpoint: String = row.try_get("endpoint")?;
        if !super::openai_api::is_usd_endpoint(&endpoint) {
            continue;
        }
        let cost: Option<f64> = row.try_get("estimated_cost_usd")?;
        let cost = cost.unwrap_or_else(|| {
            super::openai_api::estimate_cost_usd(
                &row.get::<String, _>("model"),
                row.get("input_tokens"),
                row.get("cached_tokens"),
                row.get("output_tokens"),
            )
        });
        if cost.is_finite() && cost >= 0.0 {
            *totals.entry(endpoint).or_default() += cost;
        }
    }
    ledger.restore_estimates(totals, now);
    Ok(())
}

/// Fixed, credential-safe failure handling: no provider body, URL or error is
/// logged. Failed reads use token estimates; persisted opening/top-ups survive.
pub async fn refresh(settings: &Settings, force: bool) {
    let Some(pool) = DB.get() else { return; };
    let _guard = REFRESH.lock().await;
    let now = chrono::Utc::now().timestamp_millis();
    let ledger = daily_spend::ledger();
    if rebuild(pool, settings, ledger, now).await.is_err() {
        tracing::warn!("daily spend database refresh failed; retaining ledger");
        return;
    }
    static ATTEMPTS: OnceLock<std::sync::Mutex<std::collections::HashMap<String, (i64, bool)>>> = OnceLock::new();
    let attempts = ATTEMPTS.get_or_init(Default::default);
    let client = match reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(5)).build() {
        Ok(client) => client, Err(_) => return,
    };
    for (name, endpoint) in &settings.endpoints {
        if !name.to_ascii_lowercase().contains("deepseek") { continue; }
        let cached = attempts.lock().expect("balance cache poisoned").get(name).copied();
        if !force {
            if let Some((at, success)) = cached {
                if now - at < 60_000 && daily_spend::day_start_ms(at) == daily_spend::day_start_ms(now) {
                    if !success { ledger.balance_unavailable(name, now); }
                    continue;
                }
            }
        }
        let balance = async {
            let key_ref = endpoint.key_ref.as_deref()?;
            let key = crate::secrets::get_key(key_ref).ok()??;
            fetch_balance(&client, &endpoint.base_url, &key).await
        }.await;
        let success = if let Some(balance) = balance {
            observe_and_persist(pool, ledger, name, balance, now).await.is_ok()
        } else { false };
        attempts.lock().expect("balance cache poisoned").insert(name.clone(), (now, success));
        if !success { ledger.balance_unavailable(name, now); }
    }
}

async fn fetch_balance(client: &reqwest::Client, base_url: &str, key: &str) -> Option<f64> {
    let url = format!("{}/user/balance", base_url.trim_end_matches('/').trim_end_matches("/v1"));
    let response = client.get(url).bearer_auth(key).send().await.ok()?;
    if !response.status().is_success() { return None; }
    let body = response.text().await.ok()?;
    daily_spend::parse_balance_cny(&body).filter(|balance| balance.is_finite() && *balance >= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn m217_balance_transport_uses_saved_credential_route_and_sanitizes_failures() {
        use std::io::{Read, Write};
        for (status, body, expected) in [
            ("200 OK", r#"{"balance_infos":[{"currency":"CNY","total_balance":"17.35"}]}"#, Some(17.35)),
            ("503 Unavailable", "synthetic upstream error", None),
            ("200 OK", "invalid json", None),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/v1", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut bytes = [0; 4096];
                let count = stream.read(&mut bytes).unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]).to_ascii_lowercase();
                assert!(request.starts_with("get /user/balance http/1.1"));
                assert!(request.contains("authorization: bearer synthetic-m217-key"));
                write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let client = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(5)).build().unwrap();
            assert_eq!(fetch_balance(&client, &url, "synthetic-m217-key").await, expected);
            server.join().unwrap();
        }
    }
}

pub async fn observe_and_persist(pool: &SqlitePool, ledger: &SpendLedger, endpoint: &str, balance: f64, now: i64) -> crate::errors::Result<()> {
    ledger.observe_balance_with_estimate(endpoint, balance, now);
    let (opening, topped_up, current) = ledger.balance_state(endpoint, now).expect("observed balance");
    sqlx::query("INSERT INTO endpoint_daily_balances(endpoint, day_start_ms, opening_cny, topped_up_cny, current_cny) VALUES (?, ?, ?, ?, ?) ON CONFLICT(endpoint, day_start_ms) DO UPDATE SET opening_cny=excluded.opening_cny, topped_up_cny=excluded.topped_up_cny, current_cny=excluded.current_cny")
        .bind(endpoint).bind(daily_spend::day_start_ms(now).unwrap_or(now)).bind(opening).bind(topped_up).bind(current).execute(pool).await?;
    Ok(())
}
