// SPDX-License-Identifier: Apache-2.0
//! Persist Rust panics so a production crash can be diagnosed.
//!
//! 2026-08-10, v1.78.6: the app vanished on a user's machine. The macOS crash
//! report said `SIGABRT / abort() called` on a `tokio-rt-worker` thread with 37
//! frames, every one inside the stripped main binary — `atos` resolved nothing,
//! and the panic message existed only on a stderr no GUI process is attached
//! to. All that could be established was "some async task panicked".
//!
//! That is not bad luck on one crash; `[profile.release]` sets `strip = true`
//! and `panic = "abort"`, and the release uploads no dSYM, so EVERY production
//! panic was equally opaque.
//!
//! A panic hook still runs under `panic = "abort"` — abort happens after the
//! hook returns — so the message and `file:line` are recoverable even though
//! the unwinding backtrace is not. That is the bulk of the diagnostic value for
//! a fraction of the cost of shipping symbols.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Keep the newest N crash records. Enough to spot a repeating pattern without
/// growing without bound in a directory the user never cleans.
const KEEP: usize = 20;

pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("crash")
}

/// Install the process-wide panic hook.
///
/// Chains to the previous hook so the default stderr output survives for
/// terminal runs and tests.
pub fn install(data_dir: &Path) {
    let dir = log_dir(data_dir);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::warn!("crash log dir unavailable, panics stay stderr-only: {error}");
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Never let the reporter itself panic: that would abort inside the
        // panic handler and lose even the stderr copy.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write_record(&dir, &render(info));
        }));
        previous(info);
    }));
}

/// One human-readable record: when, which thread, what, and where.
///
/// The thread name is load-bearing — the 1.78.6 report could only say
/// "tokio-rt-worker", and naming the panicking thread is what turns that into a
/// starting point.
pub fn render(info: &std::panic::PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown location>".into());
    format!(
        "time: {}\nversion: {}\nthread: {}\nlocation: {}\nmessage: {}\n",
        chrono::Utc::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION"),
        thread.name().unwrap_or("<unnamed>"),
        location,
        panic_message(info),
    )
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".into()
    }
}

/// Write one record and flush it before returning.
///
/// `sync_all` is not optional: the process aborts as soon as the hook returns,
/// so anything still buffered is lost — which is precisely the failure this
/// module exists to prevent.
fn write_record(dir: &Path, body: &str) {
    let name = format!(
        "panic-{}.log",
        chrono::Utc::now().format("%Y%m%d-%H%M%S%.3f")
    );
    if let Ok(mut file) = std::fs::File::create(dir.join(name)) {
        let _ = file.write_all(body.as_bytes());
        let _ = file.flush();
        let _ = file.sync_all();
    }
    prune(dir);
}

/// Drop the oldest records beyond [`KEEP`].
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("panic-") && n.ends_with(".log"))
        })
        .collect();
    if logs.len() <= KEEP {
        return;
    }
    // Names are timestamped, so lexical order is chronological.
    logs.sort();
    for stale in &logs[..logs.len() - KEEP] {
        let _ = std::fs::remove_file(stale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_names_the_thread_and_the_source_location() {
        // The 1.78.6 crash report could say no more than "tokio-rt-worker".
        // These four fields are what turn that into a starting point.
        let dir = tempfile::tempdir().unwrap();
        write_record(
            dir.path(),
            "time: t\nversion: 9.9.9\nthread: tokio-rt-worker\nlocation: src/x.rs:12:5\nmessage: boom\n",
        );
        let written = std::fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap();
        let body = std::fs::read_to_string(written.path()).unwrap();
        for field in ["thread: tokio-rt-worker", "location: src/x.rs:12:5", "message: boom"] {
            assert!(body.contains(field), "missing {field:?} in:\n{body}");
        }
    }

    #[test]
    fn records_are_pruned_to_a_bounded_window() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..KEEP + 5 {
            std::fs::write(dir.path().join(format!("panic-{i:04}.log")), "x").unwrap();
        }
        prune(dir.path());
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), KEEP);
        // The oldest go first; the newest must survive.
        assert!(names.contains(&format!("panic-{:04}.log", KEEP + 4)));
        assert!(!names.contains(&"panic-0000.log".to_string()));
    }

    #[test]
    fn unrelated_files_are_never_pruned() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep-me.txt"), "x").unwrap();
        for i in 0..KEEP + 3 {
            std::fs::write(dir.path().join(format!("panic-{i:04}.log")), "x").unwrap();
        }
        prune(dir.path());
        assert!(dir.path().join("keep-me.txt").exists());
    }

    /// End-to-end: install the real hook, panic on a named thread, and read the
    /// file back.
    ///
    /// The unit tests above exercise `write_record` directly, which proves the
    /// formatting but not the WIRING — and the wiring is the whole point. If
    /// `install` failed to register, or the hook never reached the disk before
    /// the process ended, the unit tests would still pass and production would
    /// stay exactly as blind as it was for v1.78.6.
    #[test]
    fn a_real_panic_on_a_worker_thread_reaches_disk() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path());

        // Panicking on a named thread mirrors the 1.78.6 report, whose only
        // clue was the thread name.
        let handle = std::thread::Builder::new()
            .name("tokio-rt-worker".into())
            .spawn(|| panic!("simulated async failure"))
            .unwrap();
        assert!(handle.join().is_err(), "the thread must actually panic");

        let record = std::fs::read_dir(log_dir(dir.path()))
            .unwrap()
            .flatten()
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .find(|body| body.contains("simulated async failure"))
            .expect("the panic must be on disk, not only on stderr");
        assert!(record.contains("thread: tokio-rt-worker"), "{record}");
        // `file!()` uses the host separator — `src/panic_log.rs` on unix,
        // `src\panic_log.rs` on Windows — so match on the parts, not the
        // joined path. Asserting the unix spelling passed locally and failed
        // the whole release build on the windows-latest runner.
        assert!(record.contains("location: src"), "{record}");
        assert!(record.contains("panic_log.rs:"), "{record}");
        assert!(record.contains(env!("CARGO_PKG_VERSION")), "{record}");

        let _ = std::panic::take_hook();
    }

    #[test]
    fn a_missing_directory_is_survivable_rather_than_fatal() {
        // Losing a crash log must never itself crash the app.
        write_record(Path::new("/nonexistent/codefactory-crash"), "body");
        prune(Path::new("/nonexistent/codefactory-crash"));
    }
}
