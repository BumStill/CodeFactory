// SPDX-License-Identifier: Apache-2.0
//! OS credential store backed secrets.
//!
//! Older CodeFactory builds wrote API keys to `CodeFactory/keys.json`.
//! That file is now treated as a legacy import source only: reads migrate
//! matching entries into the OS credential store and then remove the
//! plaintext legacy entry.

use std::collections::HashMap;
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "com.codefactory.app";

fn legacy_keys_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodeFactory")
        .join("keys.json")
}

fn load_legacy_map() -> HashMap<String, String> {
    let path = legacy_keys_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_legacy_map(map: &HashMap<String, String>) -> crate::errors::Result<()> {
    let path = legacy_keys_path();
    if map.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_string_pretty(map)?)?;
    Ok(())
}

fn remove_legacy_key(account: &str) -> crate::errors::Result<()> {
    let mut map = load_legacy_map();
    if map.remove(account).is_some() {
        save_legacy_map(&map)?;
    }
    Ok(())
}

fn entry(account: &str) -> crate::errors::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| {
        crate::errors::AppError::Other(format!(
            "Could not open OS credential entry for key_ref '{}': {}",
            account, e
        ))
    })
}

fn get_os_key(account: &str) -> crate::errors::Result<Option<String>> {
    match entry(account)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(crate::errors::AppError::Other(format!(
            "Could not read OS credential for key_ref '{}': {}",
            account, e
        ))),
    }
}

fn set_os_key(account: &str, value: &str) -> crate::errors::Result<()> {
    entry(account)?.set_password(value).map_err(|e| {
        crate::errors::AppError::Other(format!(
            "Could not save OS credential for key_ref '{}': {}",
            account, e
        ))
    })
}

fn delete_os_key(account: &str) -> crate::errors::Result<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(crate::errors::AppError::Other(format!(
            "Could not delete OS credential for key_ref '{}': {}",
            account, e
        ))),
    }
}

pub fn get_key(account: &str) -> crate::errors::Result<Option<String>> {
    if let Some(value) = get_os_key(account)? {
        return Ok(Some(value));
    }

    let Some(legacy_value) = load_legacy_map().get(account).cloned() else {
        return Ok(None);
    };

    set_key(account, &legacy_value)?;
    Ok(Some(legacy_value))
}

pub fn set_key(account: &str, value: &str) -> crate::errors::Result<()> {
    set_os_key(account, value)?;
    let stored = get_os_key(account)?.ok_or_else(|| {
        crate::errors::AppError::Other(format!(
            "OS credential write for key_ref '{}' did not persist a readable value",
            account
        ))
    })?;
    if stored != value {
        return Err(crate::errors::AppError::Other(format!(
            "OS credential read-back mismatch for key_ref '{}'",
            account
        )));
    }
    remove_legacy_key(account)?;
    Ok(())
}

pub fn delete_key(account: &str) -> crate::errors::Result<()> {
    delete_os_key(account)?;
    remove_legacy_key(account)?;
    Ok(())
}
