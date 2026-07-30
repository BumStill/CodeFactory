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

use std::io::Write;
use std::time::Duration;
use std::path::{Path, PathBuf};

use crate::errors::{AppError, Result};

use super::install::{self, Platform};

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
async fn resolve(platform: Platform) -> Result<(String, String)> {
    let index: serde_json::Value = http()?
        .get(install::VERSIONS_URL)
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
/// Idempotent: if a usable install is already present, returns its version
/// without touching the network, so a retry after a partial failure is safe and
/// a repair does not re-download a working browser.
pub async fn ensure_installed(
    on_progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<install::ChromiumInstall> {
    let platform = Platform::current().ok_or_else(|| {
        AppError::Other("No Chromium build is published for this platform".into())
    })?;
    let root = install::install_root()
        .ok_or_else(|| AppError::Other("Could not resolve the home directory".into()))?;

    if let install::InstallState::Ready(found) = install::detect(&root, platform) {
        on_progress(Progress::Done {
            version: found.version.clone(),
        });
        return Ok(found);
    }

    on_progress(Progress::Resolving);
    let (version, url) = resolve(platform).await?;

    let archive = download(&url, on_progress).await?;

    on_progress(Progress::Extracting);
    let target = install::version_dir(&root, &version);
    extract_into_place(archive.path(), &target)?;

    // Confirm before recording. A marker written over a broken extract is the
    // one failure mode that would not self-heal.
    let binary = target.join(platform.binary_relative_path());
    if !binary.is_file() {
        return Err(AppError::Other(format!(
            "The download unpacked but {} is missing — the archive layout may have changed.",
            binary.display()
        )));
    }
    make_executable(&binary);

    install::write_marker(&root, &version)
        .map_err(|error| AppError::Other(format!("Could not record the install: {error}")))?;

    on_progress(Progress::Done {
        version: version.clone(),
    });
    Ok(install::ChromiumInstall { version, binary })
}

/// Stream the archive to a temp file so a ~150 MB body never sits in memory.
async fn download(
    url: &str,
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

    let mut file = tempfile::NamedTempFile::new()
        .map_err(|error| AppError::Other(format!("Could not open a temporary file: {error}")))?;
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

/// Unpack into a temp directory, then rename it into place atomically.
fn extract_into_place(archive: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| AppError::Other(format!("Could not create {parent:?}: {error}")))?;
    }
    // Staging sits beside the target so the rename stays on one filesystem.
    let staging = tempfile::tempdir_in(target.parent().unwrap_or(target))
        .map_err(|error| AppError::Other(format!("Could not stage the extract: {error}")))?;

    let file = std::fs::File::open(archive)
        .map_err(|error| AppError::Other(format!("Could not read the download: {error}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| AppError::Other(format!("The download is not a valid zip: {error}")))?;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| AppError::Other(format!("Corrupt archive entry: {error}")))?;
        // `enclosed_name` rejects absolute paths and `..`, so a crafted archive
        // cannot write outside the staging directory.
        let Some(relative) = entry.enclosed_name() else {
            return Err(AppError::Other(
                "The archive contains an unsafe path and was not extracted".into(),
            ));
        };
        let out = staging.path().join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|error| AppError::Other(format!("Could not create {out:?}: {error}")))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AppError::Other(format!("Could not create {parent:?}: {error}")))?;
        }
        let mut sink = std::fs::File::create(&out)
            .map_err(|error| AppError::Other(format!("Could not write {out:?}: {error}")))?;
        std::io::copy(&mut entry, &mut sink)
            .map_err(|error| AppError::Other(format!("Could not unpack {out:?}: {error}")))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }

    // Replace any previous partial attempt, then move the finished tree in.
    let _ = std::fs::remove_dir_all(target);
    std::fs::rename(staging.path(), target).map_err(|error| {
        AppError::Other(format!("Could not move the browser into place: {error}"))
    })?;
    // The TempDir no longer owns the path; stop it from trying to clean up.
    std::mem::forget(staging);
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
