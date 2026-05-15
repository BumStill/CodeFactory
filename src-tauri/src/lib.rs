// SPDX-License-Identifier: Apache-2.0
mod agent;
mod commands;
mod config;
mod errors;
mod openrouter;
mod secrets;
mod storage;
mod tools;

use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::{oneshot, Mutex, RwLock};

pub type PendingPermissionMap = Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>;

pub struct AppState {
    pub db: Arc<RwLock<SqlitePool>>,
    pub settings: Arc<RwLock<config::Settings>>,
    pub pending_permissions: PendingPermissionMap,
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().expect("app data dir unavailable");
            std::fs::create_dir_all(&data_dir)?;

            let db_url = format!("sqlite:{}", data_dir.join("codefactory.db").display());
            let settings = config::settings::load();

            let pool = tauri::async_runtime::block_on(storage::db::connect(&db_url))?;

            app.manage(AppState {
                db: Arc::new(RwLock::new(pool)),
                settings: Arc::new(RwLock::new(settings)),
                pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::get_api_key,
            commands::settings::save_api_key,
            commands::settings::delete_api_key,
            commands::models::list_models,
            commands::session::list_sessions,
            commands::session::create_session,
            commands::session::get_session,
            commands::session::delete_session,
            commands::session::update_session_model,
            commands::session::get_messages,
            commands::chat::send_message,
            commands::chat::respond_to_permission,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CodeFactory");
}
