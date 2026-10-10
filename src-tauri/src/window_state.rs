// SPDX-License-Identifier: Apache-2.0
//! M42 — the main window reopens where the user left it (CF-WIN-R1 / CF-WIN-R2).
//!
//! Reported behaviour: on 2026-10-09 the app was restarted at 15:23, 18:22 and
//! 19:07 — twice of those were upgrades. The user had dragged the window back
//! onto the main display before each restart (x≈108–153), and every launch
//! reopened it at the right edge of the secondary display or beyond the main
//! one (x≈1894–1954). Nothing on disk recorded the position, so each launch
//! started from the static `tauri.conf.json` geometry.
//!
//! The pure decision logic lives in plain functions so it can be driven by
//! `tests/window_state_fidelity.rs` without a GUI:
//!
//! * [`resolve_start_state`] — restore the saved rectangle when a connected
//!   display fully contains it, otherwise place a fully visible window on the
//!   primary display (CF-WIN-R2).
//! * [`save_saved_state`] / [`load_saved_state`] — the durable record survives
//!   restarts and upgrades, and an older release that never wrote one, or a
//!   truncated file, is read as "nothing saved" instead of failing startup
//!   (Compatibility Harness).
//!
//! Only [`restore_and_track_main_window`] touches Tauri.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::Manager;

/// Window label of the main window (`src-tauri/tauri.conf.json`).
pub const MAIN_WINDOW_LABEL: &str = "main";
/// File inside the app data dir that carries the last known geometry.
pub const WINDOW_STATE_FILE: &str = "window-state.json";
/// Gap between the primary display's corner and a window we had to re-place.
pub const FALLBACK_INSET: i32 = 40;
/// Size used only when nothing at all was ever saved.
pub const DEFAULT_WINDOW_WIDTH: u32 = 1200;
pub const DEFAULT_WINDOW_HEIGHT: u32 = 800;

/// Last known main-window geometry, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedWindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A connected display, in physical pixels. A plain value type so the placement
/// rules stay testable without a window system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// True when one display contains the whole rectangle — origin and size.
///
/// "Fully inside one display" is the rule the spec states: a window that
/// straddles two displays, or pokes past an edge, must not be restored there.
pub fn fully_inside_monitor(state: SavedWindowState, monitor: MonitorRect) -> bool {
    state.x >= monitor.x
        && state.y >= monitor.y
        && i64::from(state.x) + i64::from(state.width)
            <= i64::from(monitor.x) + i64::from(monitor.width)
        && i64::from(state.y) + i64::from(state.height)
            <= i64::from(monitor.y) + i64::from(monitor.height)
}

/// True when any currently connected display contains the saved rectangle.
pub fn is_saved_state_usable(state: SavedWindowState, monitors: &[MonitorRect]) -> bool {
    monitors.iter().any(|monitor| fully_inside_monitor(state, *monitor))
}

/// A fully visible position on the primary display, keeping the window's own
/// size where it still fits (CF-WIN-R2).
///
/// `preferred` carries the size the user last chose; `None` falls back to the
/// default size. The result is always inside `primary` — never off screen, never
/// wider than the display — because a rescued position that is itself invisible
/// would repeat the defect this module exists to fix.
pub fn fallback_state(primary: Option<MonitorRect>, preferred: Option<SavedWindowState>) -> SavedWindowState {
    // No monitor reported at all: still hand back a usable, non-degenerate
    // window rather than a zero-sized one.
    let display = primary.unwrap_or(MonitorRect {
        x: 0,
        y: 0,
        width: DEFAULT_WINDOW_WIDTH + FALLBACK_INSET as u32 * 2,
        height: DEFAULT_WINDOW_HEIGHT + FALLBACK_INSET as u32 * 2,
    });
    // Keep the inset meaningful on a tiny display without ever pushing the
    // window past the edge.
    let inset_x = FALLBACK_INSET.min((display.width / 4) as i32);
    let inset_y = FALLBACK_INSET.min((display.height / 4) as i32);
    let available_width = display.width.saturating_sub(inset_x as u32 * 2).max(1);
    let available_height = display.height.saturating_sub(inset_y as u32 * 2).max(1);
    let wanted = preferred.unwrap_or(SavedWindowState {
        x: 0,
        y: 0,
        width: DEFAULT_WINDOW_WIDTH,
        height: DEFAULT_WINDOW_HEIGHT,
    });
    SavedWindowState {
        x: display.x + inset_x,
        y: display.y + inset_y,
        width: wanted.width.clamp(1, available_width),
        height: wanted.height.clamp(1, available_height),
    }
}

/// Where the main window should open (cf. `README`/spec CF-WIN-R1, R2).
pub fn resolve_start_state(
    saved: Option<SavedWindowState>,
    monitors: &[MonitorRect],
    primary: Option<MonitorRect>,
) -> SavedWindowState {
    match saved {
        // A connected display still holds the whole window: put it back exactly
        // where the user left it, same size, same display.
        Some(state) if is_saved_state_usable(state, monitors) => state,
        other => fallback_state(primary, other),
    }
}

/// `<app data dir>/window-state.json`.
pub fn window_state_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(WINDOW_STATE_FILE)
}

/// Read the last saved geometry. Any problem — missing file (an upgrade from a
/// release that never wrote one), unreadable file, truncated content — means
/// "nothing saved", never a startup failure.
pub fn load_saved_state(path: &Path) -> Option<SavedWindowState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<SavedWindowState>(&bytes).ok()
}

/// Persist the geometry atomically: write a sibling `.tmp` file, then rename.
/// A crash or power loss mid-write therefore leaves the previous good state
/// readable rather than a half-written file.
pub fn save_saved_state(path: &Path, state: SavedWindowState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec(&state)?;
    let temporary = temporary_state_path(path);
    std::fs::write(&temporary, json)?;
    std::fs::rename(&temporary, path)
}

/// `window-state.json` → `window-state.json.tmp` (sibling, same directory).
pub fn temporary_state_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn monitor_rect(monitor: &tauri::Monitor) -> MonitorRect {
    let position = monitor.position();
    let size = monitor.size();
    MonitorRect { x: position.x, y: position.y, width: size.width, height: size.height }
}

/// Put the main window back, then keep the saved state current.
///
/// Called once from `tauri::Builder::setup`, i.e. before the user sees the
/// window move.
pub fn restore_and_track_main_window(app: &tauri::App) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        // A harness built without the main window has nothing to place; that is
        // not a reason to abort startup.
        return Ok(());
    };
    let path = window_state_path(&app.path().app_data_dir()?);

    let monitors: Vec<MonitorRect> = window
        .available_monitors()?
        .iter()
        .map(monitor_rect)
        .collect();
    let primary = window.primary_monitor()?.as_ref().map(monitor_rect);

    let placement = resolve_start_state(load_saved_state(&path), &monitors, primary);
    window.set_size(tauri::PhysicalSize::new(placement.width, placement.height))?;
    window.set_position(tauri::PhysicalPosition::new(placement.x, placement.y))?;

    // Remember every position the user drags the window to, and every size they
    // resize it to. `last_written` keeps a resize storm (which fires dozens of
    // events per second) from turning into dozens of file writes.
    let tracked = window.clone();
    let last_written = std::sync::Mutex::new(Some(placement));
    window.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)) {
            return;
        }
        let (Ok(position), Ok(size)) = (tracked.outer_position(), tracked.inner_size()) else {
            return;
        };
        let state = SavedWindowState {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        };
        if let Ok(mut previous) = last_written.lock() {
            if *previous == Some(state) {
                return;
            }
            *previous = Some(state);
        }
        let _ = save_saved_state(&path, state);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_display() -> MonitorRect {
        MonitorRect { x: 0, y: 0, width: 1440, height: 900 }
    }

    fn secondary_display() -> MonitorRect {
        MonitorRect { x: 1440, y: 0, width: 1920, height: 1080 }
    }

    #[test]
    fn accepts_window_fully_on_one_monitor() {
        assert!(fully_inside_monitor(
            SavedWindowState { x: 20, y: 30, width: 800, height: 600 },
            main_display()
        ));
    }

    #[test]
    fn rejects_disconnected_monitor_and_offscreen_positions() {
        assert!(!fully_inside_monitor(
            SavedWindowState { x: 1500, y: 30, width: 800, height: 600 },
            main_display()
        ));
        assert!(!fully_inside_monitor(
            SavedWindowState { x: -100, y: 0, width: 800, height: 600 },
            main_display()
        ));
    }

    #[test]
    fn rejects_window_spanning_displays() {
        let straddling = SavedWindowState { x: 1200, y: 30, width: 800, height: 600 };
        assert!(!fully_inside_monitor(straddling, main_display()));
        assert!(!is_saved_state_usable(straddling, &[main_display(), secondary_display()]));
    }

    #[test]
    fn temporary_path_is_a_sibling_of_the_state_file() {
        let path = window_state_path(Path::new("/tmp/app-data"));
        assert_eq!(path.file_name().unwrap(), WINDOW_STATE_FILE);
        assert_eq!(
            temporary_state_path(&path).file_name().unwrap(),
            "window-state.json.tmp"
        );
    }
}
