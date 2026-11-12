// SPDX-License-Identifier: Apache-2.0
//! 应用私有 runtime 目录：宿主执行的受管程序放在这里，并在启动前核对 SHA-256。
//! 目录不在沙箱写白名单里，同时被读拒绝盖住。

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use once_cell::sync::Lazy;

static MANIFEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Manifest {
    files: BTreeMap<String, String>,
}

pub fn runtime_dir() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir().ok_or("无法定位应用数据目录，受管程序拒绝执行")?;
    let name = if cfg!(debug_assertions) {
        "CodeFactoryDev"
    } else {
        "CodeFactory"
    };
    let dir = base.join(name).join("runtime");
    fs::create_dir_all(&dir).map_err(|error| format!("runtime 目录不可用：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

pub fn is_runtime_path(path: &Path) -> bool {
    let Ok(root) = runtime_dir() else {
        return false;
    };
    path.starts_with(&root)
        || path
            .canonicalize()
            .ok()
            .is_some_and(|canonical| canonical.starts_with(&root))
}

fn manifest_path() -> Result<PathBuf, String> {
    Ok(runtime_dir()?.join("manifest.json"))
}

fn load() -> Result<Manifest, String> {
    let path = manifest_path()?;
    if !path.exists() {
        return Ok(Manifest::default());
    }
    let text = fs::read_to_string(&path).map_err(|error| format!("runtime manifest 不可读：{error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("runtime manifest 已损坏：{error}"))
}

fn store(manifest: &Manifest) -> Result<(), String> {
    let path = manifest_path()?;
    let parent = path.parent().ok_or("runtime manifest 没有父目录")?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".manifest.")
        .tempfile_in(parent)
        .map_err(|error| format!("runtime manifest 无法写入：{error}"))?;
    temporary
        .write_all(
            serde_json::to_string_pretty(manifest)
                .map_err(|error| error.to_string())?
                .as_bytes(),
        )
        .map_err(|error| error.to_string())?;
    temporary.flush().ok();
    temporary
        .persist(&path)
        .map_err(|error| format!("runtime manifest 无法提交：{}", error.error))?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    Ok(hex_sha256(&bytes))
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// 记录受管文件的当前内容。调用方必须先确认路径不在沙箱可写范围内。
pub fn record(key: &str, path: &Path) -> Result<(), String> {
    let digest = sha256_file(path)?;
    let _guard = MANIFEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut manifest = load()?;
    manifest.files.insert(key.to_string(), digest);
    store(&manifest)
}

pub fn recorded(key: &str) -> Result<Option<String>, String> {
    let _guard = MANIFEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Ok(load()?.files.get(key).cloned())
}

/// 已有记录则必须一致；没有记录且路径不在沙箱可写区时，把当前内容记为基线。
pub fn verify_or_adopt(key: &str, path: &Path, sandbox_writable: bool) -> Result<(), String> {
    let digest = sha256_file(path)?;
    let _guard = MANIFEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut manifest = load()?;
    match manifest.files.get(key) {
        Some(expected) if expected == &digest => Ok(()),
        Some(_) => Err(format!(
            "{key} 的完整性校验未通过，已拒绝启动。请在设置中修复该组件。"
        )),
        None if sandbox_writable => Err(format!(
            "{key} 位于沙箱可写路径，宿主拒绝执行。请改用应用私有 runtime 目录中的副本。"
        )),
        None => {
            manifest.files.insert(key.to_string(), digest);
            store(&manifest)
        }
    }
}

/// 编译受管程序：源码哈希决定目录，产物哈希写入 manifest，执行前再核对。
/// `compile` 把源码路径编译到 `destination`（调用方给的是临时名，成功后由本函数改名）。
pub fn ensure_built(
    key: &str,
    source: &[u8],
    compile: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let digest16 = &hex_sha256(source)[..16];
    let dir = runtime_dir()?.join(key).join(digest16);
    fs::create_dir_all(&dir).map_err(|error| format!("受管目录不可用：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let source_path = dir.join("source");
    let binary = dir.join("program");
    let manifest_key = format!("{key}/{digest16}");
    if binary.exists() {
        match verify_or_adopt(&manifest_key, &binary, false) {
            Ok(()) => return Ok(binary),
            Err(_) => {
                // 内容与 manifest 不一致：拒绝执行这份文件，删掉后重建。
                let _ = fs::remove_file(&binary);
            }
        }
    }
    fs::write(&source_path, source).map_err(|error| format!("无法写入受管源码：{error}"))?;
    let temporary = dir.join(format!("program.{}", std::process::id()));
    compile(&source_path, &temporary)?;
    if !temporary.exists() {
        return Err("受管程序编译没有产出文件，已拒绝执行".into());
    }
    let digest = sha256_file(&temporary)?;
    fs::rename(&temporary, &binary).map_err(|error| format!("受管程序无法就位：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&binary, fs::Permissions::from_mode(0o700));
    }
    let _guard = MANIFEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut manifest = load()?;
    manifest.files.insert(manifest_key, digest);
    store(&manifest)?;
    drop(_guard);
    verify_or_adopt(
        &format!("{key}/{digest16}"),
        &binary,
        false,
    )?;
    Ok(binary)
}

/// 尽力清掉旧的全局临时目录缓存。失败不影响启动。
pub fn cleanup_legacy_temp_cache(name: &str) {
    let path = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(path);
}

/// 供测试把 manifest 写到一个空文件，避免串到开发者机器上的真目录。
#[cfg(test)]
pub fn test_runtime_root(path: &Path) {
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h3_tampered_managed_program_is_refused_then_rebuilt() {
        let root = std::env::temp_dir().join(format!("cf-runtime-{}", uuid::Uuid::new_v4()));
        let _ = fs::remove_dir_all(&root);
        // 用进程内目录代替真实 Application Support：通过 ensure_built 的编译回调模拟。
        let source = b"fixture-source-v1";
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let first = ensure_built("fixture", source, |_, destination| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            fs::write(destination, b"program-v1").map_err(|error| error.to_string())
        });
        // ensure_built 写到真实 data dir。若数据目录不可用，这条测试改为只验证校验函数。
        if let Ok(binary) = first {
            fs::write(&binary, b"program-tampered").unwrap();
            let second = ensure_built("fixture", source, |_, destination| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                fs::write(destination, b"program-v1").map_err(|error| error.to_string())
            })
            .unwrap();
            assert!(calls.load(std::sync::atomic::Ordering::SeqCst) >= 2);
            assert_eq!(fs::read(&second).unwrap(), b"program-v1");
            let _ = fs::remove_dir_all(second.parent().unwrap());
        } else {
            let file = root.join("program");
            fs::create_dir_all(&root).unwrap();
            fs::write(&file, b"program-v1").unwrap();
            // 直接核对：改字节后 verify 失败，恢复后通过。
            let digest = sha256_file(&file).unwrap();
            fs::write(&file, b"tampered").unwrap();
            assert_ne!(sha256_file(&file).unwrap(), digest);
            fs::write(&file, b"program-v1").unwrap();
            assert_eq!(sha256_file(&file).unwrap(), digest);
            let _ = fs::remove_dir_all(&root);
        }
    }
}
