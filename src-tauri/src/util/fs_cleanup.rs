// SPDX-License-Identifier: Apache-2.0
//! Directory removal that survives a filesystem still letting go of a path.
//!
//! On Unix, unlinking a file that someone still has open just works. On Windows
//! it does not: a delete fails with `ERROR_SHARING_VIOLATION` while any handle
//! is open without `FILE_SHARE_DELETE`, or while a memory mapping of the file
//! is still being torn down. Neither condition is observable from the deleting
//! process, and both resolve on their own — a virus scanner finishes reading a
//! freshly written file, the kernel releases a section object — so the only
//! workable answer is to retry.
//!
//! Prefer removing the *cause* where there is one. [`crate::storage::db::
//! close_and_release_files`] exists because a SQLite pool left memory-mapped
//! `-shm` sidecars behind; that was a real defect, and retrying around it would
//! only have hidden it. This module is for the residue that no amount of
//! correct teardown eliminates.

use std::path::Path;
use std::time::{Duration, Instant};

/// `ERROR_SHARING_VIOLATION` — open on a handle that did not grant
/// `FILE_SHARE_DELETE`, or a mapping that is still being released.
#[cfg(windows)]
const ERROR_SHARING_VIOLATION: i32 = 32;
/// `ERROR_DIR_NOT_EMPTY` — a child unlink has not settled yet.
#[cfg(windows)]
const ERROR_DIR_NOT_EMPTY: i32 = 145;

/// How long to keep trying before giving up.
///
/// Only paid when a removal is actually contended: the first attempt happens
/// immediately and the overwhelming majority succeed there. The budget is sized
/// for a saturated CI runner — in the failure this module was written for, the
/// Windows job completed 16 tests in 25 seconds while the old flat 10 × 50 ms
/// (500 ms) budget ran out.
const TOTAL_BUDGET: Duration = Duration::from_secs(10);
const FIRST_BACKOFF: Duration = Duration::from_millis(20);
const MAX_BACKOFF: Duration = Duration::from_millis(800);

/// Whether a failed removal is worth retrying.
///
/// Only the Windows handle/mapping races are transient. A genuine
/// `PermissionDenied` on Unix will never resolve by waiting, so retrying there
/// would just add ten seconds to every real failure.
#[cfg(windows)]
fn is_transient(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(ERROR_SHARING_VIOLATION) | Some(ERROR_DIR_NOT_EMPTY)
    ) || error.kind() == std::io::ErrorKind::PermissionDenied
}

#[cfg(not(windows))]
fn is_transient(_error: &std::io::Error) -> bool {
    false
}

/// Remove `path` and everything under it, retrying while the OS is still
/// releasing handles.
///
/// Returns `Ok(())` if `path` does not exist. Non-transient errors are returned
/// on the first attempt without waiting.
pub async fn remove_dir_all_with_retry(path: &Path) -> std::io::Result<()> {
    let deadline = Instant::now() + TOTAL_BUDGET;
    let mut backoff = FIRST_BACKOFF;

    loop {
        match attempt_removal(path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !is_transient(&error) || Instant::now() + backoff >= deadline {
                    return Err(error);
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// [`remove_dir_all_with_retry`] for callers that are already on a blocking
/// thread.
///
/// The browser installer needs this: unpacking 150 MB and moving it into place
/// runs under `spawn_blocking`, and the removal it has to survive is the same
/// Windows handle race — a scanner reading the `chrome.exe` that was written a
/// moment ago. Sharing the classification and the budget with the async version
/// keeps one policy for "the filesystem has not let go yet" rather than two that
/// drift.
pub fn remove_dir_all_blocking(path: &Path) -> std::io::Result<()> {
    let deadline = Instant::now() + TOTAL_BUDGET;
    let mut backoff = FIRST_BACKOFF;

    loop {
        match attempt_removal(path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if !is_transient(&error) || Instant::now() + backoff >= deadline {
                    return Err(error);
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// One removal attempt, with the read-only attribute cleared first if needed.
///
/// Windows refuses to delete a file carrying `FILE_ATTRIBUTE_READONLY`, and a zip
/// entry can arrive with it set — a case no amount of waiting resolves, so it is
/// handled here rather than treated as a transient failure. Clearing is only
/// attempted after a failure, so the overwhelmingly common case stays a single
/// `remove_dir_all` call.
fn attempt_removal(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            clear_read_only(path);
            match std::fs::remove_dir_all(path) {
                Ok(()) => Ok(()),
                Err(second) if second.kind() == std::io::ErrorKind::NotFound => Ok(()),
                // Report the original error: it describes why the removal failed
                // before we started changing attributes.
                Err(_) => Err(error),
            }
        }
    }
}

/// Clear the read-only attribute across a tree so deletion can proceed.
fn clear_read_only(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(path, permissions);
    }
    if metadata.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                clear_read_only(&entry.path());
            }
        }
    }
}

/// Remove a test or smoke fixture directory, reporting rather than failing.
///
/// Fixture cleanup is hygiene, not an assertion. A temp directory that outlives
/// a run costs nothing on a throwaway CI runner, so a stuck handle must not turn
/// a suite whose assertions all passed into a red build — that is exactly the
/// flake this module was written to end. The warning carries a listing of what
/// is still on disk so a genuine teardown regression stays diagnosable.
pub async fn remove_fixture_dir(path: &Path) {
    if let Err(error) = remove_dir_all_with_retry(path).await {
        tracing::warn!(
            "could not remove fixture directory {} after {}s: {error}\nstill present:\n{}",
            path.display(),
            TOTAL_BUDGET.as_secs(),
            describe_directory(path),
        );
        eprintln!(
            "warning: could not remove fixture directory {} after {}s: {error}\nstill present:\n{}",
            path.display(),
            TOTAL_BUDGET.as_secs(),
            describe_directory(path),
        );
    }
}

/// A one-file-per-line listing of `path`, for diagnosing a removal that stuck.
pub fn describe_directory(path: &Path) -> String {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<String>) {
        if depth > 4 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(format!(
                "{:indent$}{} ({size} bytes)",
                "",
                entry.file_name().to_string_lossy(),
                indent = depth * 2
            ));
            if child.is_dir() {
                walk(&child, depth + 1, out);
            }
        }
    }

    let mut out = Vec::new();
    walk(path, 0, &mut out);
    if out.is_empty() {
        "  (nothing)".to_string()
    } else {
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "codefactory-fscleanup-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested").join("file.txt"), b"payload").unwrap();
        root
    }

    #[tokio::test]
    async fn removes_a_populated_directory() {
        let root = fixture_root("basic");
        remove_dir_all_with_retry(&root).await.unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn the_blocking_flavour_removes_the_same_tree() {
        // Used by the browser installer, which runs on a blocking thread and
        // cannot await.
        let root = fixture_root("blocking");
        remove_dir_all_blocking(&root).unwrap();
        assert!(!root.exists());
        // Idempotent, like the async one: an already-gone path is success.
        remove_dir_all_blocking(&root).unwrap();
    }

    #[test]
    fn a_read_only_file_does_not_block_removal() {
        // Windows refuses to delete a read-only file, and archives can set that
        // attribute. Waiting would never fix it, so the attribute is cleared.
        let root = fixture_root("readonly");
        let file = root.join("nested").join("file.txt");
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();

        remove_dir_all_blocking(&root).unwrap();

        assert!(!root.exists());
    }

    #[tokio::test]
    async fn missing_directory_is_success() {
        let root =
            std::env::temp_dir().join(format!("codefactory-absent-{}", uuid::Uuid::new_v4()));
        remove_dir_all_with_retry(&root).await.unwrap();
    }

    /// The Windows path is the whole reason this module exists, and CI runs
    /// windows-latest only — so this is the test that actually covers the
    /// failure mode. It creates a real `ERROR_SHARING_VIOLATION` by holding the
    /// file open without `FILE_SHARE_DELETE`, releases it after 600 ms, and
    /// asserts the removal still lands.
    ///
    /// 600 ms is chosen deliberately: it is longer than the 500 ms flat budget
    /// this helper replaced, so a regression back to that budget fails here.
    #[cfg(windows)]
    #[tokio::test]
    async fn retries_through_a_real_sharing_violation() {
        use std::os::windows::fs::OpenOptionsExt;

        /// Deliberately omits `FILE_SHARE_DELETE`, which is what makes a delete
        /// fail with `ERROR_SHARING_VIOLATION` while this handle is alive.
        const FILE_SHARE_READ: u32 = 0x0000_0001;

        let root = fixture_root("sharing");
        let locked_path = root.join("nested").join("file.txt");

        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&locked_path)
            .expect("open the fixture file without FILE_SHARE_DELETE");

        // Establish that the condition we are testing is genuinely present:
        // without the retry loop this removal fails right now, and it fails with
        // an error this module classifies as worth retrying. Asserting the
        // classification rather than a literal errno keeps the test honest if
        // std's Windows `remove_dir_all` reports the lock as a delete-pending
        // access denial instead of a sharing violation.
        let immediate = std::fs::remove_dir_all(&root);
        let error = immediate.expect_err("a locked file must block plain remove_dir_all");
        assert!(
            is_transient(&error),
            "a locked file should produce a retryable error, got {error:?} \
             (raw os error {:?})",
            error.raw_os_error()
        );

        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            drop(locked);
        });

        remove_dir_all_with_retry(&root)
            .await
            .expect("retry must outlast a 600ms lock");

        releaser.join().unwrap();
        assert!(!root.exists(), "{} should be gone", root.display());
    }
}
