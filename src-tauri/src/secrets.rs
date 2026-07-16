// SPDX-License-Identifier: Apache-2.0
//! OS credential store backed secrets.
//!
//! Older CodeFactory builds wrote API keys to `CodeFactory/keys.json`.
//! That file is now treated as a legacy import source only: reads migrate
//! matching entries into the OS credential store and then remove the
//! plaintext legacy entry.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use once_cell::sync::Lazy;

#[cfg(not(debug_assertions))]
const KEYRING_SERVICE: &str = "com.codefactory.app";
#[cfg(debug_assertions)]
const KEYRING_SERVICE: &str = "com.codefactory.app.dev";

static FALLBACK_FILE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn secret_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(if cfg!(debug_assertions) {
            "CodeFactoryDev"
        } else {
            "CodeFactory"
        })
}

fn legacy_keys_path() -> PathBuf {
    secret_config_dir().join("keys.json")
}

fn fallback_keys_path() -> PathBuf {
    secret_config_dir().join("credentials-fallback.json")
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

fn load_map(path: &std::path::Path) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

fn save_map(path: &std::path::Path, map: &HashMap<String, String>) -> crate::errors::Result<()> {
    if map.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    }

    std::fs::create_dir_all(path.parent().unwrap())?;
    let parent = path.parent().unwrap();
    let mut temporary = tempfile::Builder::new()
        .prefix(".credentials-fallback.json.tmp.")
        .tempfile_in(parent)?;
    temporary.write_all(&serde_json::to_vec(map)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn select_stored_key(
    os_result: crate::errors::Result<Option<String>>,
    recovery_copy: Option<String>,
    prefer_recovery: bool,
) -> crate::errors::Result<Option<String>> {
    if prefer_recovery && recovery_copy.is_some() {
        return Ok(recovery_copy);
    }
    match os_result {
        Ok(value) => Ok(value.or(recovery_copy)),
        Err(_) if recovery_copy.is_some() => Ok(recovery_copy),
        Err(error) => Err(error),
    }
}

fn fallback_key(account: &str) -> Option<String> {
    let _guard = FALLBACK_FILE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    load_map(&fallback_keys_path()).get(account).cloned()
}

fn save_fallback_key(account: &str, value: &str) -> crate::errors::Result<()> {
    let _guard = FALLBACK_FILE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = fallback_keys_path();
    let mut map = load_map(&path);
    map.insert(account.to_string(), value.to_string());
    save_map(&path, &map)
}

fn remove_fallback_key(account: &str) -> crate::errors::Result<bool> {
    let _guard = FALLBACK_FILE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = fallback_keys_path();
    let mut map = load_map(&path);
    let removed = map.remove(account).is_some();
    if removed {
        save_map(&path, &map)?;
    }
    Ok(removed)
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
    let recovery_copy = fallback_key(account);
    // macOS always keeps a user-only recovery copy after an authorized save.
    // Prefer it so lock-screen keychain availability cannot block model calls.
    if let Some(value) = select_stored_key(
        if recovery_copy.is_some() {
            Ok(None)
        } else {
            get_os_key(account)
        },
        recovery_copy,
        cfg!(target_os = "macos"),
    )? {
        return Ok(Some(value));
    }

    let Some(legacy_value) = load_legacy_map().get(account).cloned() else {
        return Ok(None);
    };

    set_key(account, &legacy_value)?;
    Ok(Some(legacy_value))
}

pub fn set_key(account: &str, value: &str) -> crate::errors::Result<()> {
    if fallback_key(account).is_some() {
        save_fallback_key(account, value)?;
        remove_legacy_key(account)?;
        return Ok(());
    }

    let os_result = set_os_key(account, value).and_then(|_| {
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
        Ok(())
    });
    match os_result {
        Ok(()) => {}
        Err(error) if !cfg!(target_os = "macos") => return Err(error),
        Err(_) => tracing::warn!(
            "OS credential store unavailable; using the user-only local fallback for key_ref '{}'",
            account
        ),
    }
    if cfg!(target_os = "macos") {
        // Keep the availability copy even when the first keychain read-back
        // succeeds: a later locked-screen read can otherwise strand the user.
        save_fallback_key(account, value)?;
    }
    remove_legacy_key(account)?;
    Ok(())
}

pub fn delete_key(account: &str) -> crate::errors::Result<()> {
    let had_fallback = remove_fallback_key(account)?;
    if let Err(error) = delete_os_key(account) {
        if !cfg!(target_os = "macos") || !had_fallback {
            return Err(error);
        }
    }
    remove_legacy_key(account)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn macos_read_failure_uses_the_persisted_recovery_copy() {
        let os_error = crate::errors::AppError::Other("keychain locked".into());

        assert_eq!(
            select_stored_key(Err(os_error), Some("recovery".into()), true).unwrap(),
            Some("recovery".into())
        );
    }

    #[test]
    fn fallback_map_round_trips_with_user_only_permissions() {
        let root = std::env::temp_dir().join(format!("codefactory-secret-test-{}", Uuid::new_v4()));
        let path = root.join("credentials.json");
        let mut map = HashMap::new();
        map.insert("account".to_string(), "secret".to_string());

        save_map(&path, &map).unwrap();
        map.insert("account".to_string(), "updated-secret".to_string());
        save_map(&path, &map).unwrap();

        assert_eq!(
            load_map(&path).get("account").map(String::as_str),
            Some("updated-secret")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_fallback_map_removes_the_file() {
        let root = std::env::temp_dir().join(format!("codefactory-secret-test-{}", Uuid::new_v4()));
        let path = root.join("credentials.json");
        save_map(&path, &HashMap::from([("account".into(), "secret".into())])).unwrap();

        save_map(&path, &HashMap::new()).unwrap();

        assert!(!path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
