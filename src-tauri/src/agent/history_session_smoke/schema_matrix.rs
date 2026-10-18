// SPDX-License-Identifier: Apache-2.0
//! Synthetic historical migration prefixes. Never opens a user database.
use super::*;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::borrow::Cow;

const SESSION: &str = "schema-matrix-session";
const ROOT: &str = "schema-matrix-root";
const OBJECTIVE: &str = "schema-matrix-objective";

async fn fixture(url: &str, version: i64) -> anyhow::Result<()> {
    let pool = SqlitePoolOptions::new().max_connections(1)
        .connect_with(url.parse::<SqliteConnectOptions>()?.create_if_missing(true)).await?;
    let all = sqlx::migrate!("./migrations");
    let prefix = sqlx::migrate::Migrator {
        migrations: Cow::Owned(all.iter().filter(|m| m.version <= version).cloned().collect()),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    prefix.run(&pool).await?;
    let ledger: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success=1")
        .fetch_one(&pool).await?;
    let provider: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name='provider_route_episodes'")
        .fetch_one(&pool).await?;
    if ledger != version || provider != i64::from(version >= 11) {
        bail!("historical fixture is not the requested schema prefix {version}: ledger={ledger}, provider={provider}");
    }
    sqlx::query("INSERT INTO sessions (id,title,cwd,model_id,created_at,updated_at) VALUES (?, 'Synthetic history', '.', 'synthetic', 1, 1)")
        .bind(SESSION).execute(&pool).await?;
    sqlx::query("INSERT INTO messages (id,session_id,role,content,created_at) VALUES (?,?,'user','执行合成任务并验证结果',1)")
        .bind(ROOT).bind(SESSION).execute(&pool).await?;
    if version >= 6 {
        sqlx::query("INSERT INTO chat_task_segments (id,session_id,ordinal,title,status,goal_root_turn_id,created_at,updated_at) VALUES ('schema-segment',?,1,'Synthetic task','active',?,1,1)")
            .bind(SESSION).bind(ROOT).execute(&pool).await?;
        sqlx::query("INSERT INTO chat_turn_state (root_turn_id,session_id,task_segment_id,phase,status,started_at,updated_at) VALUES (?,?,'schema-segment','recovering','active',1,1)")
            .bind(ROOT).bind(SESSION).execute(&pool).await?;
        if version >= 7 {
            sqlx::query("ALTER TABLE chat_turn_state ADD COLUMN objective_id TEXT").execute(&pool).await?;
            sqlx::query("INSERT INTO objectives (id,kind,session_id,root_turn_id,status,decision_type,domain,requested_acceptance,created_at,updated_at) VALUES (?,'local_mutation',?,?,'active','continue','chat','validated_change',1,1)")
                .bind(OBJECTIVE).bind(SESSION).bind(ROOT).execute(&pool).await?;
            sqlx::query("UPDATE chat_turn_state SET objective_id=?").bind(OBJECTIVE).execute(&pool).await?;
        }
    }
    crate::storage::db::close_and_release_files(pool).await;
    Ok(())
}

async fn counts(pool: &SqlitePool) -> anyhow::Result<(i64,i64,i64,i64)> {
    Ok(sqlx::query_as("SELECT (SELECT COUNT(*) FROM messages), (SELECT COUNT(*) FROM objectives), (SELECT COUNT(*) FROM chat_turn_state), (SELECT COUNT(*) FROM side_effect_receipts)")
        .fetch_one(pool).await?)
}

pub(super) async fn run(root: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut rows = Vec::new();
    for version in [1, 6, 10, 19] {
        let url = format!("sqlite:{}", root.join(format!("schema-{version}.db")).display());
        fixture(&url, version).await.map_err(|error| anyhow!("seed schema {version}: {error:#}"))?;
        let pool = crate::storage::db::connect(&url).await?;
        let ledger: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success=1")
            .fetch_one(&pool).await?;
        let latest = sqlx::migrate!("./migrations").iter().map(|m| m.version).max().unwrap();
        if ledger != latest { bail!("schema {version} did not migrate to current ledger"); }
        let before = counts(&pool).await?;
        let admission = crate::commands::chat::admit_headless_chat_turn(&pool, SESSION, "继续").await;
        let expected_id;
        if version == 1 {
            let error = match admission { Ok(_) => bail!("identity-free history must not create a new task"), Err(e) => e.to_string() };
            if !error.contains("这段历史记录缺少可续接的任务身份") || counts(&pool).await? != before {
                bail!("identity-free history did not fail atomically with a readable terminal");
            }
            expected_id = None;
        } else {
            let admission = admission.map_err(|e| anyhow!(e.to_string()))?;
            if admission.objective.session_id.as_deref() != Some(SESSION)
                || admission.objective.root_turn_id.as_deref() != Some(ROOT)
                || (version >= 7 && admission.objective.id != OBJECTIVE) {
                bail!("schema {version} changed historical identity");
            }
            expected_id = Some(admission.objective.id);
            let state = counts(&pool).await?;
            if state != (2,1,2,0) { bail!("schema {version} unexpected admission counts {state:?}"); }
        }
        let settled = counts(&pool).await?;
        crate::storage::db::close_and_release_files(pool).await;
        let reopened = crate::storage::db::connect(&url).await?;
        if counts(&reopened).await? != settled { bail!("schema {version} reopen duplicated state"); }
        if let Some(id) = expected_id {
            let binding: (String,String,String) = sqlx::query_as("SELECT id,session_id,root_turn_id FROM objectives").fetch_one(&reopened).await?;
            if binding != (id, SESSION.into(), ROOT.into()) { bail!("schema {version} lost durable identity"); }
        } else {
            let error = crate::commands::chat::admit_headless_chat_turn(&reopened, SESSION, "继续").await;
            if error.is_ok() || counts(&reopened).await? != settled { bail!("terminal retry was not idempotent"); }
        }
        let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE role='user'").fetch_one(&reopened).await?;
        if user_count != if version == 1 { 1 } else { 2 } { bail!("matrix required extra human input"); }
        crate::storage::db::close_and_release_files(reopened).await;
        rows.push(serde_json::json!({"source_version":version,"migrated":true,
            "identity_preserved":true,"reopen_idempotent":true,"side_effect_count":settled.3,
            "human_prompt_count":0,"outcome":if version==1 {"explicit_terminal"} else {"continued"}}));
    }
    Ok(rows)
}
