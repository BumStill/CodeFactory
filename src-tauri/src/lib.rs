// SPDX-License-Identifier: Apache-2.0
mod agent;
mod ai_text;
mod benchmark;
mod benchmark_consistency;
mod codex_auth;
mod commands;
mod config;
mod credential_broker;
mod errors;
mod git_remote;
mod http_util;
mod knowledge;
mod mcp;
mod notify;
mod openrouter;
mod secrets;
mod storage;
mod tools;
mod trajectory;
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
            let Some(date_str) = name.strip_prefix("codefactory.db.backup-") else {
                continue;
            };
            // Parse YYYYMMDD; ignore malformed.
            let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y%m%d") else {
                continue;
            };
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

/// Handle the release-only Evolution smoke mode before Tauri initializes.
/// Returns `false` for ordinary app startup and exits non-zero on smoke failure.
pub fn run_evolution_smoke_cli() -> bool {
    let mut args = std::env::args();
    let _program = args.next();
    let Some(flag) = args.next() else {
        return false;
    };
    if flag != "--evolution-smoke" {
        return false;
    }
    let Some(output) = args.next() else {
        eprintln!("usage: CodeFactory --evolution-smoke <receipt.json>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: CodeFactory --evolution-smoke <receipt.json>");
        std::process::exit(2);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Evolution release smoke could not start: {error}");
            std::process::exit(1);
        });
    match runtime.block_on(commands::evolution::run_release_smoke(
        std::path::Path::new(&output),
    )) {
        Ok(receipt) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&receipt).unwrap_or_default()
            );
            true
        }
        Err(error) => {
            eprintln!("Evolution release smoke failed: {error}");
            std::process::exit(1);
        }
    }
}

/// Release/runtime smoke for the native browser-session manager. It exercises
/// the production dispatch path against a real page, injects an action failure,
/// and proves the lease is reclaimed before the process exits.
#[cfg(not(test))]
pub fn run_browser_session_smoke_cli() -> bool {
    let mut args = std::env::args();
    let _program = args.next();
    let Some(flag) = args.next() else {
        return false;
    };
    if flag != "--browser-session-smoke" {
        return false;
    }
    let Some(output) = args.next() else {
        eprintln!("usage: CodeFactory --browser-session-smoke <receipt.json>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: CodeFactory --browser-session-smoke <receipt.json>");
        std::process::exit(2);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Browser-session smoke could not start: {error}");
            std::process::exit(1);
        });
    let output_path = std::path::PathBuf::from(output);
    let result = runtime.block_on(async {
        let cwd = output_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let owner_session_id = format!("browser-smoke-{}", uuid::Uuid::new_v4());
        let ctx = tools::ExecCtx {
            cwd,
            app: None,
            db: None,
            session_id: Some(owner_session_id.clone()),
            root_turn_id: None,
            task_id: None,
            knowledge_library_ids: None,
            settings: None,
        };

        let opened = tools::browser_session::execute(
            serde_json::json!({"action":"open","url":"https://example.com"}),
            &ctx,
        )
        .await?;
        if opened.is_error {
            return Err(errors::AppError::Other(opened.content));
        }
        let session_id = opened
            .content
            .lines()
            .find_map(|line| line.strip_prefix("Managed browser session: "))
            .ok_or_else(|| errors::AppError::Other("smoke did not receive a session id".into()))?
            .to_string();

        let snapshot = tools::browser_session::execute(
            serde_json::json!({"action":"snapshot","session_id":session_id}),
            &ctx,
        )
        .await?;
        let failed_action = tools::browser_session::execute(
            serde_json::json!({
                "action":"click",
                "session_id":session_id,
                "target":"e999999"
            }),
            &ctx,
        )
        .await?;
        let lease_gone = !tools::browser_session::list_managed_sessions()
            .iter()
            .any(|session| session.session_id == session_id);
        let receipt = serde_json::json!({
            "status": if !snapshot.is_error && failed_action.is_error && lease_gone {
                "passed"
            } else {
                "failed"
            },
            "native_tool": "browser_session",
            "opened_session": session_id,
            "snapshot_ok": !snapshot.is_error,
            "failure_detected": failed_action.is_error,
            "lease_reclaimed_after_failure": lease_gone,
        });
        std::fs::write(
            &output_path,
            serde_json::to_vec_pretty(&receipt).unwrap_or_default(),
        )?;
        tools::browser_session::close_for_session(&owner_session_id).await;
        if receipt["status"] == "passed" {
            Ok(receipt)
        } else {
            Err(errors::AppError::Other(receipt.to_string()))
        }
    });

    match result {
        Ok(receipt) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&receipt).unwrap_or_default()
            );
            true
        }
        Err(error) => {
            eprintln!("Browser-session smoke failed: {error}");
            std::process::exit(1);
        }
    }
}

/// Handle the release-only headless-construction smoke before Tauri
/// initializes (keystone slice 3). Returns `false` for ordinary app startup
/// and exits non-zero on smoke failure. Proves the packaged binary can build
/// the real `AgentLoop` with no `AppHandle` — the Windows loader path #166
/// made fragile — as a `not(test)` binary rather than the unit-test EXE.
///
/// `#[cfg(not(test))]`: as a crate-public root it would otherwise force
/// `AgentLoop` construction (and its Tauri `AppHandle` machinery) into the
/// unit-test EXE, whose Windows loader aborts with `STATUS_ENTRYPOINT_NOT_FOUND`
/// (#166). The bin (`main.rs`) links the non-test lib, so it still sees this.
#[cfg(not(test))]
pub fn run_headless_smoke_cli() -> bool {
    let mut args = std::env::args();
    let _program = args.next();
    let Some(flag) = args.next() else {
        return false;
    };
    if flag != "--headless-smoke" {
        return false;
    }
    let Some(output) = args.next() else {
        eprintln!("usage: CodeFactory --headless-smoke <receipt.json>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: CodeFactory --headless-smoke <receipt.json>");
        std::process::exit(2);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Headless smoke could not start: {error}");
            std::process::exit(1);
        });
    match runtime.block_on(agent::AgentLoop::run_headless_smoke(std::path::Path::new(
        &output,
    ))) {
        Ok(receipt) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&receipt).unwrap_or_default()
            );
            true
        }
        Err(error) => {
            eprintln!("Headless smoke failed: {error}");
            std::process::exit(1);
        }
    }
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

            tauri::async_runtime::spawn(async {
                let reclaimed = tools::browser_session::reclaim_on_startup().await;
                if reclaimed > 0 {
                    tracing::info!(
                        "startup: reclaimed {reclaimed} browser session(s) from the previous run"
                    );
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::save_api_key,
            commands::settings::delete_api_key,
            codex_auth::codex_login,
            codex_auth::codex_login_start,
            codex_auth::codex_login_status,
            codex_auth::codex_login_open,
            codex_auth::codex_login_cancel,
            codex_auth::codex_logout,
            codex_auth::codex_account,
            codex_auth::codex_models,
            codex_auth::apply_codex_models,
            commands::backup::export_user_data,
            commands::backup::import_user_data,
            commands::backup::get_data_dir,
            commands::benchmark::list_benchmark_profiles,
            commands::benchmark::probe_benchmark_environment,
            commands::benchmark::preview_benchmark_provider_bridge,
            commands::benchmark::start_benchmark_provider_run,
            commands::benchmark::import_benchmark_results,
            commands::benchmark::benchmark_consistency_report,
            commands::browser_sessions::list_browser_sessions,
            commands::browser_sessions::close_browser_session,
            commands::checkpoints::list_checkpoints,
            commands::checkpoints::checkpoint_changeset,
            commands::checkpoints::revert_checkpoint,
            commands::control_plane::get_control_plane_snapshot,
            commands::memory::read_project_memory,
            commands::memory::write_project_memory,
            commands::learning::list_learning_events,
            commands::learning::list_evolution_jobs,
            commands::learning::list_evolution_decision_jobs,
            commands::learning::get_evolution_job,
            commands::learning::list_evolution_job_events,
            commands::learning::reject_learning_event,
            commands::learning::run_postmortem,
            commands::learning::mine_cross_session_patterns,
            commands::learning::self_improvement_proposal,
            commands::learning::propose_tool_gates,
            commands::learning::apply_tool_gate,
            commands::evolution::list_evolution_candidate_states,
            commands::evolution::approve_learning_event,
            commands::evolution::list_evolution_eval_case_results,
            commands::evolution::rerun_evolution_eval,
            commands::evolution::activate_evolution_candidate,
            commands::evolution::rollback_evolution_activation,
            commands::preferences::list_user_preferences,
            commands::preferences::get_effective_user_preference,
            commands::preferences::upsert_user_preference,
            commands::interjections::queue_interjection,
            commands::knowledge::register_knowledge_library,
            commands::knowledge::list_knowledge_libraries,
            commands::knowledge::scan_knowledge_library,
            commands::knowledge::set_knowledge_library_enabled,
            commands::knowledge::delete_knowledge_library,
            commands::models::list_models,
            commands::session::list_sessions,
            commands::session::create_session,
            commands::session::materialize_draft_session,
            commands::session::update_session_reasoning_effort,
            commands::session::get_session,
            commands::session::delete_session,
            commands::session::update_session_model,
            commands::session::update_session_model_config,
            commands::session::set_endpoint_active_model,
            commands::session::get_endpoint_active_model,
            commands::session::update_session_title,
            commands::session::get_message_page,
            commands::chat_progress::get_turn_timing_profile,
            commands::chat::send_message,
            commands::chat::send_message_anonymous,
            commands::chat::respond_to_permission,
            commands::chat::cancel_chat,
            commands::chat::is_chat_running,
            commands::chat::delivery_channel_status,
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
            commands::tasks::create_task_tree,
            commands::tasks::list_tasks,
            commands::tasks::get_task_dependencies,
            commands::tasks::start_implementation,
            commands::tasks::cancel_implementation,
            commands::tasks::retry_failed_tasks,
            commands::tasks::retry_tasks,
            commands::tasks::run_verification_now,
            commands::evidence::list_evidence_packs,
            commands::evidence::get_evidence_pack,
            commands::evidence::open_evidence_pack_dir,
            commands::skills::list_skills,
            commands::skills::get_skill,
            commands::skills::enable_skill,
            commands::skills::disable_skill,
            commands::skills::install_skill_from_url,
            commands::skills::install_skill_from_directory,
            commands::skills::scan_openclaw_skills,
            commands::skills::create_skill,
            commands::skills::update_skill,
            commands::skills::delete_skill,
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
            commands::git_remote::github_cli_credential_status,
            commands::git_remote::list_git_remotes,
            commands::git_remote::add_git_remote,
            commands::git_remote::delete_git_remote,
            commands::git_remote::test_git_remote,
            commands::git_remote::list_issues,
            commands::git_remote::create_issue,
            commands::git_remote::list_prs,
            commands::git_remote::create_pr,
            commands::git_remote::workspace_delivery_status,
            commands::git_remote::list_repos,
            commands::costs::get_session_cost,
            commands::costs::get_today_cost,
            commands::costs::get_monthly_cost,
            commands::costs::get_costs_by_model,
            commands::costs::list_recent_cost_entries,
            commands::costs::get_usage_dashboard,
            commands::costs::get_usage_day_detail,
            commands::costs::get_usage_budget_status,
            commands::costs::get_session_usage,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CodeFactory");
}
