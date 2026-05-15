// SPDX-License-Identifier: Apache-2.0
//! Simple file-based secret store.
//! Keys are stored as a JSON map in the CodeFactory config directory.
//! (Plain-text for now; a future version can AES-encrypt the file.)

use std::collections::HashMap;
use std::path::PathBuf;

fn keys_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodeFactory")
        .join("keys.json")
}

fn load_map() -> HashMap<String, String> {
    let path = keys_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_map(map: &HashMap<String, String>) -> crate::errors::Result<()> {
    let path = keys_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_string_pretty(map)?)?;
    Ok(())
}

pub fn get_key(account: &str) -> crate::errors::Result<Option<String>> {
    let map = load_map();
    Ok(map.get(account).cloned())
}

pub fn set_key(account: &str, value: &str) -> crate::errors::Result<()> {
    let mut map = load_map();
    map.insert(account.to_string(), value.to_string());
    save_map(&map)
}

pub fn delete_key(account: &str) -> crate::errors::Result<()> {
    let mut map = load_map();
    map.remove(account);
    save_map(&map)
}
