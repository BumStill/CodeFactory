// SPDX-License-Identifier: Apache-2.0
//! Per-file locking + atomic writes + Windows-friendly retry.
//!
//! Why this exists: when several tool calls (or several subagents running in
//! parallel) hit the same file, two things go wrong:
//!
//!   1. **TOCTOU on edit** — read+modify+write is not atomic; one writer's
//!      changes get clobbered by another's.
//!   2. **Windows sharing violations** — `std::fs::write` against a file held
//!      open by anyone (IDE, antivirus, the other tool call) returns
//!      `OS error 32` (ERROR_SHARING_VIOLATION).
//!
//! This module provides:
//!   * `acquire(path)` — async mutex keyed on the canonical absolute path.
//!     Different files run in parallel; same file serialises automatically.
//!   * `atomic_write(path, data)` — write to a sibling temp file and rename
//!     into place. Rename is atomic on the same volume on Windows + Unix.
//!   * `with_sharing_retry(op)` — wraps an IO operation in exponential
//!     backoff against `ErrorKind::PermissionDenied` and the Windows
//!     sharing-violation code, so we self-heal through transient locks
//!     from external processes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use once_cell::sync::Lazy;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Registry mapping canonical absolute path → per-file mutex.
/// `DashMap` is a sharded concurrent map so registry lookups don't bottleneck
/// even with many parallel tasks.
static LOCKS: Lazy<DashMap<PathBuf, Arc<Mutex<()>>>> = Lazy::new(DashMap::new);

/// Acquire the lock for `path`. Holding the returned guard guarantees no
/// other call to `acquire(same_path)` proceeds until you drop it.
///
/// Path normalisation: we try `canonicalize` first (handles symlinks and
/// case-folding on Windows). If the file doesn't exist yet (write/create
/// path), fall back to lexical absolute. This is acceptable because the
/// only collision case we care about is "two callers writing the same
/// logical file" — both will produce the same lexical form.
pub async fn acquire(path: &Path) -> OwnedMutexGuard<()> {
    let key = canonical_key(path);
    let mutex = LOCKS
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    mutex.lock_owned().await
}

fn canonical_key(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    // File doesn't exist yet — canonicalise the parent, then re-join the leaf.
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(p) = parent.canonicalize() {
            return p.join(name);
        }
    }
    path.to_path_buf()
}

/// Atomic file write: drop bytes into a sibling temp file, then rename
/// over the target. Rename within the same directory is atomic on Windows
/// (when the target file is not held open by another process) and on POSIX.
///
/// On rename failure the temp file is best-effort removed so we don't leave
/// `.tmp.<uuid>` litter around the project.
pub async fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let tmp_name = format!(
        ".{}.tmp.{}",
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "out".to_string()),
        uuid::Uuid::new_v4().simple()
    );
    let tmp = path.with_file_name(tmp_name);

    let write_res = tokio::fs::write(&tmp, data).await;
    if let Err(e) = write_res {
        // Don't leave a partial temp file behind.
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }

    match with_sharing_retry(|| std::fs::rename(&tmp, path)).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            Err(e)
        }
    }
}

/// Retry a synchronous IO operation up to 4 times with exponential backoff
/// (10ms → 40ms → 160ms → 640ms) when it fails with a Windows sharing
/// violation or a generic `PermissionDenied`. External processes (IDE, AV,
/// indexer) often hold a file for milliseconds — this gives them a chance
/// to release before we give up.
pub async fn with_sharing_retry<F, T>(mut op: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let mut delay_ms = 10u64;
    for attempt in 0..4 {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if is_sharing_violation(&e) && attempt < 3 => {
                tracing::debug!(
                    "file_lock: transient {:?} ({}), retrying in {}ms (attempt {}/3)",
                    e.kind(),
                    e,
                    delay_ms,
                    attempt + 1
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                delay_ms *= 4;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn is_sharing_violation(e: &io::Error) -> bool {
    // Windows ERROR_SHARING_VIOLATION = 32, ERROR_LOCK_VIOLATION = 33
    // Both surface as PermissionDenied via std::io. Also accept the raw code
    // when present (set on Windows via raw_os_error).
    if e.kind() == io::ErrorKind::PermissionDenied {
        return true;
    }
    matches!(e.raw_os_error(), Some(32) | Some(33))
}
