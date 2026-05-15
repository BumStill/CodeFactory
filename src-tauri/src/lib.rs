// SPDX-License-Identifier: Apache-2.0
mod agent;
mod commands;
mod config;
mod errors;
mod git_remote;
mod mcp;
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

            // Create a single McpManager and wrap in Arc so it can be shared with
            // the startup task and also managed by Tauri.
            let mcp_manager = Arc::new(mcp::McpManager::new());

            // Auto-start enabled MCP servers in background
            let enabled_servers: Vec<_> = settings
                .mcp_servers
                .iter()
                .filter(|s| s.enabled)
                .cloned()
                .collect();
            {
                let mcp_clone = Arc::clone(&mcp_manager);
                tauri::async_runtime::spawn(async move {
                    for cfg in enabled_servers {
                        if let Err(e) = mcp_clone.start_server(cfg.clone()).await {
                            tracing::warn!("Failed to start MCP server '{}': {e}", cfg.id);
                        }
                    }
                });
            }

            app.manage(AppState {
                db: Arc::new(RwLock::new(pool)),
                settings: Arc::new(RwLock::new(settings)),
                pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            });
            // Manage the Arc so all commands share the same McpManager instance.
            app.manage(mcp_manager);
            app.manage(commands::terminal::TerminalState::new());
            // Phase 2: per-session scheduler cancel flags.
            let scheduler_handles: commands::tasks::SchedulerHandles =
                Arc::new(Mutex::new(HashMap::new()));
            app.manage(scheduler_handles);

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
            commands::session::update_session_title,
            commands::session::get_messages,
            commands::chat::send_message,
            commands::chat::respond_to_permission,
            commands::files::list_dir,
            commands::terminal::terminal_create,
            commands::terminal::terminal_write,
            commands::terminal::terminal_resize,
            commands::terminal::terminal_kill,
            commands::git::git_status,
            commands::git::git_log,
            commands::git::git_branches,
            commands::git::git_diff,
            commands::git::git_file_diff,
            commands::git::git_add,
            commands::git::git_commit,
            commands::git::git_checkout,
            commands::git::git_create_branch,
            commands::git::git_push,
            commands::git::git_pull,
            commands::tasks::create_task_tree,
            commands::tasks::list_tasks,
            commands::tasks::get_task_detail,
            commands::tasks::get_task_dependencies,
            commands::tasks::start_implementation,
            commands::tasks::cancel_implementation,
            commands::tasks::get_verification_results,
            commands::tasks::run_verification_now,
            commands::specs::list_specs,
            commands::specs::get_spec,
            commands::specs::save_spec,
            commands::specs::create_spec,
            commands::specs::delete_spec,
            commands::specs::approve_spec,
            commands::specs::spec_ai_assist,
            commands::evidence::generate_evidence_pack,
            commands::evidence::list_evidence_packs,
            commands::evidence::get_evidence_pack,
            commands::evidence::open_evidence_pack_dir,
            commands::skills::list_skills,
            commands::skills::get_skill,
            commands::skills::enable_skill,
            commands::skills::disable_skill,
            commands::skills::install_skill_from_url,
            commands::skills::delete_skill,
            commands::skills::list_slash_commands,
            commands::skills::fetch_marketplace_skills,
            commands::skills::install_marketplace_skill,
            commands::hooks::list_hooks,
            commands::hooks::add_hook,
            commands::hooks::update_hook,
            commands::hooks::delete_hook,
            commands::hooks::test_hook,
            commands::mcp::list_mcp_servers,
            commands::mcp::add_mcp_server,
            commands::mcp::update_mcp_server,
            commands::mcp::delete_mcp_server,
            commands::mcp::enable_mcp_server,
            commands::mcp::disable_mcp_server,
            commands::mcp::list_mcp_tools,
            commands::mcp::test_mcp_tool,
            commands::git_remote::list_git_remotes,
            commands::git_remote::add_git_remote,
            commands::git_remote::delete_git_remote,
            commands::git_remote::test_git_remote,
            commands::git_remote::list_issues,
            commands::git_remote::get_issue,
            commands::git_remote::create_issue,
            commands::git_remote::list_prs,
            commands::git_remote::create_pr,
            commands::git_remote::list_repos,
            commands::git_remote::issue_to_spec,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CodeFactory");
}
