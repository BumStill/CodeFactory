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

#[cfg(test)]
static TEST_CONFIG_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
static TEST_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn secret_config_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = TEST_CONFIG_DIR
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
    {
        return dir;
    }
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

#[cfg(test)]
struct TestOsStore {
    values: std::collections::HashMap<String, String>,
    fail_writes: std::collections::HashSet<String>,
}

#[cfg(test)]
static TEST_OS: std::sync::Mutex<Option<TestOsStore>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn test_os_mut() -> Option<std::sync::MutexGuard<'static, Option<TestOsStore>>> {
    Some(TEST_OS.lock().unwrap_or_else(|error| error.into_inner()))
}

fn get_os_key(account: &str) -> crate::errors::Result<Option<String>> {
    #[cfg(test)]
    if let Some(guard) = test_os_mut() {
        if let Some(store) = guard.as_ref() {
            return Ok(store.values.get(account).cloned());
        }
    }
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
    #[cfg(test)]
    if let Some(mut guard) = test_os_mut() {
        if let Some(store) = guard.as_mut() {
            if store.fail_writes.contains(account) {
                return Err(crate::errors::AppError::Other(
                    "synthetic keychain write failed".into(),
                ));
            }
            store.values.insert(account.to_string(), value.to_string());
            return Ok(());
        }
    }
    entry(account)?.set_password(value).map_err(|e| {
        crate::errors::AppError::Other(format!(
            "Could not save OS credential for key_ref '{}': {}",
            account, e
        ))
    })
}

fn delete_os_key(account: &str) -> crate::errors::Result<()> {
    #[cfg(test)]
    if let Some(mut guard) = test_os_mut() {
        if let Some(store) = guard.as_mut() {
            store.values.remove(account);
            return Ok(());
        }
    }
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(crate::errors::AppError::Other(format!(
            "Could not delete OS credential for key_ref '{}': {}",
            account, e
        ))),
    }
}

pub fn get_key(account: &str) -> crate::errors::Result<Option<String>> {
    crate::desktop_context::read_secret(|| get_key_normal(account))
}

#[cfg(test)]
static TEST_FORCE_KEYCHAIN: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

#[cfg(test)]
static TEST_PLAINTEXT_FALLBACK: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

fn prefer_keychain() -> bool {
    #[cfg(test)]
    if *TEST_FORCE_KEYCHAIN.lock().unwrap_or_else(|error| error.into_inner()) {
        return true;
    }
    cfg!(target_os = "macos")
}

fn plaintext_fallback_enabled() -> bool {
    #[cfg(test)]
    if let Some(value) = *TEST_PLAINTEXT_FALLBACK.lock().unwrap_or_else(|error| error.into_inner()) {
        return value;
    }
    if !cfg!(target_os = "macos") {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(crate::config::settings::config_path()) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("macos_plaintext_key_fallback")
                .and_then(|item| item.as_bool())
        })
        .unwrap_or(false)
}

fn get_key_normal(account: &str) -> crate::errors::Result<Option<String>> {
    if prefer_keychain() {
        match get_os_key(account) {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) => {}
            Err(error) => {
                if plaintext_fallback_enabled() {
                    if let Some(value) = fallback_key(account) {
                        return Ok(Some(value));
                    }
                }
                return Err(error);
            }
        }
        if plaintext_fallback_enabled() {
            if let Some(value) = fallback_key(account) {
                return Ok(Some(value));
            }
        }
    } else {
        let recovery_copy = fallback_key(account);
        if let Some(value) = select_stored_key(get_os_key(account), recovery_copy, false)? {
            return Ok(Some(value));
        }
    }

    let Some(legacy_value) = load_legacy_map().get(account).cloned() else {
        return Ok(None);
    };

    set_key(account, &legacy_value)?;
    Ok(Some(legacy_value))
}

pub fn set_key(account: &str, value: &str) -> crate::errors::Result<()> {
    crate::desktop_context::write_secret(|| set_key_normal(account, value))
}

fn set_os_key_checked(account: &str, value: &str) -> crate::errors::Result<()> {
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
    Ok(())
}

fn set_key_normal(account: &str, value: &str) -> crate::errors::Result<()> {
    if prefer_keychain() {
        set_os_key_checked(account, value)?;
        if plaintext_fallback_enabled() {
            save_fallback_key(account, value)?;
        } else {
            remove_fallback_key(account)?;
        }
        remove_legacy_key(account)?;
        return Ok(());
    }

    if fallback_key(account).is_some() {
        save_fallback_key(account, value)?;
        remove_legacy_key(account)?;
        return Ok(());
    }
    set_os_key_checked(account, value)?;
    remove_legacy_key(account)?;
    Ok(())
}

pub fn delete_key(account: &str) -> crate::errors::Result<()> {
    crate::desktop_context::write_secret(|| delete_key_normal(account))
}

fn delete_key_normal(account: &str) -> crate::errors::Result<()> {
    let had_fallback = remove_fallback_key(account)?;
    if let Err(error) = delete_os_key(account) {
        // macOS 不再吞掉钥匙串删除错误。其他平台保持原有行为。
        if cfg!(target_os = "macos") || !had_fallback {
            return Err(error);
        }
    }
    remove_legacy_key(account)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub migrated: usize,
    pub pending: Vec<String>,
}

/// 把已有明文副本逐条写入钥匙串。失败的条目留在文件里，不阻断启动。
pub fn migrate_plaintext_fallback() -> MigrationReport {
    let _guard = FALLBACK_FILE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let path = fallback_keys_path();
    let mut map = load_map(&path);
    if map.is_empty() {
        return MigrationReport { migrated: 0, pending: Vec::new() };
    }
    let mut pending = Vec::new();
    let mut migrated = 0;
    let accounts: Vec<(String, String)> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (account, value) in accounts {
        match set_os_key_checked(&account, &value) {
            Ok(()) => {
                map.remove(&account);
                migrated += 1;
            }
            Err(error) => {
                tracing::warn!(account = %account, "credential migration left one entry in place: {error}");
                pending.push(account);
            }
        }
    }
    let _ = save_map(&path, &map);
    tracing::info!(
        migrated,
        pending = pending.len(),
        "credential plaintext migration finished without secret values"
    );
    MigrationReport { migrated, pending }
}

pub fn plaintext_fallback_pending() -> usize {
    load_map(&fallback_keys_path()).len()
}

pub fn delete_plaintext_fallback_file() -> crate::errors::Result<()> {
    let _guard = FALLBACK_FILE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    save_map(&fallback_keys_path(), &HashMap::new())
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

    #[test]
    fn h2_migration_moves_successful_accounts_and_keeps_a_failed_one() {
        let _env = TEST_ENV.lock().unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join(format!("cf-h2-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        *TEST_CONFIG_DIR.lock().unwrap() = Some(root.clone());
        *TEST_OS.lock().unwrap() = Some(TestOsStore {
            values: HashMap::new(),
            fail_writes: ["gamma"].into_iter().map(str::to_string).collect(),
        });
        let mut map = HashMap::new();
        map.insert("alpha".into(), "a-secret".into());
        map.insert("beta".into(), "b-secret".into());
        map.insert("gamma".into(), "g-secret".into());
        save_map(&fallback_keys_path(), &map).unwrap();

        let report = migrate_plaintext_fallback();
        assert_eq!(report.migrated, 2);
        assert_eq!(report.pending, vec!["gamma".to_string()]);
        let left = load_map(&fallback_keys_path());
        assert_eq!(left.get("gamma").map(String::as_str), Some("g-secret"));
        assert!(!left.contains_key("alpha"));
        let store = TEST_OS.lock().unwrap();
        let values = &store.as_ref().unwrap().values;
        assert_eq!(values.get("alpha").map(String::as_str), Some("a-secret"));
        assert_eq!(values.get("beta").map(String::as_str), Some("b-secret"));
        assert!(!values.contains_key("gamma"));
        drop(store);
        *TEST_OS.lock().unwrap() = None;
        *TEST_CONFIG_DIR.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn h2_keychain_write_keeps_plaintext_only_when_the_fallback_switch_is_on() {
        let _env = TEST_ENV.lock().unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join(format!("cf-h2-switch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        *TEST_CONFIG_DIR.lock().unwrap() = Some(root.clone());
        *TEST_FORCE_KEYCHAIN.lock().unwrap() = true;
        *TEST_OS.lock().unwrap() = Some(TestOsStore {
            values: HashMap::new(),
            fail_writes: std::collections::HashSet::new(),
        });
        *TEST_PLAINTEXT_FALLBACK.lock().unwrap() = Some(false);
        set_key_normal("acct", "secret-value").unwrap();
        assert!(get_os_key("acct").unwrap().as_deref() == Some("secret-value"));
        assert!(!load_map(&fallback_keys_path()).contains_key("acct"));

        TEST_OS.lock().unwrap().as_mut().unwrap().fail_writes.insert("acct".into());
        assert!(set_key_normal("acct", "should-not-land").is_err());
        assert!(!load_map(&fallback_keys_path()).contains_key("acct"));

        TEST_OS.lock().unwrap().as_mut().unwrap().fail_writes.clear();
        TEST_OS.lock().unwrap().as_mut().unwrap().values.clear();
        *TEST_PLAINTEXT_FALLBACK.lock().unwrap() = Some(true);
        set_key_normal("acct", "copied").unwrap();
        assert_eq!(
            load_map(&fallback_keys_path()).get("acct").map(String::as_str),
            Some("copied")
        );
        TEST_OS.lock().unwrap().as_mut().unwrap().values.clear();
        assert_eq!(get_key_normal("acct").unwrap().as_deref(), Some("copied"));
        *TEST_PLAINTEXT_FALLBACK.lock().unwrap() = Some(false);
        assert_eq!(get_key_normal("acct").unwrap(), None);

        *TEST_FORCE_KEYCHAIN.lock().unwrap() = false;
        *TEST_PLAINTEXT_FALLBACK.lock().unwrap() = None;
        *TEST_OS.lock().unwrap() = None;
        *TEST_CONFIG_DIR.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn h2_default_settings_do_not_enable_plaintext_fallback() {
        let settings = crate::config::settings::Settings::default();
        assert!(!settings.macos_plaintext_key_fallback);
        assert!(!settings.dispatch_privileged_ops);
    }
}
