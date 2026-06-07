// SPDX-License-Identifier: Apache-2.0
mod agent;
mod codex_auth;
mod commands;
mod config;
mod errors;
mod git_remote;
mod http_util;
mod knowledge;
mod mcp;
mod openrouter;
mod secrets;
mod storage;
mod tools;
mod util;

use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::{oneshot, Mutex, RwLock};

pub type PendingPermissionMap = Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>;

/// Per-chat-session cancel flags. Set by the `cancel_chat` command (the chat
/// "stop" button) and polled cooperatively by the chat agent loop between
/// rounds. Entirely separate from the task scheduler's cancel flags — stopping
/// a chat turn never touches running tasks.
pub type ChatCancelMap = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

/// Make a once-per-day copy of the SQLite DB so a botched schema migration
/// or accidental delete isn't unrecoverable. Files named
/// `codefactory.db.backup-YYYYMMDD`; older than 7 days are pruned.
///
/// Best-effort: if any step fails (no DB yet, permission error, etc.) we
/// just log and continue — backups are an extra safety net, not a hard dep.
fn backup_db_daily(data_dir: &std::path::Path, db_path: &std::path::Path) -> std::io::Result<()> {
    // No DB yet → nothing to back up (fresh install).
    if !db_path.exists() {
        return Ok(());
    }

    // Local date as YYYYMMDD — chrono is already a dependency.
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let backup_path = data_dir.join(format!("codefactory.db.backup-{today}"));

    // Skip if we already snapshotted today.
    if !backup_path.exists() {
        std::fs::copy(db_path, &backup_path)?;
        tracing::info!("db backup written: {}", backup_path.display());
    }

    // Prune backups older than 7 days. Tolerate per-entry errors so one
    // stuck file doesn't block the rest.
    let cutoff = chrono::Local::now() - chrono::Duration::days(7);
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(date_str) = name.strip_prefix("codefactory.db.backup-") else { continue };
            // Parse YYYYMMDD; ignore malformed.
            let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y%m%d") else { continue };
            let backup_local = date.and_hms_opt(0, 0, 0).unwrap();
            if backup_local < cutoff.naive_local() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

pub struct AppState {
    pub db: Arc<RwLock<SqlitePool>>,
    pub settings: Arc<RwLock<config::Settings>>,
    pub pending_permissions: PendingPermissionMap,
    pub chat_cancels: ChatCancelMap,
    pub interjections: commands::interjections::InterjectionQueue,
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().expect("app data dir unavailable");
            std::fs::create_dir_all(&data_dir)?;

            // Rolling daily DB backup — one snapshot per day, 7-day retention.
            // Best-effort: failures are logged and never block startup.
            let db_path = data_dir.join("codefactory.db");
            if let Err(e) = backup_db_daily(&data_dir, &db_path) {
                tracing::warn!("db backup skipped: {e}");
            }

            let db_url = format!("sqlite:{}", db_path.display());
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
                chat_cancels: Arc::new(Mutex::new(HashMap::new())),
                interjections: Arc::new(Mutex::new(HashMap::new())),
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
            codex_auth::codex_login,
            codex_auth::codex_logout,
            codex_auth::codex_account,
            commands::backup::export_user_data,
            commands::backup::import_user_data,
            commands::backup::get_data_dir,
            commands::checkpoints::create_checkpoint,
            commands::checkpoints::list_checkpoints,
            commands::checkpoints::checkpoint_changeset,
            commands::checkpoints::revert_checkpoint,
            commands::memory::read_project_memory,
            commands::memory::write_project_memory,
            commands::memory::append_project_memory,
            commands::learning::list_learning_events,
            commands::learning::accept_learning_event,
            commands::learning::reject_learning_event,
            commands::learning::run_postmortem,
            commands::learning::mine_cross_session_patterns,
            commands::learning::self_improvement_proposal,
            commands::learning::propose_tool_gates,
            commands::learning::apply_tool_gate,
            commands::preferences::list_user_preferences,
            commands::preferences::upsert_user_preference,
            commands::preferences::delete_user_preference,
            commands::interjections::queue_interjection,
            commands::interjections::list_interjections,
            commands::knowledge::register_knowledge_library,
            commands::knowledge::list_knowledge_libraries,
            commands::knowledge::scan_knowledge_library,
            commands::knowledge::search_knowledge,
            commands::models::list_models,
            commands::session::list_sessions,
            commands::session::create_session,
            commands::session::get_or_create_quick_session,
            commands::session::create_quick_session,
            commands::session::list_quick_sessions,
            commands::session::update_session_reasoning_effort,
            commands::session::get_session,
            commands::session::delete_session,
            commands::session::update_session_model,
            commands::session::set_endpoint_active_model,
            commands::session::get_endpoint_active_model,
            commands::session::update_session_title,
            commands::session::get_messages,
            commands::chat::send_message,
            commands::chat::send_message_anonymous,
            commands::chat::respond_to_permission,
            commands::chat::cancel_chat,
            commands::files::list_dir,
            commands::files::save_chat_attachment,
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
            commands::specs::decompose_spec_to_tasks,
            commands::specs::decompose_request_to_tasks,
            commands::evidence::generate_evidence_pack,
            commands::evidence::list_evidence_packs,
            commands::evidence::get_evidence_pack,
            commands::evidence::open_evidence_pack_dir,
            commands::skills::list_skills,
            commands::skills::get_skill,
            commands::skills::enable_skill,
            commands::skills::disable_skill,
            commands::skills::install_skill_from_url,
            commands::skills::install_skill_from_directory,
            commands::skills::create_skill,
            commands::skills::update_skill,
            commands::skills::delete_skill,
            commands::skills::list_slash_commands,
            commands::skills::propose_skills_from_patterns,
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
            commands::costs::get_session_cost,
            commands::costs::get_today_cost,
            commands::costs::get_monthly_cost,
            commands::costs::get_costs_by_model,
            commands::costs::list_recent_cost_entries,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CodeFactory");
}
