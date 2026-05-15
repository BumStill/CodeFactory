// SPDX-License-Identifier: Apache-2.0
use sqlx::{migrate::MigrateDatabase, sqlite::SqlitePoolOptions, SqlitePool};

pub async fn connect(db_path: &str) -> crate::errors::Result<SqlitePool> {
    if !sqlx::Sqlite::database_exists(db_path)
        .await
        .unwrap_or(false)
    {
        sqlx::Sqlite::create_database(db_path).await?;
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(db_path)
        .await?;
    sqlx::migrate!("../migrations").run(&pool).await?;
    // Enable FK enforcement (SQLite disables it by default).
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;
    Ok(pool)
}
