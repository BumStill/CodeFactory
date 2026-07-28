// SPDX-License-Identifier: Apache-2.0
//! CodeFactory-owned Playwright CLI sessions.
//!
//! A browser process is an owned task resource, not an incidental side effect
//! of a `bash` string.  This tool gives every CLI session an opaque,
//! CodeFactory-prefixed id and keeps an on-disk lease so a later invocation can
//! reclaim a session whose owner crashed before calling `close`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;

use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};
use crate::util::command_env;
use crate::util::no_window::NoWindow;

use super::{ExecCtx, ToolOutput};

const LEASE_TTL: Duration = Duration::from_secs(20 * 60);

#[derive(Debug, Deserialize)]
struct Args {
    action: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Lease {
    session_id: String,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    owner_session_id: Option<String>,
    #[serde(default)]
    owner_pid: u32,
    updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowserSessionView {
    pub session_id: String,
    pub task_id: Option<String>,
    pub owner_session_id: Option<String>,
    pub updated_at_unix_secs: u64,
    pub expired: bool,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "browser_session".into(),
            description: "Use a CodeFactory-managed Playwright browser session. Do not use bash for Playwright. Open creates a session; every later action requires its session_id; close releases it.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {"type":"string", "enum":["open","snapshot","click","fill","press","screenshot","close"]},
                    "session_id": {"type":"string"},
                    "url": {"type":"string"},
                    "target": {"type":"string", "description":"Fresh snapshot element reference for click/fill, or key for press"},
                    "text": {"type":"string", "description":"Text for fill"},
                    "path": {"type":"string", "description":"Project-relative screenshot path"}
                },
                "required": ["action"]
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let args: Args = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(error) => {
            return Ok(ToolOutput::err(format!(
                "Invalid browser_session arguments: {error}"
            )))
        }
    };

    reclaim_expired().await;
    let session_id = match args.action.as_str() {
        "open" => new_session_id(ctx),
        _ => {
            let Some(id) = args
                .session_id
                .as_deref()
                .filter(|id| id.starts_with("codefactory-"))
            else {
                return Ok(ToolOutput::err(
                    "browser_session requires a CodeFactory session_id from browser_session.open",
                ));
            };
            let Some(lease) = read_leases()
                .into_iter()
                .find(|lease| lease.session_id == id)
            else {
                return Ok(ToolOutput::err(
                    "browser_session session is unknown or has already been reclaimed",
                ));
            };
            if !lease_belongs_to_ctx(&lease, ctx) {
                return Ok(ToolOutput::err(
                    "browser_session session belongs to a different task or chat",
                ));
            }
            id.to_owned()
        }
    };

    let command = match command_for(&args, &ctx.cwd) {
        Ok(command) => command,
        Err(error) => return Ok(ToolOutput::err(error)),
    };
    if args.action == "screenshot" {
        if let Some(parent) = command.last().and_then(|path| Path::new(path).parent()) {
            if let Err(error) = std::fs::create_dir_all(parent) {
                return Ok(ToolOutput::err(format!(
                    "browser_session could not create screenshot directory: {error}"
                )));
            }
        }
    }

    if args.action == "open" {
        write_lease(&Lease {
            session_id: session_id.clone(),
            task_id: ctx.task_id.clone(),
            owner_session_id: ctx.session_id.clone(),
            owner_pid: std::process::id(),
            updated_at_unix_secs: now_secs(),
        });
    }
    let output = run_cli(&session_id, &command).await;
    match output {
        Ok(output) => {
            if args.action == "close" {
                remove_lease(&session_id);
                Ok(ToolOutput::ok(format!(
                    "Closed managed browser session {session_id}.\n{output}"
                )))
            } else {
                write_lease(&Lease {
                    session_id: session_id.clone(),
                    task_id: ctx.task_id.clone(),
                    owner_session_id: ctx.session_id.clone(),
                    owner_pid: std::process::id(),
                    updated_at_unix_secs: now_secs(),
                });
                let heading = if args.action == "open" {
                    format!("Managed browser session: {session_id}\n")
                } else {
                    String::new()
                };
                Ok(ToolOutput::ok(format!("{heading}{output}")))
            }
        }
        Err(error) => {
            if args.action == "close" {
                remove_lease(&session_id);
                return Ok(ToolOutput::ok(format!(
                    "Closed managed browser session {session_id}; its daemon was already absent or unreachable."
                )));
            }
            // Playwright CLI historically reports some action failures with
            // exit code 0. Any surfaced error closes the owned session so a
            // caller that aborts the turn cannot leak its daemon.
            let _ = run_cli(&session_id, &["close".into()]).await;
            remove_lease(&session_id);
            Ok(ToolOutput::err(error))
        }
    }
}

fn command_for(args: &Args, cwd: &Path) -> std::result::Result<Vec<String>, String> {
    let required = |value: &Option<String>, name: &str| {
        value
            .clone()
            .ok_or_else(|| format!("browser_session.{} requires {name}", args.action))
    };
    match args.action.as_str() {
        "open" => Ok(vec!["open".into(), required(&args.url, "url")?]),
        "snapshot" => Ok(vec!["snapshot".into()]),
        "click" => Ok(vec!["click".into(), required(&args.target, "target")?]),
        "fill" => Ok(vec!["fill".into(), required(&args.target, "target")?, required(&args.text, "text")?]),
        "press" => Ok(vec!["press".into(), required(&args.target, "target")?]),
        "screenshot" => {
            let path = required(&args.path, "path")?;
            let path = Path::new(&path);
            if path.is_absolute()
                || path.components().any(|component| {
                    matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                return Err(
                    "browser_session.screenshot path must be project-relative and stay inside the project"
                        .into(),
                );
            }
            let full = cwd.join(path);
            Ok(vec!["screenshot".into(), full.to_string_lossy().into_owned()])
        }
        "close" => Ok(vec!["close".into()]),
        _ => Err("browser_session action must be open, snapshot, click, fill, press, screenshot, or close".into()),
    }
}

async fn run_cli(session_id: &str, command: &[String]) -> std::result::Result<String, String> {
    let mut process = Command::new("npx").no_window();
    process.args([
        "--yes",
        "--package",
        "@playwright/cli",
        "playwright-cli",
        "--session",
        session_id,
    ]);
    process.args(command);
    command_env::apply_developer_path(&mut process);
    let output = tokio::time::timeout(Duration::from_secs(45), process.output())
        .await
        .map_err(|_| "Managed browser session timed out after 45 seconds".to_string())?
        .map_err(|error| {
            format!(
                "Failed to start managed browser session. Ensure Node.js/npx is installed: {error}"
            )
        })?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text: String = text.chars().take(64_000).collect();
    let logical_error = output_reports_error(&text);
    if output.status.success() && !logical_error {
        Ok(text)
    } else {
        Err(format!("Managed browser session failed: {text}"))
    }
}

fn output_reports_error(output: &str) -> bool {
    let normalized = output.to_ascii_lowercase();
    normalized.contains("### error") || normalized.contains("\nerror:")
}

fn new_session_id(ctx: &ExecCtx) -> String {
    let owner = ctx
        .task_id
        .as_deref()
        .or(ctx.session_id.as_deref())
        .unwrap_or("task");
    let owner: String = owner
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(20)
        .collect();
    format!("codefactory-{}-{}", owner, uuid::Uuid::new_v4())
}

fn lease_dir() -> PathBuf {
    std::env::temp_dir().join("codefactory-browser-leases")
}
fn lease_path(session_id: &str) -> PathBuf {
    lease_dir().join(format!("{session_id}.json"))
}
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn write_lease(lease: &Lease) {
    let _ = std::fs::create_dir_all(lease_dir());
    let path = lease_path(&lease.session_id);
    let temporary_path = path.with_extension("json.tmp");
    if std::fs::write(
        &temporary_path,
        serde_json::to_vec(lease).unwrap_or_default(),
    )
    .is_ok()
    {
        let _ = std::fs::rename(temporary_path, path);
    }
}
fn remove_lease(session_id: &str) {
    let _ = std::fs::remove_file(lease_path(session_id));
}

fn read_leases() -> Vec<Lease> {
    let Ok(entries) = std::fs::read_dir(lease_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<Lease>(&bytes).ok())
        .filter(|lease| lease.session_id.starts_with("codefactory-"))
        .collect()
}

fn is_expired(lease: &Lease, at: u64) -> bool {
    at.saturating_sub(lease.updated_at_unix_secs) > LEASE_TTL.as_secs()
}

fn lease_belongs_to_ctx(lease: &Lease, ctx: &ExecCtx) -> bool {
    match (ctx.task_id.as_deref(), ctx.session_id.as_deref()) {
        (Some(task_id), _) => lease.task_id.as_deref() == Some(task_id),
        (None, Some(session_id)) => lease.owner_session_id.as_deref() == Some(session_id),
        (None, None) => false,
    }
}

#[cfg(unix)]
fn owner_process_is_running(owner_pid: u32) -> bool {
    owner_pid != 0 && unsafe { libc::kill(owner_pid as i32, 0) } == 0
}

#[cfg(not(unix))]
fn owner_process_is_running(owner_pid: u32) -> bool {
    // Windows still reclaims by TTL. Conservatively preserve recent leases
    // because another CodeFactory instance may be running.
    owner_pid != 0
}

pub fn list_managed_sessions() -> Vec<BrowserSessionView> {
    let at = now_secs();
    let mut sessions: Vec<_> = read_leases()
        .into_iter()
        .map(|lease| BrowserSessionView {
            expired: is_expired(&lease, at),
            session_id: lease.session_id,
            task_id: lease.task_id,
            owner_session_id: lease.owner_session_id,
            updated_at_unix_secs: lease.updated_at_unix_secs,
        })
        .collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at_unix_secs));
    sessions
}

pub async fn close_managed_session(session_id: &str) -> std::result::Result<(), String> {
    if !session_id.starts_with("codefactory-")
        || !read_leases()
            .iter()
            .any(|lease| lease.session_id == session_id)
    {
        return Err("Unknown CodeFactory-managed browser session".into());
    }
    let _ = run_cli(session_id, &["close".into()]).await;
    // `close` is idempotent from the product's perspective. A missing daemon
    // must not leave a stale lease forever.
    remove_lease(session_id);
    Ok(())
}

async fn close_matching(mut matches: impl FnMut(&Lease) -> bool) -> usize {
    let leases: Vec<_> = read_leases()
        .into_iter()
        .filter(|lease| matches(lease))
        .collect();
    for lease in &leases {
        let _ = run_cli(&lease.session_id, &["close".into()]).await;
        remove_lease(&lease.session_id);
    }
    leases.len()
}

pub async fn close_for_task(task_id: &str) -> usize {
    close_matching(|lease| lease.task_id.as_deref() == Some(task_id)).await
}

pub async fn close_for_session(session_id: &str) -> usize {
    close_matching(|lease| {
        lease.task_id.is_none() && lease.owner_session_id.as_deref() == Some(session_id)
    })
    .await
}

pub async fn close_all_for_owner_session(session_id: &str) -> usize {
    close_matching(|lease| lease.owner_session_id.as_deref() == Some(session_id)).await
}

/// At startup reclaim expired leases and leases whose owning process died.
/// Recent sessions owned by another live CodeFactory instance are preserved.
pub async fn reclaim_on_startup() -> usize {
    let at = now_secs();
    close_matching(|lease| is_expired(lease, at) || !owner_process_is_running(lease.owner_pid))
        .await
}

async fn reclaim_expired() {
    let at = now_secs();
    for lease in read_leases() {
        if is_expired(&lease, at) {
            let _ = run_cli(&lease.session_id, &["close".into()]).await;
            remove_lease(&lease.session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_ids_are_owned_and_unique() {
        let ctx = ExecCtx::new(std::env::temp_dir(), None);
        assert!(new_session_id(&ctx).starts_with("codefactory-"));
        assert_ne!(new_session_id(&ctx), new_session_id(&ctx));
    }

    #[test]
    fn screenshot_paths_cannot_escape_the_project() {
        let cwd = std::env::temp_dir().join("browser-session-project");
        let mut args = Args {
            action: "screenshot".into(),
            session_id: Some("codefactory-test".into()),
            url: None,
            target: None,
            text: None,
            path: Some("../outside.png".into()),
        };
        assert!(command_for(&args, &cwd).is_err());
        args.path = Some("proof/page.png".into());
        assert!(command_for(&args, &cwd).is_ok());
    }

    #[test]
    fn lease_expiration_is_strictly_bounded() {
        let lease = Lease {
            session_id: "codefactory-test".into(),
            task_id: None,
            owner_session_id: Some("session".into()),
            owner_pid: std::process::id(),
            updated_at_unix_secs: 1_000,
        };
        assert!(!is_expired(&lease, 1_000 + LEASE_TTL.as_secs()));
        assert!(is_expired(&lease, 1_001 + LEASE_TTL.as_secs()));
    }

    #[test]
    fn detects_cli_errors_even_when_the_process_exits_zero() {
        assert!(output_reports_error(
            "### Error\nError: Ref e999 not found in the current page snapshot."
        ));
        assert!(!output_reports_error(
            "### Page\n- Page URL: https://example.com/"
        ));
    }

    #[test]
    fn a_session_cannot_cross_task_ownership() {
        let lease = Lease {
            session_id: "codefactory-test".into(),
            task_id: Some("task-a".into()),
            owner_session_id: Some("session".into()),
            owner_pid: std::process::id(),
            updated_at_unix_secs: now_secs(),
        };
        let mut ctx = ExecCtx::new(std::env::temp_dir(), None);
        ctx.task_id = Some("task-b".into());
        assert!(!lease_belongs_to_ctx(&lease, &ctx));
        ctx.task_id = Some("task-a".into());
        assert!(lease_belongs_to_ctx(&lease, &ctx));
    }
}
