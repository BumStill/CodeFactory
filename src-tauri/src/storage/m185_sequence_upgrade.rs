// SPDX-License-Identifier: Apache-2.0
//! M185: production startup must repair checksum-divergent historical databases.
use crate::storage;

#[tokio::test]
async fn m185_empty_database_can_admit_and_replay_new_turn() {
    let root = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}", root.path().join("fresh.db").display());
    let pool = storage::db::connect(&url).await.unwrap();
    sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at) VALUES ('fresh','Synthetic','.','synthetic',0,0)").execute(&pool).await.unwrap();
    crate::commands::chat::admit_headless_chat_turn(&pool, "fresh", "合成首回合").await.unwrap();
    let history = storage::load_agent_history(&pool, "fresh").await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "合成首回合");
    assert_eq!((history[0].seq, history[0].revision), (Some(1), 1));
    pool.close().await;
}

#[tokio::test]
async fn m185_checksum_mismatch_repairs_sequence_before_chat_history() {
    check_upgrade(false).await;
}

#[tokio::test]
async fn m185_partial_mem_a_install_is_repaired_without_renumbering() {
    check_upgrade(true).await;
}

async fn check_upgrade(partial: bool) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("legacy.db");
    let url = format!("sqlite:{}", path.display());
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
        .connect_with(url.parse::<sqlx::sqlite::SqliteConnectOptions>().unwrap().create_if_missing(true))
        .await.unwrap();
    sqlx::raw_sql(include_str!("fixtures/v1_91_44_pre_mem_a.sql"))
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO sessions (id,title,cwd,model_id,created_at,updated_at) VALUES ('s','Synthetic','.','synthetic',0,0)").execute(&pool).await.unwrap();
    for (id, time) in [("later", 2), ("first", 1), ("tied", 1)] {
        sqlx::query("INSERT INTO messages (id,session_id,role,content,created_at) VALUES (?,'s','user','synthetic',?)").bind(id).bind(time).execute(&pool).await.unwrap();
    }
    if partial {
        sqlx::raw_sql("ALTER TABLE messages ADD COLUMN seq INTEGER; UPDATE messages SET seq=1 WHERE id='first';").execute(&pool).await.unwrap();
    }
    pool.close().await;
    for _ in 0..2 {
        let pool = storage::db::connect(&url).await.unwrap();
        let history = storage::load_agent_history(&pool, "s").await.unwrap();
        assert_eq!(history.iter().map(|m| (m.id.as_str(), m.seq, m.revision)).collect::<Vec<_>>(), vec![("first",Some(1),1),("tied",Some(2),1),("later",Some(3),1)]);
        pool.close().await;
    }
    let pool = storage::db::connect(&url).await.unwrap();
    let admitted = crate::commands::chat::admit_headless_chat_turn(&pool, "s", "合成新回合").await.unwrap();
    assert_eq!(admitted.objective.session_id.as_deref(), Some("s"));
    let history = storage::load_agent_history(&pool, "s").await.unwrap();
    assert_eq!(history.last().unwrap().content, "合成新回合");
    assert_eq!(history.last().unwrap().seq, Some(4));
    sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('reply','s','assistant','synthetic reply',3)").execute(&pool).await.unwrap();
    sqlx::query("UPDATE messages SET content='edited reply' WHERE id='reply'").execute(&pool).await.unwrap();
    let reply: (i64,i64) = sqlx::query_as("SELECT seq,revision FROM messages WHERE id='reply'").fetch_one(&pool).await.unwrap();
    assert_eq!(reply, (5,2));
    for table in ["feature_flags", "assembly_manifests"] {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?").bind(table).fetch_one(&pool).await.unwrap();
        assert_eq!(count,1);
    }
    pool.close().await;
}
