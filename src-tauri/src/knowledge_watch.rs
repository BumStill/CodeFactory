// SPDX-License-Identifier: Apache-2.0
//! Automatic index maintenance for personal knowledge libraries (M191).
//!
//! Before this module the only way a library index moved was the user pressing
//! "scan", so a note saved in Obsidian or a deck exported to `~/Documents`
//! never became retrievable. Here three ordinary triggers keep the index
//! current without any user action:
//!
//! * a filesystem watcher, debounced so one save produces one scan;
//! * a periodic reconciliation pass that catches whatever the watcher missed
//!   (watcher limits, network volumes, app closed);
//! * a startup pass that also repairs a scan interrupted by a crash.
//!
//! All of them funnel into `knowledge::scan_library`, which is incremental and
//! serialized per library — the triggers never need to coordinate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use sqlx::SqlitePool;

/// Editors write a document in several steps (temp file, rename, close). One
/// quiet window turns that burst into a single scan.
pub const DEBOUNCE: Duration = Duration::from_millis(700);
/// Safety net for events the OS never delivered (or arrived while asleep).
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(600);
/// How often pending debounced work is checked for expiry.
const TICK: Duration = Duration::from_millis(200);

static EVENT_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<String>> = OnceLock::new();

fn watchers() -> &'static Mutex<HashMap<String, notify::RecommendedWatcher>> {
    static WATCHERS: OnceLock<Mutex<HashMap<String, notify::RecommendedWatcher>>> = OnceLock::new();
    WATCHERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_watchers(
) -> std::sync::MutexGuard<'static, HashMap<String, notify::RecommendedWatcher>> {
    match watchers().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Collapses a burst of change notifications for the same library into one
/// "scan this now" instruction. Pure and clock-injected so it is testable
/// without sleeping.
#[derive(Debug, Default)]
pub struct DebounceQueue {
    window: Duration,
    pending: HashMap<String, Instant>,
}

impl DebounceQueue {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            pending: HashMap::new(),
        }
    }

    /// Record a change; repeated changes push the quiet window forward.
    pub fn note(&mut self, library_id: &str, now: Instant) {
        self.pending
            .insert(library_id.to_string(), now + self.window);
    }

    /// Libraries whose quiet window has elapsed. Each is returned exactly once.
    pub fn take_due(&mut self, now: Instant) -> Vec<String> {
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(library_id, _)| library_id.clone())
            .collect();
        for library_id in &due {
            self.pending.remove(library_id);
        }
        due
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Debounce + fan out loop plus the startup/periodic reconciliation that makes
/// "app was closed" and "watcher missed it" self-healing.
pub fn start(pool: SqlitePool) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    if EVENT_TX.set(tx).is_err() {
        return; // already running in this process
    }
    tauri::async_runtime::spawn(async move {
        match crate::knowledge::reconcile_all_libraries(&pool).await {
            Ok(ids) => tracing::info!("knowledge startup reconcile covered {} librar(ies)", ids.len()),
            Err(e) => tracing::warn!("knowledge startup reconcile failed: {e}"),
        }
        refresh(&pool).await;

        let mut queue = DebounceQueue::new(DEBOUNCE);
        let mut ticker = tokio::time::interval(TICK);
        let mut periodic = tokio::time::interval(RECONCILE_INTERVAL);
        loop {
            tokio::select! {
                Some(library_id) = rx.recv() => queue.note(&library_id, Instant::now()),
                _ = ticker.tick() => {
                    for library_id in queue.take_due(Instant::now()) {
                        let pool = pool.clone();
                        tauri::async_runtime::spawn(async move {
                            match crate::knowledge::scan_library(&pool, &library_id).await {
                                Ok(summary) => tracing::debug!(
                                    "knowledge auto-index of {library_id}: {} added, {} updated, {} removed",
                                    summary.added_documents,
                                    summary.updated_documents,
                                    summary.removed_documents
                                ),
                                Err(e) => tracing::warn!(
                                    "knowledge auto-index failed for {library_id}: {e}"
                                ),
                            }
                        });
                    }
                }
                _ = periodic.tick() => {
                    let pool = pool.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = crate::knowledge::reconcile_all_libraries(&pool).await {
                            tracing::warn!("knowledge periodic reconcile failed: {e}");
                        }
                    });
                }
            }
        }
    });
}

/// Kick off one index pass in the background. Scans are serialized per
/// library, so a trigger arriving mid-scan is merged into the running pass
/// instead of racing it (M191 R5).
pub fn spawn_scan(pool: SqlitePool, library_id: String) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::knowledge::scan_library(&pool, &library_id).await {
            tracing::warn!("knowledge index for {library_id} failed: {e}");
        }
    });
}

/// Rebuild watcher registrations from the current library list: enabled
/// libraries get a watcher, disabled or deleted ones lose theirs.
pub async fn refresh(pool: &SqlitePool) {
    let libraries = match crate::knowledge::list_libraries(pool).await {
        Ok(libraries) => libraries,
        Err(e) => {
            tracing::warn!("knowledge watcher refresh skipped: {e}");
            return;
        }
    };
    let enabled: HashMap<String, std::path::PathBuf> = libraries
        .into_iter()
        .filter(|library| library.enabled)
        .map(|library| (library.id, std::path::PathBuf::from(library.root_path)))
        .collect();

    let mut guard = lock_watchers();
    guard.retain(|library_id, _| enabled.contains_key(library_id));
    for (library_id, root) in enabled {
        if guard.contains_key(&library_id) {
            continue;
        }
        match watch_library(&library_id, &root) {
            Ok(watcher) => {
                guard.insert(library_id, watcher);
            }
            Err(e) => tracing::warn!("knowledge watcher for {library_id} unavailable: {e}"),
        }
    }
}

fn watch_library(
    library_id: &str,
    root: &Path,
) -> Result<notify::RecommendedWatcher, notify::Error> {
    let sender = EVENT_TX.get().cloned();
    let watcher = watch_into(library_id, root, move |library_id| {
        if let Some(sender) = &sender {
            let _ = sender.send(library_id.to_string());
        }
    })?;
    Ok(watcher)
}

/// Register a recursive watcher that reports the library id (not raw paths) on
/// every content change. Access-only events (opens, reads, attribute probes) are
/// ignored: they happen constantly and mean nothing changed.
pub fn watch_into<F>(
    library_id: &str,
    root: &Path,
    report: F,
) -> Result<notify::RecommendedWatcher, notify::Error>
where
    F: Fn(&str) + Send + 'static,
{
    let library_id = library_id.to_string();
    let mut watcher = notify::recommended_watcher(
        move |result: Result<notify::Event, notify::Error>| {
            let Ok(event) = result else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            report(&library_id);
        },
    )?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_collapses_a_save_burst_into_one_scan() {
        let start = Instant::now();
        let mut queue = DebounceQueue::new(DEBOUNCE);
        queue.note("kb", start);
        queue.note("kb", start + Duration::from_millis(200));
        queue.note("kb", start + Duration::from_millis(500));

        assert!(
            queue.take_due(start + Duration::from_millis(600)).is_empty(),
            "a save still in flight must not trigger a scan"
        );
        assert_eq!(
            queue.take_due(start + Duration::from_millis(1300)),
            vec!["kb".to_string()],
            "the burst must yield exactly one scan"
        );
        assert!(
            queue.take_due(start + Duration::from_millis(5000)).is_empty(),
            "an already-fired library must not scan again"
        );
    }

    #[test]
    fn debounce_keeps_libraries_independent() {
        let start = Instant::now();
        let mut queue = DebounceQueue::new(DEBOUNCE);
        queue.note("a", start);
        queue.note("b", start + Duration::from_millis(400));
        let due = queue.take_due(start + Duration::from_millis(900));
        assert_eq!(due, vec!["a".to_string()]);
        assert!(!queue.is_empty(), "library b is still inside its window");
    }

    #[tokio::test]
    async fn filesystem_change_reports_the_library_id() {
        let root = std::env::temp_dir().join(format!(
            "codefactory-kb-watch-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create watch root");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let _watcher = watch_into("watch-kb", &root, move |library_id| {
            let _ = tx.send(library_id.to_string());
        })
        .expect("register watcher");

        std::fs::write(root.join("note.docx"), b"changed").expect("write watched file");

        let received = tokio::time::timeout(Duration::from_secs(15), rx.recv())
            .await
            .expect("watcher should report a change")
            .expect("channel stays open");
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(received, "watch-kb");
    }
}
