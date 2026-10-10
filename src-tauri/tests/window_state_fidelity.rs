// SPDX-License-Identifier: Apache-2.0
//! M42 / CF-WIN-R1 + CF-WIN-R2 — the main window reopens where the user left it.
//!
//! Evidence behind the requirement: on 2026-10-09 the app was restarted three
//! times (15:23, 18:22, 19:07) after the user had dragged the window back onto
//! the main display, and each restart reopened it at the right edge of the
//! secondary display (x≈1894–1954). Nothing on disk recorded the position, so
//! every launch started from the static value in `tauri.conf.json`.
//!
//! These tests drive the real public persistence + placement API of
//! `codefactory_lib::window_state`, not a copy of the logic:
//!   * CF-WIN-R1 — an existing saved rectangle survives a save/load round trip
//!     (restart and upgrade restart both just re-read the same file), including
//!     through the atomic write used on every move/resize.
//!   * CF-WIN-R2 — a saved rectangle whose display is gone, that sits off
//!     screen, or that straddles two displays is discarded and replaced by a
//!     fully visible position on the primary display.
//!   * Compatibility — an old version left no file at all: that must load as
//!     "nothing saved" instead of panicking or restoring garbage.

use codefactory_lib::window_state::{
    fallback_state, load_saved_state, resolve_start_state, save_saved_state, window_state_path,
    MonitorRect, SavedWindowState, FALLBACK_INSET,
};
use std::path::{Path, PathBuf};

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "codefactory-window-state-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn main_display() -> MonitorRect {
    MonitorRect { x: 0, y: 0, width: 1440, height: 900 }
}

fn secondary_display() -> MonitorRect {
    MonitorRect { x: 1440, y: 0, width: 1920, height: 1080 }
}

/// A rectangle is "fully visible" when one connected display contains all of it.
fn fully_visible(state: SavedWindowState, displays: &[MonitorRect]) -> bool {
    displays.iter().any(|d| {
        state.x >= d.x
            && state.y >= d.y
            && i64::from(state.x) + i64::from(state.width) <= i64::from(d.x) + i64::from(d.width)
            && i64::from(state.y) + i64::from(state.height) <= i64::from(d.y) + i64::from(d.height)
    })
}

// ---------------------------------------------------------------- CF-WIN-R1 ---

#[test]
fn cf_win_r1_a_moved_window_survives_a_save_load_round_trip() {
    let dir = scratch_dir("round-trip");
    let path = window_state_path(&dir);
    // The user dragged the window to the main display (x≈108–153 in the report).
    let dragged = SavedWindowState { x: 128, y: 96, width: 1180, height: 740 };

    save_saved_state(&path, dragged).expect("save");
    let reloaded = load_saved_state(&path).expect("saved state is readable");

    assert_eq!(reloaded, dragged, "restart must reopen at the exact saved rect");
    // Atomic write: the temporary file must not survive, and both display
    // metrics (origin + size) must be preserved, not just the size.
    assert_eq!(window_state_path(&dir).with_extension("json.tmp").exists(), false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cf_win_r1_placement_follows_the_saved_display_not_the_primary_one() {
    let displays = [main_display(), secondary_display()];
    // The user's real case: window parked on the secondary display at x≈1900.
    let saved = SavedWindowState { x: 1900, y: 120, width: 1100, height: 700 };

    let placed = resolve_start_state(Some(saved), &displays, Some(main_display()));

    assert_eq!(placed, saved, "a window on a connected display is restored as-is");
}

#[test]
fn cf_win_r1_the_latest_position_wins_after_a_second_move() {
    let dir = scratch_dir("latest-wins");
    let path = window_state_path(&dir);
    let first = SavedWindowState { x: 40, y: 40, width: 1000, height: 700 };
    let second = SavedWindowState { x: 300, y: 220, width: 1024, height: 768 };

    save_saved_state(&path, first).expect("save first");
    save_saved_state(&path, second).expect("save second");

    assert_eq!(load_saved_state(&path), Some(second));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Directory handed to the child process so both processes agree on the file.
const RESTART_DIR_ENV: &str = "CODEFACTORY_WINDOW_STATE_RESTART_DIR";

/// CF-WIN-R1's "one real restart check": a *separate process* writes the
/// geometry the way a user's drag does, exits, and this process — standing in
/// for the app started again, possibly after an upgrade — reads it back and
/// resolves it to the same window on the same display.
#[test]
fn cf_win_r1_the_window_position_survives_a_real_process_restart() {
    const DRAGGED: SavedWindowState =
        SavedWindowState { x: 137, y: 96, width: 1234, height: 740 };

    if let Ok(child_dir) = std::env::var(RESTART_DIR_ENV) {
        // Child process: the user drags the window, then quits.
        save_saved_state(&window_state_path(Path::new(&child_dir)), DRAGGED)
            .expect("the quitting process saves its geometry");
        return;
    }

    let dir = std::env::temp_dir().join(format!(
        "codefactory-window-state-restart-{}",
        std::process::id()
    ));
    let path = window_state_path(&dir);

    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "cf_win_r1_the_window_position_survives_a_real_process_restart",
            "--nocapture",
        ])
        .env(RESTART_DIR_ENV, &dir)
        .output()
        .expect("spawn the restarting process");
    assert!(
        child.status.success(),
        "the restarting process failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );

    let restored = load_saved_state(&path).expect("the reopened app finds the saved geometry");
    assert_eq!(restored, DRAGGED, "reopening must land on the same rectangle");
    assert_eq!(
        resolve_start_state(Some(restored), &[main_display(), secondary_display()], Some(main_display())),
        DRAGGED,
        "and on the same display it was left on"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- CF-WIN-R2 ---

#[test]
fn cf_win_r2_unplugged_display_falls_back_to_a_visible_primary_position() {
    let remaining = [main_display()];
    // Saved while the secondary display existed (x≈1900), then unplugged.
    let saved = SavedWindowState { x: 1900, y: 120, width: 1100, height: 700 };

    let placed = resolve_start_state(Some(saved), &remaining, Some(main_display()));

    assert!(
        fully_visible(placed, &remaining),
        "unplugged display must not reopen off screen: {placed:?}"
    );
    assert_eq!(placed.x, main_display().x + FALLBACK_INSET);
    assert_eq!(placed.y, main_display().y + FALLBACK_INSET);
}

#[test]
fn cf_win_r2_off_screen_position_is_replaced_but_keeps_a_sane_size() {
    let displays = [main_display()];
    let saved = SavedWindowState { x: 5000, y: -400, width: 1024, height: 768 };

    let placed = resolve_start_state(Some(saved), &displays, Some(main_display()));

    assert!(fully_visible(placed, &displays), "must be fully on screen: {placed:?}");
    assert!(placed.width <= main_display().width && placed.height <= main_display().height);
}

#[test]
fn cf_win_r2_a_window_straddling_two_displays_is_not_restored() {
    let displays = [main_display(), secondary_display()];
    // Starts on the main display but hangs 540px over the secondary one.
    let straddling = SavedWindowState { x: 900, y: 100, width: 1080, height: 700 };

    let placed = resolve_start_state(Some(straddling), &displays, Some(main_display()));

    assert!(fully_visible(placed, &displays), "never reopen across displays: {placed:?}");
    assert_ne!(placed, straddling);
}

#[test]
fn cf_win_r2_window_larger_than_the_only_display_is_shrunk_into_view() {
    let laptop = [MonitorRect { x: 0, y: 0, width: 1280, height: 800 }];
    // Saved on a 4K display that is no longer attached.
    let saved = SavedWindowState { x: 0, y: 0, width: 3200, height: 1800 };

    let placed = resolve_start_state(Some(saved), &laptop, Some(laptop[0]));

    assert!(fully_visible(placed, &laptop), "must fit the remaining display: {placed:?}");
}

#[test]
fn cf_win_r2_fallback_without_any_monitor_still_returns_a_usable_rect() {
    let displays: [MonitorRect; 0] = [];
    let placed = fallback_state(None, None);
    assert!(placed.width > 0 && placed.height > 0, "never a zero-sized window");
    assert!(!fully_visible(placed, &displays));
}

// ------------------------------------------------------------ Compatibility ---

#[test]
fn compatibility_an_upgrade_from_a_version_without_a_state_file_opens_normally() {
    let dir = scratch_dir("no-file");
    let path = window_state_path(&dir);

    // No file at all — exactly what an upgrade from an older release looks like.
    assert_eq!(load_saved_state(&path), None);
    // And the placement decision still yields a fully visible window.
    let placed = resolve_start_state(None, &[main_display()], Some(main_display()));
    assert!(fully_visible(placed, &[main_display()]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compatibility_a_corrupt_state_file_is_ignored_instead_of_panicking() {
    let dir = scratch_dir("corrupt");
    let path = window_state_path(&dir);
    std::fs::write(&path, b"{ not json").expect("write garbage");

    assert_eq!(load_saved_state(&path), None);

    // A later good save must still work over the corrupt file.
    let good = SavedWindowState { x: 60, y: 60, width: 1200, height: 800 };
    save_saved_state(&path, good).expect("save over corrupt file");
    assert_eq!(load_saved_state(&path), Some(good));
    let _ = std::fs::remove_dir_all(&dir);
}
