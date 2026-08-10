// SPDX-License-Identifier: Apache-2.0
use crate::util::no_window::NoWindow;
use crate::AppState;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

pub struct PtySession {
    /// Write end of the pty (send keystrokes to the shell).
    writer: Box<dyn Write + Send>,
    /// Master side kept alive so the pty stays open.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

pub struct TerminalState(pub Arc<Mutex<HashMap<String, PtySession>>>);

impl TerminalState {
    pub fn new() -> Self {
        TerminalState(Arc::new(Mutex::new(HashMap::new())))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the best available shell on Windows.
fn resolve_shell() -> String {
    // Prefer PowerShell if it exists on PATH.
    if which_powershell() {
        "powershell.exe".to_string()
    } else {
        "cmd.exe".to_string()
    }
}

fn which_powershell() -> bool {
    std::process::Command::new("powershell.exe")
        .no_window()
        .args(["-NoProfile", "-Command", "exit 0"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn terminal_create(
    id: String,
    cols: u16,
    rows: u16,
    app_handle: AppHandle,
    state: State<'_, TerminalState>,
    app_state: State<'_, AppState>,
) -> Result<(), String> {
    // The updater holds this same map while reserving a restart. Keep the lock
    // for the whole terminal admission so a PTY cannot be spawned in the gap
    // between the safety snapshot and the reservation bit.
    let mut sessions = state.0.lock().await;
    if app_state
        .update_restart_reserved
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err("应用更新已进入安全重启阶段，请等待自动恢复工作区".into());
    }
    let pty_system = native_pty_system();

    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    let shell = resolve_shell();
    let mut cmd = CommandBuilder::new(&shell);
    // Give the shell a clean environment-ish start.
    cmd.env("TERM", "xterm-256color");

    let _child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn_command failed: {e}"))?;

    // We need a clone of the master for the reader thread.
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone_reader failed: {e}"))?;

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take_writer failed: {e}"))?;

    // Background thread: pump pty output → Tauri events.
    let event_name = format!("terminal-output:{id}");
    let app = app_handle.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let _ = app.emit(&event_name, text);
                }
            }
        }
    });

    let session = PtySession {
        writer,
        _master: pair.master,
    };

    sessions.insert(id, session);
    Ok(())
}

#[tauri::command]
pub async fn terminal_write(
    id: String,
    data: String,
    state: State<'_, TerminalState>,
) -> Result<(), String> {
    let mut map = state.0.lock().await;
    let session = map
        .get_mut(&id)
        .ok_or_else(|| format!("No terminal session with id '{id}'"))?;
    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;
    session
        .writer
        .flush()
        .map_err(|e| format!("flush failed: {e}"))?;
    Ok(())
}

#[tauri::command]
pub async fn terminal_resize(
    id: String,
    cols: u16,
    rows: u16,
    state: State<'_, TerminalState>,
) -> Result<(), String> {
    let map = state.0.lock().await;
    let session = map
        .get(&id)
        .ok_or_else(|| format!("No terminal session with id '{id}'"))?;
    session
        ._master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("resize failed: {e}"))?;
    Ok(())
}

#[tauri::command]
pub async fn terminal_kill(id: String, state: State<'_, TerminalState>) -> Result<(), String> {
    state.0.lock().await.remove(&id);
    Ok(())
}
