// SPDX-License-Identifier: Apache-2.0
mod agent;
mod ai_text;
mod benchmark;
mod benchmark_consistency;
mod browser;
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
mod panic_log;
mod secrets;
mod session_title;
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

/// Process-local handle for one chat runner. The flag remains the cooperative
/// AgentLoop stop signal; SQLite owns the exact root/opaque-Objective identity.
#[derive(Debug)]
pub struct ChatRunControl {
    pub run_instance_id: String,
    pub cancel: Arc<AtomicBool>,
    pub durable: bool,
}

impl ChatRunControl {
    pub fn pending() -> Self {
        Self {
            run_instance_id: uuid::Uuid::new_v4().to_string(),
            cancel: Arc::new(AtomicBool::new(false)),
            durable: true,
        }
    }

    pub fn ephemeral() -> Self {
        Self {
            run_instance_id: uuid::Uuid::new_v4().to_string(),
            cancel: Arc::new(AtomicBool::new(false)),
            durable: false,
        }
    }
}

/// Per-chat-session run controls. Entirely separate from the task scheduler's
/// cancellation handles — stopping a chat turn never cancels delegated tasks.
pub type ChatCancelMap = Arc<Mutex<HashMap<String, Arc<ChatRunControl>>>>;

/// Execute historical-session continuation and durable-stop restart oracles
/// before Tauri initializes. Parent and workers are the exact same executable.
#[cfg(not(test))]
pub fn run_history_session_smoke_cli() -> bool {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(flag) = args.get(1).map(String::as_str) else {
        return false;
    };
    if !matches!(flag, "--history-session-smoke" | "--history-session-worker") {
        return false;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Historical session smoke could not start: {error}");
            std::process::exit(1);
        });
    match flag {
        "--history-session-smoke" => {
            if args.len() != 3 {
                eprintln!("usage: CodeFactory --history-session-smoke <receipt.json>");
                std::process::exit(2);
            }
            let output = std::path::PathBuf::from(&args[2]);
            match runtime.block_on(agent::history_session_smoke::run_parent()) {
                Ok(receipt) => {
                    let rendered = serde_json::to_string_pretty(&receipt).unwrap_or_default();
                    if let Err(error) = std::fs::write(&output, rendered.as_bytes()) {
                        eprintln!(
                            "Historical session smoke could not write {}: {error}",
                            output.display()
                        );
                        std::process::exit(1);
                    }
                    println!("{rendered}");
                    true
                }
                Err(error) => {
                    let receipt = serde_json::json!({
                        "ok": false,
                        "scenario_ids": ["E2E-002", "E2E-003"],
                        "error": error.to_string(),
                    });
                    let rendered = serde_json::to_string_pretty(&receipt).unwrap_or_default();
                    let _ = std::fs::write(&output, rendered.as_bytes());
                    eprintln!("Historical session smoke failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        "--history-session-worker" => {
            if args.len() != 4 {
                eprintln!("usage: CodeFactory --history-session-worker <state-dir> <phase>");
                std::process::exit(2);
            }
            let state_dir = std::path::PathBuf::from(&args[2]);
            if let Err(error) = runtime.block_on(agent::history_session_smoke::run_worker(
                &state_dir, &args[3],
            )) {
                eprintln!("Historical session worker failed: {error}");
                std::process::exit(1);
            }
            true
        }
        _ => unreachable!(),
    }
}

/// Execute the real Git/SQLite DeliveryRun crash-recovery contract before
/// Tauri initializes. Parent and workers are the exact same executable.
#[cfg(not(test))]
pub fn run_delivery_recovery_smoke_cli() -> bool {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(flag) = args.get(1).map(String::as_str) else {
        return false;
    };
    if !matches!(
        flag,
        "--delivery-recovery-smoke" | "--delivery-recovery-worker"
    ) {
        return false;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        // The recovery path intentionally drives the full production DeliveryRun
        // state machine. Windows executables have a much smaller main-thread
        // stack than our Unix CI hosts, so poll that future on a Tokio worker
        // with an explicit stack instead of directly inside `block_on`.
        .thread_stack_size(8 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Delivery recovery smoke could not start: {error}");
            std::process::exit(1);
        });
    match flag {
        "--delivery-recovery-smoke" => {
            if args.len() != 3 {
                eprintln!("usage: CodeFactory --delivery-recovery-smoke <receipt.json>");
                std::process::exit(2);
            }
            let output = std::path::PathBuf::from(&args[2]);
            let result = runtime.block_on(async {
                tokio::spawn(agent::delivery_recovery_smoke::run_parent())
                    .await
                    .map_err(|error| anyhow::anyhow!("delivery recovery parent task failed: {error}"))?
            });
            match result {
                Ok(receipt) => {
                    let rendered = serde_json::to_string_pretty(&receipt).unwrap_or_default();
                    if let Err(error) = std::fs::write(&output, rendered.as_bytes()) {
                        eprintln!(
                            "Delivery recovery smoke could not write {}: {error}",
                            output.display()
                        );
                        std::process::exit(1);
                    }
                    println!("{rendered}");
                    true
                }
                Err(error) => {
                    let receipt = serde_json::json!({
                        "ok": false,
                        "scenario_id": "E2E-011",
                        "error": error.to_string(),
                    });
                    let rendered = serde_json::to_string_pretty(&receipt).unwrap_or_default();
                    let _ = std::fs::write(&output, rendered.as_bytes());
                    eprintln!("Delivery recovery smoke failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        "--delivery-recovery-worker" => {
            if args.len() != 4 {
                eprintln!(
                    "usage: CodeFactory --delivery-recovery-worker <state-dir> <seed|rebind|push|recover>"
                );
                std::process::exit(2);
            }
            let state_dir = std::path::PathBuf::from(&args[2]);
            let phase = args[3].clone();
            let result = runtime.block_on(async move {
                tokio::spawn(async move {
                    agent::delivery_recovery_smoke::run_worker(&state_dir, &phase).await
                })
                .await
                .map_err(|error| anyhow::anyhow!("delivery recovery worker task failed: {error}"))?
            });
            if let Err(error) = result {
                eprintln!("Delivery recovery worker failed: {error}");
                std::process::exit(1);
            }
            true
        }
        _ => unreachable!(),
    }
}

/// Execute the network-hermetic, cross-process long-task contract before
/// Tauri initializes. Both the parent and its internal workers are copies of
/// this exact formal executable, so release CI never substitutes a test EXE.
#[cfg(not(test))]
pub fn run_unattended_long_task_smoke_cli() -> bool {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(flag) = args.get(1).map(String::as_str) else {
        return false;
    };
    if !matches!(
        flag,
        "--unattended-long-task-smoke" | "--unattended-long-task-worker"
    ) {
        return false;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Unattended long-task smoke could not start: {error}");
            std::process::exit(1);
        });
    match flag {
        "--unattended-long-task-smoke" => {
            if args.len() != 3 {
                eprintln!("usage: CodeFactory --unattended-long-task-smoke <receipt.json>");
                std::process::exit(2);
            }
            let output = std::path::PathBuf::from(&args[2]);
            match runtime.block_on(agent::unattended_smoke::run_parent()) {
                Ok(receipt) => {
                    let rendered = serde_json::to_string_pretty(&receipt).unwrap_or_default();
                    if let Err(error) = std::fs::write(&output, rendered.as_bytes()) {
                        eprintln!(
                            "Unattended smoke could not write {}: {error}",
                            output.display()
                        );
                        std::process::exit(1);
                    }
                    println!("{rendered}");
                    true
                }
                Err(error) => {
                    let receipt = serde_json::json!({
                        "ok": false,
                        "scenario_id": "HLT-001",
                        "error": error.to_string(),
                    });
                    let rendered = serde_json::to_string_pretty(&receipt).unwrap_or_default();
                    let _ = std::fs::write(&output, rendered.as_bytes());
                    eprintln!("Unattended long-task smoke failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        "--unattended-long-task-worker" => {
            if args.len() != 5 {
                eprintln!(
                    "usage: CodeFactory --unattended-long-task-worker <state-dir> <provider-url> <phase>"
                );
                std::process::exit(2);
            }
            let state_dir = std::path::PathBuf::from(&args[2]);
            let phase = args[4].parse::<u8>().unwrap_or_else(|_| {
                eprintln!("unattended worker phase must be 1 or 2");
                std::process::exit(2);
            });
            if let Err(error) = runtime.block_on(agent::unattended_smoke::run_worker(
                &state_dir, &args[3], phase,
            )) {
                eprintln!("Unattended long-task worker failed: {error}");
                std::process::exit(1);
            }
            true
        }
        _ => unreachable!(),
    }
}

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
    /// Admission barrier set only after every live-work owner reports idle.
    /// New work must not enter between updater safety admission and relaunch.
    pub update_restart_reserved: Arc<AtomicBool>,
    /// Serializes updater restart admission with Objective remediation claims.
    /// The Objective supervisor has no entry in the chat/task runtime maps, so
    /// the atomic flag alone cannot close its snapshot-to-claim race.
    pub update_restart_admission: Arc<tokio::sync::Mutex<()>>,
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

    // Both arms carry a receipt, because every exit from here has to leave one
    // behind. Writing one only on the happy path is what left a red CI step
    // saying nothing but "exited 1" — the reason went to stderr and the
    // evidence file never existed.
    let result: std::result::Result<serde_json::Value, serde_json::Value> =
        runtime.block_on(async {
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
                outer_receipt_id: None,
                mutation_permit: None,
                knowledge_library_ids: None,
                settings: None,
            };

            // Whatever happens after a browser exists must not leave one
            // running: that is the leak this whole smoke is here to catch.
            macro_rules! give_up {
                ($stage:expr, $error:expr, $attempts:expr) => {{
                    tools::browser_session::close_for_session(&owner_session_id).await;
                    return Err(browser::smoke::failure_receipt($stage, $error, $attempts));
                }};
            }

            let objective_pool = match sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(5)
                .connect("sqlite::memory:")
                .await
            {
                Ok(pool) => pool,
                Err(error) => give_up!("objective_setup", &error.to_string(), 0),
            };
            if let Err(error) = agent::objective::ensure_schema(&objective_pool).await {
                give_up!("objective_schema", &error.to_string(), 0);
            }
            let objective_store = agent::objective::ObjectiveStore::new(objective_pool.clone());
            let objective_id = format!("browser-objective-{}", uuid::Uuid::new_v4());
            let objective = match objective_store
                .create(agent::objective::CreateObjective {
                    id: objective_id.clone(),
                    kind: agent::objective::ObjectiveKind::Informational,
                    session_id: Some(owner_session_id.clone()),
                    root_turn_id: None,
                    domain: agent::objective::RecoveryDomain::Browser,
                    requested_acceptance: "informational_answer".into(),
                    created_surface: "browser_session_smoke".into(),
                })
                .await
            {
                Ok(objective) => objective,
                Err(error) => give_up!("objective_create", &error.to_string(), 0),
            };
            if let Err(error) = sqlx::query("UPDATE objectives SET task_id=? WHERE id=?")
                .bind(format!("browser-task-{}", uuid::Uuid::new_v4()))
                .bind(&objective_id)
                .execute(&objective_pool)
                .await
            {
                give_up!("objective_task_binding", &error.to_string(), 0);
            }

            // Retried, and only here: a runner too busy to start Chrome inside
            // the launch budget is the environment failing, not this project.
            // Everything below the loop is asserted exactly once.
            let mut attempts = 0_u32;
            let opened = loop {
                attempts += 1;
                let opened = match tools::browser_session::execute(
                    serde_json::json!({"action":"open","url":"https://example.com"}),
                    &ctx,
                )
                .await
                {
                    Ok(opened) => opened,
                    Err(error) => give_up!("open", &error.to_string(), attempts),
                };
                if !opened.is_error {
                    break opened;
                }
                if !browser::smoke::should_retry_open(&opened.content, attempts) {
                    give_up!("open", &opened.content, attempts);
                }
                eprintln!(
                    "Browser-session smoke: attempt {attempts} of {} could not start a browser, \
                     retrying — {}",
                    browser::smoke::MAX_LAUNCH_ATTEMPTS,
                    opened.content
                );
                tokio::time::sleep(browser::smoke::retry_backoff(attempts)).await;
            };

            let Some(session_id) = opened
                .content
                .lines()
                .find_map(|line| line.strip_prefix("Managed browser session: "))
                .map(str::to_owned)
            else {
                give_up!("session_id", "open did not report a session id", attempts);
            };

            let snapshot = match tools::browser_session::execute(
                serde_json::json!({"action":"snapshot","session_id":session_id}),
                &ctx,
            )
            .await
            {
                Ok(snapshot) => snapshot,
                Err(error) => give_up!("snapshot", &error.to_string(), attempts),
            };
            let failed_action = match tools::browser_session::execute(
                serde_json::json!({
                    "action":"click",
                    "session_id":session_id,
                    "target":"e999999"
                }),
                &ctx,
            )
            .await
            {
                Ok(failed_action) => failed_action,
                Err(error) => give_up!("injected_failure", &error.to_string(), attempts),
            };
            // Read before the owner-wide cleanup below, or reclamation by the
            // manager and reclamation by our own teardown are indistinguishable.
            let lease_gone = !tools::browser_session::list_managed_sessions()
                .iter()
                .any(|session| session.session_id == session_id);

            let now = chrono::Utc::now().timestamp_millis();
            let binding_id = format!("browser-binding-{}", uuid::Uuid::new_v4());
            if let Err(error) = sqlx::query(
                "INSERT INTO objective_bindings
                 (id, objective_id, domain, resource_kind, resource_id,
                  resource_generation, identity_digest, created_at, updated_at)
                 VALUES (?, ?, 'browser', 'browser_session', ?, 1,
                         'sha256:browser-session-smoke', ?, ?)",
            )
            .bind(&binding_id)
            .bind(&objective_id)
            .bind(&session_id)
            .bind(now)
            .bind(now)
            .execute(&objective_pool)
            .await
            {
                give_up!("objective_browser_binding", &error.to_string(), attempts);
            }
            let waiting = match agent::objective::DecisionRouter::route(
                &objective,
                agent::objective::RouteSignal::TechnicalFailure {
                    domain: agent::objective::RecoveryDomain::Browser,
                    failure_code: "browser_logical_action_failed".into(),
                    failure_signature: "sha256:browser-session-smoke-failure".into(),
                    next_observation_at: now - 1,
                    resume_cursor: Some(session_id.clone()),
                },
            ) {
                Ok(waiting) => waiting,
                Err(error) => give_up!("objective_route", &error.to_string(), attempts),
            };
            let waiting = match objective_store
                .apply_decision(objective.revision, waiting)
                .await
            {
                Ok(waiting) => waiting,
                Err(error) => give_up!("objective_wait", &error.to_string(), attempts),
            };
            if let Err(error) = sqlx::query(
                "UPDATE objective_remediations SET binding_id=?
                 WHERE objective_id=? AND id=?",
            )
            .bind(&binding_id)
            .bind(&objective_id)
            .bind(waiting.remediation_id.as_deref().unwrap_or_default())
            .execute(&objective_pool)
            .await
            {
                give_up!("objective_remediation_binding", &error.to_string(), attempts);
            }

            let mut first_claims = match objective_store
                .claim_due_remediations("browser-smoke-crashed-owner", 1, 30_000)
                .await
            {
                Ok(claims) => claims,
                Err(error) => give_up!("objective_first_claim", &error.to_string(), attempts),
            };
            let Some(first_claim) = first_claims.pop() else {
                give_up!(
                    "objective_first_claim",
                    "browser recovery did not claim exactly once",
                    attempts
                );
            };
            if let Err(error) = sqlx::query(
                "UPDATE objective_remediations SET lease_expires_at=? WHERE id=?",
            )
            .bind(now - 1)
            .bind(&first_claim.remediation_id)
            .execute(&objective_pool)
            .await
            {
                give_up!("objective_expire_remediation", &error.to_string(), attempts);
            }
            if let Err(error) =
                sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
                    .bind(now - 1)
                    .bind(&objective_id)
                    .execute(&objective_pool)
                    .await
            {
                give_up!("objective_expire_lease", &error.to_string(), attempts);
            }
            let mut replacement_claims = match objective_store
                .claim_due_remediations("browser-smoke-replacement-owner", 1, 30_000)
                .await
            {
                Ok(claims) => claims,
                Err(error) => give_up!("objective_reclaim", &error.to_string(), attempts),
            };
            let Some(replacement_claim) = replacement_claims.pop() else {
                give_up!(
                    "objective_reclaim",
                    "replacement owner did not reclaim browser recovery",
                    attempts
                );
            };
            let stale_permit = codefactory_agent_loop::tool::MutationPermit {
                objective_id: first_claim.objective.id.clone(),
                remediation_id: first_claim.remediation_id.clone(),
                owner: "browser-smoke-crashed-owner".into(),
                claim_epoch: first_claim.claim_epoch,
                binding_id: first_claim.binding_id.clone(),
                resource_generation: first_claim.resource_generation,
            };
            let recovery_lease_reclaimed = replacement_claim.claim_epoch > first_claim.claim_epoch
                && matches!(objective_store.claim_is_current(&stale_permit).await, Ok(false));
            let attempts_before_execute: i64 = match sqlx::query_scalar(
                "SELECT execution_attempt_index FROM objective_remediations WHERE id=?",
            )
            .bind(&replacement_claim.remediation_id)
            .fetch_one(&objective_pool)
            .await
            {
                Ok(value) => value,
                Err(error) => give_up!("objective_budget_read", &error.to_string(), attempts),
            };

            let replacement_opened = match tools::browser_session::execute(
                serde_json::json!({"action":"open","url":"https://example.com"}),
                &ctx,
            )
            .await
            {
                Ok(opened) if !opened.is_error => opened,
                Ok(opened) => give_up!("replacement_open", &opened.content, attempts),
                Err(error) => give_up!("replacement_open", &error.to_string(), attempts),
            };
            let Some(replacement_session_id) = replacement_opened
                .content
                .lines()
                .find_map(|line| line.strip_prefix("Managed browser session: "))
                .map(str::to_owned)
            else {
                give_up!(
                    "replacement_session_id",
                    "replacement open did not report a session id",
                    attempts
                );
            };
            let replacement_snapshot = match tools::browser_session::execute(
                serde_json::json!({"action":"snapshot","session_id":replacement_session_id}),
                &ctx,
            )
            .await
            {
                Ok(snapshot) => snapshot,
                Err(error) => give_up!("replacement_snapshot", &error.to_string(), attempts),
            };
            let replacement_session_is_new = replacement_session_id != session_id
                && !replacement_snapshot.is_error;

            let permit = codefactory_agent_loop::tool::MutationPermit {
                objective_id: replacement_claim.objective.id.clone(),
                remediation_id: replacement_claim.remediation_id.clone(),
                owner: "browser-smoke-replacement-owner".into(),
                claim_epoch: replacement_claim.claim_epoch,
                binding_id: replacement_claim.binding_id.clone(),
                resource_generation: replacement_claim.resource_generation,
            };
            let charged = match objective_store
                .charge_claimed_remediation_attempt(
                    &replacement_claim.objective.id,
                    &replacement_claim.remediation_id,
                    &permit.owner,
                    permit.claim_epoch,
                )
                .await
            {
                Ok(charged) => charged,
                Err(error) => give_up!("objective_budget_charge", &error.to_string(), attempts),
            };
            if !charged {
                give_up!(
                    "objective_budget_charge",
                    "replacement owner lost its exact execution permit",
                    attempts
                );
            }
            let recovery_budget_attempts: i64 = match sqlx::query_scalar(
                "SELECT execution_attempt_index FROM objective_remediations WHERE id=?",
            )
            .bind(&replacement_claim.remediation_id)
            .fetch_one(&objective_pool)
            .await
            {
                Ok(value) => value,
                Err(error) => give_up!("objective_budget_verify", &error.to_string(), attempts),
            };
            let completion = match agent::objective::CompletionArbiter::decide(
                &replacement_claim.objective,
                &[agent::objective::ObjectiveEvidence {
                    id: format!("browser-evidence-{}", uuid::Uuid::new_v4()),
                    kind: agent::objective::EvidenceKind::InformationalAnswer,
                    scope: replacement_session_id.clone(),
                    digest: "sha256:browser-safe-continuation".into(),
                    evidence_ref: "smoke:browser-safe-continuation".into(),
                    observed_at: chrono::Utc::now().timestamp_millis(),
                    reached_acceptance: "informational_answer".into(),
                }],
            ) {
                Ok(completion) => completion,
                Err(error) => give_up!("objective_completion", &error.to_string(), attempts),
            };
            let completed = match objective_store
                .apply_claimed_decision(
                    replacement_claim.objective.revision,
                    completion,
                    &permit,
                )
                .await
            {
                Ok(completed) => completed,
                Err(error) => give_up!("objective_settle", &error.to_string(), attempts),
            };
            let replacement_closed = match tools::browser_session::execute(
                serde_json::json!({"action":"close","session_id":replacement_session_id}),
                &ctx,
            )
            .await
            {
                Ok(closed) => closed,
                Err(error) => give_up!("replacement_close", &error.to_string(), attempts),
            };
            let replacement_session_reclaimed = !replacement_closed.is_error
                && !tools::browser_session::list_managed_sessions()
                    .iter()
                    .any(|session| session.session_id == replacement_session_id);

            let continuation = browser::smoke::ContinuationEvidence {
                objective_id: &objective_id,
                replacement_session_id: &replacement_session_id,
                same_objective: completed.id == objective_id,
                replacement_session_is_new,
                replacement_session_reclaimed,
                recovery_lease_reclaimed,
                recovery_budget_attempts: if attempts_before_execute == 0 {
                    recovery_budget_attempts
                } else {
                    -1
                },
                objective_completed: completed.status
                    == agent::objective::ObjectiveStatus::Completed,
            };

            let receipt = browser::smoke::receipt(
                &session_id,
                !snapshot.is_error,
                failed_action.is_error,
                lease_gone,
                attempts,
                &continuation,
            );
            tools::browser_session::close_for_session(&owner_session_id).await;
            if receipt["status"] == "passed" {
                Ok(receipt)
            } else {
                Err(receipt)
            }
        });

    let (receipt, passed) = match &result {
        Ok(receipt) => (receipt, true),
        Err(receipt) => (receipt, false),
    };
    let rendered = serde_json::to_string_pretty(receipt).unwrap_or_default();
    if let Err(error) = std::fs::write(&output_path, rendered.as_bytes()) {
        eprintln!(
            "Browser-session smoke could not write {}: {error}",
            output_path.display()
        );
        std::process::exit(1);
    }
    if passed {
        println!("{rendered}");
        true
    } else {
        eprintln!("Browser-session smoke failed: {rendered}");
        std::process::exit(1);
    }
}

/// Exact-release executable gate for a previous-to-current updater restart.
/// The previous identity is supplied from the prior public `latest.json`; the
/// current identity is compiled into this candidate binary.
#[cfg(not(test))]
pub fn run_update_upgrade_smoke_cli() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--update-upgrade-smoke") {
        return false;
    }
    if args.len() != 5 {
        eprintln!(
            "usage: CodeFactory --update-upgrade-smoke <receipt.json> <previous-version> <previous-build-sha>"
        );
        std::process::exit(2);
    }
    let output = std::path::PathBuf::from(&args[2]);
    let state_path = output.with_extension("sqlite");
    let current_build = option_env!("CODEFACTORY_BUILD_GIT_SHA").unwrap_or_default();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Update upgrade smoke could not start: {error}");
            std::process::exit(1);
        });
    match runtime.block_on(commands::update_safety::run_update_upgrade_smoke_fixture(
        &state_path,
        &args[3],
        &args[4],
        env!("CARGO_PKG_VERSION"),
        current_build,
    )) {
        Ok(receipt) => {
            let rendered = serde_json::to_string_pretty(&receipt).unwrap_or_default();
            if let Err(error) = std::fs::write(&output, rendered.as_bytes()) {
                eprintln!("Update upgrade smoke could not write receipt: {error}");
                std::process::exit(1);
            }
            let _ = std::fs::remove_file(&state_path);
            if receipt.get("status").and_then(serde_json::Value::as_str) != Some("pass") {
                eprintln!("Update upgrade smoke receipt did not pass: {rendered}");
                std::process::exit(1);
            }
            println!("{rendered}");
            true
        }
        Err(error) => {
            let receipt = serde_json::json!({
                "scenario_id": "E2E-006",
                "status": "fail",
                "error": error.to_string(),
            });
            let rendered = serde_json::to_string_pretty(&receipt).unwrap_or_default();
            let _ = std::fs::write(&output, rendered.as_bytes());
            let _ = std::fs::remove_file(&state_path);
            eprintln!("Update upgrade smoke failed: {error}");
            std::process::exit(1);
        }
    }
}

/// Release/runtime smoke for the existing-Chrome attachment path. It exercises
/// the native tool (never a naked Playwright process), verifies signed-in
/// Chrome can be observed, and proves cleanup detaches without closing Chrome.
#[cfg(not(test))]
pub fn run_browser_chrome_attach_smoke_cli() -> bool {
    let mut args = std::env::args();
    let _program = args.next();
    let Some(flag) = args.next() else {
        return false;
    };
    if flag != "--browser-chrome-attach-smoke" {
        return false;
    }
    let Some(output) = args.next() else {
        eprintln!("usage: CodeFactory --browser-chrome-attach-smoke <receipt.json>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: CodeFactory --browser-chrome-attach-smoke <receipt.json>");
        std::process::exit(2);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Chrome-attachment smoke could not start: {error}");
            std::process::exit(1);
        });
    let output_path = std::path::PathBuf::from(output);
    let result = runtime.block_on(async {
        let cwd = output_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let browser_fixture = std::env::var_os("CODEFACTORY_BROWSER_CHROME_FIXTURE")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                errors::AppError::Other(
                    "CODEFACTORY_BROWSER_CHROME_FIXTURE must be 'managed' or name the Chrome for Testing executable used by the release smoke"
                        .into(),
                )
            })?;
        let browser_binary = if browser_fixture == std::ffi::OsStr::new("managed") {
            browser::download::ensure_installed(&|_| {}).await?.binary
        } else {
            std::path::PathBuf::from(browser_fixture)
        };
        let bridge = std::sync::Arc::clone(&tools::browser_session::BRIDGE);
        let pairing = bridge.start().await?;
        let extension_dir = browser::extension_package::prepare(pairing.port, &pairing.token)
            .map_err(errors::AppError::Other)?;
        let browser_profile = tempfile::Builder::new()
            .prefix("browser-chrome-attach-profile-")
            .tempdir_in(&cwd)?;
        let fixture_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let fixture_port = fixture_listener.local_addr()?.port();
        let fixture_server = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt as _;
            while let Ok((mut stream, _)) = fixture_listener.accept().await {
                let body = b"<!doctype html><title>CodeFactory attachment fixture</title><h1>Attachment fixture</h1>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(body).await;
            }
        });
        let mut browser_process = tokio::process::Command::new(&browser_binary);
        browser_process
            .arg(format!(
                "--user-data-dir={}",
                browser_profile.path().display()
            ))
            .arg(format!("--load-extension={}", extension_dir.display()))
            .arg(format!(
                "--disable-extensions-except={}",
                extension_dir.display()
            ))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--no-sandbox")
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .arg("--disable-sync")
            .arg("--window-position=-10000,-10000")
            .arg("--window-size=800,600")
            .arg(format!("http://127.0.0.1:{fixture_port}/"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut browser_process = browser_process.spawn().map_err(|error| {
            errors::AppError::Other(format!("could not start the Chrome fixture: {error}"))
        })?;
        if !bridge
            .wait_until_connected(std::time::Duration::from_secs(40))
            .await
        {
            return Err(errors::AppError::Other(
                "Chrome fixture did not connect to the exact-artifact extension bridge within 40 seconds"
                    .into(),
            ));
        }
        let owner_session_id = format!("browser-attach-smoke-{}", uuid::Uuid::new_v4());
        let ctx = tools::ExecCtx {
            cwd,
            app: None,
            db: None,
            session_id: Some(owner_session_id.clone()),
            root_turn_id: None,
            task_id: None,
            outer_receipt_id: None,
            mutation_permit: None,
            knowledge_library_ids: None,
            settings: None,
        };

        let attached =
            tools::browser_session::execute(serde_json::json!({"action":"attach"}), &ctx).await?;
        if attached.status != tools::ToolExecutionStatus::Done {
            return Err(errors::AppError::Other(attached.content));
        }
        let session_id = attached
            .content
            .lines()
            .find_map(|line| {
                line.strip_prefix("Attached user Chrome session: ")
                    .and_then(|value| value.split('.').next())
            })
            .ok_or_else(|| {
                errors::AppError::Other("attach smoke did not receive a session id".into())
            })?
            .to_string();
        let attached_kind = tools::browser_session::list_managed_sessions()
            .iter()
            .find(|session| session.session_id == session_id)
            .map(|session| session.kind.as_str())
            == Some("attached_chrome");
        let tabs_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let tab_observation_ok = loop {
            let tabs = tools::browser_session::execute(
                serde_json::json!({"action":"tabs","session_id":session_id}),
                &ctx,
            )
            .await?;
            let observed = !tabs.is_error
                && tabs.status == tools::ToolExecutionStatus::Done
                && !tabs.content.contains("No readable tabs are open");
            if observed || tokio::time::Instant::now() >= tabs_deadline {
                break observed;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        };
        let closed = tools::browser_session::execute(
            serde_json::json!({"action":"close","session_id":session_id}),
            &ctx,
        )
        .await?;
        let lease_gone = !tools::browser_session::list_managed_sessions()
            .iter()
            .any(|session| session.session_id == session_id);
        let browser_process_alive_after_detach = browser_process.try_wait()?.is_none();
        let receipt = serde_json::json!({
            "status": if attached_kind
                && tab_observation_ok
                && !closed.is_error
                && lease_gone
                && browser_process_alive_after_detach
            {
                "passed"
            } else {
                "failed"
            },
            "native_tool": "browser_session",
            "connection_kind": "attached_chrome",
            "tab_observation_ok": tab_observation_ok,
            "detached_without_managed_close": closed.content.contains("Chrome was left open"),
            "lease_reclaimed_after_detach": lease_gone,
            "browser_process_alive_after_detach": browser_process_alive_after_detach,
        });
        std::fs::write(
            &output_path,
            serde_json::to_vec_pretty(&receipt).unwrap_or_default(),
        )?;
        tools::browser_session::close_for_session(&owner_session_id).await;
        let _ = browser_process.start_kill();
        let _ = browser_process.wait().await;
        fixture_server.abort();
        if receipt["status"] == "passed"
            && receipt["detached_without_managed_close"] == true
            && receipt["browser_process_alive_after_detach"] == true
        {
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
            eprintln!("Chrome-attachment smoke failed: {error}");
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

            // Before anything else can panic: a stripped release binary with
            // `panic = "abort"` leaves a crash report that names no symbols and
            // a message on a stderr nobody reads (v1.78.6, 2026-08-10).
            crate::panic_log::install(&data_dir);

            // Rolling daily DB backup — one snapshot per day, 7-day retention.
            // Best-effort: failures are logged and never block startup.
            let db_path = data_dir.join("codefactory.db");
            if let Err(e) = backup_db_daily(&data_dir, &db_path) {
                tracing::warn!("db backup skipped: {e}");
            }

            let db_url = format!("sqlite:{}", db_path.display());
            let settings = config::settings::load();

            let pool = tauri::async_runtime::block_on(storage::db::connect(&db_url))?;

            // Continuously claim expired identity-complete delivery runs, then
            // resume only already-authorized work. This also recovers a lost
            // in-process tool future; startup is merely the first poll.
            let process_instance = format!(
                "{}:{}",
                std::process::id(),
                storage::db::current_process_start_token()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
            );
            let app_version = app.package_info().version.to_string();
            let delivery_settings = settings.clone();
            let process_identity = agent::delivery_run::ProcessIdentity::new(
                process_instance.clone(),
                &app_version,
                option_env!("CODEFACTORY_BUILD_NUMBER").unwrap_or(&app_version),
            );
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

            let objective_pool = pool.clone();
            app.manage(AppState {
                db: Arc::new(RwLock::new(pool)),
                settings: Arc::new(RwLock::new(settings)),
                pending_permissions: Arc::new(Mutex::new(HashMap::new())),
                chat_cancels: Arc::new(Mutex::new(HashMap::new())),
                interjections: Arc::new(Mutex::new(HashMap::new())),
                update_restart_reserved: Arc::new(AtomicBool::new(false)),
                update_restart_admission: Arc::new(tokio::sync::Mutex::new(())),
            });
            // Manage the Arc so all commands share the same McpManager instance.
            app.manage(mcp_manager);
            // Recovery adapters can run as soon as the first supervisor poll
            // fires, so every process-local dependency must already be
            // managed before stale Objectives are made due.
            let scheduler_handles: commands::tasks::SchedulerHandles =
                Arc::new(Mutex::new(HashMap::new()));
            app.manage(scheduler_handles);
            let objective_store = agent::objective::ObjectiveStore::new(objective_pool.clone());
            let cancelled_sessions = tauri::async_runtime::block_on(
                objective_store.consume_pending_chat_session_cancellations(),
            )?;
            if cancelled_sessions > 0 {
                tracing::info!(
                    count = cancelled_sessions,
                    "startup: persisted session stops settled before recovery admission"
                );
            }
            let cancelled_objectives = tauri::async_runtime::block_on(
                objective_store.consume_pending_chat_cancellations(),
            )?;
            if cancelled_objectives > 0 {
                tracing::info!(
                    count = cancelled_objectives,
                    "startup: persisted chat cancellations settled before recovery admission"
                );
            }
            let stale_chat_runs = tauri::async_runtime::block_on(
                objective_store.reconcile_stale_chat_run_controls(&process_instance),
            )?;
            if stale_chat_runs > 0 {
                tracing::info!(
                    count = stale_chat_runs,
                    "startup: retired prior-process chat transports before objective recovery"
                );
            }
            let recovered_exhausted_reprompts = tauri::async_runtime::block_on(
                objective_store.reconcile_unconsumed_exhausted_chat_reprompts(),
            )?;
            if recovered_exhausted_reprompts > 0 {
                tracing::info!(
                    count = recovered_exhausted_reprompts,
                    "startup: resumed user messages swallowed by the exhausted recovery ceiling"
                );
            }
            let reclassified_technical_handbacks = tauri::async_runtime::block_on(
                objective_store.reclassify_synthetic_technical_handbacks(),
            )?;
            if reclassified_technical_handbacks > 0 {
                tracing::info!(
                    count = reclassified_technical_handbacks,
                    "startup: reclassified synthetic technical handbacks as system-owned incidents"
                );
            }
            tauri::async_runtime::block_on(objective_store.sync_recovery_capabilities())?;
            let reactivated_incidents = tauri::async_runtime::block_on(
                objective_store.reactivate_eligible_incidents(32),
            )?;
            if !reactivated_incidents.is_empty() {
                tracing::info!(
                    count = reactivated_incidents.len(),
                    capability_revision = agent::objective::RECOVERY_CAPABILITY_REVISION,
                    "startup: newer recovery contract reactivated parked system incidents"
                );
            }
            let provider_recoveries = tauri::async_runtime::block_on(
                agent::objective_supervisor::reconcile_provider_recovery_on_startup(
                    &objective_pool,
                ),
            )?;
            if provider_recoveries > 0 {
                tracing::info!(
                    count = provider_recoveries,
                    "startup: provider attempts moved to evidence-gated recovery before generic active reconciliation"
                );
            }
            let browser_recoveries = tauri::async_runtime::block_on(
                agent::objective_supervisor::reconcile_browser_recovery_on_startup(
                    &objective_pool,
                ),
            )?;
            if browser_recoveries > 0 {
                tracing::info!(
                    count = browser_recoveries,
                    "startup: browser operations moved to evidence-gated recovery before generic active reconciliation"
                );
            }
            let stale_permission_prompts = tauri::async_runtime::block_on(
                agent::permission_intent::PermissionIntentStore::new(objective_pool.clone())
                    .reconcile_stale_process_channels(
                        &process_instance,
                        chrono::Utc::now().timestamp_millis(),
                    ),
            )?;
            if stale_permission_prompts > 0 {
                tracing::info!(
                    count = stale_permission_prompts,
                    "startup: prior-process permission prompts moved to typed recovery"
                );
            }
            let stale_objectives = tauri::async_runtime::block_on(
                objective_store.reconcile_stale_active_objectives(&process_instance),
            )?;
            if stale_objectives > 0 {
                tracing::info!(
                    count = stale_objectives,
                    "startup: active objectives moved to system-owned recovery"
                );
            }
            match tauri::async_runtime::block_on(codex_auth::observe_chatgpt_auth_on_startup(
                &objective_pool,
            )) {
                Ok(result) if result.receipts_recorded > 0 => tracing::info!(
                    queued = result.queued_objectives,
                    receipts = result.receipts_recorded,
                    "startup: ChatGPT Keychain capability reconciled before recovery admission"
                ),
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    %error,
                    "startup: ChatGPT capability observation deferred"
                ),
            }
            tools::delivery::spawn_delivery_recovery_supervisor(
                objective_pool.clone(),
                delivery_settings,
                process_identity,
            );
            agent::objective_supervisor::spawn_objective_recovery_supervisor(
                app.handle().clone(),
                objective_pool.clone(),
            );
            app.manage(commands::terminal::TerminalState::new());

            let browser_reclaim_pool = objective_pool.clone();
            tauri::async_runtime::spawn(async move {
                match tools::browser_session::reclaim_on_startup_with_pool(&browser_reclaim_pool)
                    .await
                {
                    Ok(reclaimed) if reclaimed > 0 => tracing::info!(
                        "startup: reclaimed {reclaimed} browser session(s) with no unresolved recovery contract"
                    ),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        %error,
                        "startup: browser session reclamation deferred to preserve recovery evidence"
                    ),
                }
            });

            // Start the extension bridge as part of coming up, not on the first
            // visit to Settings. An extension the user paired weeks ago is
            // dialling for a listener the moment their browser is open, and
            // starting here is also what refreshes the pairing file in the
            // extension's folder — so a restart re-pairs itself instead of
            // sending the user back to Settings to copy a new port.
            let browser_pairing_pool = objective_pool.clone();
            tauri::async_runtime::spawn(async move {
                let bridge = std::sync::Arc::clone(&tools::browser_session::BRIDGE);
                match bridge.start().await {
                    Ok(pairing) => {
                        tracing::info!(
                            "startup: browser extension bridge listening on {}",
                            pairing.port
                        );
                        loop {
                            if bridge.connected().await {
                                match commands::browser_sessions::resume_browser_pairing_objectives(
                                    &browser_pairing_pool,
                                )
                                .await
                                {
                                    Ok(resumed) if resumed > 0 => tracing::info!(
                                        count = resumed,
                                        "browser pairing restored; objectives queued for automatic continuation"
                                    ),
                                    Ok(_) => {}
                                    Err(error) => tracing::warn!(
                                        %error,
                                        "browser pairing recovery observation deferred"
                                    ),
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                    }
                    Err(error) => {
                        // Not fatal: everything except the extension backend works
                        // without it, and Settings will report the failure if the
                        // user goes looking.
                        tracing::warn!("startup: could not start the extension bridge: {error}")
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::save_api_key,
            commands::settings::delete_api_key,
            commands::update_safety::reserve_update_install,
            commands::update_safety::observe_update_install,
            commands::update_safety::release_update_install_reservation,
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
            commands::browser_sessions::browser_bridge_pairing,
            commands::browser_sessions::browser_extension_prepare,
            commands::browser_sessions::browser_extension_reveal,
            commands::browser_sessions::browser_open_extensions_page,
            commands::browser_sessions::browser_chromium_status,
            commands::browser_sessions::browser_download_chromium,
            commands::browser_sessions::embedded_browser_mount,
            commands::browser_sessions::embedded_browser_resize,
            commands::browser_sessions::embedded_browser_set_visible,
            commands::browser_sessions::embedded_browser_unmount,
            commands::checkpoints::list_checkpoints,
            commands::checkpoints::checkpoint_changeset,
            commands::checkpoints::revert_checkpoint,
            commands::control_plane::get_control_plane_snapshot,
            commands::objective_health::get_objective_health,
            commands::document::read_document,
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
            commands::session::update_session_permission_mode,
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
            commands::chat::get_session_execution_workspace,
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
            commands::skills::select_skill_source_directory,
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
