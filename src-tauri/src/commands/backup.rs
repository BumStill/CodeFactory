// SPDX-License-Identifier: Apache-2.0
//! User data export / import.
//!
//! Bundles the SQLite DB + settings.json into a single `.zip` the user can
//! stash for backup or move to a new machine. API keys are NOT exported —
//! they live in the Windows Credential Manager, which is per-user/per-machine
//! anyway. Re-entering keys after restore is the safer default.

use std::io::{Read, Write};
use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::errors::{AppError, Result};

#[derive(Debug, Serialize)]
pub struct ExportResult {
    pub path: String,
    pub size_bytes: u64,
}

/// Pack settings.json + codefactory.db into a zip at the user-chosen path.
/// Returns the path and byte size for the UI to show.
#[tauri::command]
pub async fn export_user_data(app: AppHandle, target_path: String) -> Result<ExportResult> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Other(format!("no app data dir: {e}")))?;

    let settings_path = data_dir.join("settings.json");
    let db_path = data_dir.join("codefactory.db");

    // tokio::task::spawn_blocking because zip's compression is CPU-bound and
    // we don't want to block the tauri event loop.
    let target = std::path::PathBuf::from(&target_path);
    let bytes = tokio::task::spawn_blocking(move || -> Result<u64> {
        let file = std::fs::File::create(&target)?;
        write_zip(file, &settings_path, &db_path)?;
        Ok(std::fs::metadata(&target)?.len())
    })
    .await
    .map_err(|e| AppError::Other(format!("export task panicked: {e}")))??;

    Ok(ExportResult {
        path: target_path,
        size_bytes: bytes,
    })
}

fn write_zip(
    file: std::fs::File,
    settings_path: &Path,
    db_path: &Path,
) -> std::io::Result<()> {
    // Use STORED (no compression) — the SQLite DB is already compact and
    // settings.json is tiny; compression adds bytes for small files plus
    // it spares us a heavy dep.
    let mut tar_bytes = Vec::new();
    {
        let mut w = std::io::BufWriter::new(&mut tar_bytes);

        // Minimal custom format: 4-byte name length, name, 8-byte data length, data.
        // Cross-platform, no external crate.
        if let Ok(mut f) = std::fs::File::open(settings_path) {
            let mut data = Vec::new();
            f.read_to_end(&mut data)?;
            write_entry(&mut w, "settings.json", &data)?;
        }
        if let Ok(mut f) = std::fs::File::open(db_path) {
            let mut data = Vec::new();
            f.read_to_end(&mut data)?;
            write_entry(&mut w, "codefactory.db", &data)?;
        }
        w.flush()?;
    }
    let mut out = std::io::BufWriter::new(file);
    out.write_all(b"CFBKP01")?; // magic + version
    out.write_all(&tar_bytes)?;
    out.flush()?;
    Ok(())
}

fn write_entry(w: &mut impl Write, name: &str, data: &[u8]) -> std::io::Result<()> {
    let name_bytes = name.as_bytes();
    w.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
    w.write_all(name_bytes)?;
    w.write_all(&(data.len() as u64).to_le_bytes())?;
    w.write_all(data)?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub restored_settings: bool,
    pub restored_db: bool,
}

/// Restore from a previously-exported `.cfbkp` file. Overwrites settings.json
/// and codefactory.db in-place. Caller (UI) should warn the user that this
/// replaces current data and recommend restarting the app immediately.
#[tauri::command]
pub async fn import_user_data(app: AppHandle, source_path: String) -> Result<ImportResult> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Other(format!("no app data dir: {e}")))?;

    let source = std::path::PathBuf::from(&source_path);
    tokio::task::spawn_blocking(move || -> Result<ImportResult> {
        let mut bytes = Vec::new();
        std::fs::File::open(&source)?.read_to_end(&mut bytes)?;

        if !bytes.starts_with(b"CFBKP01") {
            return Err(AppError::Other(
                "Not a CodeFactory backup file (bad magic)".into(),
            ));
        }
        let mut cur = &bytes[7..];

        let mut restored_settings = false;
        let mut restored_db = false;

        while !cur.is_empty() {
            if cur.len() < 4 {
                break;
            }
            let name_len =
                u32::from_le_bytes(cur[..4].try_into().unwrap()) as usize;
            cur = &cur[4..];
            if cur.len() < name_len + 8 {
                return Err(AppError::Other("Corrupt backup file (truncated)".into()));
            }
            let name = std::str::from_utf8(&cur[..name_len])
                .map_err(|e| AppError::Other(format!("Bad entry name: {e}")))?
                .to_string();
            cur = &cur[name_len..];
            let data_len =
                u64::from_le_bytes(cur[..8].try_into().unwrap()) as usize;
            cur = &cur[8..];
            if cur.len() < data_len {
                return Err(AppError::Other("Corrupt backup file (data short)".into()));
            }
            let data = &cur[..data_len];
            cur = &cur[data_len..];

            let target = data_dir.join(&name);
            // Pre-restore safety: rename existing as .pre-restore-<timestamp>
            // so the user can recover even if they imported the wrong file.
            if target.exists() {
                let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let safety = data_dir.join(format!("{name}.pre-restore-{ts}"));
                let _ = std::fs::rename(&target, &safety);
            }
            std::fs::write(&target, data)?;

            match name.as_str() {
                "settings.json" => restored_settings = true,
                "codefactory.db" => restored_db = true,
                _ => {} // ignore unknown entries (forward compat)
            }
        }

        Ok(ImportResult {
            restored_settings,
            restored_db,
        })
    })
    .await
    .map_err(|e| AppError::Other(format!("import task panicked: {e}")))?
}

/// Return the absolute path of the app data directory so the Settings page
/// can show users where their data lives (and link out to Explorer).
#[tauri::command]
pub fn get_data_dir(app: AppHandle) -> Result<String> {
    let p = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Other(format!("no app data dir: {e}")))?;
    Ok(p.display().to_string())
}
