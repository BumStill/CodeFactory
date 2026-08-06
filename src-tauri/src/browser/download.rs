// SPDX-License-Identifier: Apache-2.0
//! Fetching and unpacking the browser.
//!
//! Separate from [`super::install`], which owns the *rules* (where things live,
//! what counts as installed, when to repair). This module only moves bytes, so
//! the rules stay unit-testable and this stays the one place that touches the
//! network and the filesystem in bulk.
//!
//! Two ordering decisions matter and are the reason a half-finished download is
//! recoverable rather than fatal:
//!
//!   1. Extraction goes to a temporary directory and is *renamed* into place
//!      only once it is complete. A kill -9 during extraction leaves a stray
//!      temp directory, never a version directory that looks usable.
//!   2. The version marker is written last, after the binary is confirmed
//!      present. `install::detect` therefore cannot mistake an interrupted
//!      install for a finished one.
//!
//! ## Why the last step is the hard one on Windows
//!
//! Everything up to the final directory rename is ordinary I/O. The rename is
//! not: on Windows a directory move is refused outright while *any* handle is
//! open anywhere inside it, and something usually is — Defender scans a freshly
//! written `chrome.exe` the moment it is closed, Search Indexer walks new
//! folders, Controlled Folder Access can veto writes under the user profile
//! entirely. All of them surface the same way, as `Access is denied (os error
//! 5)` on the very last operation of a 150 MB download, which is exactly the
//! failure this module is built to survive:
//!
//!   * the destination is *proved* writable — including a directory rename —
//!     before a byte is fetched, so a blocked folder costs a message, not a
//!     download;
//!   * the move is retried with backoff, because a scanner's handle is
//!     transient;
//!   * a previous install is moved aside rather than deleted in place, so a
//!     locked file in the *old* tree cannot block the *new* one;
//!   * a rename that stays refused falls back to a plain recursive copy;
//!   * and if a whole folder turns out to be unusable, the archive that is
//!     already on disk is unpacked into the next candidate root instead of
//!     making the user download it again.

use std::io::Write;
use std::time::Duration;
use std::path::{Path, PathBuf};

use crate::errors::{AppError, Result};

use super::install::{self, Platform};

/// Attempts at the final directory move, and the pause between them.
///
/// An anti-virus handle on a new executable lives for a moment, not forever, so
/// a short escalating backoff converts the common Windows failure into a pause.
/// The total (~3 s) is bounded so a genuinely blocked folder still fails fast
/// enough to fall back to another root.
const MOVE_ATTEMPTS: u32 = 6;
const MOVE_BACKOFF: Duration = Duration::from_millis(120);

/// Subdirectory of the install root that holds the in-flight archive.
///
/// The download lands next to its destination rather than in `%TEMP%`: it is a
/// folder we have already proved writable, on the same volume as the target, and
/// large enough by definition — `%TEMP%` can be redirected to a tiny or
/// locked-down location on managed Windows machines.
const SCRATCH_DIR: &str = ".download";

/// Abandon scratch files older than this; anything younger may be another
/// CodeFactory window's download in progress.
const SCRATCH_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Progress while the browser is being fetched, for the UI to render.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Progress {
    Resolving,
    Downloading {
        received_bytes: u64,
        /// `None` when the server sends no length — show a spinner, not a bar.
        total_bytes: Option<u64>,
    },
    Extracting,
    Done {
        version: String,
    },
}

/// HTTP client for the download.
///
/// Deliberately no total timeout: a ~150 MB transfer on a slow link is normal
/// and a wall clock would kill a download that is making fine progress. A
/// genuinely stalled connection surfaces as a stream error instead.
fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| AppError::Other(format!("Could not create an HTTP client: {error}")))
}

/// Ask Chrome for Testing which build to fetch.
async fn resolve(platform: Platform, versions_url: &str) -> Result<(String, String)> {
    let index: serde_json::Value = http()?
        .get(versions_url)
        .send()
        .await
        .map_err(|error| {
            AppError::Other(format!("Could not reach the Chromium version index: {error}"))
        })?
        .json()
        .await
        .map_err(|error| {
            AppError::Other(format!("The Chromium version index was unreadable: {error}"))
        })?;

    install::download_url_from_index(&index, "Stable", platform).ok_or_else(|| {
        AppError::Other(format!(
            "Chrome for Testing publishes no Stable build for {}",
            platform.id()
        ))
    })
}

/// Download and unpack Chromium, reporting progress as it goes.
///
/// Idempotent: if a usable install is already present *in any* candidate root,
/// returns its version without touching the network, so a retry after a partial
/// failure is safe and a repair does not re-download a working browser.
pub async fn ensure_installed(
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<install::ChromiumInstall> {
    ensure_installed_in(
        &install::install_root_candidates(),
        install::VERSIONS_URL,
        on_progress,
    )
    .await
}

/// [`ensure_installed`] against explicit roots and version index.
///
/// The parameters exist so the whole pipeline — resolve, download, unpack, move
/// into place, record — can be exercised end to end against a local server and a
/// temporary directory. This is the path that keeps breaking in ways unit tests
/// on its pieces do not catch, and a 150 MB download from a network CI may not
/// even be able to reach is no way to guard it.
pub(crate) async fn ensure_installed_in(
    candidates: &[PathBuf],
    versions_url: &str,
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<install::ChromiumInstall> {
    let platform = Platform::current().ok_or_else(|| {
        AppError::Other("No Chromium build is published for this platform".into())
    })?;
    let candidates = candidates.to_vec();
    if candidates.is_empty() {
        return Err(AppError::Other(
            "Could not resolve a folder for the browser — no home or app-data directory is available."
                .into(),
        ));
    }

    if let (_, install::InstallState::Ready(found)) = install::detect_in_any(&candidates, platform) {
        on_progress(Progress::Done {
            version: found.version.clone(),
        });
        return Ok(found);
    }

    // Prove the destination before spending the download on it.
    let mut remaining = candidates.clone();
    let root = take_writable_root(&mut remaining)
        .map_err(|attempts| AppError::Other(install::unwritable_message(&attempts)))?;

    on_progress(Progress::Resolving);
    let (version, url) = resolve(platform, versions_url).await?;

    let archive = download(&url, &root, on_progress).await?;

    on_progress(Progress::Extracting);
    let mut root = root;
    loop {
        let archive_path = archive.path().to_path_buf();
        let (target_root, install_version) = (root.clone(), version.clone());
        // Unzipping 150 MB and the retrying move are both blocking work; keeping
        // them off the async worker means progress events and the rest of the UI
        // stay responsive for the ~10 s this takes.
        let outcome = tokio::task::spawn_blocking(move || {
            install_archive(&archive_path, &target_root, &install_version, platform)
        })
        .await
        .map_err(|error| AppError::Other(format!("The install task failed: {error}")))?;

        let failure = match outcome {
            Ok(installed) => {
                on_progress(Progress::Done {
                    version: installed.version.clone(),
                });
                return Ok(installed);
            }
            Err(failure) => failure,
        };

        // A folder that refuses the install is not going to start accepting it.
        // Re-unpack the archive we already have somewhere else rather than making
        // the user download 150 MB a second time. Anything else — a corrupt zip,
        // a missing executable — would fail identically in every folder.
        if !failure.try_another_root {
            return Err(AppError::Other(failure.message));
        }
        match take_writable_root(&mut remaining) {
            Ok(next_root) => {
                tracing::warn!(
                    "browser install: {} — retrying in {}",
                    failure.message,
                    next_root.display()
                );
                root = next_root;
            }
            // Nowhere left to try: report the failure that actually happened, not
            // the fact that we ran out of folders.
            Err(_) => return Err(AppError::Other(failure.message)),
        }
    }
}

/// Take the next candidate root this process can write to, consuming the ones
/// that were tried so a retry never lands on the same folder twice.
fn take_writable_root(remaining: &mut Vec<PathBuf>) -> std::result::Result<PathBuf, Vec<install::RootAttempt>> {
    let chosen = install::first_writable_root(remaining)?;
    if let Some(position) = remaining.iter().position(|root| root == &chosen) {
        remaining.drain(..=position);
    }
    Ok(chosen)
}

/// A step that failed, and whether another folder is worth trying.
#[derive(Debug)]
struct StepError {
    message: String,
    /// True for permission-shaped failures: a different root may well work.
    try_another_root: bool,
}

impl StepError {
    fn other(message: String) -> Self {
        Self {
            message,
            try_another_root: false,
        }
    }

    /// A filesystem failure, classified by whether the folder is the problem.
    fn io(action: &str, path: &Path, error: &std::io::Error) -> Self {
        let blocked = is_permission_shaped(error);
        let message = if blocked {
            format!(
                "{action} {} was refused: {error}. On Windows this is usually anti-virus \
                 (\"controlled folder access\" or a real-time scan holding the new files), a \
                 redirected AppData folder, or a CodeFactory-managed browser still running from \
                 that folder — close it and try again.",
                path.display()
            )
        } else {
            format!("{action} {} failed: {error}", path.display())
        };
        Self {
            message,
            try_another_root: blocked,
        }
    }
}

/// Does this error mean "this folder will not let us", rather than "the data was
/// bad"? Only the former is worth retrying somewhere else.
fn is_permission_shaped(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    match error.raw_os_error() {
        // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION,
        // ERROR_CURRENT_DIRECTORY, ERROR_DIR_NOT_EMPTY, ERROR_ALREADY_EXISTS,
        // ERROR_VIRUS_INFECTED, ERROR_VIRUS_DELETED.
        #[cfg(windows)]
        Some(5 | 32 | 33 | 16 | 145 | 183 | 1920 | 1921) => true,
        // EPERM, EACCES, EROFS.
        #[cfg(not(windows))]
        Some(1 | 13 | 30) => true,
        _ => false,
    }
}

/// Unpack a downloaded archive into `root` and record it as installed.
///
/// Blocking by design — the caller runs it on a blocking thread. Split out from
/// [`ensure_installed`] so the same archive can be retried against a second root
/// without re-downloading, and so every filesystem failure is classified in one
/// place.
fn install_archive(
    archive: &Path,
    root: &Path,
    version: &str,
    platform: Platform,
) -> std::result::Result<install::ChromiumInstall, StepError> {
    sweep_leftovers(root);
    let target = install::version_dir(root, version);
    extract_into_place(archive, &target)?;

    // Confirm before recording. A marker written over a broken extract is the
    // one failure mode that would not self-heal.
    let binary = target.join(platform.binary_relative_path());
    if !binary.is_file() {
        // An extract that produced a tree but no executable on Windows is very
        // often anti-virus quarantining `chrome.exe` between the two steps, so
        // say so instead of blaming the archive layout.
        let unpacked_anything = target.is_dir();
        return Err(StepError {
            message: format!(
                "The download unpacked but {} is missing — {}",
                binary.display(),
                if unpacked_anything {
                    "anti-virus may have quarantined the browser executable, or the archive \
                     layout changed. Add an exclusion for that folder and try again."
                } else {
                    "the archive layout may have changed."
                }
            ),
            try_another_root: unpacked_anything,
        });
    }
    make_executable(&binary);

    install::write_marker(root, version).map_err(|error| {
        StepError::io("Recording the install in", root, &error)
    })?;

    Ok(install::ChromiumInstall {
        version: version.to_string(),
        binary,
    })
}

/// Stream the archive to a temp file so a ~150 MB body never sits in memory.
async fn download(
    url: &str,
    root: &Path,
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<tempfile::NamedTempFile> {
    use futures::StreamExt;

    let response = http()?
        .get(url)
        .send()
        .await
        .map_err(|error| AppError::Other(format!("Chromium download failed to start: {error}")))?;
    if !response.status().is_success() {
        return Err(AppError::Other(format!(
            "Chromium download failed with HTTP {}",
            response.status()
        )));
    }
    let total_bytes = response.content_length();

    let scratch = scratch_dir(root)
        .map_err(|error| AppError::Other(format!("Could not prepare the download folder: {error}")))?;
    let mut file = tempfile::Builder::new()
        .prefix("chromium-")
        .suffix(".zip")
        .tempfile_in(&scratch)
        .map_err(|error| {
            AppError::Other(format!(
                "Could not open a download file in {}: {error}",
                scratch.display()
            ))
        })?;
    let mut received_bytes = 0u64;
    let mut last_reported = 0u64;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| AppError::Other(format!("Chromium download interrupted: {error}")))?;
        file.write_all(&chunk)
            .map_err(|error| AppError::Other(format!("Could not write the download: {error}")))?;
        received_bytes += chunk.len() as u64;
        // Report about every megabyte: often enough for a smooth bar, rarely
        // enough not to flood the event channel.
        if received_bytes - last_reported >= 1_000_000 {
            last_reported = received_bytes;
            on_progress(Progress::Downloading {
                received_bytes,
                total_bytes,
            });
        }
    }
    file.flush()
        .map_err(|error| AppError::Other(format!("Could not finish the download: {error}")))?;
    Ok(file)
}

/// Where the in-flight archive is written, cleared of anything abandoned.
fn scratch_dir(root: &Path) -> std::io::Result<PathBuf> {
    let scratch = root.join(SCRATCH_DIR);
    std::fs::create_dir_all(&scratch)?;
    // A killed process leaves a ~150 MB file behind. Sweeping only what is old
    // keeps a second CodeFactory window's in-flight download intact.
    if let Ok(entries) = std::fs::read_dir(&scratch) {
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .map(|modified| {
                    modified
                        .elapsed()
                        .map(|age| age > SCRATCH_TTL)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(scratch)
}

/// Prefixes of directories this module creates and does not always get to delete.
const LEFTOVER_PREFIXES: &[&str] = &[".retired-", ".staging-"];

/// Delete abandoned staging and retired-install directories.
///
/// Each is a whole browser — around half a gigabyte — and on a machine where
/// deletion keeps failing (the case [`retire`] deliberately tolerates) they would
/// otherwise pile up one per install. Best effort: a leftover that still cannot be
/// deleted is left for the next attempt rather than failing an install over it.
fn sweep_leftovers(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !LEFTOVER_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            let _ = crate::util::fs_cleanup::remove_dir_all_blocking(&entry.path());
        }
    }
}

/// Unpack into a staging directory, then move it into place.
fn extract_into_place(archive: &Path, target: &Path) -> std::result::Result<(), StepError> {
    let parent = target.parent().unwrap_or(target);
    std::fs::create_dir_all(parent).map_err(|error| StepError::io("Creating", parent, &error))?;

    // Staging sits beside the target so the move stays on one filesystem.
    let staging = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(parent)
        .map_err(|error| StepError::io("Staging the extract in", parent, &error))?;

    let file = std::fs::File::open(archive)
        .map_err(|error| StepError::io("Reading the download", archive, &error))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|error| {
        StepError::other(format!("The download is not a valid zip: {error}"))
    })?;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| StepError::other(format!("Corrupt archive entry: {error}")))?;
        // `enclosed_name` rejects absolute paths and `..`, so a crafted archive
        // cannot write outside the staging directory.
        let Some(relative) = entry.enclosed_name() else {
            return Err(StepError::other(
                "The archive contains an unsafe path and was not extracted".into(),
            ));
        };
        let out = staging.path().join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|error| StepError::io("Creating", &out, &error))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| StepError::io("Creating", parent, &error))?;
        }
        let mut sink = std::fs::File::create(&out)
            .map_err(|error| StepError::io("Writing", &out, &error))?;
        std::io::copy(&mut entry, &mut sink)
            .map_err(|error| StepError::io("Unpacking", &out, &error))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }

    move_into_place(staging.path(), target)?;
    // The TempDir no longer owns the path; stop it from trying to clean up.
    std::mem::forget(staging);
    Ok(())
}

/// Put a finished staging tree at `target`, replacing whatever is there.
///
/// The single most failure-prone operation in the installer on Windows; see the
/// module docs. Ordered so that each fallback is strictly weaker than the last
/// and none of them can leave a directory that `install::detect` would mistake
/// for a working install.
fn move_into_place(staging: &Path, target: &Path) -> std::result::Result<(), StepError> {
    if target.exists() {
        retire(target)?;
    }

    match rename_with_retries(staging, target) {
        Ok(()) => return Ok(()),
        Err(error) if !is_permission_shaped(&error) => {
            return Err(StepError::io("Moving the browser into", target, &error));
        }
        Err(error) => {
            // Renaming stayed refused for the whole backoff. A plain copy uses
            // ordinary file writes, which a scanner's handle on the *source*
            // does not block, and is worth the extra seconds to avoid failing a
            // download that is otherwise complete.
            tracing::warn!(
                "browser install: renaming into {} was refused ({error}); copying instead",
                target.display()
            );
            if let Err(copy_error) = copy_tree(staging, target) {
                // Leave nothing that looks installed behind.
                let _ = crate::util::fs_cleanup::remove_dir_all_blocking(target);
                return Err(StepError::io("Copying the browser into", target, &copy_error));
            }
            let _ = crate::util::fs_cleanup::remove_dir_all_blocking(staging);
            Ok(())
        }
    }
}

/// Get an existing install out of the way.
///
/// Renaming it aside first is what makes this survivable: a locked file in the
/// *old* tree (a browser still running from it, a scanner mid-scan) would block
/// deletion, but a directory rename inside the same folder usually still works,
/// and once it is out of the path the leftover can be deleted later — or never,
/// without breaking anything.
fn retire(target: &Path) -> std::result::Result<(), StepError> {
    let parent = target.parent().unwrap_or(target);
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "install".into());
    let aside = parent.join(format!(
        ".retired-{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default()
    ));

    if std::fs::rename(target, &aside).is_ok() {
        // Best effort: a leftover directory is harmless, a failed install is not.
        if crate::util::fs_cleanup::remove_dir_all_blocking(&aside).is_err() {
            tracing::warn!(
                "browser install: could not delete the previous install at {}; it can be removed by hand",
                aside.display()
            );
        }
        return Ok(());
    }

    crate::util::fs_cleanup::remove_dir_all_blocking(target).map_err(|error| {
        StepError::io("Replacing the previous browser in", target, &error)
    })
}

/// Rename, retrying while the failure still looks transient.
fn rename_with_retries(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut delay = MOVE_BACKOFF;
    let mut last = None;
    for attempt in 1..=MOVE_ATTEMPTS {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let transient = is_permission_shaped(&error);
                last = Some(error);
                if !transient || attempt == MOVE_ATTEMPTS {
                    break;
                }
                std::thread::sleep(delay);
                delay *= 2;
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename did not run")))
}

/// Recursive copy, used only when renaming is refused outright.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source, &destination)?;
        } else {
            std::fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

/// Zip archives do not always carry the execute bit on every platform.
fn make_executable(binary: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(binary) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o755);
            let _ = std::fs::set_permissions(binary, permissions);
        }
    }
    #[cfg(not(unix))]
    let _ = binary;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a zip in memory so extraction is tested without the network.
    fn zip_with(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            for (name, body) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(body).unwrap();
            }
            writer.finish().unwrap();
        }
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&buffer.into_inner()).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn an_archive_lands_as_a_complete_directory() {
        let archive = zip_with(&[
            ("chrome-linux64/chrome", b"#!/bin/sh\n"),
            ("chrome-linux64/resources/x.pak", b"data"),
        ]);
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("151.0.0.1");

        extract_into_place(archive.path(), &target).unwrap();

        assert!(target.join("chrome-linux64/chrome").is_file());
        assert!(target.join("chrome-linux64/resources/x.pak").is_file());
    }

    #[test]
    fn an_archive_cannot_write_outside_the_target() {
        // A crafted archive must not be able to drop a file anywhere it likes.
        let archive = zip_with(&[("../../escaped.sh", b"pwned")]);
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("151.0.0.1");

        let result = extract_into_place(archive.path(), &target);

        assert!(result.is_err(), "unsafe path must be refused");
        assert!(!root.path().join("escaped.sh").exists());
        assert!(!target.exists(), "nothing is moved into place on refusal");
    }

    #[test]
    fn a_previous_partial_attempt_is_replaced_rather_than_merged() {
        // Left-over files from a failed extract must not survive into the new
        // install, where they would be mistaken for part of a good download.
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("151.0.0.1");
        std::fs::create_dir_all(target.join("chrome-linux64")).unwrap();
        std::fs::write(target.join("chrome-linux64/stale"), b"old").unwrap();

        let archive = zip_with(&[("chrome-linux64/chrome", b"new")]);
        extract_into_place(archive.path(), &target).unwrap();

        assert!(target.join("chrome-linux64/chrome").is_file());
        assert!(!target.join("chrome-linux64/stale").exists());
    }

    #[test]
    fn extraction_failure_leaves_no_directory_that_looks_installed() {
        // The ordering guarantee: detect() must not find a usable install after
        // a failed extract, so the next attempt re-downloads instead of trying
        // to launch a browser that is not there.
        let archive = zip_with(&[("../escape", b"x")]);
        let root = tempfile::tempdir().unwrap();
        let target = install::version_dir(root.path(), "151.0.0.1");

        let _ = extract_into_place(archive.path(), &target);

        assert_eq!(
            install::detect(root.path(), Platform::Linux64),
            install::InstallState::Missing { previous: None }
        );
    }

    #[test]
    fn a_previous_install_is_moved_aside_rather_than_deleted_in_place() {
        // The Windows failure this protects against: a file in the *old* tree is
        // locked, so deleting it in place fails and would take the new install
        // down with it. Renaming it out of the path first has to be what happens.
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("151.0.0.1");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old"), b"previous").unwrap();

        retire(&target).expect("the old install is retired");

        assert!(!target.exists(), "the path must be clear for the new install");
    }

    #[test]
    fn a_copy_is_used_when_the_move_cannot_be_done_by_rename() {
        // Exercised directly because the condition that triggers it (a scanner
        // holding a handle) cannot be created in a test. What must hold is that
        // the fallback produces the same tree.
        let base = tempfile::tempdir().unwrap();
        let staging = base.path().join("staging");
        std::fs::create_dir_all(staging.join("chrome-win64/nested")).unwrap();
        std::fs::write(staging.join("chrome-win64/chrome.exe"), b"MZ").unwrap();
        std::fs::write(staging.join("chrome-win64/nested/x.pak"), b"data").unwrap();
        let target = base.path().join("151.0.0.1");

        copy_tree(&staging, &target).expect("copy");

        assert_eq!(
            std::fs::read(target.join("chrome-win64/chrome.exe")).unwrap(),
            b"MZ"
        );
        assert!(target.join("chrome-win64/nested/x.pak").is_file());
    }

    #[test]
    fn a_refused_folder_is_marked_for_retry_elsewhere_and_says_what_to_do() {
        // Classification is the whole mechanism: a permission failure must be
        // retried in another folder, a corrupt download must not be.
        let refused = StepError::io(
            "Moving the browser into",
            Path::new("C:\\Users\\Ada\\AppData\\Local\\CodeFactory"),
            &std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Access is denied"),
        );
        assert!(refused.try_another_root);
        assert!(refused.message.contains("anti-virus"));
        assert!(refused.message.contains("CodeFactory"));

        let corrupt = StepError::other("The download is not a valid zip".into());
        assert!(!corrupt.try_another_root);

        let missing = StepError::io(
            "Reading the download",
            Path::new("/tmp/x.zip"),
            &std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        );
        assert!(!missing.try_another_root, "a missing file is not a folder problem");
    }

    #[test]
    fn every_candidate_root_is_tried_at_most_once() {
        // Without draining, a permission failure would retry the same folder
        // forever instead of moving on.
        let base = tempfile::tempdir().unwrap();
        let first = base.path().join("first");
        let second = base.path().join("second");
        let mut remaining = vec![first.clone(), second.clone()];

        assert_eq!(take_writable_root(&mut remaining).unwrap(), first);
        assert_eq!(take_writable_root(&mut remaining).unwrap(), second);
        assert!(
            take_writable_root(&mut remaining).is_err(),
            "nothing left to try must be an error, not a repeat"
        );
    }

    #[test]
    fn the_archive_is_written_beside_its_destination_not_in_the_system_temp() {
        // %TEMP% on a managed Windows machine can be redirected somewhere small
        // or locked down; the install root has already been proved writable.
        let root = tempfile::tempdir().unwrap();
        let scratch = scratch_dir(root.path()).expect("scratch");
        assert!(scratch.starts_with(root.path()));
        assert!(scratch.is_dir());
    }

    #[test]
    fn an_abandoned_download_is_swept_but_a_fresh_one_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let scratch = scratch_dir(root.path()).expect("scratch");
        let fresh = scratch.join("chromium-live.zip");
        std::fs::write(&fresh, b"in flight").unwrap();
        let stale = scratch.join("chromium-abandoned.zip");
        std::fs::write(&stale, b"leftover").unwrap();
        let long_ago = std::time::SystemTime::now() - (SCRATCH_TTL + Duration::from_secs(60));
        std::fs::File::open(&stale)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        scratch_dir(root.path()).expect("scratch again");

        assert!(fresh.is_file(), "another window's download must survive");
        assert!(!stale.exists(), "a 150 MB leftover must not accumulate");
    }

    #[test]
    fn abandoned_half_gigabyte_leftovers_do_not_accumulate() {
        // `retire` tolerates a previous install it cannot delete, which is right —
        // but each leftover is a whole browser, so the next install has to clear
        // them or a machine with a locking scanner slowly fills up.
        let root = tempfile::tempdir().unwrap();
        let retired = root.path().join(".retired-151.0.0.1-1700000000000");
        let staging = root.path().join(".staging-abc123");
        std::fs::create_dir_all(retired.join("chrome-linux64")).unwrap();
        std::fs::write(retired.join("chrome-linux64/chrome"), b"old browser").unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        // A real install directory must survive the sweep.
        let keep = install::version_dir(root.path(), "151.0.0.1");
        std::fs::create_dir_all(&keep).unwrap();

        let archive = zip_with(&[("chrome-linux64/chrome", b"#!/bin/sh\n")]);
        install_archive(archive.path(), root.path(), "151.0.0.2", Platform::Linux64).unwrap();

        assert!(!retired.exists(), "a retired install must be swept");
        assert!(!staging.exists(), "an abandoned staging dir must be swept");
        assert!(keep.is_dir(), "an installed version must not be swept");
    }

    #[test]
    fn an_install_records_itself_only_after_the_binary_is_there() {
        let root = tempfile::tempdir().unwrap();
        let archive = zip_with(&[("chrome-linux64/chrome", b"#!/bin/sh\n")]);

        let installed =
            install_archive(archive.path(), root.path(), "151.0.0.1", Platform::Linux64).unwrap();

        assert_eq!(installed.version, "151.0.0.1");
        assert!(installed.binary.is_file());
        assert_eq!(
            install::detect(root.path(), Platform::Linux64),
            install::InstallState::Ready(installed)
        );
    }

    #[test]
    fn an_archive_without_the_executable_is_not_recorded_as_installed() {
        // Anti-virus deleting chrome.exe between extract and check is a real
        // Windows outcome; it must not leave a marker claiming success.
        let root = tempfile::tempdir().unwrap();
        let archive = zip_with(&[("chrome-linux64/resources/x.pak", b"data")]);

        let failure =
            install_archive(archive.path(), root.path(), "151.0.0.1", Platform::Linux64)
                .expect_err("no binary");

        assert!(failure.message.contains("anti-virus"));
        assert_eq!(
            install::detect(root.path(), Platform::Linux64),
            install::InstallState::Missing { previous: None }
        );
    }

    #[test]
    fn progress_serialises_with_a_stage_the_ui_can_switch_on() {
        let json = serde_json::to_string(&Progress::Downloading {
            received_bytes: 1_000_000,
            total_bytes: Some(150_000_000),
        })
        .unwrap();
        assert!(json.contains("\"stage\":\"downloading\""));
        assert!(json.contains("1000000"));

        // No total means the UI shows a spinner instead of a bar.
        let json = serde_json::to_string(&Progress::Downloading {
            received_bytes: 5,
            total_bytes: None,
        })
        .unwrap();
        assert!(json.contains("\"total_bytes\":null"));
    }
}

/// The whole install, end to end, without the network.
///
/// Stands up a local server that answers like Chrome for Testing and serves an
/// archive shaped like a real one, then runs the same code path the app runs.
/// Everything the pieces cannot cover in isolation is what this is for: that the
/// index is parsed into a real download, that the archive lands beside its
/// destination, that a finished tree is moved into place, and that the marker is
/// written only afterwards so [`install::detect`] agrees the browser is there.
#[cfg(test)]
mod end_to_end {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Build a zip laid out like the platform's real Chrome for Testing archive.
    fn platform_archive(platform: Platform) -> Vec<u8> {
        let binary = platform
            .binary_relative_path()
            .to_string_lossy()
            .replace('\\', "/");
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            writer.start_file(&binary, options).unwrap();
            writer.write_all(b"#!/bin/sh\necho Chrome 151.0.7922.47\n").unwrap();
            // A second, nested entry so the move handles a tree, not one file.
            writer
                .start_file(
                    format!(
                        "chrome-{}/locales/en-GB.pak",
                        platform.id()
                    ),
                    options,
                )
                .unwrap();
            writer.write_all(b"pak").unwrap();
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    /// A stand-in for Chrome for Testing: the version index and the archive.
    ///
    /// Written against a raw TCP listener rather than adding an HTTP test server
    /// dependency; two fixed routes need no framework.
    async fn serve(platform: Platform, archive: Vec<u8>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requested: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requested);

        let index = serde_json::json!({
            "channels": {
                "Stable": {
                    "version": "151.0.7922.47",
                    "downloads": {
                        "chrome": [{
                            "platform": platform.id(),
                            "url": format!("http://127.0.0.1:{port}/chrome.zip"),
                        }],
                    },
                },
            },
        })
        .to_string();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { break };
                let index = index.clone();
                let archive = archive.clone();
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut head = [0u8; 1024];
                    let read = stream.read(&mut head).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&head[..read]).to_string();
                    let path = request
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .to_string();
                    seen.lock().unwrap().push(path.clone());

                    let (content_type, body) = if path.ends_with(".zip") {
                        ("application/zip", archive)
                    } else {
                        ("application/json", index.into_bytes())
                    };
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                    let _ = stream.flush().await;
                });
            }
        });

        (format!("http://127.0.0.1:{port}/versions.json"), requested)
    }

    #[tokio::test]
    async fn a_download_becomes_an_install_the_app_can_find() {
        let platform = Platform::current().expect("a published platform");
        let (versions_url, _) = serve(platform, platform_archive(platform)).await;
        let root = tempfile::tempdir().unwrap();
        let stages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&stages);

        let installed = ensure_installed_in(
            &[root.path().to_path_buf()],
            &versions_url,
            &move |progress| {
                seen.lock().unwrap().push(match progress {
                    Progress::Resolving => "resolving".into(),
                    Progress::Downloading { .. } => "downloading".into(),
                    Progress::Extracting => "extracting".into(),
                    Progress::Done { version } => format!("done {version}"),
                });
            },
        )
        .await
        .expect("install");

        assert_eq!(installed.version, "151.0.7922.47");
        assert!(installed.binary.is_file(), "{:?} is missing", installed.binary);
        assert!(installed.binary.starts_with(root.path()));
        // The marker is what makes the install visible to every other entry point.
        assert_eq!(
            install::detect(root.path(), platform),
            install::InstallState::Ready(installed)
        );
        // Nested entries survive the move into place.
        assert!(install::version_dir(root.path(), "151.0.7922.47")
            .join(format!("chrome-{}/locales/en-GB.pak", platform.id()))
            .is_file());

        let stages = stages.lock().unwrap().clone();
        assert_eq!(stages.first().map(String::as_str), Some("resolving"));
        assert_eq!(
            stages.last().map(String::as_str),
            Some("done 151.0.7922.47"),
            "the UI must be told the install finished: {stages:?}"
        );
    }

    #[tokio::test]
    async fn a_second_run_reuses_the_install_instead_of_downloading_again() {
        let platform = Platform::current().expect("a published platform");
        let (versions_url, requested) = serve(platform, platform_archive(platform)).await;
        let root = tempfile::tempdir().unwrap();
        let roots = [root.path().to_path_buf()];

        ensure_installed_in(&roots, &versions_url, &|_| {}).await.expect("first install");
        let requests_after_first = requested.lock().unwrap().len();
        ensure_installed_in(&roots, &versions_url, &|_| {}).await.expect("second call");

        assert_eq!(
            requested.lock().unwrap().len(),
            requests_after_first,
            "an existing install must not be re-downloaded"
        );
    }

    #[tokio::test]
    async fn an_unusable_first_folder_does_not_cost_the_user_a_download() {
        // The Windows shape of this: %LOCALAPPDATA% is blocked, so the install has
        // to happen somewhere else — and it must not take a second 150 MB transfer
        // to get there.
        let platform = Platform::current().expect("a published platform");
        let (versions_url, requested) = serve(platform, platform_archive(platform)).await;
        let base = tempfile::tempdir().unwrap();
        let blocker = base.path().join("blocked");
        std::fs::write(&blocker, b"a file where a folder must go").unwrap();
        let fallback = base.path().join("fallback");

        let installed = ensure_installed_in(
            &[blocker.join("chromium"), fallback.clone()],
            &versions_url,
            &|_| {},
        )
        .await
        .expect("install lands in the fallback root");

        assert!(installed.binary.starts_with(&fallback));
        assert_eq!(
            requested.lock().unwrap().iter().filter(|path| path.ends_with(".zip")).count(),
            1,
            "the archive must be fetched exactly once"
        );
    }

    #[tokio::test]
    async fn an_interrupted_install_is_replaced_rather_than_merged() {
        // Left-over files from a previous attempt must not survive into the new
        // install, where they would look like part of a good download.
        let platform = Platform::current().expect("a published platform");
        let (versions_url, _) = serve(platform, platform_archive(platform)).await;
        let root = tempfile::tempdir().unwrap();
        let target = install::version_dir(root.path(), "151.0.7922.47");
        std::fs::create_dir_all(target.join(format!("chrome-{}", platform.id()))).unwrap();
        std::fs::write(
            target.join(format!("chrome-{}/stale.pak", platform.id())),
            b"from a failed attempt",
        )
        .unwrap();

        let installed = ensure_installed_in(&[root.path().to_path_buf()], &versions_url, &|_| {})
            .await
            .expect("install");

        assert!(installed.binary.is_file());
        assert!(
            !target.join(format!("chrome-{}/stale.pak", platform.id())).exists(),
            "the previous attempt must not be merged into the new install"
        );
    }
}

/// Live check: really download Chromium and confirm the binary runs.
///
/// Ignored by default — it pulls ~150 MB. Run it explicitly when changing the
/// installer:
///
///   cargo test --lib browser::download::live -- --ignored --nocapture
#[cfg(test)]
mod live {
    #[tokio::test]
    #[ignore = "downloads ~150 MB"]
    async fn chromium_downloads_and_reports_its_version() {
        let install = super::ensure_installed(&|progress| {
            if let super::Progress::Downloading { received_bytes, .. } = progress {
                if received_bytes % 50_000_000 < 1_000_000 {
                    eprintln!("… {} MB", received_bytes / 1_000_000);
                }
            }
        })
        .await
        .expect("download");

        eprintln!("installed {} at {}", install.version, install.binary.display());
        assert!(install.binary.is_file());

        // The real proof is that the binary executes. `.no_window()` so this
        // matches the rule the rest of the codebase is held to — a probe that
        // flashes a console window on Windows is a bug wherever it lives.
        use crate::util::no_window::NoWindow;
        let output = std::process::Command::new(&install.binary)
            .no_window()
            .arg("--version")
            .output()
            .expect("run chromium");
        let reported = String::from_utf8_lossy(&output.stdout);
        eprintln!("--version says: {}", reported.trim());
        assert!(
            reported.contains("Chrome") || reported.contains("Chromium"),
            "unexpected --version output: {reported}"
        );
    }
}
